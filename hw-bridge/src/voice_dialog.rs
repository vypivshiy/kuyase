use crate::audio::play_expressive_tts_to_alsa;
use crate::ipc::{broadcast_alice_speech, speech_subscribers};
use crate::proto::{frame_quasar_packet, push_len_delimited};
use crate::state::{AliceState, BridgeState};
use protocol::GuestPacket;
use std::io::Write;
use std::net::TcpStream;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const AIRPLAY_PRIVATE_BUS_PATH: &str = "/run/station-airplay/private-bus";

pub fn parse_phrase_volume(phrase: &str) -> Option<u32> {
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

pub fn push_volume_to_luna(level: u32) {
    let percent = level.min(10) * 10;

    let luna_addr = std::net::SocketAddr::from(([127, 0, 0, 1], 8099));
    if let Ok(mut s) = TcpStream::connect_timeout(&luna_addr, Duration::from_millis(80)) {
        let _ = s.set_write_timeout(Some(Duration::from_millis(250)));
        let body = format!("{{\"percent\":{percent}}}");
        let req = format!(
            "POST /api/volume/set HTTP/1.0\r\nHost: 127.0.0.1:8099\r\nContent-Type: application/json\r\nX-Luna-Local: 1\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = s.write_all(req.as_bytes());
    }
}

pub fn set_host_volume(state: &BridgeState, level: u32) {
    state.set_volume_level(level);
    push_volume_to_luna(level);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportCommand {
    Pause,
    Play,
    Next,
    Previous,
}

impl TransportCommand {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pause => "Pause",
            Self::Play => "Play",
            Self::Next => "Next",
            Self::Previous => "Previous",
        }
    }
}

pub fn parse_phrase_transport(phrase: &str) -> Option<TransportCommand> {
    let p = phrase.to_lowercase();
    if p.contains("пауз") || p.contains("pause") || p.contains("стоп") || p.contains("останов") {
        Some(TransportCommand::Pause)
    } else if p.contains("продолж")
        || p.contains("resume")
        || p.contains("играй")
        || p.contains("плей")
        || p.contains("play")
    {
        Some(TransportCommand::Play)
    } else if p.contains("дальше") || p.contains("следующ") || p.contains("next") {
        Some(TransportCommand::Next)
    } else if p.contains("назад") || p.contains("предыдущ") || p.contains("previous") {
        Some(TransportCommand::Previous)
    } else {
        None
    }
}

pub fn send_airplay_transport(command: TransportCommand) -> Result<(), String> {
    let method = format!(
        "org.gnome.ShairportSync.RemoteControl.{}",
        command.as_str()
    );
    match Command::new("dbus-send")
        .arg(format!("--bus=unix:path={AIRPLAY_PRIVATE_BUS_PATH}"))
        .arg("--print-reply")
        .arg("--dest=org.gnome.ShairportSync")
        .arg("/org/gnome/ShairportSync")
        .arg(method)
        .output()
    {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(format!(
            "dbus-send failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(error) => Err(format!("dbus-send failed: {error}")),
    }
}

pub fn send_local_aliced_packet(payload: &[u8]) -> bool {
    let Ok(mut s) = TcpStream::connect("127.0.0.1:9875") else {
        return false;
    };
    let _ = s.set_nodelay(true);
    let framed = frame_quasar_packet(payload);
    let ok = s.write_all(&framed).is_ok();
    let _ = s.flush();
    ok
}

pub fn handle_inject_voice(
    state: &Arc<BridgeState>,
    phrase: String,
    doa_angle: Option<u32>,
    rms: Option<u32>,
    wake_only: bool,
) -> GuestPacket {
    if state.mics_muted.load(Ordering::Relaxed) {
        state.log("WARN Voice Input ignored: Microphones are MUTED (red LED ring active)");
        return GuestPacket::CommandAck {
            ok: false,
            error: Some("mics_muted".to_string()),
            message: None,
        };
    }

    let audio_cfg = state.audio_config();
    if let Some(doa) = doa_angle {
        state.doa_angle.store(doa % 360, Ordering::SeqCst);
    }
    if let Some(r) = rms {
        state.mic_rms.store(r, Ordering::SeqCst);
    } else {
        state
            .mic_rms
            .store(audio_cfg.mic_burst_rms, Ordering::SeqCst);
    }
    let phrase_clean = phrase.trim().to_string();

    let seq = state.dialog_seq.fetch_add(1, Ordering::SeqCst) + 1;
    state.set_alice_state(AliceState::Listening);
    state.mic_burst_active.store(true, Ordering::SeqCst);
    state.append_maind_log("[sdk] onAliceStateChanged: LISTENING");
    if !wake_only && !phrase_clean.is_empty() {
        state.log(format!(
            "Voice Input -> DOA={}°, recognizedPhrase='{}' ({}ms hijack window started)",
            state.doa_angle.load(Ordering::Relaxed),
            phrase_clean,
            audio_cfg.hijack_window_ms
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

    if !wake_only && !phrase_clean.is_empty() {
        let request_id = format!("emu-{seq}");
        let transcript = phrase_clean.clone();
        let st_event = state.clone();
        let rec_delay_ms = audio_cfg.recognition_delay_ms as u64;
        thread::spawn(move || {
            if rec_delay_ms > 0 {
                thread::sleep(Duration::from_millis(rec_delay_ms));
            }
            broadcast_alice_speech(&transcript, &request_id);
            st_event.log(format!(
                "Voice recognition -> aliced event request_id={} ({} subscriber(s))",
                request_id,
                speech_subscribers()
            ));
        });
    }

    let st = state.clone();
    let has_phrase = !wake_only && !phrase_clean.is_empty();
    let phrase_for_tts = phrase_clean.clone();
    let parsed_vol = parse_phrase_volume(&phrase_clean);
    let parsed_transport = parse_phrase_transport(&phrase_clean);
    let burst_ms = audio_cfg.burst_duration_ms as u64;
    let hijack_ms = audio_cfg.hijack_window_ms as u64;
    let post_burst_wait_ms = hijack_ms.saturating_sub(burst_ms);
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(burst_ms));
        if st.dialog_seq.load(Ordering::SeqCst) == seq {
            st.mic_burst_active.store(false, Ordering::SeqCst);
            if !st.mics_muted.load(Ordering::Relaxed) {
                st.mic_rms
                    .store(st.audio_config().mic_idle_rms, Ordering::SeqCst);
            }
        }

        if has_phrase {
            if post_burst_wait_ms > 0 {
                thread::sleep(Duration::from_millis(post_burst_wait_ms));
            }
            if st.dialog_seq.load(Ordering::SeqCst) == seq
                && st.hijacked_seq.load(Ordering::SeqCst) < seq
            {
                if let Some(v) = parsed_vol {
                    set_host_volume(&st, v);
                }
                if let Some(cmd) = parsed_transport {
                    if Path::new(AIRPLAY_PRIVATE_BUS_PATH).exists() {
                        let how = if speech_subscribers() == 0 {
                            "direct, no aliced subscriber"
                        } else {
                            "direct, plugin did not claim"
                        };
                        match send_airplay_transport(cmd) {
                            Ok(()) => {
                                st.log(format!(
                                    "Voice transport -> AirPlay {} ({}, dbus-send -> ok)",
                                    cmd.as_str(),
                                    how
                                ));
                                st.log(format!(
                                    "Dialog Hijack window ({}ms) expired without CANCEL_DIALOG -> transport handled silently",
                                    hijack_ms
                                ));
                                st.set_alice_state(AliceState::Idle);
                                st.append_maind_log("[sdk] onAliceStateChanged: IDLE");
                                return;
                            }
                            Err(err) => {
                                st.log(format!(
                                    "Voice transport -> AirPlay {} ({}, {})",
                                    cmd.as_str(),
                                    how,
                                    err
                                ));
                            }
                        }
                    }
                }
                let auto_reply = format!("Выполняю: {}", phrase_for_tts);
                *st.last_spoken_text.lock().unwrap() = auto_reply.clone();
                st.set_alice_state(AliceState::Speaking);
                st.log(format!(
                    "Dialog Hijack window ({}ms) expired without CANCEL_DIALOG -> native Alice SPEAKING",
                    hijack_ms
                ));
                st.append_maind_log("[sdk] onAliceStateChanged: SPEAKING");
                st.append_maind_log("[sdk] onTtsStarted: request_id=alice_cloud_auto");
                let _ = play_expressive_tts_to_alsa(&st, &auto_reply, seq);
                if st.dialog_seq.load(Ordering::SeqCst) == seq {
                    st.set_alice_state(AliceState::Idle);
                    st.append_maind_log("[sdk] onTtsCompleted: request_id=alice_cloud_auto");
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

pub fn handle_ipc_test(cmd: String, text: Option<String>) -> GuestPacket {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_phrase_volume_and_transport() {
        assert_eq!(parse_phrase_volume("поставь громкость 4"), Some(4));
        assert_eq!(parse_phrase_volume("volume 9"), Some(9));
        assert_eq!(parse_phrase_volume("volume 15"), Some(10));
        assert_eq!(parse_phrase_volume("включи ночной режим"), None);

        assert_eq!(
            parse_phrase_transport("поставь на паузу"),
            Some(TransportCommand::Pause)
        );
        assert_eq!(
            parse_phrase_transport("продолжи воспроизведение"),
            Some(TransportCommand::Play)
        );
        assert_eq!(
            parse_phrase_transport("следующий трек"),
            Some(TransportCommand::Next)
        );
        assert_eq!(
            parse_phrase_transport("предыдущий трек"),
            Some(TransportCommand::Previous)
        );
        assert_eq!(parse_phrase_transport("какая погода"), None);
    }
}
