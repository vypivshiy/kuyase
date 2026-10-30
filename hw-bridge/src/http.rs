use crate::audio::{build_wav_bytes, play_expressive_tts_to_alsa, play_synthetic_tone_to_alsa};
use crate::input::emit_compat_input_event;
use crate::proto::push_len_delimited;
use crate::state::{AliceState, BridgeState, MAIND_LOG_PATH};
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const INDEX_HTML: &str = include_str!("dashboard.html");

pub fn run_server(state: Arc<BridgeState>) {
    let bind_addr = env::var("HW_BRIDGE_BIND").unwrap_or_else(|_| "0.0.0.0:8091".to_string());
    let listener = TcpListener::bind(&bind_addr).expect("failed to bind qemu-hw-bridge on :8091");
    eprintln!("[qemu-hw-bridge] listening on {}", bind_addr);

    for stream in listener.incoming().flatten() {
        let st = state.clone();
        thread::spawn(move || {
            let _ = handle_http(stream, st);
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
    let mut framed = Vec::with_capacity(4 + payload.len());
    framed.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    framed.extend_from_slice(payload);
    s.write_all(&framed).is_ok()
}

fn handle_http(mut stream: TcpStream, state: Arc<BridgeState>) -> std::io::Result<()> {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let cloned = stream.try_clone()?;
    let mut reader = BufReader::new(cloned);

    let mut req_line = String::new();
    if reader.read_line(&mut req_line)? == 0 {
        return Ok(());
    }
    let parts: Vec<&str> = req_line.trim().split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(());
    }
    let method = parts[0];
    let (path, query) = parts[1].split_once('?').unwrap_or((parts[1], ""));

    let mut content_length = 0usize;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
    }

    let mut body_bytes = vec![0u8; content_length.min(1024 * 1024)];
    if !body_bytes.is_empty() {
        let _ = reader.read_exact(&mut body_bytes);
    }
    let body: Value = serde_json::from_slice(&body_bytes).unwrap_or_else(|_| json!({}));

    if method == "OPTIONS" {
        return send_resp(&mut stream, "204 No Content", "text/plain", b"");
    }
    if method == "GET" && (path == "/" || path == "/index.html") {
        return send_resp(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            INDEX_HTML.as_bytes(),
        );
    }
    if method == "GET" && path == "/api/audio/output.wav" {
        let from_offset = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("from="))
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0)
            & !3;
        let wav = {
            let cap = state.captured_pcm.lock().unwrap();
            let slice = if from_offset < cap.len() {
                &cap[from_offset..]
            } else {
                &[][..]
            };
            build_wav_bytes(slice, 48_000, 2)
        };
        return send_resp(&mut stream, "200 OK", "audio/wav", &wav);
    }

    let resp = match (method, path) {
        ("GET", "/api/state") => {
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
            json!({
                "ok": true,
                "board_slug": state.board_slug,
                "board_name": state.board_name,
                "ring_leds": state.ring_leds,
                "logo_leds": state.logo_leds,
                "ambilight_leds": state.ambilight_leds,
                "aux_light_kind": state.peripherals.aux_light.aux_kind(),
                "has_clock": state.has_clock,
                "display_kind": disp_out.display_kind,
                "screen_width": disp_out.screen_width,
                "screen_height": disp_out.screen_height,
                "screen_pixels": disp_out.screen_pixels,
                "ring_pixels": ring_out.pixels,
                "logo_pixels": aux_px,
                "ring_trigger": ring_out.trigger,
                "clock_segments": disp_out.clock_segments,
                "amplifiers": audio_hw_out.amplifiers,
                "ultrasound": audio_hw_out.ultrasound,
                "doa_angle": state.doa_angle.load(Ordering::Relaxed),
                "tof_mm": tof_mm,
                "als_lux": state.als_lux.load(Ordering::Relaxed),
                "temp_c": (state.temp_milli_c.load(Ordering::Relaxed) as f32) / 1000.0,
                "volume_level": vol_level,
                "master_volume": master_vol,
                "mics_muted": state.mics_muted.load(Ordering::Relaxed),
                "forced_led_pattern": forced_led,
                "mic_rms": state.mic_rms.load(Ordering::Relaxed),
                "mic_burst_active": state.mic_burst_active.load(Ordering::Relaxed),
                "speaker_rms": state.speaker_rms.load(Ordering::Relaxed),
                "captured_audio_bytes": cap_len,
                "alice_state": state.alice_state().as_str(),
                "aliced_hijack_count": state.aliced_hijack_count.load(Ordering::Relaxed),
                "aliced_say_count": state.aliced_say_count.load(Ordering::Relaxed),
                "ipc_error_count": state.ipc_error_count.load(Ordering::Relaxed),
                "led_trigger_conflict_count": new_conflicts,
                "last_spoken_text": last_spoken,
                "recent_logs": logs,
                "maind_logs": maind_tail,
            })
        }
        ("POST", "/api/hw/button") => {
            let btn = body
                .get("button")
                .and_then(|v| v.as_str())
                .unwrap_or("action");
            let action = body
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("single");

            match btn {
                "mute" => {
                    let new_muted = !state.mics_muted.load(Ordering::SeqCst);
                    state.mics_muted.store(new_muted, Ordering::SeqCst);
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
                    state.log(format!("Inject button: {} ({})", btn, action));
                    if action == "single" && state.alice_state() != AliceState::Idle {
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
            let act_s = action.to_string();
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
            json!({
                "ok": true,
                "mics_muted": state.mics_muted.load(Ordering::Relaxed),
                "volume_level": state.volume_level.load(Ordering::Relaxed),
            })
        }
        ("POST", "/api/hw/volume") => {
            if let Some(lvl) = body.get("volume_level").and_then(|v| v.as_u64()) {
                state.set_volume_level(lvl as u32);
            }
            json!({
                "ok": true,
                "volume_level": state.volume_level.load(Ordering::Relaxed),
            })
        }
        ("POST", "/api/hw/led") => {
            let pat = body
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            *state.forced_led_pattern.lock().unwrap() = pat.clone();
            if pat.is_empty() {
                state.log("LED Pattern Tester -> Returned to automatic state mode");
            } else {
                state.log(format!("LED Pattern Tester -> Forced pattern '{}'", pat));
            }
            json!({"ok": true, "pattern": pat})
        }
        ("POST", "/api/ipc/test") => {
            let cmd = body
                .get("cmd")
                .and_then(|v| v.as_str())
                .unwrap_or("cancel_dialog");
            let text = body
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("Проверка локального синтеза речи Алисы")
                .trim();
            let mut alice_method = Vec::new();
            match cmd {
                "stop_tts" => {
                    alice_method.extend_from_slice(&[0x08, 14]);
                }
                "vocalize" => {
                    let mut voc_dir = Vec::new();
                    push_len_delimited(&mut voc_dir, 1, text.as_bytes());
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
            json!({"ok": sent, "cmd": cmd})
        }
        ("POST", "/api/hw/sensor") => {
            if let Some(doa) = body.get("doa_angle").and_then(|v| v.as_u64()) {
                state.doa_angle.store((doa % 360) as u32, Ordering::SeqCst);
            }
            if let Some(rms) = body.get("mic_rms").and_then(|v| v.as_u64()) {
                state.mic_rms.store(rms as u32, Ordering::SeqCst);
            }
            if let Some(als) = body.get("als_lux").and_then(|v| v.as_u64()) {
                state.als_lux.store(als as u32, Ordering::SeqCst);
                let _ = fs::write(
                    "/sys/bus/iio/devices/iio:device0/in_illuminance_raw",
                    als.to_string(),
                );
            }
            if let Some(tc) = body.get("temp_c").and_then(|v| v.as_f64()) {
                let milli = (tc * 1000.0) as u32;
                state.temp_milli_c.store(milli, Ordering::SeqCst);
                let _ = fs::write("/sys/class/thermal/thermal_zone0/temp", milli.to_string());
                let saradc = ((tc * 10.0) + 1795.0).round() as i32;
                let _ = fs::write(
                    "/sys/bus/iio/devices/iio:device0/in_voltage6_mean_raw",
                    saradc.to_string(),
                );
            }
            if body.get("swipe").and_then(|v| v.as_bool()).unwrap_or(false) {
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
            } else if let Some(tof) = body.get("tof_mm").and_then(|v| v.as_u64()) {
                let mm = tof as i32;
                state.tof_mm.store(mm as u32, Ordering::SeqCst);
                if let Some(ref mut f) = *state.tof_uinput.lock().unwrap() {
                    emit_compat_input_event(f, 3, 0x19, if mm < 500 { 0 } else { 4 });
                    emit_compat_input_event(f, 3, 0x12, mm);
                    emit_compat_input_event(f, 0, 0, 0);
                }
            }
            json!({"ok": true})
        }
        ("POST", "/api/hw/clock") => {
            let br = body
                .get("brightness")
                .and_then(|v| v.as_u64())
                .unwrap_or(255)
                .min(255) as u8;

            if let Some(segs_obj) = body.get("segments").and_then(|v| v.as_object()) {
                let replace_all = body
                    .get("replace")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(segs_obj.len() >= 28);
                let mut seg_map = std::collections::BTreeMap::new();
                for (k, v) in segs_obj {
                    if let Some(val) = v.as_u64() {
                        seg_map.insert(k.clone(), val.min(255) as u8);
                    }
                }
                state
                    .peripherals
                    .front_display
                    .write_display_segments(&seg_map, replace_all);
                state.log(format!(
                    "Set front display segments ({} keys, replace={})",
                    seg_map.len(),
                    replace_all
                ));
            } else if let Some(px_arr) = body.get("pixels").and_then(|v| v.as_array()) {
                let pixels: Vec<u8> = px_arr
                    .iter()
                    .map(|v| v.as_u64().unwrap_or(0).min(255) as u8)
                    .collect();
                state.peripherals.front_display.write_display_pixels(&pixels);
                state.log(format!(
                    "Set front display matrix pixels ({} px, active={})",
                    pixels.len(),
                    pixels.iter().filter(|&&x| x > 0).count()
                ));
            } else if let Some(led_str) = body.get("led_frame").and_then(|v| v.as_str()) {
                let ok = state
                    .peripherals
                    .front_display
                    .write_display_led_frame(led_str, br);
                state.log(format!(
                    "Load .led frame into front display (ok={}, brightness={})",
                    ok, br
                ));
            } else if let Some(scale_br) = body.get("scale_brightness").and_then(|v| v.as_u64()) {
                let target_br = scale_br.min(255) as u8;
                state
                    .peripherals
                    .front_display
                    .scale_active_brightness(target_br);
                state.log(format!(
                    "Scale active front display brightness -> PWM {}",
                    target_br
                ));
            } else {
                let text = body.get("text").and_then(|v| v.as_str()).unwrap_or("");
                state.peripherals.front_display.write_display_text(text, br);
                state.log(format!(
                    "Set front display ({}): \"{}\" (PWM {})",
                    state.peripherals.front_display.display_kind(),
                    text,
                    br
                ));
            }

            let disp_out = state.peripherals.front_display.read_display();
            json!({
                "ok": true,
                "display_kind": disp_out.display_kind,
                "clock_segments": disp_out.clock_segments,
                "screen_pixels": disp_out.screen_pixels,
            })
        }
        ("POST", "/api/voice/inject") => {
            if state.mics_muted.load(Ordering::Relaxed) {
                state.log("WARN Voice Input ignored: Microphones are MUTED (red LED ring active)");
                return send_resp(
                    &mut stream,
                    "200 OK",
                    "application/json; charset=utf-8",
                    &serde_json::to_vec(&json!({"ok": false, "error": "mics_muted"}))
                        .unwrap_or_default(),
                );
            }

            if let Some(doa) = body.get("doa_angle").and_then(|v| v.as_u64()) {
                state.doa_angle.store((doa % 360) as u32, Ordering::SeqCst);
            }
            if let Some(rms) = body.get("rms").and_then(|v| v.as_u64()) {
                state.mic_rms.store(rms as u32, Ordering::SeqCst);
            }
            let wake_only = body
                .get("wake_only")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let phrase = body
                .get("phrase")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();

            let seq = state.dialog_seq.fetch_add(1, Ordering::SeqCst) + 1;
            state.set_alice_state(AliceState::Listening);
            state.mic_burst_active.store(true, Ordering::SeqCst);
            state.append_maind_log("[sdk] onAliceStateChanged: LISTENING");
            if !wake_only && !phrase.is_empty() {
                state.log(format!(
                    "Voice Input -> DOA={}°, recognizedPhrase='{}' (1680ms hijack window started)",
                    state.doa_angle.load(Ordering::Relaxed),
                    phrase
                ));
                state.append_maind_log(&format!(
                    "[sdk] onRequestStateChanged: LISTENING, recognizedPhrase='{}'",
                    phrase
                ));
            } else {
                state.log(format!(
                    "Voice Wake Word -> DOA={}°",
                    state.doa_angle.load(Ordering::Relaxed)
                ));
            }

            let st = state.clone();
            let has_phrase = !wake_only && !phrase.is_empty();
            let phrase_for_tts = phrase.clone();
            let parsed_vol = parse_phrase_volume(&phrase);
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
            json!({"ok": true, "phrase": phrase, "dialog_seq": seq})
        }
        ("POST", "/api/audio/test_tone") => {
            state.log("Playing 440 Hz test tone to ALSA default -> snd-aloop");
            thread::spawn(|| {
                play_synthetic_tone_to_alsa(440.0, 350);
            });
            json!({"ok": true})
        }
        ("POST", "/api/audio/clear") => {
            state.captured_pcm.lock().unwrap().clear();
            state.speaker_rms.store(0, Ordering::Relaxed);
            state.log("Cleared captured speaker audio buffer");
            json!({"ok": true})
        }
        ("POST", "/api/hw/poweroff") => {
            state.log("Poweroff requested via HTTP API");
            thread::spawn(|| {
                thread::sleep(Duration::from_millis(100));
                let _ = Command::new("poweroff").arg("-f").status();
            });
            json!({"ok": true})
        }
        _ => json!({"ok": false, "error": "not_found"}),
    };

    let out = serde_json::to_vec(&resp).unwrap_or_default();
    send_resp(&mut stream, "200 OK", "application/json; charset=utf-8", &out)
}

fn send_resp(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let hdr = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nConnection: close\r\n\r\n",
        status,
        content_type,
        body.len()
    );
    stream.write_all(hdr.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    Ok(())
}
