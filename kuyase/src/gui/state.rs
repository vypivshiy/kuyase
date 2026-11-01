use crate::bridge_client::BridgeClient;
use crate::gui::host_audio::HostAudioPlayer;
use crate::session::{
    discover_guest_images, resolve_bridge_binary, resolve_kernel_image, resolve_qemu_binary,
    EmulatorSession, SessionConfig,
};
use protocol::{normalize_board_slug, BridgeStateSnapshot, HostCommand};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const MAX_SIGNAL_LOGS: usize = 500;
pub const TOF_WAVE_DURATION_MS: u64 = 900;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalDirection {
    Tx,
    Rx,
}

impl SignalDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tx => "TX",
            Self::Rx => "RX",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalCategory {
    Bridge,
    TofButtons,
    TtsIpc,
    LedDisplay,
    Audio,
}

impl SignalCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bridge => "BRIDGE",
            Self::TofButtons => "TOF/KEY",
            Self::TtsIpc => "TTS/IPC",
            Self::LedDisplay => "LED/DISP",
            Self::Audio => "AUDIO",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalFilter {
    All,
    TofButtons,
    TtsIpc,
    LedDisplay,
    Audio,
}

impl SignalFilter {
    pub fn matches(self, cat: SignalCategory) -> bool {
        match self {
            Self::All => true,
            Self::TofButtons => cat == SignalCategory::TofButtons,
            Self::TtsIpc => cat == SignalCategory::TtsIpc,
            Self::LedDisplay => cat == SignalCategory::LedDisplay,
            Self::Audio => cat == SignalCategory::Audio,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalLogEntry {
    pub timestamp: String,
    pub direction: SignalDirection,
    pub category: SignalCategory,
    pub message: String,
}

pub fn format_hms_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

pub fn describe_host_command(cmd: &HostCommand) -> (SignalCategory, String) {
    match cmd {
        HostCommand::RequestState => (
            SignalCategory::Bridge,
            "RequestState -> запрос снимка состояния".to_string(),
        ),
        HostCommand::PressButton { button, action } => (
            SignalCategory::TofButtons,
            format!("PressButton {{ button: \"{}\", action: \"{}\" }}", button, action),
        ),
        HostCommand::SetVolume { volume_level } => (
            SignalCategory::Audio,
            format!("SetVolume {{ volume_level: {} }}", volume_level),
        ),
        HostCommand::SetLedPattern { pattern } => (
            SignalCategory::LedDisplay,
            format!(
                "SetLedPattern {{ pattern: \"{}\" }}",
                if pattern.is_empty() { "auto" } else { pattern }
            ),
        ),
        HostCommand::SetClockDisplay {
            segments,
            text,
            pixels,
            brightness,
            scale_brightness,
            ..
        } => {
            if let Some(ref txt) = text {
                (
                    SignalCategory::LedDisplay,
                    format!(
                        "SetClockDisplay {{ text: \"{}\", brightness: {} }}",
                        txt,
                        brightness.unwrap_or(255)
                    ),
                )
            } else if let Some(ref segs) = segments {
                let summary: Vec<String> = segs
                    .iter()
                    .take(4)
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect();
                (
                    SignalCategory::LedDisplay,
                    format!("SetClockDisplay {{ segments: [{}] }}", summary.join(", ")),
                )
            } else if let Some(ref px) = pixels {
                let lit = px.iter().filter(|&&b| b > 0).count();
                (
                    SignalCategory::LedDisplay,
                    format!("SetClockDisplay {{ matrix_21x12: {} активных пикс. }}", lit),
                )
            } else if let Some(sb) = scale_brightness {
                (
                    SignalCategory::LedDisplay,
                    format!("SetClockDisplay {{ scale_brightness: {} }}", sb),
                )
            } else {
                (
                    SignalCategory::LedDisplay,
                    "SetClockDisplay (обновление дисплея)".to_string(),
                )
            }
        }
        HostCommand::SetSensor {
            doa_angle,
            mic_rms,
            als_lux,
            temp_c,
            tof_mm,
            swipe,
            swipe_direction,
        } => {
            let mut parts = Vec::new();
            if let Some(mm) = tof_mm {
                parts.push(format!("tof_mm={}", mm));
            }
            if let Some(ref dir) = swipe_direction {
                parts.push(format!("swipe=\"{}\"", dir));
            } else if swipe == &Some(true) {
                parts.push("swipe=true".to_string());
            }
            if let Some(doa) = doa_angle {
                parts.push(format!("doa={} град", doa));
            }
            if let Some(lux) = als_lux {
                parts.push(format!("als={} лк", lux));
            }
            if let Some(t) = temp_c {
                parts.push(format!("temp={:.1} C", t));
            }
            if let Some(rms) = mic_rms {
                parts.push(format!("mic_rms={}", rms));
            }
            (
                SignalCategory::TofButtons,
                format!("SetSensor {{ {} }}", parts.join(", ")),
            )
        }
        HostCommand::InjectVoice {
            phrase,
            doa_angle,
            wake_only,
            ..
        } => (
            SignalCategory::TtsIpc,
            format!(
                "InjectVoice {{ phrase: \"{}\", doa: {}, wake_only: {} }} -> :9850",
                phrase,
                doa_angle.unwrap_or(0),
                wake_only
            ),
        ),
        HostCommand::IpcTest { cmd, text } => {
            if let Some(ref t) = text {
                (
                    SignalCategory::TtsIpc,
                    format!("IpcTest {{ cmd: \"{}\", text: \"{}\" }} -> aliced :9875", cmd, t),
                )
            } else {
                (
                    SignalCategory::TtsIpc,
                    format!("IpcTest {{ cmd: \"{}\" }} -> aliced :9875", cmd),
                )
            }
        }
        HostCommand::PlayTestTone => (
            SignalCategory::Audio,
            "PlayTestTone (синус 440 Гц -> snd-aloop 48 кГц)".to_string(),
        ),
        HostCommand::ClearAudio => (
            SignalCategory::Audio,
            "ClearAudio (очистка буфера PCM)".to_string(),
        ),
        HostCommand::PowerOff => (
            SignalCategory::Bridge,
            "PowerOff (завершение работы гостевой ОС)".to_string(),
        ),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub selected_board: String,
    pub mode: String,
    pub clean_boot_snapshot: bool,
    pub persist_data_disk: bool,
    pub bind_all_interfaces: bool,
    pub lan_tap_interface: String,
    pub lan_mac_address: String,
    pub bridge_port: u16,
    pub adb_port: u16,
    pub last_adb_target: String,
    pub custom_qemu_path: String,
    pub custom_kernel_path: String,
    pub custom_images_dir: String,
    pub custom_image_file: String,
    pub audio_muted: bool,
    pub build_source_mode: String,
    pub selected_dump_path: String,
    pub active_terminal_tab: String,
    pub dock_collapsed: bool,
    pub dock_large: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            selected_board: "cucumber".to_string(),
            mode: "stub".to_string(),
            clean_boot_snapshot: true,
            persist_data_disk: false,
            bind_all_interfaces: false,
            lan_tap_interface: String::new(),
            lan_mac_address: crate::session::DEFAULT_LAN_MAC_BASE.to_string(),
            bridge_port: 18091,
            adb_port: 15555,
            last_adb_target: "192.168.1.100:5555".to_string(),
            custom_qemu_path: String::new(),
            custom_kernel_path: String::new(),
            custom_images_dir: String::new(),
            custom_image_file: String::new(),
            audio_muted: false,
            build_source_mode: "adb".to_string(),
            selected_dump_path: String::new(),
            active_terminal_tab: "signals".to_string(),
            dock_collapsed: false,
            dock_large: false,
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
    pub serial_cwd: String,
    pub serial_dir_cache: std::collections::BTreeMap<String, (Instant, Vec<String>)>,
    pub serial_history: Vec<String>,
    pub serial_history_index: Option<usize>,
    pub serial_draft_input: String,
    pub terminal_autoscroll: bool,
    pub show_image_manager_modal: bool,
    pub connecting_bridge: Arc<AtomicBool>,
    pub pending_bridge: Arc<Mutex<Option<Result<BridgeClient, String>>>>,
    pub pending_picked_file: Arc<Mutex<Option<PathBuf>>>,
    pub pending_picked_dir: Arc<Mutex<Option<PathBuf>>>,
    pub pending_picked_qemu: Arc<Mutex<Option<PathBuf>>>,
    pub pending_picked_kernel: Arc<Mutex<Option<PathBuf>>>,
    pub signal_logs: VecDeque<SignalLogEntry>,
    pub signal_filter: SignalFilter,
    pub last_logged_guest_line: String,
    pub tof_wave_reset_at: Option<Instant>,
    pub tof_restore_distance: u32,
    // Controls
    pub button_press_action: String,
    pub selected_led_pattern: String,
    pub clock_text_input: String,
    pub clock_brightness: u8,
    pub clock_active_mask: std::collections::BTreeMap<String, bool>,
    pub matrix_text_input: String,
    pub matrix_custom_pixels: Vec<u8>,
    pub matrix_active_mask: Vec<bool>,
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
            if let Ok(mut cfg) = serde_json::from_str::<UiConfig>(&raw) {
                cfg.mode = "stub".to_string();
                if !matches!(
                    cfg.active_terminal_tab.as_str(),
                    "signals" | "serial" | "maind"
                ) {
                    cfg.active_terminal_tab = "signals".to_string();
                }
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
            status_message: String::from("Готов к запуску"),
            serial_input: String::new(),
            serial_cwd: String::from("/"),
            serial_dir_cache: std::collections::BTreeMap::new(),
            serial_history: Vec::new(),
            serial_history_index: None,
            serial_draft_input: String::new(),
            terminal_autoscroll: true,
            show_image_manager_modal: false,
            connecting_bridge: Arc::new(AtomicBool::new(false)),
            pending_bridge: Arc::new(Mutex::new(None)),
            pending_picked_file: Arc::new(Mutex::new(None)),
            pending_picked_dir: Arc::new(Mutex::new(None)),
            pending_picked_qemu: Arc::new(Mutex::new(None)),
            pending_picked_kernel: Arc::new(Mutex::new(None)),
            signal_logs: VecDeque::with_capacity(MAX_SIGNAL_LOGS),
            signal_filter: SignalFilter::All,
            last_logged_guest_line: String::new(),
            tof_wave_reset_at: None,
            tof_restore_distance: 8190,
            button_press_action: "single".to_string(),
            selected_led_pattern: String::new(),
            clock_text_input: "19:30".to_string(),
            clock_brightness: 255,
            clock_active_mask: crate::gui::canvas::build_local_segments_from_text("19:30", 255)
                .into_iter()
                .filter(|(_, v)| *v > 0)
                .map(|(k, _)| (k, true))
                .collect(),
            matrix_text_input: "19:30".to_string(),
            matrix_custom_pixels: crate::gui::canvas::render_text_to_matrix_21x12("19:30", 255),
            matrix_active_mask: crate::gui::canvas::render_text_to_matrix_21x12("19:30", 255)
                .into_iter()
                .map(|v| v > 0)
                .collect(),
            doa_slider: 90,
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
        app.push_signal(
            SignalDirection::Rx,
            SignalCategory::Bridge,
            format!(
                "Инициализирован интерфейс KUYASE (найдено образов: {})",
                app.guest_images.len()
            ),
        );
        let _ = app.save_config();
        app
    }

    pub fn push_signal(
        &mut self,
        direction: SignalDirection,
        category: SignalCategory,
        message: impl Into<String>,
    ) {
        if self.signal_logs.len() >= MAX_SIGNAL_LOGS {
            self.signal_logs.pop_front();
        }
        self.signal_logs.push_back(SignalLogEntry {
            timestamp: format_hms_now(),
            direction,
            category,
            message: message.into(),
        });
    }

    pub fn clear_signals(&mut self) {
        self.signal_logs.clear();
    }

    pub fn filtered_signals(&self) -> Vec<SignalLogEntry> {
        self.signal_logs
            .iter()
            .filter(|e| self.signal_filter.matches(e.category))
            .cloned()
            .collect()
    }

    pub fn record_state_transition(
        &mut self,
        prev_opt: Option<&BridgeStateSnapshot>,
        next: &BridgeStateSnapshot,
    ) {
        if let Some(prev) = prev_opt {
            if prev.alice_state != next.alice_state {
                self.push_signal(
                    SignalDirection::Rx,
                    SignalCategory::TtsIpc,
                    format!(
                        "aliced (:9875): состояние {} -> {}",
                        prev.alice_state, next.alice_state
                    ),
                );
            }
            if prev.last_spoken_text != next.last_spoken_text && !next.last_spoken_text.is_empty() {
                self.push_signal(
                    SignalDirection::Rx,
                    SignalCategory::TtsIpc,
                    format!("Синтезатор речи TTS: \"{}\"", next.last_spoken_text),
                );
            }
            if next.aliced_hijack_count > prev.aliced_hijack_count {
                self.push_signal(
                    SignalDirection::Rx,
                    SignalCategory::TtsIpc,
                    format!(
                        "Перехват диалога в окне 1680 мс (aliced_hijack_count = {})",
                        next.aliced_hijack_count
                    ),
                );
            }
            if next.led_trigger_conflict_count > prev.led_trigger_conflict_count {
                self.push_signal(
                    SignalDirection::Rx,
                    SignalCategory::LedDisplay,
                    format!(
                        "Конфликт триггера sysfs [ledfb-ring] (led_trigger_conflict_count = {})",
                        next.led_trigger_conflict_count
                    ),
                );
            }
            if prev.ring_trigger != next.ring_trigger {
                self.push_signal(
                    SignalDirection::Rx,
                    SignalCategory::LedDisplay,
                    format!("Триггер кольца LED: {} -> {}", prev.ring_trigger, next.ring_trigger),
                );
            }
            if prev.mics_muted != next.mics_muted {
                self.push_signal(
                    SignalDirection::Rx,
                    SignalCategory::TofButtons,
                    format!(
                        "Микрофоны: {}",
                        if next.mics_muted { "ОТКЛЮЧЕНЫ (MUTE)" } else { "ВКЛЮЧЕНЫ" }
                    ),
                );
            }
            if prev.volume_level != next.volume_level {
                self.push_signal(
                    SignalDirection::Rx,
                    SignalCategory::Audio,
                    format!("Громкость изменена: {}/10", next.volume_level),
                );
            }
            if (prev.speaker_rms <= 5 && next.speaker_rms > 5)
                || (prev.speaker_rms > 5 && next.speaker_rms <= 5)
            {
                let msg = if next.speaker_rms > 5 {
                    format!("Активен аудиопоток 48 кГц (Speaker RMS: {})", next.speaker_rms)
                } else {
                    "Аудиопоток 48 кГц завершён (Speaker RMS: 0)".to_string()
                };
                self.push_signal(SignalDirection::Rx, SignalCategory::Audio, msg);
            }
        } else {
            self.push_signal(
                SignalDirection::Rx,
                SignalCategory::Bridge,
                format!(
                    "Подключен мост: {} ({}, {} LED, триггер: {})",
                    next.board_name, next.board_slug, next.ring_leds, next.ring_trigger
                ),
            );
        }

        if let Some(last_line) = next.recent_logs.last() {
            if last_line != &self.last_logged_guest_line && !last_line.is_empty() {
                self.last_logged_guest_line = last_line.clone();
                let cat = if last_line.contains("ToF")
                    || last_line.contains("KEY_")
                    || last_line.contains("button")
                {
                    SignalCategory::TofButtons
                } else if last_line.contains("LED")
                    || last_line.contains("Clock")
                    || last_line.contains("SN3236")
                {
                    SignalCategory::LedDisplay
                } else if last_line.contains("TTS")
                    || last_line.contains("VOICE")
                    || last_line.contains("HIJACK")
                    || last_line.contains("IPC")
                {
                    SignalCategory::TtsIpc
                } else if last_line.contains("Audio") || last_line.contains("tone") {
                    SignalCategory::Audio
                } else {
                    SignalCategory::Bridge
                };
                self.push_signal(SignalDirection::Rx, cat, format!("hw-bridge: {}", last_line));
            }
        }
    }

    pub fn trigger_tof_wave(&mut self) {
        if self.tof_wave_reset_at.is_none() {
            self.tof_restore_distance = if self.tof_slider <= 200 {
                8190
            } else {
                self.tof_slider
            };
        }
        self.tof_slider = 140;
        self.tof_wave_reset_at = Some(Instant::now() + Duration::from_millis(TOF_WAVE_DURATION_MS));
        self.send_command(HostCommand::SetSensor {
            doa_angle: None,
            mic_rms: None,
            als_lux: None,
            temp_c: None,
            tof_mm: Some(140),
            swipe: Some(true),
            swipe_direction: Some("wave".to_string()),
        });
    }

    pub fn set_custom_images_dir(&mut self, dir: PathBuf) {
        if dir.is_dir() {
            self.config.custom_images_dir = dir.display().to_string();
            self.refresh_images_and_dumps();
            let _ = self.save_config();
            self.push_signal(
                SignalDirection::Tx,
                SignalCategory::Bridge,
                format!(
                    "Выбрана папка образов: {} (найдено образов: {})",
                    dir.display(),
                    self.guest_images.len()
                ),
            );
        }
    }

    pub fn select_custom_file(&mut self, file: PathBuf) {
        if !file.is_file() {
            return;
        }
        let name = file
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if name.ends_with(".ext4") {
            let stem = name
                .trim_end_matches("-system.ext4")
                .trim_end_matches(".ext4");
            let slug = normalize_board_slug(stem);
            self.config.custom_image_file = file.display().to_string();
            if let Some(parent) = file.parent() {
                self.config.custom_images_dir = parent.display().to_string();
            }
            self.config.selected_board = slug.clone();
            self.refresh_images_and_dumps();
            let _ = self.save_config();
            self.push_signal(
                SignalDirection::Tx,
                SignalCategory::Bridge,
                format!("Выбран файл образа: {} (профиль: {})", file.display(), slug),
            );
            self.status_message = format!("Выбран образ: {}", file.display());
        } else {
            self.config.selected_dump_path = file.display().to_string();
            let _ = self.save_config();
            self.push_signal(
                SignalDirection::Tx,
                SignalCategory::Bridge,
                format!("Выбран файл дампа для сборки .ext4: {}", file.display()),
            );
            self.status_message = format!("Выбран файл дампа: {}", file.display());
        }
    }

    pub fn open_native_file_dialog(&self) {
        let slot = self.pending_picked_file.clone();
        let start_dir = Path::new(self.config.selected_dump_path.trim())
            .parent()
            .filter(|p| p.is_dir())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| {
                let backups = self.root_dir.join("backups");
                if backups.is_dir() {
                    backups
                } else {
                    self.root_dir.clone()
                }
            });
        thread::spawn(move || {
            if let Some(p) = run_os_file_dialog(&start_dir) {
                *slot.lock().unwrap() = Some(p);
            }
        });
    }

    pub fn open_native_folder_dialog(&self) {
        let slot = self.pending_picked_dir.clone();
        let start_dir = if !self.config.custom_images_dir.trim().is_empty() {
            PathBuf::from(self.config.custom_images_dir.trim())
        } else {
            self.root_dir.join("work")
        };
        thread::spawn(move || {
            if let Some(p) = run_os_folder_dialog(&start_dir) {
                *slot.lock().unwrap() = Some(p);
            }
        });
    }

    pub fn open_qemu_file_dialog(&self) {
        let slot = self.pending_picked_qemu.clone();
        let start_dir = self
            .qemu_path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| self.root_dir.clone());
        thread::spawn(move || {
            if let Some(p) = run_os_custom_file_dialog(
                &start_dir,
                "Выберите исполняемый файл qemu-system-aarch64",
                "QEMU (qemu-system-aarch64*)|qemu-system-aarch64*;*.exe|Все файлы (*.*)|*.*",
            ) {
                *slot.lock().unwrap() = Some(p);
            }
        });
    }

    pub fn open_kernel_file_dialog(&self) {
        let slot = self.pending_picked_kernel.clone();
        let start_dir = self
            .kernel_path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| self.root_dir.join("kernel"));
        thread::spawn(move || {
            if let Some(p) = run_os_custom_file_dialog(
                &start_dir,
                "Выберите образ гостевого ядра Linux (kuyase-kernel-arm64.img / Image)",
                "Ядро Linux (kuyase-kernel-arm64.img; Image*)|*.img;Image*;*vmlinuz*|Все файлы (*.*)|*.*",
            ) {
                *slot.lock().unwrap() = Some(p);
            }
        });
    }

    pub fn resolve_custom_or_rel_file(&self, raw: &str) -> Option<PathBuf> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        let direct = PathBuf::from(trimmed);
        if direct.is_file() {
            return Some(direct);
        }
        let rel = self.root_dir.join(trimmed);
        if rel.is_file() {
            return Some(rel);
        }
        None
    }

    pub fn refresh_dependencies(&mut self) {
        self.qemu_path = if !self.config.custom_qemu_path.trim().is_empty() {
            self.resolve_custom_or_rel_file(&self.config.custom_qemu_path)
        } else {
            resolve_qemu_binary(&self.root_dir).ok()
        };

        self.kernel_path = if !self.config.custom_kernel_path.trim().is_empty() {
            self.resolve_custom_or_rel_file(&self.config.custom_kernel_path)
        } else {
            resolve_kernel_image(&self.root_dir).ok()
        };

        self.bridge_bin_path = resolve_bridge_binary(&self.root_dir).ok();
    }

    pub fn refresh_images_and_dumps(&mut self) {
        let mut list: Vec<(String, PathBuf, u64)> = Vec::new();

        if !self.config.custom_images_dir.trim().is_empty() {
            let custom_dir = PathBuf::from(self.config.custom_images_dir.trim());
            for (slug, path) in discover_images_in_folder(&custom_dir) {
                let sz = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                list.push((slug, path, sz));
            }
        }

        for (slug, path) in discover_guest_images(&self.root_dir) {
            if !list.iter().any(|(_, p, _)| p == &path) {
                let sz = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                list.push((slug, path, sz));
            }
        }

        if !self.config.custom_image_file.trim().is_empty() {
            let cf = PathBuf::from(self.config.custom_image_file.trim());
            if cf.is_file() && !list.iter().any(|(_, p, _)| p == &cf) {
                let stem = cf
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("custom")
                    .trim_end_matches("-system.ext4")
                    .trim_end_matches(".ext4")
                    .trim_end_matches(".img");
                let slug = normalize_board_slug(stem);
                let sz = fs::metadata(&cf).map(|m| m.len()).unwrap_or(0);
                list.push((slug, cf, sz));
            }
        }

        list.sort_by(|a, b| a.0.cmp(&b.0));
        self.guest_images = list;

        if !self.guest_images.is_empty()
            && !self
                .guest_images
                .iter()
                .any(|(b, _, _)| b == &self.config.selected_board)
        {
            self.config.selected_board = self.guest_images[0].0.clone();
        }

        let dumps = crate::image_builder::discover_firmware_dumps(&self.root_dir);
        if self.config.selected_dump_path.is_empty() {
            if let Some((_, first)) = dumps.first() {
                self.config.selected_dump_path = first.display().to_string();
            }
        }
        self.firmware_dumps = dumps;
    }

    pub fn active_images_dir(&self) -> PathBuf {
        let trimmed = self.config.custom_images_dir.trim();
        if !trimmed.is_empty() {
            let p = PathBuf::from(trimmed);
            if p.is_dir() {
                return p;
            }
        }
        self.root_dir.join("work")
    }

    pub fn selected_board_data_image_path(&self) -> PathBuf {
        crate::image_builder::persistent_data_image_path(
            &self.active_images_dir(),
            &self.config.selected_board,
        )
    }

    pub fn resolve_selected_image_path(&self) -> Option<PathBuf> {
        if !self.config.custom_image_file.trim().is_empty() {
            let cf = PathBuf::from(self.config.custom_image_file.trim());
            if cf.is_file()
                && cf
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|n| n.starts_with(&self.config.selected_board))
                    .unwrap_or(false)
            {
                return Some(cf);
            }
        }
        self.guest_images
            .iter()
            .find(|(slug, _, _)| slug == &self.config.selected_board)
            .map(|(_, path, _)| path.clone())
    }

    pub fn start_vm(&mut self) {
        self.stop_vm();
        self.refresh_dependencies();
        self.refresh_images_and_dumps();

        let data_img_exists = self.selected_board_data_image_path().is_file();
        let use_persist_data = self.config.persist_data_disk || data_img_exists;
        let lan_tap = Some(self.config.lan_tap_interface.trim().to_string())
            .filter(|s| !s.is_empty());
        let lan_mac = Some(self.config.lan_mac_address.trim().to_string())
            .filter(|s| !s.is_empty());

        let mut cfg = match SessionConfig::for_board(
            &self.root_dir,
            Some(&self.config.selected_board),
        ) {
            Ok(c) => c,
            Err(_) => {
                if let Some(img_path) = self.resolve_selected_image_path() {
                    SessionConfig {
                        root_dir: self.root_dir.clone(),
                        board: self.config.selected_board.clone(),
                        board_cmdline: "auto".to_string(),
                        mode: self.config.mode.clone(),
                        snapshot: self.config.clean_boot_snapshot,
                        persist_data: use_persist_data,
                        bind_all: self.config.bind_all_interfaces,
                        lan_tap: lan_tap.clone(),
                        lan_mac: lan_mac.clone(),
                        port_offset: 0,
                        bridge_port: self.config.bridge_port,
                        adb_port: self.config.adb_port,
                        sdk_port: Some(18090),
                        drive_img: img_path,
                    }
                } else {
                    self.status_message = format!(
                        "Не найден образ для платы '{}'",
                        self.config.selected_board
                    );
                    return;
                }
            }
        };
        if let Some(custom_img) = self.resolve_selected_image_path() {
            cfg.drive_img = custom_img;
        }
        cfg.mode = self.config.mode.clone();
        cfg.snapshot = self.config.clean_boot_snapshot;
        cfg.persist_data = use_persist_data;
        cfg.bind_all = self.config.bind_all_interfaces;
        cfg.lan_tap = lan_tap;
        cfg.lan_mac = lan_mac;
        cfg.bridge_port = self.config.bridge_port;
        cfg.adb_port = self.config.adb_port;

        let cq = self.qemu_path.as_deref();
        let ck = self.kernel_path.as_deref();
        match EmulatorSession::start_with_overrides(cfg.clone(), cq, ck) {
            Ok(sess) => {
                self.status_message = format!(
                    "Запущена ВМ '{}' ({}, mode={}, snapshot={}). Подключение к :{}...",
                    cfg.board,
                    cfg.drive_img.display(),
                    cfg.mode,
                    cfg.snapshot,
                    cfg.bridge_port
                );
                self.push_signal(
                    SignalDirection::Tx,
                    SignalCategory::Bridge,
                    format!(
                        "Старт QEMU: плата={}, образ={}, режим={}, snapshot={}",
                        cfg.board,
                        cfg.drive_img.display(),
                        cfg.mode,
                        cfg.snapshot
                    ),
                );
                self.session = Some(sess);
                let _ = self.save_config();
                self.spawn_bridge_connector(cfg.bridge_port);
            }
            Err(e) => {
                self.status_message = format!("Ошибка запуска ВМ: {}", e);
                self.push_signal(
                    SignalDirection::Rx,
                    SignalCategory::Bridge,
                    format!("Ошибка запуска QEMU: {}", e),
                );
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
        let was_running = self.session.is_some() || self.bridge.is_some();
        if let Some(ref b) = self.bridge {
            b.close();
        }
        self.bridge = None;
        self.latest_state = None;
        self.connecting_bridge.store(false, Ordering::SeqCst);
        *self.pending_bridge.lock().unwrap() = None;
        if let Some(mut s) = self.session.take() {
            s.stop();
            self.status_message = "ВМ остановлена.".to_string();
        }
        if was_running {
            self.push_signal(
                SignalDirection::Tx,
                SignalCategory::Bridge,
                "Виртуальная машина QEMU остановлена".to_string(),
            );
        }
    }

    pub fn restart_vm(&mut self) {
        self.stop_vm();
        self.start_vm();
    }

    pub fn poll_runtime(&mut self) {
        let picked_f = self.pending_picked_file.lock().unwrap().take();
        if let Some(f) = picked_f {
            self.select_custom_file(f);
        }
        let picked_d = self.pending_picked_dir.lock().unwrap().take();
        if let Some(d) = picked_d {
            self.set_custom_images_dir(d);
        }
        let picked_q = self.pending_picked_qemu.lock().unwrap().take();
        if let Some(q) = picked_q {
            self.config.custom_qemu_path = q.display().to_string();
            self.refresh_dependencies();
            let _ = self.save_config();
        }
        let picked_k = self.pending_picked_kernel.lock().unwrap().take();
        if let Some(k) = picked_k {
            self.config.custom_kernel_path = k.display().to_string();
            self.refresh_dependencies();
            let _ = self.save_config();
        }

        if let Some(reset_at) = self.tof_wave_reset_at {
            if Instant::now() >= reset_at {
                self.tof_wave_reset_at = None;
                let restore = self.tof_restore_distance;
                self.tof_slider = restore;
                if self.bridge.is_some() {
                    self.send_command(HostCommand::SetSensor {
                        doa_angle: None,
                        mic_rms: None,
                        als_lux: None,
                        temp_c: None,
                        tof_mm: Some(restore),
                        swipe: None,
                        swipe_direction: None,
                    });
                } else {
                    self.push_signal(
                        SignalDirection::Rx,
                        SignalCategory::TofButtons,
                        format!(
                            "Автовозврат датчика ToF (VL53L0X) после взмаха -> {} мм",
                            restore
                        ),
                    );
                }
            }
        }

        let now_bg = self.bg_task_running.load(Ordering::Relaxed);
        if self.was_bg_running && !now_bg {
            self.refresh_dependencies();
            self.refresh_images_and_dumps();
            let msg = self.bg_task_status.lock().unwrap().clone();
            self.push_signal(SignalDirection::Rx, SignalCategory::Bridge, msg);
        }
        self.was_bg_running = now_bg;

        let pending_res = self.pending_bridge.lock().unwrap().take();
        if let Some(res) = pending_res {
            match res {
                Ok(client) => {
                    self.audio_player.attach_bridge(&client);
                    let snap = client.latest_state();
                    self.volume_slider = snap.volume_level;
                    self.doa_slider = snap.doa_angle;
                    self.tof_slider = snap.tof_mm;
                    self.als_slider = snap.als_lux;
                    self.temp_slider = snap.temp_c;
                    self.record_state_transition(None, &snap);
                    self.latest_state = Some(snap);
                    self.bridge = Some(Arc::new(client));
                    if let Some(ref sess) = self.session {
                        let _ = sess.send_serial_input("\n");
                    }
                    self.status_message =
                        format!("Мост подключен на 127.0.0.1:{}", self.config.bridge_port);
                }
                Err(e) => {
                    self.status_message = format!("Ошибка подключения моста: {}", e);
                    self.push_signal(
                        SignalDirection::Rx,
                        SignalCategory::Bridge,
                        format!("Ошибка подключения моста: {}", e),
                    );
                }
            }
        }
        if let Some(ref b) = self.bridge.clone() {
            if b.is_connected() {
                let next_snap = b.latest_state();
                let prev_snap = self.latest_state.clone();
                self.record_state_transition(prev_snap.as_ref(), &next_snap);
                self.latest_state = Some(next_snap);
            } else {
                self.status_message = "Мост отключен.".to_string();
                self.push_signal(
                    SignalDirection::Rx,
                    SignalCategory::Bridge,
                    "Дуплексный мост отключился".to_string(),
                );
                self.bridge = None;
            }
        }
        if let Some(ref mut s) = self.session {
            if !s.is_running() {
                self.status_message = "Процесс QEMU завершился.".to_string();
                self.session = None;
                self.bridge = None;
            }
        }
    }

    pub fn send_command(&mut self, cmd: HostCommand) {
        if matches!(cmd, HostCommand::InjectVoice { .. }) {
            self.last_voice_inject_at = Some(Instant::now());
        }
        let (cat, desc) = describe_host_command(&cmd);
        self.push_signal(SignalDirection::Tx, cat, desc);

        let Some(bridge) = self.bridge.clone() else {
            self.status_message =
                "Команда записана в лог сигналов (ВМ не запущена).".to_string();
            return;
        };
        match bridge.send_command(&cmd) {
            Ok(ack) => {
                let next_snap = bridge.latest_state();
                let prev_snap = self.latest_state.clone();
                self.record_state_transition(prev_snap.as_ref(), &next_snap);
                self.latest_state = Some(next_snap);
                if !ack.ok {
                    let err_msg = ack.error.unwrap_or_else(|| "error".to_string());
                    self.status_message = format!("Команда отклонена: {}", err_msg);
                    self.push_signal(
                        SignalDirection::Rx,
                        cat,
                        format!("Отклонено мостом: {}", err_msg),
                    );
                }
            }
            Err(e) => {
                self.status_message = format!("Ошибка отправки команды: {}", e);
                self.push_signal(
                    SignalDirection::Rx,
                    SignalCategory::Bridge,
                    format!("Ошибка отправки команды: {}", e),
                );
            }
        }
    }

    pub fn record_serial_history(&mut self, cmd_line: &str) {
        let trimmed = cmd_line.trim();
        if !trimmed.is_empty() && !trimmed.chars().all(|c| c.is_control()) {
            if self.serial_history.last().map(|s| s.as_str()) != Some(trimmed) {
                self.serial_history.push(trimmed.to_string());
            }
        }
        self.serial_history_index = None;
        self.serial_draft_input.clear();
    }

    pub fn history_up(&mut self) -> bool {
        if self.serial_history.is_empty() {
            return false;
        }
        match self.serial_history_index {
            None => {
                self.serial_draft_input = self.serial_input.clone();
                let idx = self.serial_history.len() - 1;
                self.serial_history_index = Some(idx);
                self.serial_input = self.serial_history[idx].clone();
                true
            }
            Some(idx) if idx > 0 => {
                let next_idx = idx - 1;
                self.serial_history_index = Some(next_idx);
                self.serial_input = self.serial_history[next_idx].clone();
                true
            }
            _ => false,
        }
    }

    pub fn history_down(&mut self) -> bool {
        let Some(idx) = self.serial_history_index else {
            return false;
        };
        if idx + 1 < self.serial_history.len() {
            let next_idx = idx + 1;
            self.serial_history_index = Some(next_idx);
            self.serial_input = self.serial_history[next_idx].clone();
        } else {
            self.serial_history_index = None;
            self.serial_input = std::mem::take(&mut self.serial_draft_input);
        }
        true
    }

    pub fn update_cwd_from_command(&mut self, cmd_line: &str) {
        let trimmed = cmd_line.trim();
        if trimmed == "cd" || trimmed == "cd ~" {
            self.serial_cwd = "/".to_string();
            return;
        }
        if let Some(rest) = trimmed.strip_prefix("cd ") {
            let token = rest
                .split(&[' ', ';', '&', '|'][..])
                .find(|s| !s.is_empty())
                .unwrap_or("")
                .trim_matches(&['"', '\''][..]);
            if !token.is_empty() && !token.contains('$') && !token.contains('*') {
                self.serial_cwd = resolve_unix_path(&self.serial_cwd, token);
            }
        }
    }

    pub fn list_guest_directory(&mut self, abs_dir: &str) -> Vec<String> {
        let clean_dir = if abs_dir.is_empty() { "/" } else { abs_dir };
        let now = Instant::now();
        if let Some((ts, cached)) = self.serial_dir_cache.get(clean_dir) {
            if now.saturating_duration_since(*ts) < Duration::from_secs(3) {
                return cached.clone();
            }
        }

        let board = self
            .latest_state
            .as_ref()
            .map(|s| s.board_slug.clone())
            .unwrap_or_else(|| self.config.selected_board.clone());
        let mut entries = builtin_guest_dir_entries(clean_dir, &board);

        if self.session.is_some() {
            if let Ok(adb) =
                crate::adb::AdbClient::from_host_port("127.0.0.1", self.config.adb_port)
            {
                let sh_cmd = format!("ls -1p \"{}\" 2>/dev/null", clean_dir);
                if let Ok(out) = adb.shell(&sh_cmd, Duration::from_millis(160)) {
                    for line in out.lines() {
                        let item = line.trim();
                        if !item.is_empty()
                            && !item.contains("can't open")
                            && !item.contains("No such file")
                            && !entries.iter().any(|e| e == item)
                        {
                            entries.push(item.to_string());
                        }
                    }
                }
            }
        }

        entries.sort();
        entries.dedup();
        self.serial_dir_cache
            .insert(clean_dir.to_string(), (now, entries.clone()));
        entries
    }

    pub fn tab_complete_serial_input(&mut self) -> bool {
        self.serial_input.retain(|c| c != '\t');
        let current = self.serial_input.clone();
        let cwd = self.serial_cwd.clone();
        let board = self
            .latest_state
            .as_ref()
            .map(|s| s.board_slug.clone())
            .unwrap_or_else(|| self.config.selected_board.clone());

        let result = compute_tab_completion(&current, &cwd, |abs_dir| {
            self.list_guest_directory(abs_dir)
        });

        match result {
            TabCompletionResult::NoMatch => false,
            TabCompletionResult::Completed(next_input) => {
                let changed = next_input != self.serial_input;
                self.serial_input = next_input;
                changed
            }
            TabCompletionResult::Multiple {
                new_input,
                candidates,
            } => {
                if let Some(ref sess) = self.session {
                    sess.push_serial_line(format!("root@{}:{}# {}", board, cwd, current));
                    sess.push_serial_line(candidates.join("  "));
                }
                self.serial_input = new_input;
                true
            }
        }
    }

    pub fn send_serial_command(&mut self, cmd_line: &str) {
        self.record_serial_history(cmd_line);
        self.update_cwd_from_command(cmd_line);
        let Some(ref sess) = self.session else {
            self.status_message = "ВМ не запущена.".to_string();
            return;
        };
        let trimmed = cmd_line.trim().to_string();
        let payload = if cmd_line == "\x03" {
            "\x03\n".to_string()
        } else {
            format!("{}\n", cmd_line.trim_end_matches(&['\r', '\n'][..]))
        };
        if let Err(e) = sess.send_serial_input(&payload) {
            self.status_message = format!("Ошибка записи в ttyAMA0: {}", e);
            return;
        }
        if !trimmed.is_empty() {
            let label = if cmd_line == "\x03" {
                "^C (SIGINT)"
            } else {
                &trimmed
            };
            self.push_signal(
                SignalDirection::Tx,
                SignalCategory::Bridge,
                format!("ttyAMA0 # {}", label),
            );
        }
    }
}

pub const GUEST_SHELL_COMMANDS: &[&str] = &[
    "adbd",
    "alias",
    "alsactl",
    "amixer",
    "aplay",
    "arecord",
    "ash",
    "awk",
    "basename",
    "bg",
    "break",
    "cat",
    "cd",
    "chdir",
    "chmod",
    "chown",
    "clear",
    "command",
    "continue",
    "cp",
    "cut",
    "date",
    "dd",
    "df",
    "dirname",
    "dmesg",
    "du",
    "echo",
    "env",
    "eval",
    "exec",
    "exit",
    "export",
    "expr",
    "false",
    "fg",
    "find",
    "free",
    "getopts",
    "grep",
    "gunzip",
    "gzip",
    "hash",
    "head",
    "help",
    "hexdump",
    "history",
    "i2cdetect",
    "i2cdump",
    "i2cget",
    "i2cset",
    "i2ctransfer",
    "id",
    "ifconfig",
    "ip",
    "jobs",
    "kill",
    "killall",
    "let",
    "ln",
    "local",
    "ls",
    "md5sum",
    "mkdir",
    "mount",
    "mv",
    "netstat",
    "nohup",
    "pidof",
    "ping",
    "printf",
    "ps",
    "pwd",
    "qemu-hw-bridge",
    "read",
    "readonly",
    "reset",
    "return",
    "rm",
    "rmdir",
    "sed",
    "seq",
    "set",
    "sh",
    "sha256sum",
    "shift",
    "sleep",
    "sort",
    "source",
    "stat",
    "strings",
    "stty",
    "sync",
    "tail",
    "tar",
    "tee",
    "test",
    "times",
    "top",
    "touch",
    "tr",
    "trap",
    "true",
    "type",
    "ulimit",
    "umask",
    "umount",
    "unalias",
    "uname",
    "uniq",
    "unset",
    "uptime",
    "vi",
    "wait",
    "watch",
    "wc",
    "wget",
    "which",
    "xargs",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabCompletionResult {
    NoMatch,
    Completed(String),
    Multiple {
        new_input: String,
        candidates: Vec<String>,
    },
}

pub fn resolve_unix_path(cwd: &str, path_part: &str) -> String {
    let trimmed = path_part.trim();
    if trimmed.is_empty() {
        return if cwd.is_empty() {
            "/".to_string()
        } else {
            cwd.to_string()
        };
    }
    if trimmed == "~" {
        return "/".to_string();
    }
    let mut stack: Vec<&str> = Vec::new();
    if !trimmed.starts_with('/') {
        for seg in cwd.split('/') {
            if !seg.is_empty() && seg != "." {
                stack.push(seg);
            }
        }
    }
    for seg in trimmed.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    if stack.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", stack.join("/"))
    }
}

fn longest_common_prefix(items: &[String]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let mut prefix: Vec<char> = first.chars().collect();
    for item in &items[1..] {
        let mut match_len = 0;
        for (a, b) in prefix.iter().zip(item.chars()) {
            if *a == b {
                match_len += 1;
            } else {
                break;
            }
        }
        prefix.truncate(match_len);
        if prefix.is_empty() {
            break;
        }
    }
    prefix.into_iter().collect()
}

fn is_in_command_position(prefix_before_token: &str) -> bool {
    let trimmed = prefix_before_token.trim();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.ends_with(';')
        || trimmed.ends_with('|')
        || trimmed.ends_with('&')
        || trimmed.ends_with('(')
    {
        return true;
    }
    let last_word = trimmed
        .rsplit(&[' ', '\t', ';', '|', '&', '('][..])
        .find(|s| !s.is_empty())
        .unwrap_or("");
    matches!(last_word, "do" | "then" | "else" | "elif" | "!")
}

pub fn compute_tab_completion<F>(
    input: &str,
    cwd: &str,
    mut list_dir: F,
) -> TabCompletionResult
where
    F: FnMut(&str) -> Vec<String>,
{
    let sep_chars = [' ', '\t', ';', '|', '&', '>', '<', '(', '"', '\''];
    let token_start = input
        .rfind(|c: char| sep_chars.contains(&c))
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let prefix_before = &input[..token_start];
    let token = &input[token_start..];

    let cmd_pos = is_in_command_position(prefix_before);
    if cmd_pos && !token.contains('/') && !token.starts_with('.') {
        let mut matches: Vec<String> = GUEST_SHELL_COMMANDS
            .iter()
            .filter(|&&cmd| cmd.starts_with(token))
            .map(|&s| s.to_string())
            .collect();

        if matches.is_empty() && !token.is_empty() {
            let dir_entries = list_dir(cwd);
            matches = dir_entries
                .into_iter()
                .filter(|e| e.starts_with(token))
                .collect();
        }

        matches.sort();
        matches.dedup();
        return finalize_tab_matches(prefix_before, "", token, matches);
    }

    let (dir_part, name_prefix, abs_dir) = if let Some(slash_idx) = token.rfind('/') {
        let d_part = &token[..=slash_idx];
        let n_pref = &token[slash_idx + 1..];
        let resolved = resolve_unix_path(cwd, d_part);
        (d_part, n_pref, resolved)
    } else {
        ("", token, resolve_unix_path(cwd, ""))
    };

    let entries = list_dir(&abs_dir);
    let mut matches: Vec<String> = entries
        .into_iter()
        .filter(|name| name.starts_with(name_prefix))
        .collect();
    matches.sort();
    matches.dedup();

    finalize_tab_matches(prefix_before, dir_part, name_prefix, matches)
}

fn finalize_tab_matches(
    prefix_before: &str,
    dir_part: &str,
    name_prefix: &str,
    matches: Vec<String>,
) -> TabCompletionResult {
    match matches.len() {
        0 => TabCompletionResult::NoMatch,
        1 => {
            let single = &matches[0];
            let suffix = if single.ends_with('/') { "" } else { " " };
            TabCompletionResult::Completed(format!(
                "{}{}{}{}",
                prefix_before, dir_part, single, suffix
            ))
        }
        _ => {
            let lcp = longest_common_prefix(&matches);
            let chosen = if lcp.len() > name_prefix.len() {
                lcp.as_str()
            } else {
                name_prefix
            };
            let new_input = format!("{}{}{}", prefix_before, dir_part, chosen);
            TabCompletionResult::Multiple {
                new_input,
                candidates: matches,
            }
        }
    }
}

pub fn builtin_guest_dir_entries(abs_dir: &str, board_slug: &str) -> Vec<String> {
    let norm = abs_dir.trim_end_matches('/');
    let dir = if norm.is_empty() { "/" } else { norm };
    let list: &[&str] = match dir {
        "/" => &[
            "bin/",
            "bkupgrade/",
            "boot/",
            "data/",
            "dev/",
            "equasar-qemu-init.sh",
            "etc/",
            "lib/",
            "lib64/",
            "linuxrc",
            "lost+found/",
            "media/",
            "mnt/",
            "opt/",
            "ota/",
            "proc/",
            "quasar-qemu-init.sh",
            "root/",
            "run/",
            "sbin/",
            "sys/",
            "system/",
            "tee/",
            "tmp/",
            "usr/",
            "var/",
        ],
        "/tmp" => &[
            "daemons_logs/",
            "hw-bridge.log",
            "quasar-emu-hw.json",
            "quasar-onboot.log",
        ],
        "/tmp/daemons_logs" => &["maind.log"],
        "/sys" => &["bus/", "class/", "devices/", "firmware/", "fs/", "kernel/", "module/"],
        "/sys/class" => &["graphics/", "i2c-adapter/", "input/", "leds/", "net/", "sound/"],
        "/sys/class/graphics" => &["fb0/", "fb1/", "fb2/"],
        "/sys/class/leds" => {
            let prefix = match board_slug {
                "orion" => "orion",
                "cucumber" => "cucumber",
                "yandexmini2" | "yandexmini2nc" => "mini2",
                "bergamot" => "bergamot",
                "pickle" => "pickle",
                "plum" | "plumnc" => "plum",
                "station2" => "station2",
                "yandexmicro" => "micro",
                "mango" => "mango",
                "saturn" => "saturn",
                _ => "led",
            };
            return vec![
                format!("{}:red:ring0/", prefix),
                format!("{}:green:ring0/", prefix),
                format!("{}:blue:ring0/", prefix),
                format!("{}:red:ring1/", prefix),
                format!("{}:green:ring1/", prefix),
                format!("{}:blue:ring1/", prefix),
            ];
        }
        "/usr" => &["bin/", "lib/", "local/", "sbin/", "share/"],
        "/usr/local" => &["bin/"],
        "/usr/local/bin" => &["qemu-hw-bridge"],
        "/system" => &["etc/", "vendor/", "workdir/"],
        "/system/vendor" => &["quasar/"],
        "/system/vendor/quasar" => &["ledpatterns/", "maind", "onboot.sh", "quasar.cfg"],
        "/dev" => &[
            "fb0",
            "fb1",
            "fb2",
            "i2c-0",
            "i2c-2",
            "i2c-3",
            "input/",
            "null",
            "snd/",
            "ttyAMA0",
            "uinput",
            "vda",
            "vdb",
            "zero",
        ],
        "/etc" => &["asound.conf", "fstab", "hostname", "init.d/", "inittab", "passwd", "profile"],
        _ => &[],
    };
    list.iter().map(|&s| s.to_string()).collect()
}

impl Drop for GuiAppState {
    fn drop(&mut self) {
        self.stop_vm();
        let _ = self.save_config();
    }
}

pub fn discover_images_in_folder(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(dir) else {
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
        if (name.ends_with(".ext4") || name.ends_with("-system.img"))
            && !name.ends_with("-orig.ext4")
            && !name.ends_with("-data.ext4")
        {
            let stem = name
                .trim_end_matches("-system.ext4")
                .trim_end_matches(".ext4")
                .trim_end_matches("-system.img");
            found.push((normalize_board_slug(stem), path));
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

fn run_os_file_dialog(start_dir: &Path) -> Option<PathBuf> {
    run_os_custom_file_dialog(
        start_dir,
        "Выберите образ (.ext4) или архив дампа",
        "Образы и дампы (*.ext4;*.img;*.gz;*.tgz;*.tar;*.zip;*.ubi;*.squashfs)|*.ext4;*.img;*.gz;*.tgz;*.tar;*.zip;*.ubi;*.squashfs|Все файлы (*.*)|*.*",
    )
}

fn run_os_custom_file_dialog(start_dir: &Path, title: &str, filter: &str) -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let init = start_dir.display().to_string().replace('\'', "''");
        let safe_title = title.replace('\'', "''");
        let safe_filter = filter.replace('\'', "''");
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; \
             $d = New-Object System.Windows.Forms.OpenFileDialog; \
             $d.Title = '{}'; \
             if (Test-Path '{}') {{ $d.InitialDirectory = '{}' }}; \
             $d.Filter = '{}'; \
             if ($d.ShowDialog() -eq 'OK') {{ [Console]::Out.Write($d.FileName) }}",
            safe_title, init, init, safe_filter
        );
        let out = Command::new("powershell")
            .args(["-NoProfile", "-STA", "-Command", &script])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(PathBuf::from(s));
        }
        None
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (start_dir, filter);
        let out = Command::new("zenity")
            .args(["--file-selection", &format!("--title={}", title)])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(PathBuf::from(s));
        }
        None
    }
}

fn run_os_folder_dialog(start_dir: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let init = start_dir.display().to_string().replace('\'', "''");
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; \
             $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
             $d.Description = 'Выберите папку с гостевыми образами (*-system.ext4)'; \
             if (Test-Path '{}') {{ $d.SelectedPath = '{}' }}; \
             if ($d.ShowDialog() -eq 'OK') {{ [Console]::Out.Write($d.SelectedPath) }}",
            init, init
        );
        let out = Command::new("powershell")
            .args(["-NoProfile", "-STA", "-Command", &script])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(PathBuf::from(s));
        }
        None
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = start_dir;
        let out = Command::new("zenity")
            .args([
                "--file-selection",
                "--directory",
                "--title=Выберите папку с образами",
            ])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(PathBuf::from(s));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ui_config_persistence_and_custom_image_paths() {
        let tmp =
            std::env::temp_dir().join(format!("kuyase-gui-state-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("bin")).unwrap();
        fs::create_dir_all(tmp.join("kernel")).unwrap();
        let extra_dir = tmp.join("extra_images");
        fs::create_dir_all(&extra_dir).unwrap();
        fs::write(tmp.join("bin").join("qemu-system-aarch64.exe"), b"mock").unwrap();
        fs::write(tmp.join("kernel").join("Image"), b"mock-kernel").unwrap();
        let custom_ext4 = extra_dir.join("orion-system.ext4");
        fs::write(&custom_ext4, b"mock-ext4-image").unwrap();

        let mut app = GuiAppState::new(&tmp);
        assert!(GuiAppState::ui_config_path(&tmp).is_file());

        app.set_custom_images_dir(extra_dir.clone());
        assert_eq!(app.config.custom_images_dir, extra_dir.display().to_string());
        assert!(app.guest_images.iter().any(|(slug, _, _)| slug == "orion"));

        app.select_custom_file(custom_ext4.clone());
        assert_eq!(app.config.selected_board, "orion");
        assert_eq!(
            app.config.custom_image_file,
            custom_ext4.display().to_string()
        );

        let reloaded = GuiAppState::load_config(&tmp);
        assert_eq!(reloaded.selected_board, "orion");
        assert_eq!(reloaded.custom_images_dir, extra_dir.display().to_string());
        assert_eq!(reloaded.custom_image_file, custom_ext4.display().to_string());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_signal_log_tx_rx_and_tof_wave_pulse() {
        let tmp =
            std::env::temp_dir().join(format!("kuyase-signal-log-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("work")).unwrap();

        let mut app = GuiAppState::new(&tmp);
        app.clear_signals();

        // 1. Trigger ToF wave (140 -> 8190 mm)
        app.trigger_tof_wave();
        assert_eq!(app.tof_slider, 140);
        assert!(app.tof_wave_reset_at.is_some());
        assert_eq!(app.signal_logs.len(), 1);
        assert_eq!(app.signal_logs[0].direction, SignalDirection::Tx);
        assert_eq!(app.signal_logs[0].category, SignalCategory::TofButtons);
        assert!(app.signal_logs[0].message.contains("tof_mm=140"));

        // 2. Trigger Custom TTS command
        app.send_command(HostCommand::IpcTest {
            cmd: "vocalize".to_string(),
            text: Some("Проверка TTS".to_string()),
        });
        assert_eq!(app.signal_logs.len(), 2);
        assert_eq!(app.signal_logs[1].category, SignalCategory::TtsIpc);

        // 3. Simulate RX state transition
        let mut s1 = BridgeStateSnapshot::default();
        s1.alice_state = "IDLE".to_string();
        let mut s2 = s1.clone();
        s2.alice_state = "SPEAKING".to_string();
        s2.last_spoken_text = "Проверка TTS".to_string();
        s2.led_trigger_conflict_count = 1;
        app.record_state_transition(Some(&s1), &s2);

        app.signal_filter = SignalFilter::TtsIpc;
        let tts_signals = app.filtered_signals();
        assert!(tts_signals.len() >= 3);
        assert!(tts_signals
            .iter()
            .any(|e| e.direction == SignalDirection::Rx && e.message.contains("IDLE -> SPEAKING")));

        app.signal_filter = SignalFilter::LedDisplay;
        let led_signals = app.filtered_signals();
        assert_eq!(led_signals.len(), 1);
        assert!(led_signals[0].message.contains("led_trigger_conflict_count = 1"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_terminal_command_history_navigation() {
        let tmp =
            std::env::temp_dir().join(format!("kuyase-term-history-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("work")).unwrap();

        let mut app = GuiAppState::new(&tmp);
        assert!(!app.history_up());
        assert!(!app.history_down());

        app.record_serial_history("ps");
        app.record_serial_history("ps"); // consecutive duplicate should be deduplicated
        app.record_serial_history("cat /tmp/quasar-emu-hw.json");
        app.record_serial_history("\x03"); // control char should not pollute history
        assert_eq!(
            app.serial_history,
            vec!["ps".to_string(), "cat /tmp/quasar-emu-hw.json".to_string()]
        );

        app.serial_input = "draft cmd".to_string();
        assert!(app.history_up());
        assert_eq!(app.serial_input, "cat /tmp/quasar-emu-hw.json");
        assert!(app.history_up());
        assert_eq!(app.serial_input, "ps");
        assert!(!app.history_up()); // already at oldest

        assert!(app.history_down());
        assert_eq!(app.serial_input, "cat /tmp/quasar-emu-hw.json");
        assert!(app.history_down());
        assert_eq!(app.serial_input, "draft cmd"); // draft restored
        assert!(!app.history_down());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_unix_tab_completion_commands_and_paths() {
        let tmp =
            std::env::temp_dir().join(format!("kuyase-tab-complete-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("work")).unwrap();

        let mut app = GuiAppState::new(&tmp);

        // 1. Multiple command matches ("he" -> head, help, hexdump)
        let res_multi = compute_tab_completion("he", "/", |d| builtin_guest_dir_entries(d, "orion"));
        assert_eq!(
            res_multi,
            TabCompletionResult::Multiple {
                new_input: "he".to_string(),
                candidates: vec![
                    "head".to_string(),
                    "help".to_string(),
                    "hexdump".to_string()
                ],
            }
        );

        // 2. Unique command match ("hel" -> "help ")
        app.serial_input = "hel".to_string();
        assert!(app.tab_complete_serial_input());
        assert_eq!(app.serial_input, "help ");

        // 3. Path argument completion in root ("ls eq" -> "ls equasar-qemu-init.sh ")
        app.serial_input = "ls eq".to_string();
        assert!(app.tab_complete_serial_input());
        assert_eq!(app.serial_input, "ls equasar-qemu-init.sh ");

        // 4. Directory completion keeps trailing slash without space ("cd /sys/c" -> "cd /sys/class/")
        app.serial_input = "cd /sys/c".to_string();
        assert!(app.tab_complete_serial_input());
        assert_eq!(app.serial_input, "cd /sys/class/");

        // 5. Chained directory completion ("cd /sys/class/l" -> "cd /sys/class/leds/")
        app.serial_input.push('l');
        assert!(app.tab_complete_serial_input());
        assert_eq!(app.serial_input, "cd /sys/class/leds/");

        // 6. CWD tracking via cd command and relative completion
        app.update_cwd_from_command("cd /tmp");
        assert_eq!(app.serial_cwd, "/tmp");
        app.serial_input = "cat hw".to_string();
        assert!(app.tab_complete_serial_input());
        assert_eq!(app.serial_input, "cat hw-bridge.log ");

        app.update_cwd_from_command("cd ..");
        assert_eq!(app.serial_cwd, "/");

        let _ = fs::remove_dir_all(&tmp);
    }
}
