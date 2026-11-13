use protocol::{BridgeStateSnapshot, HostCommand};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_SIGNAL_LOGS: usize = 500;

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
    pub fn as_str_for_board(self, board_slug: &str) -> &'static str {
        match self {
            Self::Bridge => "BRIDGE",
            Self::TofButtons => {
                if protocol::normalize_board_slug(board_slug) == "yandexmini" {
                    "TOF/KEY"
                } else {
                    "SENS/KEY"
                }
            }
            Self::TtsIpc => "TTS/IPC",
            Self::LedDisplay => "LED/DISP",
            Self::Audio => "AUDIO",
        }
    }
}

pub fn signal_filter_tab_label(filter: SignalFilter, board_slug: &str) -> &'static str {
    match filter {
        SignalFilter::All => "Все",
        SignalFilter::TofButtons => {
            if protocol::normalize_board_slug(board_slug) == "yandexmini" {
                "ToF / Кнопки"
            } else {
                "Датчики / Кнопки"
            }
        }
        SignalFilter::TtsIpc => "TTS / IPC",
        SignalFilter::LedDisplay => "LED / Экран",
        SignalFilter::Audio => "Аудио",
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
            format!(
                "PressButton {{ button: \"{}\", action: \"{}\" }}",
                button, action
            ),
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
        HostCommand::SetAudioConfig { config } => (
            SignalCategory::Audio,
            format!(
                "SetAudioConfig {{ mic: {} Гц / {} мс (int={} мс, lag_cap={}), hijack: {} мс, aloop: {} Гц / {} мс }}",
                config.mic_sample_rate,
                config.mic_frame_ms,
                config.mic_send_interval_ms,
                config.mic_lag_cap,
                config.hijack_window_ms,
                config.capture_sample_rate,
                config.capture_chunk_ms
            ),
        ),
        HostCommand::IpcTest { cmd, text } => {
            if let Some(ref t) = text {
                (
                    SignalCategory::TtsIpc,
                    format!(
                        "IpcTest {{ cmd: \"{}\", text: \"{}\" }} -> aliced :9875",
                        cmd, t
                    ),
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

pub fn diff_state_transitions(
    prev_opt: Option<&BridgeStateSnapshot>,
    next: &BridgeStateSnapshot,
    last_logged_guest_line: &mut String,
) -> Vec<(SignalCategory, String)> {
    let mut events = Vec::new();

    if let Some(prev) = prev_opt {
        if prev.alice_state != next.alice_state {
            events.push((
                SignalCategory::TtsIpc,
                format!(
                    "aliced (:9875): состояние {} -> {}",
                    prev.alice_state, next.alice_state
                ),
            ));
        }
        if prev.last_spoken_text != next.last_spoken_text && !next.last_spoken_text.is_empty() {
            events.push((
                SignalCategory::TtsIpc,
                format!("Синтезатор речи TTS: \"{}\"", next.last_spoken_text),
            ));
        }
        if next.aliced_hijack_count > prev.aliced_hijack_count {
            events.push((
                SignalCategory::TtsIpc,
                format!(
                    "Перехват диалога в окне {} мс (aliced_hijack_count = {})",
                    next.audio_config.hijack_window_ms, next.aliced_hijack_count
                ),
            ));
        }
        if next.led_trigger_conflict_count > prev.led_trigger_conflict_count {
            events.push((
                SignalCategory::LedDisplay,
                format!(
                    "Конфликт триггера sysfs [ledfb-ring] (led_trigger_conflict_count = {})",
                    next.led_trigger_conflict_count
                ),
            ));
        }
        if prev.ring_trigger != next.ring_trigger {
            events.push((
                SignalCategory::LedDisplay,
                format!(
                    "Триггер кольца LED: {} -> {}",
                    prev.ring_trigger, next.ring_trigger
                ),
            ));
        }
        if prev.mics_muted != next.mics_muted {
            events.push((
                SignalCategory::TofButtons,
                format!(
                    "Микрофоны: {}",
                    if next.mics_muted {
                        "ОТКЛЮЧЕНЫ (MUTE)"
                    } else {
                        "ВКЛЮЧЕНЫ"
                    }
                ),
            ));
        }
        if prev.volume_level != next.volume_level {
            events.push((
                SignalCategory::Audio,
                format!("Громкость изменена: {}/10", next.volume_level),
            ));
        }
        let gate = next.audio_config.silence_gate_rms.max(1);
        let rate_khz = next.audio_config.capture_sample_rate / 1000;
        if (prev.speaker_rms <= gate && next.speaker_rms > gate)
            || (prev.speaker_rms > gate && next.speaker_rms <= gate)
        {
            let msg = if next.speaker_rms > gate {
                format!(
                    "Активен аудиопоток {} кГц (Speaker RMS: {})",
                    rate_khz, next.speaker_rms
                )
            } else {
                format!("Аудиопоток {} кГц завершён (Speaker RMS: 0)", rate_khz)
            };
            events.push((SignalCategory::Audio, msg));
        }
    } else {
        events.push((
            SignalCategory::Bridge,
            format!(
                "Подключен мост: {} ({}, {} LED, триггер: {})",
                next.board_name, next.board_slug, next.ring_leds, next.ring_trigger
            ),
        ));
    }

    if let Some(last_line) = next.recent_logs.last() {
        if last_line != last_logged_guest_line && !last_line.is_empty() {
            *last_logged_guest_line = last_line.clone();
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
            events.push((cat, format!("hw-bridge: {}", last_line)));
        }
    }

    events
}
