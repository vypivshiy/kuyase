use crate::bridge_client::BridgeClient;
use protocol::build_wav_bytes;
use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

const WAVEFORM_CAPACITY: usize = 512;
const MAX_CAPTURED_PCM_BYTES: usize = 16 * 1024 * 1024;

pub struct HostAudioPlayer {
    muted: Arc<AtomicBool>,
    waveform: Arc<Mutex<VecDeque<f32>>>,
    captured_pcm: Arc<Mutex<Vec<u8>>>,
    total_received_bytes: Arc<AtomicUsize>,
    live_rms: Arc<AtomicU32>,
    live_peak_milli: Arc<AtomicU32>,
    sink_tx: Option<mpsc::Sender<Vec<u8>>>,
    playback_tx: mpsc::SyncSender<Vec<u8>>,
}

impl HostAudioPlayer {
    pub fn new(muted: bool) -> Self {
        let (playback_tx, playback_rx) = mpsc::sync_channel::<Vec<u8>>(32);
        spawn_os_audio_worker(playback_rx);

        Self {
            muted: Arc::new(AtomicBool::new(muted)),
            waveform: Arc::new(Mutex::new(VecDeque::with_capacity(WAVEFORM_CAPACITY))),
            captured_pcm: Arc::new(Mutex::new(Vec::new())),
            total_received_bytes: Arc::new(AtomicUsize::new(0)),
            live_rms: Arc::new(AtomicU32::new(0)),
            live_peak_milli: Arc::new(AtomicU32::new(0)),
            sink_tx: None,
            playback_tx,
        }
    }

    pub fn attach_bridge(&mut self, bridge: &BridgeClient) {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        bridge.set_audio_chunk_sink(Some(tx.clone()));
        self.sink_tx = Some(tx);

        let muted = self.muted.clone();
        let waveform = self.waveform.clone();
        let captured_pcm = self.captured_pcm.clone();
        let total_received_bytes = self.total_received_bytes.clone();
        let live_rms = self.live_rms.clone();
        let live_peak_milli = self.live_peak_milli.clone();
        let playback_tx = self.playback_tx.clone();

        thread::spawn(move || {
            while let Ok(chunk) = rx.recv() {
                ingest_pcm_chunk(
                    &chunk,
                    &waveform,
                    &captured_pcm,
                    &total_received_bytes,
                    &live_rms,
                    &live_peak_milli,
                );
                if !muted.load(Ordering::Relaxed) && !chunk.is_empty() {
                    let _ = playback_tx.try_send(chunk);
                }
            }
        });
    }

    pub fn feed_pcm_chunk(&self, chunk: &[u8]) {
        ingest_pcm_chunk(
            chunk,
            &self.waveform,
            &self.captured_pcm,
            &self.total_received_bytes,
            &self.live_rms,
            &self.live_peak_milli,
        );
        if !self.is_muted() && !chunk.is_empty() {
            let _ = self.playback_tx.try_send(chunk.to_vec());
        }
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
        self.captured_pcm.lock().unwrap().clear();
        self.total_received_bytes.store(0, Ordering::Relaxed);
        self.live_rms.store(0, Ordering::Relaxed);
        self.live_peak_milli.store(0, Ordering::Relaxed);
    }

    pub fn total_received_bytes(&self) -> usize {
        self.total_received_bytes.load(Ordering::Relaxed)
    }

    pub fn captured_bytes_len(&self) -> usize {
        self.captured_pcm.lock().unwrap().len()
    }

    pub fn live_rms(&self) -> u32 {
        self.live_rms.load(Ordering::Relaxed)
    }

    pub fn live_peak(&self) -> f32 {
        self.live_peak_milli.load(Ordering::Relaxed) as f32 / 1000.0
    }

    pub fn export_wav(&self, path: &Path) -> Result<usize, String> {
        let pcm = self.captured_pcm.lock().unwrap().clone();
        let wav = build_wav_bytes(&pcm, 48_000, 2);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("failed to create directory {}: {}", parent.display(), e))?;
            }
        }
        fs::write(path, &wav)
            .map_err(|e| format!("failed to write WAV file {}: {}", path.display(), e))?;
        Ok(wav.len())
    }
}

fn ingest_pcm_chunk(
    chunk: &[u8],
    waveform: &Mutex<VecDeque<f32>>,
    captured_pcm: &Mutex<Vec<u8>>,
    total_received_bytes: &AtomicUsize,
    live_rms: &AtomicU32,
    live_peak_milli: &AtomicU32,
) {
    if chunk.is_empty() {
        return;
    }
    total_received_bytes.fetch_add(chunk.len(), Ordering::Relaxed);
    {
        let mut cap = captured_pcm.lock().unwrap();
        if cap.len() + chunk.len() <= MAX_CAPTURED_PCM_BYTES {
            cap.extend_from_slice(chunk);
        }
    }

    let mut sum_sq = 0.0f64;
    let mut count = 0usize;
    let mut peak = 0.0f32;
    let mut wf = waveform.lock().unwrap();

    for (idx, frame) in chunk.chunks_exact(4).enumerate() {
        let l = i16::from_le_bytes([frame[0], frame[1]]) as f32;
        let r = i16::from_le_bytes([frame[2], frame[3]]) as f32;
        let mono_i16 = (l + r) * 0.5;
        let norm = (mono_i16 / 32768.0).clamp(-1.0, 1.0);
        peak = peak.max(norm.abs());
        sum_sq += (mono_i16 as f64) * (mono_i16 as f64);
        count += 1;

        if idx % 16 == 0 {
            if wf.len() >= WAVEFORM_CAPACITY {
                wf.pop_front();
            }
            wf.push_back(norm);
        }
    }

    if count > 0 {
        let rms = (sum_sq / (count as f64)).sqrt().round() as u32;
        live_rms.store(rms, Ordering::Relaxed);
        live_peak_milli.store((peak * 1000.0).round() as u32, Ordering::Relaxed);
    }
}

#[cfg(target_os = "windows")]
mod os_sink {
    use std::collections::VecDeque;
    use std::sync::mpsc;

    const WAVE_MAPPER: u32 = 0xFFFF_FFFF;
    const WAVE_FORMAT_PCM: u16 = 1;
    const WHDR_DONE: u32 = 0x0000_0001;

    #[repr(C)]
    struct WaveFormatEx {
        w_format_tag: u16,
        n_channels: u16,
        n_samples_per_sec: u32,
        n_avg_bytes_per_sec: u32,
        n_block_align: u16,
        w_bits_per_sample: u16,
        cb_size: u16,
    }

    #[repr(C)]
    struct WaveHdr {
        lp_data: *mut u8,
        dw_buffer_length: u32,
        dw_bytes_recorded: u32,
        dw_user: usize,
        dw_flags: u32,
        dw_loops: u32,
        lp_next: *mut WaveHdr,
        reserved: usize,
    }

    #[link(name = "winmm")]
    extern "system" {
        fn waveOutOpen(
            phwo: *mut usize,
            u_device_id: u32,
            pwfx: *const WaveFormatEx,
            dw_callback: usize,
            dw_instance: usize,
            fdw_open: u32,
        ) -> u32;
        fn waveOutPrepareHeader(hwo: usize, pwh: *mut WaveHdr, cbwh: u32) -> u32;
        fn waveOutWrite(hwo: usize, pwh: *mut WaveHdr, cbwh: u32) -> u32;
        fn waveOutUnprepareHeader(hwo: usize, pwh: *mut WaveHdr, cbwh: u32) -> u32;
        fn waveOutReset(hwo: usize) -> u32;
        fn waveOutClose(hwo: usize) -> u32;
    }

    pub fn run_worker(rx: mpsc::Receiver<Vec<u8>>) {
        let wfx = WaveFormatEx {
            w_format_tag: WAVE_FORMAT_PCM,
            n_channels: 2,
            n_samples_per_sec: 48_000,
            n_avg_bytes_per_sec: 48_000 * 4,
            n_block_align: 4,
            w_bits_per_sample: 16,
            cb_size: 0,
        };
        let mut hwo: usize = 0;
        let open_res = unsafe { waveOutOpen(&mut hwo, WAVE_MAPPER, &wfx, 0, 0, 0) };
        if open_res != 0 || hwo == 0 {
            while rx.recv().is_ok() {}
            return;
        }

        let hdr_size = std::mem::size_of::<WaveHdr>() as u32;
        let mut in_flight: VecDeque<(Box<WaveHdr>, Box<[u8]>)> = VecDeque::new();

        while let Ok(chunk) = rx.recv() {
            while let Some((hdr, _)) = in_flight.front() {
                if (hdr.dw_flags & WHDR_DONE) == 0 {
                    break;
                }
                if let Some((mut done_hdr, _done_buf)) = in_flight.pop_front() {
                    unsafe {
                        let _ = waveOutUnprepareHeader(hwo, &mut *done_hdr, hdr_size);
                    }
                }
            }
            if in_flight.len() >= 24 || chunk.is_empty() {
                continue;
            }
            let mut pcm_buf = chunk.into_boxed_slice();
            let mut hdr = Box::new(WaveHdr {
                lp_data: pcm_buf.as_mut_ptr(),
                dw_buffer_length: pcm_buf.len() as u32,
                dw_bytes_recorded: 0,
                dw_user: 0,
                dw_flags: 0,
                dw_loops: 0,
                lp_next: std::ptr::null_mut(),
                reserved: 0,
            });
            unsafe {
                if waveOutPrepareHeader(hwo, &mut *hdr, hdr_size) == 0 {
                    if waveOutWrite(hwo, &mut *hdr, hdr_size) == 0 {
                        in_flight.push_back((hdr, pcm_buf));
                    } else {
                        let _ = waveOutUnprepareHeader(hwo, &mut *hdr, hdr_size);
                    }
                }
            }
        }

        unsafe {
            let _ = waveOutReset(hwo);
            for (mut hdr, _buf) in in_flight.drain(..) {
                let _ = waveOutUnprepareHeader(hwo, &mut *hdr, hdr_size);
            }
            let _ = waveOutClose(hwo);
        }
    }
}

#[cfg(target_os = "linux")]
mod os_sink {
    use std::io::Write;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;

    fn spawn_linux_player() -> Option<Child> {
        Command::new("aplay")
            .args(["-q", "-t", "raw", "-f", "S16_LE", "-r", "48000", "-c", "2", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
            .or_else(|| {
                Command::new("pw-play")
                    .args(["--raw", "--rate=48000", "--channels=2", "--format=s16", "-"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .ok()
            })
    }

    pub fn run_worker(rx: mpsc::Receiver<Vec<u8>>) {
        let mut child = spawn_linux_player();
        while let Ok(chunk) = rx.recv() {
            if let Some(ref mut proc) = child {
                if let Some(ref mut stdin) = proc.stdin {
                    if stdin.write_all(&chunk).is_err() {
                        let _ = proc.kill();
                        child = None;
                    }
                }
            }
        }
        if let Some(mut proc) = child {
            let _ = proc.kill();
            let _ = proc.wait();
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod os_sink {
    use std::sync::mpsc;

    pub fn run_worker(rx: mpsc::Receiver<Vec<u8>>) {
        while rx.recv().is_ok() {}
    }
}

fn spawn_os_audio_worker(rx: mpsc::Receiver<Vec<u8>>) {
    thread::spawn(move || {
        os_sink::run_worker(rx);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_audio_player_waveform_rms_and_wav_export() {
        let player = HostAudioPlayer::new(true);
        assert!(player.is_muted());
        assert_eq!(player.captured_bytes_len(), 0);

        let mut pcm = Vec::with_capacity(4800 * 4);
        for i in 0..4800 {
            let angle = (i as f32) * 2.0 * std::f32::consts::PI * 440.0 / 48_000.0;
            let sample = (angle.sin() * 16_000.0) as i16;
            pcm.extend_from_slice(&sample.to_le_bytes());
            pcm.extend_from_slice(&sample.to_le_bytes());
        }

        player.feed_pcm_chunk(&pcm);
        assert_eq!(player.captured_bytes_len(), pcm.len());
        assert_eq!(player.total_received_bytes(), pcm.len());
        assert!(player.live_rms() > 10_000);
        assert!(player.live_peak() > 0.45);

        let wf = player.recent_waveform(256);
        assert!(!wf.is_empty());
        assert!(wf.len() <= 256);

        let wav_path =
            std::env::temp_dir().join(format!("kuyase-host-audio-test-{}.wav", std::process::id()));
        let written = player.export_wav(&wav_path).expect("export_wav should succeed");
        assert_eq!(written, 44 + pcm.len());
        assert!(wav_path.is_file());
        let _ = fs::remove_file(&wav_path);

        player.clear_waveform();
        assert_eq!(player.captured_bytes_len(), 0);
        assert_eq!(player.live_rms(), 0);
        assert!(player.recent_waveform(64).is_empty());
    }
}
