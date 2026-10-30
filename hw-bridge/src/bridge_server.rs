use crate::audio::{play_expressive_tts_to_alsa, play_synthetic_tone_to_alsa};
use crate::input::emit_compat_input_event;
use crate::proto::push_len_delimited;
use crate::state::{AliceState, BridgeState, MAIND_LOG_PATH};
use protocol::{
    read_host_command, write_guest_packet, BridgeStateSnapshot, GuestPacket, HostCommand,
};
use std::env;
use std::fs;
use std::io::{BufReader, Write};
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

fn parse_phrase_volume(phrase: &str) -> Option<u32> {
    let lower = phrase.to_lowercase();
    if !lower.contains("громкост") && !lower.contains("volume") {
        return None;
    }
    for tok in lower.split(|c: char| !c.is_ascii_digit()) {
        if !tok.is_empty() {
            if let Ok(v) = tok.parse::<u32>() {
                return Some(v.min(10));
            }
        }
    }
    None
}

fn send_local_aliced_packet(payload: &[u8]) -> bool {
    let Ok(mut s) = TcpStream::connect("127.0.0.1:9875") else {
        return false;
    };
    let _ = s.set_nodelay(true);
    let mut framed = Vec::with_capacity(4 + payload.len());
    framed.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    framed.extend_from_slice(payload);
    let ok = s.write_all(&framed).is_ok();
    let _ = s.flush();
    ok
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
                        state.mic_rms.store(60, Ordering::SeqCst);
                        state.append_maind_log(
                            "[event_loop_audio_device_input_controller.cpp:129] Unmute microphones",
                        );
                        state.log("Button MUTE -> Microphones UNMUTED (active)");
                    }
                    crate::leds::sync_immediate_led_state(state);
                }
                "volume_up" => {
                    let cur = state.volume_level.load(Ordering::Relaxed);
                    state.set_volume_level((cur + 1).min(10));
                }
                "volume_down" => {
                    let cur = state.volume_level.load(Ordering::Relaxed);
                    state.set_volume_level(cur.saturating_sub(1));
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

            let st = state.clone();
            let btn_s = btn.to_string();
            let act_s = act.to_string();
            thread::spawn(move || {
                let code: u16 = match btn_s.as_str() {
                    "volume_up" => 115,
                    "volume_down" => 114,
                    "mute" => 113,
                    _ => 582,
                };
                let clicks = match act_s.as_str() {
                    "double" => 2,
                    "triple" => 3,
                    _ => 1,
                };
                let hold_ms = if act_s == "long" { 900 } else { 50 };
                for c in 0..clicks {
                    if let Some(ref mut f) = *st.key_uinput.lock().unwrap() {
                        emit_compat_input_event(f, 1, code, 1);
                        emit_compat_input_event(f, 0, 0, 0);
                    }
                    thread::sleep(Duration::from_millis(hold_ms));
                    if let Some(ref mut f) = *st.key_uinput.lock().unwrap() {
                        emit_compat_input_event(f, 1, code, 0);
                        emit_compat_input_event(f, 0, 0, 0);
                    }
                    if c + 1 < clicks {
                        thread::sleep(Duration::from_millis(90));
                    }
                }
            });

            GuestPacket::CommandAck {
                ok: true,
                error: None,
                message: None,
            }
        }
        HostCommand::SetVolume { volume_level } => {
            state.set_volume_level(volume_level);
            GuestPacket::CommandAck {
                ok: true,
                error: None,
                message: None,
            }
        }
        HostCommand::SetLedPattern { pattern } => {
            let pat = pattern.trim().to_string();
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
            if swipe.unwrap_or(false) {
                state.log("Inject ToF swipe gesture (140mm -> 8190mm)");
                let st = state.clone();
                thread::spawn(move || {
                    if let Some(ref mut f) = *st.tof_uinput.lock().unwrap() {
                        emit_compat_input_event(f, 3, 0x19, 0);
                        emit_compat_input_event(f, 3, 0x12, 140);
                        emit_compat_input_event(f, 0, 0, 0);
                    }
                    st.tof_mm.store(140, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(110));
                    if let Some(ref mut f) = *st.tof_uinput.lock().unwrap() {
                        emit_compat_input_event(f, 3, 0x19, 4);
                        emit_compat_input_event(f, 3, 0x12, 8190);
                        emit_compat_input_event(f, 0, 0, 0);
                    }
                    st.tof_mm.store(8190, Ordering::SeqCst);
                });
            } else if let Some(tof) = tof_mm {
                let mm = tof as i32;
                state.tof_mm.store(tof, Ordering::SeqCst);
                if let Some(ref mut f) = *state.tof_uinput.lock().unwrap() {
                    emit_compat_input_event(f, 3, 0x19, if mm < 500 { 0 } else { 4 });
                    emit_compat_input_event(f, 3, 0x12, mm);
                    emit_compat_input_event(f, 0, 0, 0);
                }
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
        } => {
            if state.mics_muted.load(Ordering::Relaxed) {
                state.log("WARN Voice Input ignored: Microphones are MUTED (red LED ring active)");
                return GuestPacket::CommandAck {
                    ok: false,
                    error: Some("mics_muted".to_string()),
                    message: None,
                };
            }

            if let Some(doa) = doa_angle {
                state.doa_angle.store(doa % 360, Ordering::SeqCst);
            }
            if let Some(r) = rms {
                state.mic_rms.store(r, Ordering::SeqCst);
            }
            let phrase_clean = phrase.trim().to_string();

            let seq = state.dialog_seq.fetch_add(1, Ordering::SeqCst) + 1;
            state.set_alice_state(AliceState::Listening);
            state.mic_burst_active.store(true, Ordering::SeqCst);
            state.append_maind_log("[sdk] onAliceStateChanged: LISTENING");
            if !wake_only && !phrase_clean.is_empty() {
                state.log(format!(
                    "Voice Input -> DOA={}°, recognizedPhrase='{}' (1680ms hijack window started)",
                    state.doa_angle.load(Ordering::Relaxed),
                    phrase_clean
                ));
                state.append_maind_log(&format!(
                    "[sdk] onRequestStateChanged: LISTENING, recognizedPhrase='{}'",
                    phrase_clean
                ));
            } else {
                state.log(format!(
                    "Voice Wake Word -> DOA={}°",
                    state.doa_angle.load(Ordering::Relaxed)
                ));
            }

            let st = state.clone();
            let has_phrase = !wake_only && !phrase_clean.is_empty();
            let phrase_for_tts = phrase_clean.clone();
            let parsed_vol = parse_phrase_volume(&phrase_clean);
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(650));
                st.mic_burst_active.store(false, Ordering::SeqCst);
                if !st.mics_muted.load(Ordering::Relaxed) {
                    st.mic_rms.store(60, Ordering::SeqCst);
                }

                if has_phrase {
                    thread::sleep(Duration::from_millis(1030));
                    if st.dialog_seq.load(Ordering::SeqCst) == seq
                        && st.hijacked_seq.load(Ordering::SeqCst) < seq
                    {
                        if let Some(v) = parsed_vol {
                            st.set_volume_level(v);
                        }
                        let auto_reply = format!("Выполняю: {}", phrase_for_tts);
                        *st.last_spoken_text.lock().unwrap() = auto_reply.clone();
                        st.set_alice_state(AliceState::Speaking);
                        st.log("Dialog Hijack window (1680ms) expired without CANCEL_DIALOG -> native Alice SPEAKING");
                        st.append_maind_log("[sdk] onAliceStateChanged: SPEAKING");
                        st.append_maind_log("[sdk] onTtsStarted: request_id=alice_cloud_auto");
                        let _ = play_expressive_tts_to_alsa(&st, &auto_reply, seq);
                        if st.dialog_seq.load(Ordering::SeqCst) == seq {
                            st.set_alice_state(AliceState::Idle);
                            st.append_maind_log(
                                "[sdk] onTtsCompleted: request_id=alice_cloud_auto",
                            );
                            st.append_maind_log("[sdk] onAliceStateChanged: IDLE");
                        }
                    }
                } else {
                    thread::sleep(Duration::from_millis(1200));
                    if st.dialog_seq.load(Ordering::SeqCst) == seq
                        && st.hijacked_seq.load(Ordering::SeqCst) < seq
                    {
                        st.set_alice_state(AliceState::Idle);
                        st.append_maind_log("[sdk] onAliceStateChanged: IDLE");
                    }
                }
            });

            GuestPacket::CommandAck {
                ok: true,
                error: None,
                message: Some(phrase_clean),
            }
        }
        HostCommand::IpcTest { cmd, text } => {
            let cmd_str = if cmd.is_empty() {
                "cancel_dialog"
            } else {
                cmd.as_str()
            };
            let txt = text
                .as_deref()
                .unwrap_or("Проверка локального синтеза речи Алисы")
                .trim();
            let mut alice_method = Vec::new();
            match cmd_str {
                "stop_tts" => {
                    alice_method.extend_from_slice(&[0x08, 14]);
                }
                "vocalize" => {
                    let mut voc_dir = Vec::new();
                    push_len_delimited(&mut voc_dir, 1, txt.as_bytes());
                    voc_dir.extend_from_slice(&[0x10, 0x01]);
                    alice_method.extend_from_slice(&[0x08, 18, 0x18, 0x00]);
                    push_len_delimited(&mut alice_method, 13, &voc_dir);
                }
                _ => {
                    alice_method.extend_from_slice(&[0x08, 0x06]);
                }
            }
            let mut remoting = Vec::new();
            push_len_delimited(&mut remoting, 7, b"AliceCapability");
            push_len_delimited(&mut remoting, 8, &alice_method);
            let mut qmsg = Vec::new();
            push_len_delimited(&mut qmsg, 210, &remoting);
            let sent = send_local_aliced_packet(&qmsg);
            // Allow local aliced IPC server thread (:9875) to process packet before snapshot
            thread::sleep(Duration::from_millis(15));
            GuestPacket::CommandAck {
                ok: sent,
                error: if sent {
                    None
                } else {
                    Some("aliced_connect_failed".to_string())
                },
                message: Some(cmd_str.to_string()),
            }
        }
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
    state.bridge_clients.lock().unwrap().push(tx.clone());

    // Immediately push initial StateSnapshot on connect
    let initial_snap = build_state_snapshot(&state);
    let _ = tx.send(GuestPacket::StateSnapshot(initial_snap.clone()));

    let alive = Arc::new(AtomicBool::new(true));

    // Dedicated writer thread for this client
    let alive_writer = alive.clone();
    thread::spawn(move || {
        for pkt in rx {
            if write_guest_packet(&mut write_stream, &pkt).is_err() {
                break;
            }
        }
        alive_writer.store(false, Ordering::SeqCst);
    });

    // Periodic state push thread (~22 Hz when state changes, 250ms heartbeat)
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

    // Reader loop receiving HostCommand frames
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
