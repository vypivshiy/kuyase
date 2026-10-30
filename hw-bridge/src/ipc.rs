use crate::audio::play_expressive_tts_to_alsa;
use crate::proto::{
    build_yio_audio_frame, decode_quasar_io_control_packet, decode_quasar_remoting_packet,
    DecodedIoHubPacket, DecodedRemoting,
};
use crate::state::{AliceState, BridgeState};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

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
                let mut sub = [0u8; 16];
                let _ = s.read(&mut sub);
                let _ = s.set_write_timeout(Some(Duration::from_millis(1000)));
                let mut seq = 0u64;
                loop {
                    let doa = st.doa_angle.load(Ordering::Relaxed) as f32;
                    let muted = st.mics_muted.load(Ordering::Relaxed);
                    let burst = st.mic_burst_active.load(Ordering::Relaxed);
                    let rms = if muted {
                        0
                    } else if burst {
                        st.mic_rms.load(Ordering::Relaxed).max(2400) as u16
                    } else {
                        st.mic_rms.load(Ordering::Relaxed) as u16
                    };
                    let frame = build_yio_audio_frame(doa, rms, seq);
                    if s.write_all(&frame).is_err() {
                        break;
                    }
                    seq = seq.wrapping_add(1);
                    thread::sleep(Duration::from_millis(40));
                }
            });
        }
    });
}

pub fn spawn_aliced_ipc_server(state: Arc<BridgeState>) {
    thread::spawn(move || {
        let Ok(listener) = TcpListener::bind("127.0.0.1:9875") else {
            state.log("Port 127.0.0.1:9875 already bound by native maind");
            return;
        };
        state.log("Listening on 127.0.0.1:9875 (aliced strict QuasarMessage.remoting IPC)");
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
                    match decode_quasar_remoting_packet(&payload) {
                        Ok(DecodedRemoting::CancelDialog) => {
                            st.aliced_hijack_count.fetch_add(1, Ordering::SeqCst);
                            let cur_seq = st.dialog_seq.load(Ordering::SeqCst);
                            st.hijacked_seq.store(cur_seq, Ordering::SeqCst);
                            st.set_alice_state(AliceState::Idle);
                            st.append_maind_log("[sdk] onAliceStateChanged: IDLE");
                            st.log("IPC :9875 -> CANCEL_DIALOG_AND_CLEAR_QUEUE (Dialog Hijack OK)");
                        }
                        Ok(DecodedRemoting::StopTts) => {
                            st.aliced_hijack_count.fetch_add(1, Ordering::SeqCst);
                            let cur_seq = st.dialog_seq.load(Ordering::SeqCst);
                            st.hijacked_seq.store(cur_seq, Ordering::SeqCst);
                            st.set_alice_state(AliceState::Idle);
                            st.append_maind_log("[sdk] onAliceStateChanged: IDLE");
                            st.log("IPC :9875 -> STOP_TTS (14)");
                        }
                        Ok(DecodedRemoting::StartVocalize(text)) => {
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
                                    st_tts.append_maind_log("[sdk] onTtsCompleted: request_id=emu");
                                    st_tts.append_maind_log("[sdk] onAliceStateChanged: IDLE");
                                }
                            });
                        }
                        Ok(DecodedRemoting::AliceMethod(m)) => {
                            st.log(format!("IPC :9875 -> AliceCapabilityMethod ({})", m));
                        }
                        Ok(DecodedRemoting::PlaybackControl(m)) => {
                            st.log(format!(
                                "IPC :9875 -> PlaybackControlCapabilityMethod ({})",
                                m
                            ));
                        }
                        Ok(DecodedRemoting::DeviceState { mics_muted }) => {
                            if let Some(muted) = mics_muted {
                                st.mics_muted.store(muted, Ordering::SeqCst);
                                st.mic_rms.store(if muted { 0 } else { 60 }, Ordering::SeqCst);
                                st.log(format!(
                                    "IPC :9875 -> DeviceStateCapability (mics_muted={})",
                                    muted
                                ));
                            }
                        }
                        Ok(DecodedRemoting::OtherCapability { .. }) => {
                            // Valid internal Quasar Remoting sync (EndpointStorage_, FilePlayerCapability, etc.)
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
