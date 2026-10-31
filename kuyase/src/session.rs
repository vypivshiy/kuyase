use protocol::normalize_board_slug;
use std::collections::VecDeque;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

pub const EMBEDDED_HW_BRIDGE: &[u8] = include_bytes!("../../guest-init/bin/qemu-hw-bridge");

pub fn resolve_repo_root() -> PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for ancestor in cwd.ancestors().take(4) {
        if ancestor.join("guest-init").is_dir()
            || ancestor.join("work").is_dir()
            || ancestor.join("Releases").is_dir()
        {
            return ancestor.to_path_buf();
        }
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            if exe_dir.join("Image").is_file() && !exe_dir.join("..").join("Cargo.toml").is_file() {
                return exe_dir.to_path_buf();
            }
        }
        for ancestor in exe.ancestors().skip(1).take(4) {
            if ancestor.join("guest-init").is_dir()
                || ancestor.join("work").is_dir()
                || ancestor.join("Releases").is_dir()
            {
                return ancestor.to_path_buf();
            }
        }
        if let Some(exe_dir) = exe.parent() {
            if exe_dir.join("Image").is_file() {
                return exe_dir.to_path_buf();
            }
        }
    }
    cwd
}

fn find_in_path(bin_names: &[&str]) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        for name in bin_names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn resolve_qemu_binary(root: &Path) -> Result<PathBuf, String> {
    let local_candidates = [
        root.join("qemu-system-aarch64.exe"),
        root.join("qemu-system-aarch64"),
        root.join("bin").join("qemu-system-aarch64.exe"),
        root.join("bin").join("qemu-system-aarch64"),
        root.join("Releases").join("qemu-system-aarch64.exe"),
        root.join("Releases").join("qemu-system-aarch64"),
    ];
    for c in &local_candidates {
        if c.is_file() {
            return Ok(c.clone());
        }
    }

    if let Some(p) = find_in_path(&["qemu-system-aarch64.exe", "qemu-system-aarch64"]) {
        return Ok(p);
    }

    let system_candidates = [
        PathBuf::from(r"C:\Program Files\qemu\qemu-system-aarch64.exe"),
        PathBuf::from("/usr/bin/qemu-system-aarch64"),
        PathBuf::from("/usr/local/bin/qemu-system-aarch64"),
    ];
    for c in &system_candidates {
        if c.is_file() {
            return Ok(c.clone());
        }
    }

    Err(
        "qemu-system-aarch64 not found in ./bin, PATH, or C:\\Program Files\\qemu\\qemu-system-aarch64.exe"
            .to_string(),
    )
}

pub fn resolve_kernel_image(root: &Path) -> Result<PathBuf, String> {
    let candidates = [
        root.join("Image"),
        root.join("kernel").join("Image"),
        root.join("Releases").join("Image"),
        root.join("bin").join("Image"),
    ];
    for c in &candidates {
        if c.is_file() {
            return Ok(c.clone());
        }
    }
    Err(format!(
        "Guest Linux kernel Image not found in {}/Image, {}/kernel/Image, {}/Releases/Image, or {}/bin/Image",
        root.display(),
        root.display(),
        root.display(),
        root.display()
    ))
}

fn is_valid_elf(path: &Path) -> Option<SystemTime> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() < 64 {
        return None;
    }
    let mut f = File::open(path).ok()?;
    let mut magic = [0u8; 4];
    use std::io::Read;
    f.read_exact(&mut magic).ok()?;
    if &magic == b"\x7fELF" {
        Some(meta.modified().unwrap_or(SystemTime::UNIX_EPOCH))
    } else {
        None
    }
}

pub fn resolve_bridge_binary(root: &Path) -> Result<PathBuf, String> {
    let candidates = [
        root.join("target")
            .join("armv7-unknown-linux-musleabihf")
            .join("release")
            .join("qemu-hw-bridge"),
        root.join("hw-bridge")
            .join("target")
            .join("armv7-unknown-linux-musleabihf")
            .join("release")
            .join("qemu-hw-bridge"),
        root.join("guest-init").join("bin").join("qemu-hw-bridge"),
        root.join("Releases").join("qemu-hw-bridge"),
        root.join("qemu-hw-bridge"),
    ];

    let mut newest: Option<(PathBuf, SystemTime)> = None;
    for c in &candidates {
        if let Some(mtime) = is_valid_elf(c) {
            if newest.as_ref().map(|(_, t)| mtime > *t).unwrap_or(true) {
                newest = Some((c.clone(), mtime));
            }
        }
    }

    if let Some((best_path, _)) = newest {
        let init_bin = root.join("guest-init").join("bin").join("qemu-hw-bridge");
        if best_path != init_bin && root.join("guest-init").is_dir() {
            if let Some(parent) = init_bin.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::copy(&best_path, &init_bin);
        }
        return Ok(best_path);
    }

    if EMBEDDED_HW_BRIDGE.len() >= 64 && &EMBEDDED_HW_BRIDGE[0..4] == b"\x7fELF" {
        let work_dir = root.join("work");
        let _ = fs::create_dir_all(&work_dir);
        let extracted = work_dir.join("qemu-hw-bridge.bin");
        fs::write(&extracted, EMBEDDED_HW_BRIDGE)
            .map_err(|e| format!("failed to write embedded qemu-hw-bridge: {}", e))?;
        return Ok(extracted);
    }

    Err("qemu-hw-bridge binary not found on disk or in embedded resources".to_string())
}

pub fn discover_guest_images(root: &Path) -> Vec<(String, PathBuf)> {
    let work_dir = root.join("work");
    let Ok(entries) = fs::read_dir(&work_dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.ends_with("-system.ext4") && !name.ends_with("-system-orig.ext4") {
            let board = name.trim_end_matches("-system.ext4").to_string();
            found.push((board, path));
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

pub fn resolve_board_and_image(
    root: &Path,
    requested_board: Option<&str>,
) -> Result<(String, PathBuf), String> {
    let work_dir = root.join("work");
    if let Some(req) = requested_board {
        let trimmed = req.trim();
        if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("auto") {
            let slug = normalize_board_slug(trimmed);
            let img = work_dir.join(format!("{}-system.ext4", slug));
            if img.is_file() {
                return Ok((slug, img));
            }
            return Err(format!(
                "Guest Image not found for board '{}': expected {}",
                slug,
                img.display()
            ));
        }
    }

    let last_board_file = work_dir.join(".last-board");
    if let Ok(last) = fs::read_to_string(&last_board_file) {
        let slug = normalize_board_slug(last.trim());
        if !slug.is_empty() {
            let img = work_dir.join(format!("{}-system.ext4", slug));
            if img.is_file() {
                return Ok((slug, img));
            }
        }
    }

    let discovered = discover_guest_images(root);
    if let Some((slug, img)) = discovered.into_iter().next() {
        return Ok((slug, img));
    }

    Err(format!(
        "No *-system.ext4 Guest Images found in {}/work",
        root.display()
    ))
}

#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub root_dir: PathBuf,
    pub board: String,
    pub board_cmdline: String,
    pub mode: String,
    pub snapshot: bool,
    pub bridge_port: u16,
    pub adb_port: u16,
    pub sdk_port: Option<u16>,
    pub drive_img: PathBuf,
}

impl SessionConfig {
    pub fn for_board(root: &Path, board: Option<&str>) -> Result<Self, String> {
        let (slug, drive_img) = resolve_board_and_image(root, board)?;
        Ok(Self {
            root_dir: root.to_path_buf(),
            board: slug,
            board_cmdline: "auto".to_string(),
            mode: "stub".to_string(),
            snapshot: false,
            bridge_port: 18091,
            adb_port: 15555,
            sdk_port: Some(18090),
            drive_img,
        })
    }
}

pub struct EmulatorSession {
    pub config: SessionConfig,
    pub qemu_bin: PathBuf,
    pub kernel_img: PathBuf,
    pub bridge_bin: PathBuf,
    child: Option<Child>,
    serial_stdin: Mutex<Option<ChildStdin>>,
    serial_lines: Arc<Mutex<VecDeque<String>>>,
}

pub fn strip_ansi_and_control_chars(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\x07' {
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
            continue;
        }
        if ch == '\t' || ch >= ' ' {
            out.push(ch);
        }
    }
    out
}

fn spawn_serial_reader<R: std::io::Read + Send + 'static>(
    stream: R,
    lines_ref: Arc<Mutex<VecDeque<String>>>,
    log_path: PathBuf,
    log_prefix: &'static str,
    line_prefix: &'static str,
) {
    thread::spawn(move || {
        let reader = BufReader::new(stream);
        let mut log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .ok();
        for line in reader.lines().map_while(Result::ok) {
            let clean = strip_ansi_and_control_chars(line.trim_end_matches('\r'));
            if let Some(ref mut f) = log_file {
                let _ = writeln!(f, "{}{}", log_prefix, clean);
            }
            let mut guard = lines_ref.lock().unwrap();
            if guard.len() >= 2000 {
                guard.pop_front();
            }
            let formatted = if line_prefix.is_empty() {
                clean
            } else {
                format!("{}{}", line_prefix, clean)
            };
            guard.push_back(formatted);
        }
    });
}

impl EmulatorSession {
    pub fn start(config: SessionConfig) -> Result<Self, String> {
        Self::start_with_overrides(config, None, None)
    }

    pub fn start_with_overrides(
        config: SessionConfig,
        custom_qemu: Option<&Path>,
        custom_kernel: Option<&Path>,
    ) -> Result<Self, String> {
        let qemu_bin = match custom_qemu {
            Some(p) if p.is_file() => p.to_path_buf(),
            _ => resolve_qemu_binary(&config.root_dir)?,
        };
        let kernel_img = match custom_kernel {
            Some(p) if p.is_file() => p.to_path_buf(),
            _ => resolve_kernel_image(&config.root_dir)?,
        };
        let bridge_src = resolve_bridge_binary(&config.root_dir)?;

        if !config.drive_img.is_file() {
            return Err(format!(
                "Guest Image does not exist: {}",
                config.drive_img.display()
            ));
        }

        let work_dir = config.root_dir.join("work");
        let _ = fs::create_dir_all(&work_dir);
        let staged_bridge = work_dir.join(format!("qemu-hw-bridge-{}.bin", config.board));
        let _ = fs::copy(&bridge_src, &staged_bridge);
        let bridge_bin = if staged_bridge.is_file() {
            staged_bridge
        } else {
            bridge_src
        };
        let serial_log_path = work_dir.join(format!("{}-serial.log", config.board));
        let _ = File::create(&serial_log_path);

        let mut hostfwd = format!(
            "user,id=net0,hostfwd=tcp:127.0.0.1:{}-:8091,hostfwd=tcp:127.0.0.1:{}-:5555",
            config.bridge_port, config.adb_port
        );
        if let Some(sdk_p) = config.sdk_port {
            hostfwd.push_str(&format!(",hostfwd=tcp:127.0.0.1:{}-:8090", sdk_p));
        }

        let append_cmdline = format!(
            "console=ttyAMA0 root=/dev/vda rw ip=dhcp init=/quasar-qemu-init.sh quasar.board={} quasar.mode={} panic=-1",
            config.board_cmdline, config.mode
        );

        let mut cmd = Command::new(&qemu_bin);
        cmd.args([
            "-M",
            "virt",
            "-cpu",
            "cortex-a53",
            "-smp",
            "2",
            "-m",
            "512M",
            "-kernel",
        ])
        .arg(&kernel_img)
        .arg("-append")
        .arg(&append_cmdline)
        .arg("-drive")
        .arg(format!("file={},if=virtio,format=raw", config.drive_img.display()))
        .arg("-drive")
        .arg(format!(
            "file={},if=virtio,format=raw,readonly=on",
            bridge_bin.display()
        ));

        if config.snapshot {
            cmd.arg("-snapshot");
        }

        cmd.arg("-netdev")
            .arg(&hostfwd)
            .args([
                "-device",
                "virtio-net-pci,netdev=net0",
                "-serial",
                "stdio",
                "-display",
                "none",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            format!(
                "failed to spawn qemu-system-aarch64 ('{}'): {}",
                qemu_bin.display(),
                e
            )
        })?;

        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let serial_lines = Arc::new(Mutex::new(VecDeque::with_capacity(2048)));

        if let Some(out) = stdout {
            spawn_serial_reader(out, serial_lines.clone(), serial_log_path.clone(), "", "");
        }

        if let Some(err) = stderr {
            spawn_serial_reader(
                err,
                serial_lines.clone(),
                serial_log_path,
                "[stderr] ",
                "[qemu] ",
            );
        }

        Ok(Self {
            config,
            qemu_bin,
            kernel_img,
            bridge_bin,
            child: Some(child),
            serial_stdin: Mutex::new(stdin),
            serial_lines,
        })
    }

    pub fn send_serial_input(&self, input: &str) -> Result<(), String> {
        let mut guard = self.serial_stdin.lock().unwrap();
        let Some(ref mut stdin) = *guard else {
            return Err("serial stdin is closed".to_string());
        };
        // ARM PL011 UART (ttyAMA0) has a 16/32-byte hardware RX FIFO.
        // Pace writes in 12-byte chunks so long one-liners never overflow the UART FIFO.
        let bytes = input.as_bytes();
        for (i, chunk) in bytes.chunks(12).enumerate() {
            if i > 0 {
                thread::sleep(Duration::from_millis(4));
            }
            stdin
                .write_all(chunk)
                .and_then(|_| stdin.flush())
                .map_err(|e| format!("failed to write to serial stdin: {}", e))?;
        }
        Ok(())
    }

    pub fn clear_serial_lines(&self) {
        self.serial_lines.lock().unwrap().clear();
    }

    pub fn push_serial_line(&self, line: impl Into<String>) {
        let mut guard = self.serial_lines.lock().unwrap();
        if guard.len() >= 2000 {
            guard.pop_front();
        }
        guard.push_back(line.into());
    }

    pub fn serial_lines(&self) -> Vec<String> {
        self.serial_lines.lock().unwrap().iter().cloned().collect()
    }

    pub fn is_running(&mut self) -> bool {
        let Some(ref mut child) = self.child else {
            return false;
        };
        matches!(child.try_wait(), Ok(None))
    }

    pub fn stop(&mut self) {
        *self.serial_stdin.lock().unwrap() = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Ok(Some(_)) = child.try_wait() {
                    break;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

impl Drop for EmulatorSession {
    fn drop(&mut self) {
        self.stop();
    }
}
