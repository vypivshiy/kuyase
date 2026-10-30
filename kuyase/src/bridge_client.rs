use protocol::{
    build_wav_bytes, read_guest_packet, write_host_command, BridgeStateSnapshot, CommandAckPayload,
    GuestPacket, HostCommand,
};
use std::collections::VecDeque;
use std::io::BufReader;
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

struct SharedState {
    latest: BridgeStateSnapshot,
    seq: u64,
}

pub struct BridgeClient {
    addr: SocketAddr,
    writer: Mutex<TcpStream>,
    state: Arc<(Mutex<SharedState>, Condvar)>,
    ack_queue: Arc<(Mutex<VecDeque<CommandAckPayload>>, Condvar)>,
    captured_pcm: Arc<Mutex<Vec<u8>>>,
    audio_sink: Arc<Mutex<Option<mpsc::Sender<Vec<u8>>>>>,
    connected: Arc<AtomicBool>,
}

impl BridgeClient {
    pub fn connect_with_timeout(addr: SocketAddr, timeout: Duration) -> Result<Self, String> {
        let deadline = Instant::now() + timeout;
        let mut last_err = format!("timed out waiting for bridge at {}", addr);

        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let conn_timeout = remaining.clamp(Duration::from_millis(500), Duration::from_millis(2500));

            let stream = match TcpStream::connect_timeout(&addr, conn_timeout) {
                Ok(s) => s,
                Err(e) => {
                    last_err = format!("connect to {} failed: {}", addr, e);
                    thread::sleep(Duration::from_millis(180));
                    continue;
                }
            };

            let _ = stream.set_nodelay(true);
            let _ = stream.set_read_timeout(Some(Duration::from_millis(2000)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(2500)));

            let Ok(writer_stream) = stream.try_clone() else {
                thread::sleep(Duration::from_millis(150));
                continue;
            };
            let mut reader = BufReader::new(stream);

            // Wait for the initial StateSnapshot pushed on connect by qemu-hw-bridge
            match read_guest_packet(&mut reader) {
                Ok(GuestPacket::StateSnapshot(initial_state)) => {
                    let _ = reader.get_mut().set_read_timeout(None);
                    let state = Arc::new((
                        Mutex::new(SharedState {
                            latest: initial_state,
                            seq: 1,
                        }),
                        Condvar::new(),
                    ));
                    let ack_queue = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
                    let captured_pcm = Arc::new(Mutex::new(Vec::new()));
                    let audio_sink: Arc<Mutex<Option<mpsc::Sender<Vec<u8>>>>> =
                        Arc::new(Mutex::new(None));
                    let connected = Arc::new(AtomicBool::new(true));

                    let state_bg = state.clone();
                    let ack_bg = ack_queue.clone();
                    let pcm_bg = captured_pcm.clone();
                    let sink_bg = audio_sink.clone();
                    let conn_bg = connected.clone();

                    thread::spawn(move || {
                        while conn_bg.load(Ordering::Relaxed) {
                            match read_guest_packet(&mut reader) {
                                Ok(GuestPacket::StateSnapshot(snap)) => {
                                    let (lock, cv) = &*state_bg;
                                    let mut guard = lock.lock().unwrap();
                                    guard.latest = snap;
                                    guard.seq = guard.seq.wrapping_add(1);
                                    cv.notify_all();
                                }
                                Ok(GuestPacket::PcmAudioChunk(chunk)) => {
                                    if !chunk.is_empty() {
                                        {
                                            let mut cap = pcm_bg.lock().unwrap();
                                            if cap.len() < 8 * 1024 * 1024 {
                                                cap.extend_from_slice(&chunk);
                                            }
                                        }
                                        if let Some(ref tx) = *sink_bg.lock().unwrap() {
                                            let _ = tx.send(chunk);
                                        }
                                    }
                                }
                                Ok(GuestPacket::CommandAck { ok, error, message }) => {
                                    let (lock, cv) = &*ack_bg;
                                    let mut guard = lock.lock().unwrap();
                                    guard.push_back(CommandAckPayload { ok, error, message });
                                    cv.notify_all();
                                }
                                Err(_) => break,
                            }
                        }
                        conn_bg.store(false, Ordering::SeqCst);
                        state_bg.1.notify_all();
                        ack_bg.1.notify_all();
                    });

                    return Ok(Self {
                        addr,
                        writer: Mutex::new(writer_stream),
                        state,
                        ack_queue,
                        captured_pcm,
                        audio_sink,
                        connected,
                    });
                }
                Ok(_) => {
                    last_err = "unexpected initial packet from bridge (expected StateSnapshot)"
                        .to_string();
                }
                Err(e) => {
                    last_err = format!("waiting for initial StateSnapshot failed: {}", e);
                }
            }

            thread::sleep(Duration::from_millis(180));
        }

        Err(last_err)
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn latest_state(&self) -> BridgeStateSnapshot {
        self.state.0.lock().unwrap().latest.clone()
    }

    pub fn send_command(&self, cmd: &HostCommand) -> Result<CommandAckPayload, String> {
        if !self.is_connected() {
            return Err("bridge client is disconnected".to_string());
        }

        if matches!(cmd, HostCommand::ClearAudio) {
            self.captured_pcm.lock().unwrap().clear();
        }

        let seq_before = self.state.0.lock().unwrap().seq;
        {
            let mut w = self.writer.lock().unwrap();
            // Clear any stale acks before sending command
            self.ack_queue.0.lock().unwrap().clear();
            write_host_command(&mut *w, cmd)
                .map_err(|e| format!("failed to send HostCommand: {}", e))?;
        }

        // Wait for CommandAck
        let ack_deadline = Instant::now() + Duration::from_secs(8);
        let ack = {
            let (lock, cv) = &*self.ack_queue;
            let mut guard = lock.lock().unwrap();
            loop {
                if let Some(a) = guard.pop_front() {
                    break a;
                }
                if !self.is_connected() {
                    return Err("bridge disconnected while waiting for CommandAck".to_string());
                }
                let rem = ack_deadline.saturating_duration_since(Instant::now());
                if rem.is_zero() {
                    return Err("timed out waiting for CommandAck from bridge".to_string());
                }
                let (next_guard, _) = cv.wait_timeout(guard, rem).unwrap();
                guard = next_guard;
            }
        };

        // Wait up to 600ms for the post-command StateSnapshot that immediately follows CommandAck
        if !matches!(cmd, HostCommand::PowerOff) {
            let snap_deadline = Instant::now() + Duration::from_millis(600);
            let (lock, cv) = &*self.state;
            let mut guard = lock.lock().unwrap();
            while guard.seq == seq_before && self.is_connected() {
                let rem = snap_deadline.saturating_duration_since(Instant::now());
                if rem.is_zero() {
                    break;
                }
                let (next_guard, _) = cv.wait_timeout(guard, rem).unwrap();
                guard = next_guard;
            }
        }

        Ok(ack)
    }

    pub fn request_state(&self) -> Result<BridgeStateSnapshot, String> {
        let _ = self.send_command(&HostCommand::RequestState)?;
        Ok(self.latest_state())
    }

    pub fn captured_pcm_bytes(&self) -> Vec<u8> {
        self.captured_pcm.lock().unwrap().clone()
    }

    pub fn captured_wav_bytes(&self) -> Vec<u8> {
        let pcm = self.captured_pcm_bytes();
        build_wav_bytes(&pcm, 48_000, 2)
    }

    pub fn clear_captured_pcm(&self) {
        self.captured_pcm.lock().unwrap().clear();
    }

    pub fn set_audio_chunk_sink(&self, sink: Option<mpsc::Sender<Vec<u8>>>) {
        *self.audio_sink.lock().unwrap() = sink;
    }

    pub fn close(&self) {
        self.connected.store(false, Ordering::SeqCst);
        if let Ok(w) = self.writer.lock() {
            let _ = w.shutdown(Shutdown::Both);
        }
    }
}

impl Drop for BridgeClient {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{read_host_command, write_guest_packet};
    use std::net::TcpListener;

    #[test]
    fn test_bridge_client_duplex_handshake_and_commands() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut snap = BridgeStateSnapshot::default();
            snap.board_slug = "yandexmini2".to_string();
            snap.volume_level = 5;
            write_guest_packet(&mut stream, &GuestPacket::StateSnapshot(snap.clone())).unwrap();

            // Stream a PCM audio chunk
            write_guest_packet(&mut stream, &GuestPacket::PcmAudioChunk(vec![0x10; 160])).unwrap();

            // Read a command from host
            let cmd = read_host_command(&mut stream).unwrap();
            assert_eq!(cmd, HostCommand::SetVolume { volume_level: 9 });

            write_guest_packet(
                &mut stream,
                &GuestPacket::CommandAck {
                    ok: true,
                    error: None,
                    message: None,
                },
            )
            .unwrap();
            snap.volume_level = 9;
            snap.master_volume = 9000;
            write_guest_packet(&mut stream, &GuestPacket::StateSnapshot(snap)).unwrap();
        });

        let client = BridgeClient::connect_with_timeout(addr, Duration::from_secs(3)).unwrap();
        assert_eq!(client.latest_state().board_slug, "yandexmini2");
        assert_eq!(client.latest_state().volume_level, 5);

        let ack = client
            .send_command(&HostCommand::SetVolume { volume_level: 9 })
            .unwrap();
        assert!(ack.ok);
        assert_eq!(client.latest_state().volume_level, 9);
        assert_eq!(client.captured_pcm_bytes().len(), 160);

        client.close();
        server.join().unwrap();
    }
}
