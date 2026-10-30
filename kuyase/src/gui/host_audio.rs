use crate::bridge_client::BridgeClient;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

pub struct HostAudioPlayer {
    muted: Arc<AtomicBool>,
    waveform: Arc<Mutex<VecDeque<f32>>>,
    sink_tx: Option<mpsc::Sender<Vec<u8>>>,
}

impl HostAudioPlayer {
    pub fn new(muted: bool) -> Self {
        Self {
            muted: Arc::new(AtomicBool::new(muted)),
            waveform: Arc::new(Mutex::new(VecDeque::with_capacity(512))),
            sink_tx: None,
        }
    }

    pub fn attach_bridge(&mut self, bridge: &BridgeClient) {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        bridge.set_audio_chunk_sink(Some(tx.clone()));
        self.sink_tx = Some(tx);

        let muted = self.muted.clone();
        let waveform = self.waveform.clone();
        thread::spawn(move || {
            while let Ok(chunk) = rx.recv() {
                let mut wf = waveform.lock().unwrap();
                for frame in chunk.chunks_exact(4).step_by(16) {
                    let l = i16::from_le_bytes([frame[0], frame[1]]) as f32 / 32768.0;
                    let r = i16::from_le_bytes([frame[2], frame[3]]) as f32 / 32768.0;
                    if wf.len() >= 512 {
                        wf.pop_front();
                    }
                    wf.push_back((l + r) * 0.5);
                }
                let _is_muted = muted.load(Ordering::Relaxed);
            }
        });
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    pub fn recent_waveform(&self, max_points: usize) -> Vec<f32> {
        let wf = self.waveform.lock().unwrap();
        let skip = wf.len().saturating_sub(max_points);
        wf.iter().skip(skip).copied().collect()
    }

    pub fn clear_waveform(&self) {
        self.waveform.lock().unwrap().clear();
    }
}
