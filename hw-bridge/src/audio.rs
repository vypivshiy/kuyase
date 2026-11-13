use crate::state::BridgeState;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
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
        let start_gen = state.capture_config_generation.load(Ordering::Relaxed);
        let init_cfg = state.audio_config();
        let rate_str = init_cfg.capture_sample_rate.to_string();
        let chunk_bytes = init_cfg.capture_chunk_bytes();
        let alsa_dev = if init_cfg.capture_sample_rate == protocol::DEFAULT_CAPTURE_SAMPLE_RATE {
            "hw:0,1,0"
        } else {
            "plughw:0,1,0"
        };
        let mut cmd = build_aplay_command();
        let Ok(mut child) = cmd
            .args([
                "-C", "-D", alsa_dev, "-q", "-t", "raw", "-f", "S16_LE", "-r", &rate_str, "-c",
                "2", "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            thread::sleep(Duration::from_millis(1000));
            continue;
        };

        if let Some(mut stdout) = child.stdout.take() {
            let mut buf = vec![0u8; chunk_bytes];
            let mut hangover_chunks = 0u32;
            'capture_loop: loop {
                let mut filled = 0usize;
                while filled < chunk_bytes {
                    match stdout.read(&mut buf[filled..]) {
                        Ok(0) | Err(_) => break 'capture_loop,
                        Ok(k) => filled += k,
                    }
                    if state.capture_config_generation.load(Ordering::Relaxed) != start_gen {
                        let _ = child.kill();
                        break 'capture_loop;
                    }
                }
                let samples = chunk_bytes / 2;
                let mut sum_sq = 0u64;
                for chunk in buf.chunks_exact(2) {
                    let s = i16::from_le_bytes([chunk[0], chunk[1]]) as i64;
                    sum_sq += (s * s) as u64;
                }
                let rms = ((sum_sq / samples as u64) as f64).sqrt() as u32;
                state.speaker_rms.store(rms, Ordering::Relaxed);
                let live_cfg = state.audio_config();
                let gate_rms = live_cfg.silence_gate_rms;
                let max_hangover_chunks = live_cfg.hangover_ms / live_cfg.capture_chunk_ms;
                let pass_gate = gate_rms == 0 || rms > gate_rms;
                if pass_gate {
                    hangover_chunks = max_hangover_chunks;
                }
                if pass_gate || hangover_chunks > 0 {
                    if !pass_gate {
                        hangover_chunks = hangover_chunks.saturating_sub(1);
                    }
                    {
                        let mut cap = state.captured_pcm.lock().unwrap();
                        if cap.len() < 4 * 1024 * 1024 {
                            cap.extend_from_slice(&buf);
                        }
                    }
                    state.broadcast_audio_chunk(buf.clone());
                }
            }
        }
        let _ = child.wait();
        // A 500 ms gap left the loopback undrained long enough for the playback
        // side to xrun (EPIPE, audible dropout every ~25 s). Restart at once;
        // the short sleep only guards against a spin when the child cannot start.
        thread::sleep(Duration::from_millis(20));
    });
}

/// Generates a symbol-version-patched `/bkupgrade/data/opt/lib/libasound.so.2` and bind-mounts
/// it over both `/system/vendor/quasar/libs/libasound.so.2*` and `/usr/lib/libasound.so.2*`.
///
/// Why this exists:
/// 1. In 32-bit Quasar firmware images, `/system/vendor/quasar/libs/libasound.so.2.0.0` exports
///    ALSA symbols with `.gnu.version == 0x8002` (`ALSA_0.9` hidden/unpopulated) and `0x0003`
///    (`ALSA_0.9.0rc4`), whereas `/usr/lib/libasound.so.2` lacks `VERDEF` version tags entirely.
/// 2. The stock `/system/vendor/quasar/q` script overrides `LD_LIBRARY_PATH=/system/vendor/quasar/libs`,
///    bypassing `/bkupgrade/data/opt/lib` and causing the dynamic linker to emit
///    `libasound.so.2: no version information available` on `stderr`.
/// 3. Bind-mounting the patched library directly over `/system/vendor/quasar/libs/libasound.so.2.0.0`,
///    `/system/vendor/quasar/libs/libasound.so.2`, `/usr/lib/libasound.so.2.0.0`, and
///    `/usr/lib/libasound.so.2` ensures zero linker warnings regardless of `LD_LIBRARY_PATH`.
pub fn ensure_alsa_compat_lib() {
    const ALSA_COMPAT_LIB_DIR: &str = "/bkupgrade/data/opt/lib";
    const ALSA_COMPAT_LIB_PATH: &str = "/bkupgrade/data/opt/lib/libasound.so.2";
    const SYSTEM_LIBASOUND_PATH: &str = "/system/vendor/quasar/libs/libasound.so.2.0.0";
    const BIND_TARGETS: [&str; 4] = [
        "/system/vendor/quasar/libs/libasound.so.2.0.0",
        "/system/vendor/quasar/libs/libasound.so.2",
        "/usr/lib/libasound.so.2.0.0",
        "/usr/lib/libasound.so.2",
    ];

    let bind_patched_alsa_lib = || {
        for target in BIND_TARGETS {
            let _ = crate::runtime_env::bind_mount_file(ALSA_COMPAT_LIB_PATH, target);
        }
    };

    if fs::metadata(ALSA_COMPAT_LIB_PATH)
        .map(|m| m.len() > 500_000)
        .unwrap_or(false)
    {
        bind_patched_alsa_lib();
        return;
    }
    let Ok(mut data) = fs::read(SYSTEM_LIBASOUND_PATH) else {
        // On 32-bit Gen 3 boards (`bergamot`, `yandexmini2*`, `plum*`, `station2`, `yandexmicro`),
        // `/system/vendor/quasar/libs/libasound.so.2.0.0` is absent and Buildroot's
        // `/usr/lib/libasound.so.2.0.0` was compiled with `--without-versioned` (no `DT_VERDEF`),
        // whereas `/system/vendor/quasar/maind` and `libquasar_gstreamer.so` have `DT_VERNEED`
        // entries for `libasound.so.2` (`ALSA_0.9`, `ALSA_0.9.0rc4`). Copy them to RAM
        // (`/tmp/.kuyase-stubs/`), strip the leading `libasound.so.2` `Verneed` entry on the RAM
        // copies, and bind-mount them over `/system/vendor/quasar/maind` and
        // `/system/vendor/quasar/libs/libquasar_gstreamer.so` so `ld-linux-armhf.so.3` never
        // prints `libasound.so.2: no version information available` without mutating the rootfs.
        if usr_libasound_lacks_verdef(Path::new("/usr/lib/libasound.so.2.0.0")) {
            let _ = fs::create_dir_all("/tmp/.kuyase-stubs");
            for (target, ram_copy) in [
                (
                    "/system/vendor/quasar/maind",
                    "/tmp/.kuyase-stubs/maind-patched",
                ),
                (
                    "/system/vendor/quasar/libs/libquasar_gstreamer.so",
                    "/tmp/.kuyase-stubs/libquasar_gstreamer.so",
                ),
            ] {
                if Path::new(target).is_file() {
                    if !Path::new(ram_copy).is_file() && fs::copy(target, ram_copy).is_ok() {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            let _ =
                                fs::set_permissions(ram_copy, fs::Permissions::from_mode(0o755));
                        }
                        let _ = strip_elf32_alsa_verneed_in_place(Path::new(ram_copy));
                    }
                    let _ = crate::runtime_env::bind_mount_file(ram_copy, target);
                }
            }
        }
        return;
    };
    if data.len() < 64 || &data[0..4] != b"\x7fELF" || data[4] != 1 {
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
    if fs::write(ALSA_COMPAT_LIB_PATH, &data).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(ALSA_COMPAT_LIB_PATH, fs::Permissions::from_mode(0o755));
        }
        bind_patched_alsa_lib();
    }
}

fn usr_libasound_lacks_verdef(path: &Path) -> bool {
    let Ok(data) = fs::read(path) else {
        return false;
    };
    if data.len() < 64 || &data[0..4] != b"\x7fELF" || data[4] != 1 {
        return false;
    }
    !data.windows(8).any(|w| w == b"ALSA_0.9")
}

/// Strips the leading `libasound.so.2` `Elf32_Verneed` entry (`ALSA_0.9`, `ALSA_0.9.0rc4`,
/// `ALSA_0.9.0rc8`) in-place from a 32-bit ELF binary (`maind` or `libquasar_gstreamer.so`)
/// when the guest rootfs ships an unversioned `/usr/lib/libasound.so.2.0.0`.
///
/// Specifically:
/// 1. Reads `.gnu.version_r` and verifies that `Verneed[0].vn_file` points to `"libasound.so.2"`
///    with `vn_next > 0`.
/// 2. Collects all `vna_other` version indices from `Verneed[0]`'s `Elf32_Vernaux` list and
///    rewrites matching 16-bit entries in `.gnu.version` (`DT_VERSYM`) to `1` (`VER_NDX_GLOBAL`).
/// 3. Advances `DT_VERNEED` (`0x6ffffffe`) by `vn_next` and decrements `DT_VERNEEDNUM`
///    (`0x6fffffff`) by `1` in `.dynamic` (and updates the `.gnu.version_r` section header).
///    Because all `vn_aux` and `vn_next` offsets in `Elf32_Verneed` are self-relative, the remaining
///    `Verneed` entries (`libdl.so.2`, `librt.so.1`, `libpthread.so.0`, `libc.so.6`, `libm.so.6`)
///    stay intact while only ~2 KB of metadata is written in-place via `Seek` + `write_all`.
pub fn strip_elf32_alsa_verneed_in_place(path: &Path) -> bool {
    let Ok(mut file) = fs::OpenOptions::new().read(true).write(true).open(path) else {
        return false;
    };
    let mut ehdr = [0u8; 52];
    if file.read_exact(&mut ehdr).is_err() {
        return false;
    }
    if &ehdr[0..4] != b"\x7fELF" || ehdr[4] != 1 || ehdr[5] != 1 {
        return false;
    }

    let read_u16 = |b: &[u8], off: usize| -> u16 { u16::from_le_bytes([b[off], b[off + 1]]) };
    let read_u32 = |b: &[u8], off: usize| -> u32 {
        u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    };

    let e_shoff = read_u32(&ehdr, 32) as u64;
    let e_shentsize = read_u16(&ehdr, 46) as usize;
    let e_shnum = read_u16(&ehdr, 48) as usize;
    let e_shstrndx = read_u16(&ehdr, 50) as usize;
    if e_shoff == 0
        || e_shentsize < 40
        || e_shnum == 0
        || e_shstrndx >= e_shnum
        || e_shnum > 256
    {
        return false;
    }

    let mut sh_table = vec![0u8; e_shnum * e_shentsize];
    if file.seek(SeekFrom::Start(e_shoff)).is_err() || file.read_exact(&mut sh_table).is_err() {
        return false;
    }

    let shstr_hdr_off = e_shstrndx * e_shentsize;
    let shstr_off = read_u32(&sh_table, shstr_hdr_off + 16) as u64;
    let shstr_size = read_u32(&sh_table, shstr_hdr_off + 20) as usize;
    if shstr_size == 0 || shstr_size > 1024 * 1024 {
        return false;
    }
    let mut shstrtab = vec![0u8; shstr_size];
    if file.seek(SeekFrom::Start(shstr_off)).is_err() || file.read_exact(&mut shstrtab).is_err() {
        return false;
    }

    #[derive(Default, Clone, Copy)]
    struct ElfSectionSpan {
        sh_pos: usize,
        sh_type: u32,
        sh_addr: u64,
        sh_offset: usize,
        sh_info: u32,
    }

    let mut dynstr_sec = (0u64, 0usize);
    let mut gnu_ver_sec = (0u64, 0usize);
    let mut verneed_sec = ElfSectionSpan::default();
    let mut verneed_size = 0usize;
    let mut dynamic_sec = (0u64, 0usize);

    for i in 0..e_shnum {
        let off = i * e_shentsize;
        let sh_name = read_u32(&sh_table, off) as usize;
        let sh_type = read_u32(&sh_table, off + 4);
        let sh_addr = read_u32(&sh_table, off + 12) as u64;
        let sh_offset = read_u32(&sh_table, off + 16) as u64;
        let sh_size = read_u32(&sh_table, off + 20) as usize;
        let sh_info = read_u32(&sh_table, off + 28);
        if sh_name >= shstrtab.len() {
            continue;
        }
        let name_end = shstrtab[sh_name..]
            .iter()
            .position(|&c| c == 0)
            .map(|p| sh_name + p)
            .unwrap_or(shstrtab.len());
        match &shstrtab[sh_name..name_end] {
            b".dynstr" => dynstr_sec = (sh_offset, sh_size),
            b".gnu.version" => gnu_ver_sec = (sh_offset, sh_size),
            b".gnu.version_r" => {
                verneed_sec = ElfSectionSpan {
                    sh_pos: i,
                    sh_type,
                    sh_addr,
                    sh_offset: sh_offset as usize,
                    sh_info,
                };
                verneed_size = sh_size;
            }
            b".dynamic" => dynamic_sec = (sh_offset, sh_size),
            _ => {}
        }
    }

    if dynstr_sec.1 == 0
        || gnu_ver_sec.1 == 0
        || verneed_sec.sh_type == 0
        || verneed_size < 16
        || verneed_sec.sh_info <= 1
        || dynamic_sec.1 == 0
    {
        return false;
    }

    let mut dynstr = vec![0u8; dynstr_sec.1];
    if file.seek(SeekFrom::Start(dynstr_sec.0)).is_err() || file.read_exact(&mut dynstr).is_err() {
        return false;
    }
    let mut verneed = vec![0u8; verneed_size];
    if file
        .seek(SeekFrom::Start(verneed_sec.sh_offset as u64))
        .is_err()
        || file.read_exact(&mut verneed).is_err()
    {
        return false;
    }

    let vn_cnt = read_u16(&verneed, 2) as usize;
    let vn_file = read_u32(&verneed, 4) as usize;
    let vn_aux = read_u32(&verneed, 8) as usize;
    let vn_next = read_u32(&verneed, 12);
    if vn_next == 0 || vn_file >= dynstr.len() {
        return false;
    }
    let file_name_end = dynstr[vn_file..]
        .iter()
        .position(|&c| c == 0)
        .map(|p| vn_file + p)
        .unwrap_or(dynstr.len());
    if &dynstr[vn_file..file_name_end] != b"libasound.so.2" {
        return false;
    }

    let mut alsa_ver_indices = Vec::with_capacity(vn_cnt);
    let mut apos = vn_aux;
    for _ in 0..vn_cnt {
        if apos + 16 > verneed.len() {
            break;
        }
        let vna_other = read_u16(&verneed, apos + 6) & 0x7fff;
        let vna_next = read_u32(&verneed, apos + 12) as usize;
        if vna_other >= 2 {
            alsa_ver_indices.push(vna_other);
        }
        if vna_next == 0 {
            break;
        }
        apos += vna_next;
    }
    if alsa_ver_indices.is_empty() {
        return false;
    }

    let mut gnu_ver = vec![0u8; gnu_ver_sec.1];
    if file.seek(SeekFrom::Start(gnu_ver_sec.0)).is_err()
        || file.read_exact(&mut gnu_ver).is_err()
    {
        return false;
    }
    for chunk in gnu_ver.chunks_exact_mut(2) {
        let ver = u16::from_le_bytes([chunk[0], chunk[1]]) & 0x7fff;
        if alsa_ver_indices.contains(&ver) {
            chunk.copy_from_slice(&1u16.to_le_bytes());
        }
    }
    if file.seek(SeekFrom::Start(gnu_ver_sec.0)).is_err() || file.write_all(&gnu_ver).is_err() {
        return false;
    }

    let mut dynamic = vec![0u8; dynamic_sec.1];
    if file.seek(SeekFrom::Start(dynamic_sec.0)).is_err()
        || file.read_exact(&mut dynamic).is_err()
    {
        return false;
    }
    for entry in dynamic.chunks_exact_mut(8) {
        let d_tag = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
        let d_val = u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]);
        if d_tag == 0x6fff_fffe {
            // DT_VERNEED
            entry[4..8].copy_from_slice(&d_val.wrapping_add(vn_next).to_le_bytes());
        } else if d_tag == 0x6fff_ffff && d_val > 1 {
            // DT_VERNEEDNUM
            entry[4..8].copy_from_slice(&(d_val - 1).to_le_bytes());
        }
    }
    if file.seek(SeekFrom::Start(dynamic_sec.0)).is_err() || file.write_all(&dynamic).is_err() {
        return false;
    }

    let sec_hdr_off = verneed_sec.sh_pos * e_shentsize;
    let new_sh_addr = (verneed_sec.sh_addr as u32).wrapping_add(vn_next);
    let new_sh_offset = (verneed_sec.sh_offset as u32).wrapping_add(vn_next);
    let new_sh_size = (verneed_size as u32).saturating_sub(vn_next);
    let new_sh_info = verneed_sec.sh_info.saturating_sub(1);
    sh_table[sec_hdr_off + 12..sec_hdr_off + 16].copy_from_slice(&new_sh_addr.to_le_bytes());
    sh_table[sec_hdr_off + 16..sec_hdr_off + 20].copy_from_slice(&new_sh_offset.to_le_bytes());
    sh_table[sec_hdr_off + 20..sec_hdr_off + 24].copy_from_slice(&new_sh_size.to_le_bytes());
    sh_table[sec_hdr_off + 28..sec_hdr_off + 32].copy_from_slice(&new_sh_info.to_le_bytes());

    let sec_file_off = e_shoff + (sec_hdr_off as u64);
    if file.seek(SeekFrom::Start(sec_file_off)).is_err()
        || file
            .write_all(&sh_table[sec_hdr_off..sec_hdr_off + e_shentsize])
            .is_err()
    {
        return false;
    }
    let _ = file.flush();
    true
}
