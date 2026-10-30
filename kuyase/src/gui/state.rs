use crate::bridge_client::BridgeClient;
use crate::gui::host_audio::HostAudioPlayer;
use crate::session::{
    discover_guest_images, resolve_bridge_binary, resolve_kernel_image, resolve_qemu_binary,
    EmulatorSession, SessionConfig,
};
use protocol::{BridgeStateSnapshot, HostCommand};
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub selected_board: String,
    pub mode: String,
    pub clean_boot_snapshot: bool,
    pub layout_preset: String,
    pub bridge_port: u16,
    pub adb_port: u16,
    pub last_adb_target: String,
    pub custom_qemu_path: String,
    pub custom_kernel_path: String,
    pub audio_muted: bool,
    pub selected_dump_path: String,
    pub active_terminal_tab: String,
    pub tile_tree: Option<serde_json::Value>,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            selected_board: "yandexmini2".to_string(),
            mode: "stub".to_string(),
            clean_boot_snapshot: true,
            layout_preset: "workbench".to_string(),
            bridge_port: 18091,
            adb_port: 15555,
            last_adb_target: "192.168.1.100:5555".to_string(),
            custom_qemu_path: String::new(),
            custom_kernel_path: String::new(),
            audio_muted: false,
            selected_dump_path: String::new(),
            active_terminal_tab: "serial".to_string(),
            tile_tree: None,
        }
    }
}

pub struct GuiAppState {
    pub root_dir: PathBuf,
    pub config: UiConfig,
    pub session: Option<EmulatorSession>,
    pub bridge: Option<Arc<BridgeClient>>,
    pub latest_state: Option<BridgeStateSnapshot>,
    pub audio_player: HostAudioPlayer,
    pub guest_images: Vec<(String, PathBuf, u64)>,
    pub firmware_dumps: Vec<(String, PathBuf)>,
    pub qemu_path: Option<PathBuf>,
    pub kernel_path: Option<PathBuf>,
    pub bridge_bin_path: Option<PathBuf>,
    pub bg_task_running: Arc<AtomicBool>,
    pub bg_task_status: Arc<Mutex<String>>,
    pub was_bg_running: bool,
    pub status_message: String,
    pub serial_input: String,
    pub terminal_autoscroll: bool,
    pub connecting_bridge: Arc<AtomicBool>,
    pub pending_bridge: Arc<Mutex<Option<Result<BridgeClient, String>>>>,
    // Interactive Twin & Dock controls (shared with Tickets 05 & 06)
    pub button_press_action: String,
    pub selected_led_pattern: String,
    pub clock_text_input: String,
    pub clock_brightness: u8,
    pub matrix_text_input: String,
    pub matrix_custom_pixels: Vec<u8>,
    pub doa_slider: u32,
    pub tof_slider: u32,
    pub als_slider: u32,
    pub temp_slider: f32,
    pub volume_slider: u32,
    pub voice_phrase: String,
    pub voice_doa: u32,
    pub voice_rms: u32,
    pub voice_wake_only: bool,
    pub ipc_vocalize_text: String,
    pub last_voice_inject_at: Option<Instant>,
}

impl GuiAppState {
    pub fn ui_config_path(root_dir: &Path) -> PathBuf {
        root_dir.join("work").join("kuyase-ui.json")
    }

    pub fn load_config(root_dir: &Path) -> UiConfig {
        let path = Self::ui_config_path(root_dir);
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<UiConfig>(&raw) {
                return cfg;
            }
        }
        UiConfig::default()
    }

    pub fn save_config(&self) -> Result<(), String> {
        let path = Self::ui_config_path(&self.root_dir);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(&self.config).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())
    }

    pub fn new(root_dir: &Path) -> Self {
        let config = Self::load_config(root_dir);
        let audio_player = HostAudioPlayer::new(config.audio_muted);
        let mut app = Self {
            root_dir: root_dir.to_path_buf(),
            config,
            session: None,
            bridge: None,
            latest_state: None,
            audio_player,
            guest_images: Vec::new(),
            firmware_dumps: Vec::new(),
            qemu_path: None,
            kernel_path: None,
            bridge_bin_path: None,
            bg_task_running: Arc::new(AtomicBool::new(false)),
            bg_task_status: Arc::new(Mutex::new(String::from("Idle"))),
            was_bg_running: false,
            status_message: String::from("Ready"),
            serial_input: String::new(),
            terminal_autoscroll: true,
            connecting_bridge: Arc::new(AtomicBool::new(false)),
            pending_bridge: Arc::new(Mutex::new(None)),
            button_press_action: "single".to_string(),
            selected_led_pattern: String::new(),
            clock_text_input: "19:30°".to_string(),
            clock_brightness: 255,
            matrix_text_input: "19:30°".to_string(),
            matrix_custom_pixels: vec![0u8; 252],
            doa_slider: 0,
            tof_slider: 8190,
            als_slider: 240,
            temp_slider: 40.0,
            volume_slider: 7,
            voice_phrase: "включи ночной режим".to_string(),
            voice_doa: 90,
            voice_rms: 2200,
            voice_wake_only: false,
            ipc_vocalize_text: "Привет! Сегодня в Москве плюс 21 градус, без осадков.".to_string(),
            last_voice_inject_at: None,
        };
        app.refresh_dependencies();
        app.refresh_images_and_dumps();
        let _ = app.save_config();
        app
    }

    pub fn set_layout_preset(&mut self, preset: &str) {
        self.config.layout_preset = if preset == "compact" {
            "compact".to_string()
        } else {
            "workbench".to_string()
        };
        let tree = crate::gui::build_tree_for_preset(&self.config.layout_preset);
        self.config.tile_tree = serde_json::to_value(&tree).ok();
        let _ = self.save_config();
    }

    pub fn refresh_dependencies(&mut self) {
        let custom_q = PathBuf::from(self.config.custom_qemu_path.trim());
        self.qemu_path = if !self.config.custom_qemu_path.trim().is_empty() && custom_q.is_file() {
            Some(custom_q)
        } else {
            resolve_qemu_binary(&self.root_dir).ok()
        };

        let custom_k = PathBuf::from(self.config.custom_kernel_path.trim());
        self.kernel_path =
            if !self.config.custom_kernel_path.trim().is_empty() && custom_k.is_file() {
                Some(custom_k)
            } else {
                resolve_kernel_image(&self.root_dir).ok()
            };

        self.bridge_bin_path = resolve_bridge_binary(&self.root_dir).ok();
    }

    pub fn refresh_images_and_dumps(&mut self) {
        self.guest_images = discover_guest_images(&self.root_dir)
            .into_iter()
            .map(|(slug, path)| {
                let sz = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                (slug, path, sz)
            })
            .collect();

        if !self.guest_images.is_empty()
            && !self
                .guest_images
                .iter()
                .any(|(b, _, _)| b == &self.config.selected_board)
        {
            self.config.selected_board = self.guest_images[0].0.clone();
        }

        let mut dumps = Vec::new();
        let backups = self.root_dir.join("backups");
        if let Ok(rd) = fs::read_dir(&backups) {
            for e in rd.flatten() {
                let p = e.path();
                let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if name == "README.md" || name.starts_with('.') {
                    continue;
                }
                dumps.push((name.to_string(), p));
            }
        }
        dumps.sort_by(|a, b| a.0.cmp(&b.0));
        if self.config.selected_dump_path.is_empty() {
            if let Some((_, first)) = dumps.first() {
                self.config.selected_dump_path = first.display().to_string();
            }
        }
        self.firmware_dumps = dumps;
    }

    pub fn start_vm(&mut self) {
        self.stop_vm();
        self.refresh_dependencies();
        let mut cfg = match SessionConfig::for_board(
            &self.root_dir,
            Some(&self.config.selected_board),
        ) {
            Ok(c) => c,
            Err(e) => {
                self.status_message = format!("Cannot start VM: {}", e);
                return;
            }
        };
        cfg.mode = self.config.mode.clone();
        cfg.snapshot = self.config.clean_boot_snapshot;
        cfg.bridge_port = self.config.bridge_port;
        cfg.adb_port = self.config.adb_port;

        let cq = self.qemu_path.as_deref();
        let ck = self.kernel_path.as_deref();
        match EmulatorSession::start_with_overrides(cfg.clone(), cq, ck) {
            Ok(sess) => {
                self.status_message = format!(
                    "VM started for '{}' (mode={}, snapshot={}). Connecting bridge :{}...",
                    cfg.board, cfg.mode, cfg.snapshot, cfg.bridge_port
                );
                self.session = Some(sess);
                let _ = self.save_config();
                self.spawn_bridge_connector(cfg.bridge_port);
            }
            Err(e) => {
                self.status_message = format!("VM launch failed: {}", e);
            }
        }
    }

    fn spawn_bridge_connector(&mut self, bridge_port: u16) {
        if self.connecting_bridge.swap(true, Ordering::SeqCst) {
            return;
        }
        let conn_flag = self.connecting_bridge.clone();
        let slot = self.pending_bridge.clone();
        thread::spawn(move || {
            let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bridge_port);
            let res = BridgeClient::connect_with_timeout(addr, Duration::from_secs(28));
            *slot.lock().unwrap() = Some(res);
            conn_flag.store(false, Ordering::SeqCst);
        });
    }

    pub fn stop_vm(&mut self) {
        if let Some(ref b) = self.bridge {
            b.close();
        }
        self.bridge = None;
        self.latest_state = None;
        self.connecting_bridge.store(false, Ordering::SeqCst);
        *self.pending_bridge.lock().unwrap() = None;
        if let Some(mut s) = self.session.take() {
            s.stop();
            self.status_message = "VM stopped.".to_string();
        }
    }

    pub fn restart_vm(&mut self) {
        self.stop_vm();
        self.start_vm();
    }

    pub fn poll_runtime(&mut self) {
        let now_bg = self.bg_task_running.load(Ordering::Relaxed);
        if self.was_bg_running && !now_bg {
            self.refresh_dependencies();
            self.refresh_images_and_dumps();
        }
        self.was_bg_running = now_bg;

        if let Some(res) = self.pending_bridge.lock().unwrap().take() {
            match res {
                Ok(client) => {
                    self.audio_player.attach_bridge(&client);
                    let snap = client.latest_state();
                    self.volume_slider = snap.volume_level;
                    self.doa_slider = snap.doa_angle;
                    self.tof_slider = snap.tof_mm;
                    self.als_slider = snap.als_lux;
                    self.temp_slider = snap.temp_c;
                    self.latest_state = Some(snap);
                    self.bridge = Some(Arc::new(client));
                    self.status_message =
                        format!("Bridge connected on 127.0.0.1:{}", self.config.bridge_port);
                }
                Err(e) => {
                    self.status_message = format!("Bridge connection failed: {}", e);
                }
            }
        }
        if let Some(ref b) = self.bridge {
            if b.is_connected() {
                self.latest_state = Some(b.latest_state());
            } else {
                self.status_message = "Bridge disconnected.".to_string();
                self.bridge = None;
            }
        }
        if let Some(ref mut s) = self.session {
            if !s.is_running() {
                self.status_message = "QEMU child process exited.".to_string();
                self.session = None;
                self.bridge = None;
            }
        }
    }

    pub fn send_command(&mut self, cmd: HostCommand) {
        if matches!(cmd, HostCommand::InjectVoice { .. }) {
            self.last_voice_inject_at = Some(Instant::now());
        }
        let Some(bridge) = self.bridge.clone() else {
            self.status_message = "Bridge is not connected.".to_string();
            return;
        };
        match bridge.send_command(&cmd) {
            Ok(ack) => {
                self.latest_state = Some(bridge.latest_state());
                if !ack.ok {
                    self.status_message = format!(
                        "Command rejected: {}",
                        ack.error.unwrap_or_else(|| "error".to_string())
                    );
                }
            }
            Err(e) => self.status_message = format!("Command error: {}", e),
        }
    }

    pub fn send_serial_command(&mut self, cmd_line: &str) {
        let Some(ref sess) = self.session else {
            self.status_message = "VM is not running.".to_string();
            return;
        };
        let payload = format!("{}\n", cmd_line.trim_end_matches(&['\r', '\n'][..]));
        if let Err(e) = sess.send_serial_input(&payload) {
            self.status_message = format!("ttyAMA0 write failed: {}", e);
        }
    }
}

impl Drop for GuiAppState {
    fn drop(&mut self) {
        self.stop_vm();
        let _ = self.save_config();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ui_config_portable_persistence_and_presets() {
        let tmp = std::env::temp_dir().join(format!("kuyase-gui-state-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("bin")).unwrap();
        fs::create_dir_all(tmp.join("kernel")).unwrap();
        fs::write(tmp.join("bin").join("qemu-system-aarch64.exe"), b"mock").unwrap();
        fs::write(tmp.join("kernel").join("Image"), b"mock-kernel").unwrap();

        let mut app = GuiAppState::new(&tmp);
        assert!(GuiAppState::ui_config_path(&tmp).is_file());
        assert!(app.qemu_path.is_some());
        assert!(app.kernel_path.is_some());
        assert_eq!(app.config.layout_preset, "workbench");

        app.config.selected_board = "orion".to_string();
        app.config.mode = "full".to_string();
        app.config.clean_boot_snapshot = false;
        app.set_layout_preset("compact");

        let reloaded = GuiAppState::load_config(&tmp);
        assert_eq!(reloaded.selected_board, "orion");
        assert_eq!(reloaded.mode, "full");
        assert!(!reloaded.clean_boot_snapshot);
        assert_eq!(reloaded.layout_preset, "compact");
        assert!(reloaded.tile_tree.is_some());

        let restored_tree: egui_tiles::Tree<crate::gui::Pane> =
            serde_json::from_value(reloaded.tile_tree.unwrap()).unwrap();
        assert_eq!(
            restored_tree.id(),
            eframe::egui::Id::new("kuyase_compact_tree")
        );

        let _ = fs::remove_dir_all(&tmp);
    }
}
