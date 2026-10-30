use crate::state::BridgeState;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

fn playback_mutex() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn wait_child_bounded(child: &mut Child, max_wait: Duration) {
    let deadline = Instant::now() + max_wait;
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn build_aplay_command() -> Command {
    let aplay = ["/system/workdir/bin/aplay", "/usr/bin/aplay"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .unwrap_or("aplay");
    let mut cmd = Command::new(aplay);
    cmd.env(
        "LD_LIBRARY_PATH",
        "/bkupgrade/data/opt/lib:/system/workdir/lib:/system/vendor/quasar/libs:/usr/lib:/lib",
    );
    if Path::new("/tmp/alsa/alsa.conf").exists() {
        cmd.env("ALSA_CONFIG_DIR", "/tmp/alsa/")
            .env("ALSA_CONFIG_PATH", "/tmp/alsa/alsa.conf");
    }
    cmd
}

pub fn play_synthetic_tone_to_alsa(freq_hz: f32, duration_ms: u32) {
    let _guard = playback_mutex().lock().unwrap_or_else(|e| e.into_inner());
    let mut cmd = build_aplay_command();
    let Ok(mut child) = cmd
        .args([
            "-D", "default", "-q", "-t", "raw", "-f", "S16_LE", "-r", "48000", "-c", "2", "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };

    if let Some(mut stdin) = child.stdin.take() {
        let frames = (48 * duration_ms) as usize;
        let mut pcm = Vec::with_capacity(frames * 4);
        for i in 0..frames {
            let t = (i as f32) / 48000.0;
            let s = ((2.0 * std::f32::consts::PI * freq_hz * t).sin() * 9000.0) as i16;
            let bytes = s.to_le_bytes();
            pcm.extend_from_slice(&bytes);
            pcm.extend_from_slice(&bytes);
        }
        let _ = stdin.write_all(&pcm);
        let _ = stdin.flush();
    }
    wait_child_bounded(&mut child, Duration::from_millis((duration_ms as u64) + 500));
}

pub fn play_expressive_tts_to_alsa(state: &Arc<BridgeState>, text: &str, expected_seq: u32) -> bool {
    if state.dialog_seq.load(Ordering::SeqCst) != expected_seq
        || state.alice_state() != crate::state::AliceState::Speaking
    {
        return false;
    }

    let (stereo_pcm, _dur_ms) = crate::tts::synthesize_speech_pcm(text);
    let _guard = playback_mutex().lock().unwrap_or_else(|e| e.into_inner());

    if state.dialog_seq.load(Ordering::SeqCst) != expected_seq
        || state.alice_state() != crate::state::AliceState::Speaking
    {
        return false;
    }

    let mut cmd = build_aplay_command();
    let Ok(mut child) = cmd
        .args([
            "-D", "default", "-q", "-t", "raw", "-f", "S16_LE", "-r", "48000", "-c", "2", "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };

    let mut interrupted = false;
    if let Some(mut stdin) = child.stdin.take() {
        // Stream in 20 ms chunks (960 stereo frames = 1920 i16 samples = 3840 bytes)
        // Paced at real-time speed so pipe buffer never accumulates >40ms of un-cancellable audio
        const CHUNK_SAMPLES: usize = 1920;
        let mut raw_bytes = Vec::with_capacity(CHUNK_SAMPLES * 2);
        for (chunk_idx, chunk) in stereo_pcm.chunks(CHUNK_SAMPLES).enumerate() {
            if state.dialog_seq.load(Ordering::SeqCst) != expected_seq
                || state.alice_state() != crate::state::AliceState::Speaking
            {
                interrupted = true;
                break;
            }
            raw_bytes.clear();
            for &s in chunk {
                raw_bytes.extend_from_slice(&s.to_le_bytes());
            }
            if stdin.write_all(&raw_bytes).is_err() {
                break;
            }
            let _ = stdin.flush();
            if chunk_idx >= 2 {
                thread::sleep(Duration::from_millis(16));
            }
        }
    }

    // Drop stdin (EOF) so aplay cleanly closes ALSA dmix without leaving SysV semaphores locked
    wait_child_bounded(&mut child, Duration::from_millis(600));
    !interrupted && state.dialog_seq.load(Ordering::SeqCst) == expected_seq
}

pub fn spawn_alsa_loopback_capture(state: Arc<BridgeState>) {
    thread::spawn(move || loop {
        let mut cmd = build_aplay_command();
        let Ok(mut child) = cmd
            .args([
                "-C", "-D", "hw:0,1,0", "-q", "-t", "raw", "-f", "S16_LE", "-r", "48000", "-c",
                "2", "-",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            thread::sleep(Duration::from_millis(1000));
            continue;
        };

        if let Some(mut stdout) = child.stdout.take() {
            let mut buf = [0u8; 3840]; // 20 ms at 48kHz 2ch S16_LE
            let mut hangover_chunks = 0u32;
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let samples = n / 2;
                let mut sum_sq = 0u64;
                for chunk in buf[..n].chunks_exact(2) {
                    let s = i16::from_le_bytes([chunk[0], chunk[1]]) as i64;
                    sum_sq += (s * s) as u64;
                }
                let rms = if samples > 0 {
                    ((sum_sq / samples as u64) as f64).sqrt() as u32
                } else {
                    0
                };
                state.speaker_rms.store(rms, Ordering::Relaxed);
                if rms > 5 {
                    hangover_chunks = 20; // Keep 400 ms of inter-word / punctuation pauses
                }
                if rms > 5 || hangover_chunks > 0 {
                    if rms <= 5 {
                        hangover_chunks = hangover_chunks.saturating_sub(1);
                    }
                    let mut cap = state.captured_pcm.lock().unwrap();
                    if cap.len() < 4 * 1024 * 1024 {
                        cap.extend_from_slice(&buf[..n]);
                    }
                }
            }
        }
        let _ = child.wait();
        thread::sleep(Duration::from_millis(500));
    });
}

pub fn build_wav_bytes(pcm: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
    let data_len = pcm.len() as u32;
    let byte_rate = sample_rate * (channels as u32) * 2;
    let block_align = channels * 2;
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

pub fn ensure_alsa_compat_lib() {
    const ALSA_COMPAT_LIB_DIR: &str = "/bkupgrade/data/opt/lib";
    const ALSA_COMPAT_LIB_PATH: &str = "/bkupgrade/data/opt/lib/libasound.so.2";
    const SYSTEM_LIBASOUND_PATH: &str = "/system/vendor/quasar/libs/libasound.so.2.0.0";

    if fs::metadata(ALSA_COMPAT_LIB_PATH)
        .map(|m| m.len() > 500_000)
        .unwrap_or(false)
    {
        return;
    }
    let Ok(mut data) = fs::read(SYSTEM_LIBASOUND_PATH) else {
        return;
    };
    if data.len() < 64 || &data[0..4] != b"\x7fELF" {
        return;
    }

    let read_u16 = |b: &[u8], off: usize| -> u16 { u16::from_le_bytes([b[off], b[off + 1]]) };
    let read_u32 = |b: &[u8], off: usize| -> u32 {
        u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    };

    let e_shoff = read_u32(&data, 32) as usize;
    let e_shentsize = read_u16(&data, 46) as usize;
    let e_shnum = read_u16(&data, 48) as usize;
    let e_shstrndx = read_u16(&data, 50) as usize;
    if e_shoff == 0 || e_shentsize < 40 || e_shoff + e_shnum * e_shentsize > data.len() {
        return;
    }

    let shstr_off = read_u32(&data, e_shoff + e_shstrndx * e_shentsize + 16) as usize;
    let mut dynsym = (0usize, 0usize);
    let mut dynstr = 0usize;
    let mut gnu_ver = 0usize;

    for i in 0..e_shnum {
        let off = e_shoff + i * e_shentsize;
        let sh_name = read_u32(&data, off) as usize;
        let sh_offset = read_u32(&data, off + 16) as usize;
        let sh_size = read_u32(&data, off + 20) as usize;
        let name_start = shstr_off + sh_name;
        if name_start >= data.len() {
            continue;
        }
        let name_end = data[name_start..]
            .iter()
            .position(|&c| c == 0)
            .map(|p| name_start + p)
            .unwrap_or(data.len());
        match &data[name_start..name_end] {
            b".dynsym" => dynsym = (sh_offset, sh_size),
            b".dynstr" => dynstr = sh_offset,
            b".gnu.version" => gnu_ver = sh_offset,
            _ => {}
        }
    }

    if dynsym.0 == 0 || dynstr == 0 || gnu_ver == 0 {
        return;
    }

    let count = dynsym.1 / 16;
    let mut rc4_entries: Vec<(Vec<u8>, [u8; 12])> = Vec::new();
    for i in 0..count {
        let sym_off = dynsym.0 + i * 16;
        let ver = read_u16(&data, gnu_ver + i * 2);
        if ver == 0x0003 {
            let st_name = read_u32(&data, sym_off) as usize;
            let s_start = dynstr + st_name;
            if let Some(len) = data.get(s_start..).and_then(|s| s.iter().position(|&c| c == 0)) {
                let mut payload = [0u8; 12];
                payload.copy_from_slice(&data[sym_off + 4..sym_off + 16]);
                rc4_entries.push((data[s_start..s_start + len].to_vec(), payload));
            }
        }
    }

    for i in 0..count {
        let sym_off = dynsym.0 + i * 16;
        let ver_off = gnu_ver + i * 2;
        let ver = read_u16(&data, ver_off);
        if ver == 0x8002 {
            let st_name = read_u32(&data, sym_off) as usize;
            let s_start = dynstr + st_name;
            if let Some(len) = data.get(s_start..).and_then(|s| s.iter().position(|&c| c == 0)) {
                let sym_name = &data[s_start..s_start + len];
                if let Some((_, payload)) =
                    rc4_entries.iter().find(|(n, _)| n.as_slice() == sym_name)
                {
                    data[sym_off + 4..sym_off + 16].copy_from_slice(payload);
                }
            }
        }
    }

    let _ = fs::create_dir_all(ALSA_COMPAT_LIB_DIR);
    let _ = fs::write(ALSA_COMPAT_LIB_PATH, &data);
}
