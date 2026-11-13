use crate::audio::play_synthetic_tone_to_alsa;
use crate::input::{
    spawn_button_press_uinput, spawn_tof_swipe_uinput, write_tof_distance_uinput,
};
use crate::state::{AliceState, BridgeState, MAIND_LOG_PATH};
use crate::voice_dialog::{handle_inject_voice, handle_ipc_test, set_host_volume};
use protocol::{
    read_host_command, write_guest_packet, BridgeStateSnapshot, GuestPacket, HostCommand,
};
use std::env;
use std::fs;
use std::io::BufReader;
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

pub fn run_server(state: Arc<BridgeState>) {
    let bind_addr = env::var("HW_BRIDGE_BIND").unwrap_or_else(|_| "0.0.0.0:8091".to_string());
    let listener = TcpListener::bind(&bind_addr).expect("failed to bind qemu-hw-bridge on :8091");
    eprintln!("[qemu-hw-bridge] duplex stream server listening on {}", bind_addr);

    for stream in listener.incoming().flatten() {
        let st = state.clone();
        thread::spawn(move || {
            let _ = handle_client(stream, st);
        });
    }
}

fn read_maind_log_tail(max_lines: usize) -> Vec<String> {
    let Ok(raw) = fs::read_to_string(MAIND_LOG_PATH) else {
        return Vec::new();
    };
    let all: Vec<&str> = raw.lines().collect();
    let start = all.len().saturating_sub(max_lines);
    all[start..].iter().map(|s| (*s).to_string()).collect()
}

pub fn build_state_snapshot(state: &Arc<BridgeState>) -> BridgeStateSnapshot {
    let prev_conflicts = state.led_trigger_conflict_count.load(Ordering::Relaxed);
    let native_ring = state.native_led_frame.lock().unwrap().clone();
    let native_aux = state.native_aux_frame.lock().unwrap().clone();
    let ring_out = state
        .peripherals
        .ring
        .read_ring(&native_ring, &state.led_trigger_conflict_count);
    let new_conflicts = state.led_trigger_conflict_count.load(Ordering::Relaxed);
    if new_conflicts > prev_conflicts && new_conflicts <= 5 {
        state.log(format!(
            "WARN LED: sysfs brightness write overwritten by active trigger [{}]! Write 'none' to trigger first.",
            ring_out.trigger
        ));
    }
    let aux_px = state.peripherals.aux_light.read_zones(&native_aux);
    let disp_out = state.peripherals.front_display.read_display();
    let tof_mm = state.tof_mm.load(Ordering::Relaxed);
    let audio_hw_out = state.peripherals.audio_hw.read_audio_hw(tof_mm);

    let vol_level = state.volume_level.load(Ordering::Relaxed);
    let master_vol = vol_level * 1000;
    let cap_len = state.captured_pcm.lock().unwrap().len();
    let logs: Vec<String> = state.recent_logs.lock().unwrap().iter().cloned().collect();
    let maind_tail = read_maind_log_tail(40);
    let last_spoken = state.last_spoken_text.lock().unwrap().clone();
    let forced_led = state.forced_led_pattern.lock().unwrap().clone();

    BridgeStateSnapshot {
        ok: true,
        board_slug: state.board_slug.clone(),
        board_name: state.board_name.clone(),
        ring_leds: state.ring_leds,
        logo_leds: state.logo_leds,
        ambilight_leds: state.ambilight_leds,
        aux_light_kind: state.peripherals.aux_light.aux_kind().to_string(),
        has_clock: state.has_clock,
        display_kind: disp_out.display_kind.to_string(),
        screen_width: disp_out.screen_width,
        screen_height: disp_out.screen_height,
        screen_pixels: disp_out.screen_pixels,
        ring_pixels: ring_out.pixels,
        logo_pixels: aux_px,
        ring_trigger: ring_out.trigger,
        clock_segments: disp_out.clock_segments,
        amplifiers: audio_hw_out.amplifiers,
        ultrasound: audio_hw_out.ultrasound,
        doa_angle: state.doa_angle.load(Ordering::Relaxed),
        tof_mm,
        als_lux: state.als_lux.load(Ordering::Relaxed),
        temp_c: (state.temp_milli_c.load(Ordering::Relaxed) as f32) / 1000.0,
        volume_level: vol_level,
        master_volume: master_vol,
        mics_muted: state.mics_muted.load(Ordering::Relaxed),
        forced_led_pattern: forced_led,
        mic_rms: state.mic_rms.load(Ordering::Relaxed),
        mic_burst_active: state.mic_burst_active.load(Ordering::Relaxed),
        speaker_rms: state.speaker_rms.load(Ordering::Relaxed),
        captured_audio_bytes: cap_len,
        alice_state: state.alice_state().as_str().to_string(),
        aliced_hijack_count: state.aliced_hijack_count.load(Ordering::Relaxed),
        aliced_say_count: state.aliced_say_count.load(Ordering::Relaxed),
        ipc_error_count: state.ipc_error_count.load(Ordering::Relaxed),
        led_trigger_conflict_count: new_conflicts,
        last_spoken_text: last_spoken,
        recent_logs: logs,
        maind_logs: maind_tail,
        available_led_patterns: state.available_led_patterns.clone(),
        audio_config: state.audio_config(),
    }
}

pub fn handle_host_command(state: &Arc<BridgeState>, cmd: HostCommand) -> GuestPacket {
    match cmd {
        HostCommand::RequestState => GuestPacket::CommandAck {
            ok: true,
            error: None,
            message: None,
        },
        HostCommand::PressButton { button, action } => {
            let btn = if button.is_empty() {
                "action"
            } else {
                button.as_str()
            };
            let act = if action.is_empty() {
                "single"
            } else {
                action.as_str()
            };

            match btn {
                "mute" => {
                    let new_muted = !state.mics_muted.load(Ordering::SeqCst);
                    state.mics_muted.store(new_muted, Ordering::SeqCst);
                    state.volume_overlay_until_ms.store(0, Ordering::SeqCst);
                    state.peripherals.audio_hw.on_volume_or_mute_changed(
                        state.volume_level.load(Ordering::Relaxed),
                        new_muted,
                    );
                    if new_muted {
                        state.mic_burst_active.store(false, Ordering::SeqCst);
                        state.mic_rms.store(0, Ordering::SeqCst);
                        state.set_alice_state(AliceState::Idle);
                        state.append_maind_log(
                            "[event_loop_audio_device_input_controller.cpp:125] Mute microphones",
                        );
                        state.log(
                            "Button MUTE -> Microphones MUTED (LED ring solid RED: mute_mics.led)",
                        );
                    } else {
                        state
                            .mic_rms
                            .store(state.audio_config().mic_idle_rms, Ordering::SeqCst);
                        state.append_maind_log(
                            "[event_loop_audio_device_input_controller.cpp:129] Unmute microphones",
                        );
                        state.log("Button MUTE -> Microphones UNMUTED (active)");
                    }
                    crate::leds::sync_immediate_led_state(state);
                }
                "volume_up" => {
                    let cur = state.volume_level.load(Ordering::Relaxed);
                    set_host_volume(state, (cur + 1).min(10));
                }
                "volume_down" => {
                    let cur = state.volume_level.load(Ordering::Relaxed);
                    set_host_volume(state, cur.saturating_sub(1));
                }
                _ => {
                    state.log(format!("Inject button: {} ({})", btn, act));
                    if act == "single" && state.alice_state() != AliceState::Idle {
                        let cur_seq = state.dialog_seq.load(Ordering::SeqCst);
                        state.hijacked_seq.store(cur_seq, Ordering::SeqCst);
                        state.set_alice_state(AliceState::Idle);
                        state.append_maind_log("[sdk] onAliceStateChanged: IDLE");
                        state.log("Action button -> stopped active Alice dialog");
                    }
                }
            }

            spawn_button_press_uinput(state.clone(), btn, act);

            GuestPacket::CommandAck {
                ok: true,
                error: None,
                message: None,
            }
        }
        HostCommand::SetVolume { volume_level } => {
            set_host_volume(state, volume_level);
            GuestPacket::CommandAck {
                ok: true,
                error: None,
                message: None,
            }
        }
        HostCommand::SetLedPattern { pattern } => {
            let pat = pattern.trim().to_string();
            state.volume_overlay_until_ms.store(0, Ordering::SeqCst);
            *state.forced_led_pattern.lock().unwrap() = pat.clone();
            if pat.is_empty() {
                state.log("LED Pattern Tester -> Returned to automatic state mode");
            } else {
                state.log(format!("LED Pattern Tester -> Forced pattern '{}'", pat));
            }
            crate::leds::sync_immediate_led_state(state);
            GuestPacket::CommandAck {
                ok: true,
                error: None,
                message: Some(pat),
            }
        }
        HostCommand::SetClockDisplay {
            segments,
            replace,
            pixels,
            led_frame,
            brightness,
            scale_brightness,
            text,
        } => {
            let br = brightness.unwrap_or(255);
            if let Some(seg_map) = segments {
                let replace_all = replace.unwrap_or(seg_map.len() >= 28);
                state
                    .peripherals
                    .front_display
                    .write_display_segments(&seg_map, replace_all);
                state.log(format!(
                    "Set front display segments ({} keys, replace={})",
                    seg_map.len(),
                    replace_all
                ));
            } else if let Some(px) = pixels {
                state.peripherals.front_display.write_display_pixels(&px);
                state.log(format!(
                    "Set front display matrix pixels ({} px, active={})",
                    px.len(),
                    px.iter().filter(|&&x| x > 0).count()
                ));
            } else if let Some(led_str) = led_frame {
                let ok = state
                    .peripherals
                    .front_display
                    .write_display_led_frame(&led_str, br);
                state.log(format!(
                    "Load .led frame into front display (ok={}, brightness={})",
                    ok, br
                ));
            } else if let Some(scale_br) = scale_brightness {
                state
                    .peripherals
                    .front_display
                    .scale_active_brightness(scale_br);
                state.log(format!(
                    "Scale active front display brightness -> PWM {}",
                    scale_br
                ));
            } else {
                let txt = text.as_deref().unwrap_or("");
                state.peripherals.front_display.write_display_text(txt, br);
                state.log(format!(
                    "Set front display ({}): \"{}\" (PWM {})",
                    state.peripherals.front_display.display_kind(),
                    txt,
                    br
                ));
            }
            GuestPacket::CommandAck {
                ok: true,
                error: None,
                message: None,
            }
        }
        HostCommand::SetSensor {
            doa_angle,
            mic_rms,
            als_lux,
            temp_c,
            tof_mm,
            swipe,
            swipe_direction,
        } => {
            if let Some(doa) = doa_angle {
                state.doa_angle.store(doa % 360, Ordering::SeqCst);
            }
            if let Some(rms) = mic_rms {
                state.mic_rms.store(rms, Ordering::SeqCst);
            }
            if let Some(als) = als_lux {
                state.als_lux.store(als, Ordering::SeqCst);
                let _ = fs::write(
                    "/sys/bus/iio/devices/iio:device0/in_illuminance_raw",
                    als.to_string(),
                );
            }
            if let Some(tc) = temp_c {
                let milli = ((tc as f64) * 1000.0).round() as u32;
                state.temp_milli_c.store(milli, Ordering::SeqCst);
                let _ = fs::write("/sys/class/thermal/thermal_zone0/temp", milli.to_string());
                let saradc = (((tc as f64) * 10.0) + 1795.0).round() as i32;
                let _ = fs::write(
                    "/sys/bus/iio/devices/iio:device0/in_voltage6_mean_raw",
                    saradc.to_string(),
                );
            }
            if swipe.unwrap_or(false) || swipe_direction.is_some() {
                let dir = swipe_direction
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("left")
                    .to_ascii_lowercase();
                if doa_angle.is_none() {
                    let cur_doa = state.doa_angle.load(Ordering::Relaxed);
                    let next_doa = match dir.as_str() {
                        "left" => (cur_doa + 315) % 360,
                        "right" => (cur_doa + 45) % 360,
                        "up" => 0,
                        "down" => 180,
                        _ => cur_doa,
                    };
                    state.doa_angle.store(next_doa, Ordering::SeqCst);
                }
                let dir_code: i32 = match dir.as_str() {
                    "left" => 1,
                    "right" => 2,
                    "up" => 3,
                    "down" => 4,
                    _ => 0,
                };
                let prev_tof = state.tof_mm.load(Ordering::Relaxed);
                let restore_tof = if dir == "wave" || prev_tof == 140 {
                    8190
                } else {
                    prev_tof
                };
                state.log(format!(
                    "Inject ToF swipe gesture [{}] (140mm -> {}mm)",
                    dir, restore_tof
                ));
                spawn_tof_swipe_uinput(state.clone(), dir_code, restore_tof);
            } else if let Some(tof) = tof_mm {
                write_tof_distance_uinput(state, tof);
            }
            GuestPacket::CommandAck {
                ok: true,
                error: None,
                message: None,
            }
        }
        HostCommand::InjectVoice {
            phrase,
            doa_angle,
            rms,
            wake_only,
        } => handle_inject_voice(state, phrase, doa_angle, rms, wake_only),
        HostCommand::SetAudioConfig { config } => {
            let applied = state.set_audio_config(config);
            state.log(format!(
                "AudioConfig updated -> mic={}Hz/{}ms (interval={}ms, lag_cap={}, idle_rms={}, burst_rms={}), dialog={}/{}/{}ms, capture={}Hz/{}ms (gate={}, hangover={}ms)",
                applied.mic_sample_rate,
                applied.mic_frame_ms,
                applied.mic_send_interval_ms,
                applied.mic_lag_cap,
                applied.mic_idle_rms,
                applied.mic_burst_rms,
                applied.recognition_delay_ms,
                applied.burst_duration_ms,
                applied.hijack_window_ms,
                applied.capture_sample_rate,
                applied.capture_chunk_ms,
                applied.silence_gate_rms,
                applied.hangover_ms,
            ));
            GuestPacket::CommandAck {
                ok: true,
                error: None,
                message: None,
            }
        }
        HostCommand::IpcTest { cmd, text } => handle_ipc_test(cmd, text),
        HostCommand::PlayTestTone => {
            state.log("Playing 440 Hz test tone to ALSA default -> snd-aloop");
            thread::spawn(|| {
                play_synthetic_tone_to_alsa(440.0, 350);
            });
            GuestPacket::CommandAck {
                ok: true,
                error: None,
                message: None,
            }
        }
        HostCommand::ClearAudio => {
            state.captured_pcm.lock().unwrap().clear();
            state.speaker_rms.store(0, Ordering::Relaxed);
            state.log("Cleared captured speaker audio buffer");
            GuestPacket::CommandAck {
                ok: true,
                error: None,
                message: None,
            }
        }
        HostCommand::PowerOff => {
            state.log("Poweroff requested via duplex bridge");
            thread::spawn(|| {
                thread::sleep(Duration::from_millis(100));
                let _ = Command::new("poweroff").arg("-f").status();
            });
            GuestPacket::CommandAck {
                ok: true,
                error: None,
                message: None,
            }
        }
    }
}

fn handle_client(stream: TcpStream, state: Arc<BridgeState>) -> std::io::Result<()> {
    let _ = stream.set_nodelay(true);
    let mut write_stream = stream.try_clone()?;
    let _ = write_stream.set_nodelay(true);

    let (tx, rx) = mpsc::sync_channel::<GuestPacket>(512);

    let initial_snap = build_state_snapshot(&state);
    let _ = tx.send(GuestPacket::StateSnapshot(initial_snap.clone()));
    state.bridge_clients.lock().unwrap().push(tx.clone());

    let alive = Arc::new(AtomicBool::new(true));

    let alive_writer = alive.clone();
    thread::spawn(move || {
        for pkt in rx {
            if write_guest_packet(&mut write_stream, &pkt).is_err() {
                break;
            }
        }
        alive_writer.store(false, Ordering::SeqCst);
    });

    let alive_pusher = alive.clone();
    let tx_pusher = tx.clone();
    let state_pusher = state.clone();
    thread::spawn(move || {
        let mut last_snap = initial_snap;
        let mut last_push = Instant::now();
        while alive_pusher.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(45));
            if !alive_pusher.load(Ordering::Relaxed) {
                break;
            }
            let snap = build_state_snapshot(&state_pusher);
            if snap != last_snap || last_push.elapsed() >= Duration::from_millis(250) {
                last_snap = snap.clone();
                last_push = Instant::now();
                match tx_pusher.try_send(GuestPacket::StateSnapshot(snap)) {
                    Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
                    Err(mpsc::TrySendError::Disconnected(_)) => break,
                }
            }
        }
    });

    let mut reader = BufReader::new(stream);
    while alive.load(Ordering::Relaxed) {
        match read_host_command(&mut reader) {
            Ok(cmd) => {
                let ack = handle_host_command(&state, cmd);
                let updated_snap = build_state_snapshot(&state);
                if tx.send(GuestPacket::StateSnapshot(updated_snap)).is_err() {
                    break;
                }
                if tx.send(ack).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    alive.store(false, Ordering::SeqCst);
    Ok(())
}
