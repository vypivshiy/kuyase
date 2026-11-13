use crate::audio::play_expressive_tts_to_alsa;
use crate::proto::{
    build_alice_idle_event_with_format, build_alice_listening_event_with_format,
    build_alice_voice_event_with_format, build_endpoint_storage_add_endpoint_packet,
    build_file_player_handshake_response_packet, build_yio_audio_frame_with_params,
    decode_quasar_io_control_packet, decode_quasar_remoting_packet_with_format,
    frame_quasar_packet, DecodedIoHubPacket, DecodedRemoting, QuasarMessage, QuasarWireFormat,
};
use crate::state::{AliceState, BridgeState};
pub use crate::glagol::spawn_glagold_ipc_server;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

pub fn spawn_yio_audio_server(state: Arc<BridgeState>) {
    thread::spawn(move || {
        let Ok(listener) = TcpListener::bind("127.0.0.1:9850") else {
            state.log("Port 127.0.0.1:9850 already bound by native maind");
            return;
        };
        state.log("Listening on 127.0.0.1:9850 (yio_audio virtual 4-mic array)");
        for stream in listener.incoming().flatten() {
            let st = state.clone();
            thread::spawn(move || {
                let mut s = stream;
                let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                let mut sub = [0u8; 64];
                let mut peer_wf = QuasarWireFormat::LegacyDirect;
                if let Ok(n) = s.read(&mut sub) {
                    if n >= 6 {
                        let declared_len =
                            u32::from_le_bytes([sub[0], sub[1], sub[2], sub[3]]) as usize;
                        if declared_len > 0 && 4 + declared_len <= n {
                            if let Ok(qmsg) = QuasarMessage::decode(&sub[4..4 + declared_len]) {
                                peer_wf = qmsg.wire_format;
                            }
                        }
                    }
                }
                let _ = s.set_write_timeout(Some(Duration::from_millis(1000)));
                let mut seq = 0u64;
                let mut next_frame = Instant::now();
                loop {
                    let audio_cfg = st.audio_config();
                    let doa = st.doa_angle.load(Ordering::Relaxed) as f32;
                    let muted = st.mics_muted.load(Ordering::Relaxed);
                    let rms = if muted {
                        0
                    } else {
                        st.mic_rms.load(Ordering::Relaxed) as u16
                    };
                    let n_samples = audio_cfg.mic_frame_samples();
                    let frame = build_yio_audio_frame_with_params(
                        doa,
                        rms,
                        seq,
                        audio_cfg.mic_sample_rate,
                        n_samples,
                        peer_wf,
                    );
                    if s.write_all(&frame).is_err() {
                        break;
                    }
                    seq = seq.wrapping_add(1);
                    let frame_interval =
                        Duration::from_millis(audio_cfg.mic_send_interval_ms as u64);
                    next_frame += frame_interval;
                    let now = Instant::now();
                    if audio_cfg.mic_lag_cap
                        && now.saturating_duration_since(next_frame) > frame_interval
                    {
                        next_frame = now;
                    } else {
                        thread::sleep(next_frame.saturating_duration_since(now));
                    }
                }
            });
        }
    });
}

/// Client subscribed to `aliced`'s event stream (Luna's `/speech/next` or `q cli`).
/// Each subscriber tracks its negotiated `QuasarWireFormat` (`LegacyDirect` vs `Envelope252`).
struct SpeechSubscriber {
    id: u64,
    stream: std::net::TcpStream,
    format: QuasarWireFormat,
}

static SPEECH_WRITERS: OnceLock<Mutex<Vec<SpeechSubscriber>>> = OnceLock::new();
static NEXT_SPEECH_CLIENT_ID: AtomicU64 = AtomicU64::new(1);

fn speech_writers() -> &'static Mutex<Vec<SpeechSubscriber>> {
    SPEECH_WRITERS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn speech_subscribers() -> usize {
    speech_writers().lock().map(|w| w.len()).unwrap_or(0)
}

fn update_client_wire_format(client_id: u64, wf: QuasarWireFormat) {
    if let Ok(mut clients) = speech_writers().lock() {
        for sub in clients.iter_mut() {
            if sub.id == client_id {
                sub.format = wf;
            }
        }
    }
}

pub fn broadcast_alice_listening_state() {
    let Ok(mut clients) = speech_writers().lock() else {
        return;
    };
    clients.retain_mut(|sub| {
        let pkt = frame_quasar_packet(&build_alice_listening_event_with_format(sub.format));
        sub.stream.write_all(&pkt).is_ok()
    });
}

pub fn broadcast_alice_idle_packet() {
    let Ok(mut clients) = speech_writers().lock() else {
        return;
    };
    clients.retain_mut(|sub| {
        let pkt = frame_quasar_packet(&build_alice_idle_event_with_format(sub.format));
        sub.stream.write_all(&pkt).is_ok()
    });
}

pub fn broadcast_alice_speech(transcript: &str, request_id: &str) {
    // LISTENING and the recognition travel in one write: Luna marks the first
    // frame of a fresh IPC connection as replayed state (initial snapshot) and
    // the plugin ignores those, so the recognition must not be that frame.
    let Ok(mut clients) = speech_writers().lock() else {
        return;
    };
    clients.retain_mut(|sub| {
        let mut buf = frame_quasar_packet(&build_alice_listening_event_with_format(sub.format));
        buf.extend(frame_quasar_packet(&build_alice_voice_event_with_format(
            transcript,
            request_id,
            sub.format,
        )));
        sub.stream.write_all(&buf).is_ok()
    });
}

pub fn spawn_aliced_ipc_server(state: Arc<BridgeState>) {
    thread::spawn(move || {
        let Ok(listener) = TcpListener::bind("127.0.0.1:9875") else {
            state.log("Port 127.0.0.1:9875 already bound by native maind");
            return;
        };
        state.log("Listening on 127.0.0.1:9875 (aliced QuasarMessage.remoting IPC)");
        for stream in listener.incoming().flatten() {
            let st = state.clone();
            let client_id = NEXT_SPEECH_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
            let initial_wf = st.wire_format();
            if let Ok(writer) = stream.try_clone() {
                let _ = writer.set_write_timeout(Some(Duration::from_millis(200)));
                if let Ok(mut clients) = speech_writers().lock() {
                    clients.push(SpeechSubscriber {
                        id: client_id,
                        stream: writer,
                        format: initial_wf,
                    });
                }
            }
            thread::spawn(move || {
                let mut s = stream;
                let _ = s.set_write_timeout(Some(Duration::from_millis(200)));
                let mut len_buf = [0u8; 4];
                while s.read_exact(&mut len_buf).is_ok() {
                    let len = u32::from_le_bytes(len_buf) as usize;
                    if len == 0 || len > 65536 {
                        break;
                    }
                    let mut payload = vec![0u8; len];
                    if s.read_exact(&mut payload).is_err() {
                        break;
                    }
                    match decode_quasar_remoting_packet_with_format(&payload) {
                        Ok((decoded, peer_wf)) => {
                            st.set_wire_format(peer_wf);
                            update_client_wire_format(client_id, peer_wf);
                            match decoded {
                                DecodedRemoting::CancelDialog => {
                                    st.aliced_hijack_count.fetch_add(1, Ordering::SeqCst);
                                    let cur_seq = st.dialog_seq.load(Ordering::SeqCst);
                                    st.hijacked_seq.store(cur_seq, Ordering::SeqCst);
                                    st.set_alice_state(AliceState::Idle);
                                    st.append_maind_log("[sdk] onAliceStateChanged: IDLE");
                                    st.log(
                                        "IPC :9875 -> CANCEL_DIALOG_AND_CLEAR_QUEUE (Dialog Hijack OK)",
                                    );
                                }
                                DecodedRemoting::StopTts => {
                                    st.aliced_hijack_count.fetch_add(1, Ordering::SeqCst);
                                    let cur_seq = st.dialog_seq.load(Ordering::SeqCst);
                                    st.hijacked_seq.store(cur_seq, Ordering::SeqCst);
                                    st.set_alice_state(AliceState::Idle);
                                    st.append_maind_log("[sdk] onAliceStateChanged: IDLE");
                                    st.log("IPC :9875 -> STOP_TTS (14)");
                                }
                                DecodedRemoting::StartConversation => {
                                    let _ = st.dialog_seq.fetch_add(1, Ordering::SeqCst) + 1;
                                    st.set_alice_state(AliceState::Listening);
                                    st.append_maind_log("[sdk] onAliceStateChanged: LISTENING");
                                    broadcast_alice_listening_state();
                                    st.log(
                                        "IPC :9875 -> START_CONVERSATION (2) -> AliceState::Listening",
                                    );
                                }
                                DecodedRemoting::ToggleConversation => {
                                    let next_state = match st.alice_state() {
                                        AliceState::Idle => AliceState::Listening,
                                        AliceState::Listening | AliceState::Speaking => {
                                            AliceState::Idle
                                        }
                                    };
                                    let cur_seq = st.dialog_seq.fetch_add(1, Ordering::SeqCst) + 1;
                                    if next_state == AliceState::Idle {
                                        st.hijacked_seq.store(cur_seq, Ordering::SeqCst);
                                    }
                                    st.set_alice_state(next_state);
                                    st.append_maind_log(&format!(
                                        "[sdk] onAliceStateChanged: {}",
                                        next_state.as_str()
                                    ));
                                    if next_state == AliceState::Listening {
                                        broadcast_alice_listening_state();
                                    } else {
                                        broadcast_alice_idle_packet();
                                    }
                                    st.log(format!(
                                        "IPC :9875 -> TOGGLE_CONVERSATION (4) -> {}",
                                        next_state.as_str()
                                    ));
                                }
                                DecodedRemoting::StartRequestText(text) => {
                                    st.log(format!(
                                        "IPC :9875 -> START_REQUEST text_input: \"{}\"",
                                        text
                                    ));
                                    let _ = crate::voice_dialog::handle_inject_voice(
                                        &st, text, None, None, false,
                                    );
                                }
                                DecodedRemoting::StartVocalize(text) => {
                                    st.aliced_say_count.fetch_add(1, Ordering::SeqCst);
                                    let tts_seq = st.dialog_seq.fetch_add(1, Ordering::SeqCst) + 1;
                                    st.hijacked_seq.store(tts_seq, Ordering::SeqCst);
                                    *st.last_spoken_text.lock().unwrap() = text.clone();
                                    st.set_alice_state(AliceState::Speaking);
                                    st.log(format!("IPC :9875 -> START_VOCALIZE: \"{}\"", text));
                                    st.append_maind_log("[sdk] onAliceStateChanged: SPEAKING");
                                    st.append_maind_log("[sdk] onTtsStarted: request_id=emu");
                                    let st_tts = st.clone();
                                    thread::spawn(move || {
                                        let _ = play_expressive_tts_to_alsa(&st_tts, &text, tts_seq);
                                        if st_tts.dialog_seq.load(Ordering::SeqCst) == tts_seq {
                                            st_tts.set_alice_state(AliceState::Idle);
                                            st_tts.append_maind_log(
                                                "[sdk] onTtsCompleted: request_id=emu",
                                            );
                                            st_tts.append_maind_log(
                                                "[sdk] onAliceStateChanged: IDLE",
                                            );
                                        }
                                    });
                                }
                                DecodedRemoting::AliceMethod(m) => {
                                    st.log(format!("IPC :9875 -> AliceCapabilityMethod ({})", m));
                                }
                                DecodedRemoting::PlaybackControl(m) => {
                                    st.handle_playback_control(m);
                                }
                                DecodedRemoting::DeviceState { mics_muted } => {
                                    if let Some(muted) = mics_muted {
                                        st.set_mics_muted(muted);
                                        st.log(format!(
                                            "IPC :9875 -> DeviceStateCapability (mics_muted={})",
                                            muted
                                        ));
                                    }
                                }
                                DecodedRemoting::OtherCapability {
                                    object_id,
                                    method_field,
                                } => {
                                    // Reply to q cli / SDK capability handshakes on :9875:
                                    // 1. EndpointStorage_ (field 1): send ADD_ENDPOINT with
                                    //    storage_name="aliced" so PlaybackControlCapabilityProxy
                                    //    and all other proxies mark their remote endpoint connected.
                                    // 2. FilePlayerCapability (field 12): send handshake_response
                                    //    (field 17.2) in the peer's wire format.
                                    if object_id == "EndpointStorage_" && method_field == 1 {
                                        let reply = build_endpoint_storage_add_endpoint_packet(
                                            "EMU0001", peer_wf,
                                        );
                                        let _ = s.write_all(&reply);
                                        let _ = s.flush();
                                    } else if object_id == "FilePlayerCapability"
                                        && method_field == 12
                                    {
                                        let reply =
                                            build_file_player_handshake_response_packet(peer_wf);
                                        let _ = s.write_all(&reply);
                                        let _ = s.flush();
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            st.ipc_error_count.fetch_add(1, Ordering::SeqCst);
                            st.log(format!(
                                "ERROR IPC :9875 -> Invalid Protobuf packet ({}B): {}",
                                len, err
                            ));
                        }
                    }
                }
                if let Ok(mut clients) = speech_writers().lock() {
                    clients.retain(|sub| sub.id != client_id);
                }
            });
        }
    });
}

pub fn spawn_iohub_services_ipc_server(state: Arc<BridgeState>) {
    thread::spawn(move || {
        let Ok(listener) = TcpListener::bind("127.0.0.1:9903") else {
            state.log("Port 127.0.0.1:9903 already bound by native maind");
            return;
        };
        state.log("Listening on 127.0.0.1:9903 (iohub_services strict IOControlCommand IPC)");
        for stream in listener.incoming().flatten() {
            let st = state.clone();
            thread::spawn(move || {
                let mut s = stream;
                let mut len_buf = [0u8; 4];
                while s.read_exact(&mut len_buf).is_ok() {
                    let len = u32::from_le_bytes(len_buf) as usize;
                    if len == 0 || len > 65536 {
                        break;
                    }
                    let mut payload = vec![0u8; len];
                    if s.read_exact(&mut payload).is_err() {
                        break;
                    }
                    match decode_quasar_io_control_packet(&payload) {
                        Ok(DecodedIoHubPacket::ExternalDirectives(directives_json)) => {
                            st.log(format!(
                                "IPC :9903 (iohub_services) -> external_directives: {}",
                                directives_json
                            ));
                        }
                        Ok(DecodedIoHubPacket::IoControlField(fnum)) => {
                            st.log(format!(
                                "IPC :9903 (iohub_services) -> IOControlCommand field {}",
                                fnum
                            ));
                        }
                        Ok(
                            DecodedIoHubPacket::RetrieveStateHandshake
                            | DecodedIoHubPacket::ServiceReady(_)
                            | DecodedIoHubPacket::IoEvent,
                        ) => {
                            // Valid iohub_services subscription / readiness packet
                        }
                        Err(err) => {
                            st.ipc_error_count.fetch_add(1, Ordering::SeqCst);
                            st.log(format!(
                                "ERROR IPC :9903 -> Invalid Protobuf packet ({}B): {}",
                                len, err
                            ));
                        }
                    }
                }
            });
        }
    });
}
