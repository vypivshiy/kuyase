pub mod boards;
pub mod display;
pub mod led_patterns;

pub use boards::{
    board_display_label, board_has_matrix_21x12, board_has_segment_clock, default_board_ring_leds,
    detect_board_hint_from_filename, detect_board_hint_from_path, detect_board_slug_from_probe,
    find_board_descriptor, normalize_board_slug, BoardDescriptor, BOARD_CATALOG,
};
pub use led_patterns::{
    build_led_pattern_info, classify_rgb_color_ru, extract_led_pattern_primary_rgb, format_rgb_hex,
    is_supported_led_pattern_path, parse_led_pattern_frames, LedPatternInfo,
};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};

pub const KIND_STATE_SNAPSHOT: u8 = 1;
pub const KIND_PCM_AUDIO_CHUNK: u8 = 2;
pub const KIND_COMMAND_ACK: u8 = 3;
pub const KIND_HOST_COMMAND: u8 = 1;

pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

pub const DEFAULT_MIC_SAMPLE_RATE: u32 = 16_000;
pub const DEFAULT_MIC_FRAME_MS: u32 = 40;
pub const DEFAULT_MIC_SEND_INTERVAL_MS: u32 = 40;
pub const DEFAULT_MIC_LAG_CAP: bool = true;
pub const DEFAULT_MIC_IDLE_RMS: u32 = 60;
pub const DEFAULT_MIC_BURST_RMS: u32 = 2400;
pub const DEFAULT_VOICE_RECOGNITION_DELAY_MS: u32 = 300;
pub const DEFAULT_VOICE_BURST_DURATION_MS: u32 = 650;
pub const DEFAULT_HIJACK_WINDOW_MS: u32 = 1680;
pub const DEFAULT_CAPTURE_SAMPLE_RATE: u32 = 48_000;
pub const MIN_CAPTURE_SAMPLE_RATE: u32 = 8_000;
pub const MAX_CAPTURE_SAMPLE_RATE: u32 = 96_000;
pub const DEFAULT_CAPTURE_CHUNK_MS: u32 = 20;
pub const DEFAULT_SILENCE_GATE_RMS: u32 = 5;
pub const DEFAULT_HANGOVER_MS: u32 = 400;
pub const DEFAULT_HOST_MAX_INFLIGHT_CHUNKS: u32 = 24;
pub const MIN_HOST_INFLIGHT_CHUNKS: u32 = 2;
pub const MAX_HOST_INFLIGHT_CHUNKS: u32 = 128;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub mic_sample_rate: u32,
    pub mic_frame_ms: u32,
    pub mic_send_interval_ms: u32,
    pub mic_lag_cap: bool,
    pub mic_idle_rms: u32,
    pub mic_burst_rms: u32,
    pub recognition_delay_ms: u32,
    pub burst_duration_ms: u32,
    pub hijack_window_ms: u32,
    pub capture_sample_rate: u32,
    pub capture_chunk_ms: u32,
    pub silence_gate_rms: u32,
    pub hangover_ms: u32,
    pub host_max_inflight_chunks: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            mic_sample_rate: DEFAULT_MIC_SAMPLE_RATE,
            mic_frame_ms: DEFAULT_MIC_FRAME_MS,
            mic_send_interval_ms: DEFAULT_MIC_SEND_INTERVAL_MS,
            mic_lag_cap: DEFAULT_MIC_LAG_CAP,
            mic_idle_rms: DEFAULT_MIC_IDLE_RMS,
            mic_burst_rms: DEFAULT_MIC_BURST_RMS,
            recognition_delay_ms: DEFAULT_VOICE_RECOGNITION_DELAY_MS,
            burst_duration_ms: DEFAULT_VOICE_BURST_DURATION_MS,
            hijack_window_ms: DEFAULT_HIJACK_WINDOW_MS,
            capture_sample_rate: DEFAULT_CAPTURE_SAMPLE_RATE,
            capture_chunk_ms: DEFAULT_CAPTURE_CHUNK_MS,
            silence_gate_rms: DEFAULT_SILENCE_GATE_RMS,
            hangover_ms: DEFAULT_HANGOVER_MS,
            host_max_inflight_chunks: DEFAULT_HOST_MAX_INFLIGHT_CHUNKS,
        }
    }
}

impl AudioConfig {
    pub fn sanitized(&self) -> Self {
        Self {
            mic_sample_rate: self.mic_sample_rate.clamp(8_000, 48_000),
            mic_frame_ms: self.mic_frame_ms.clamp(5, 200),
            mic_send_interval_ms: self.mic_send_interval_ms.clamp(5, 500),
            mic_lag_cap: self.mic_lag_cap,
            mic_idle_rms: self.mic_idle_rms.clamp(0, 10_000),
            mic_burst_rms: self.mic_burst_rms.clamp(100, 28_000),
            recognition_delay_ms: self.recognition_delay_ms.clamp(0, 5_000),
            burst_duration_ms: self.burst_duration_ms.clamp(50, 10_000),
            hijack_window_ms: self.hijack_window_ms.clamp(100, 15_000),
            capture_sample_rate: self
                .capture_sample_rate
                .clamp(MIN_CAPTURE_SAMPLE_RATE, MAX_CAPTURE_SAMPLE_RATE),
            capture_chunk_ms: self.capture_chunk_ms.clamp(5, 100),
            silence_gate_rms: self.silence_gate_rms.clamp(0, 5_000),
            hangover_ms: self.hangover_ms.clamp(0, 5_000),
            host_max_inflight_chunks: self
                .host_max_inflight_chunks
                .clamp(MIN_HOST_INFLIGHT_CHUNKS, MAX_HOST_INFLIGHT_CHUNKS),
        }
    }

    pub fn mic_frame_samples(&self) -> usize {
        let cfg = self.sanitized();
        (cfg.mic_sample_rate as usize * cfg.mic_frame_ms as usize) / 1000
    }

    pub fn capture_chunk_bytes(&self) -> usize {
        let cfg = self.sanitized();
        let frames = (cfg.capture_sample_rate as usize * cfg.capture_chunk_ms as usize) / 1000;
        frames * 4
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AmplifierSlotState {
    pub i2c_device: String,
    pub role: String,
    pub alsa_prefix: String,
    pub settings_file: String,
    pub gain_db: f32,
    pub ramp_db: f32,
    pub muted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UltrasoundState {
    pub enabled: bool,
    pub tdm_device: String,
    pub sample_rate_hz: u32,
    pub carrier_freq_hz: u32,
    pub bandwidth_hz: u32,
    pub channels: u32,
    pub active: bool,
    pub rms: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BridgeStateSnapshot {
    pub ok: bool,
    pub board_slug: String,
    pub board_name: String,
    pub ring_leds: usize,
    pub logo_leds: usize,
    pub ambilight_leds: usize,
    pub aux_light_kind: String,
    pub has_clock: bool,
    pub display_kind: String,
    pub screen_width: usize,
    pub screen_height: usize,
    pub screen_pixels: Vec<u8>,
    pub ring_pixels: Vec<[u8; 3]>,
    pub logo_pixels: Vec<[u8; 3]>,
    pub ring_trigger: String,
    pub clock_segments: BTreeMap<String, u8>,
    pub amplifiers: Vec<AmplifierSlotState>,
    pub ultrasound: Option<UltrasoundState>,
    pub doa_angle: u32,
    pub tof_mm: u32,
    pub als_lux: u32,
    pub temp_c: f32,
    pub volume_level: u32,
    pub master_volume: u32,
    pub mics_muted: bool,
    pub forced_led_pattern: String,
    pub mic_rms: u32,
    pub mic_burst_active: bool,
    pub speaker_rms: u32,
    pub captured_audio_bytes: usize,
    pub alice_state: String,
    pub aliced_hijack_count: u32,
    pub aliced_say_count: u32,
    pub ipc_error_count: u32,
    pub led_trigger_conflict_count: u32,
    pub last_spoken_text: String,
    pub recent_logs: Vec<String>,
    pub maind_logs: Vec<String>,
    #[serde(default)]
    pub available_led_patterns: Vec<LedPatternInfo>,
    #[serde(default)]
    pub audio_config: AudioConfig,
}

impl Default for BridgeStateSnapshot {
    fn default() -> Self {
        Self {
            ok: true,
            board_slug: "yandexmini".to_string(),
            board_name: "Яндекс Станция Мини 1 (YNDX-0004 / yandexmini)".to_string(),
            ring_leds: 6,
            logo_leds: 0,
            ambilight_leds: 0,
            aux_light_kind: "none".to_string(),
            has_clock: false,
            display_kind: "none".to_string(),
            screen_width: 21,
            screen_height: 12,
            screen_pixels: vec![0u8; 252],
            ring_pixels: vec![[0u8; 3]; 6],
            logo_pixels: Vec::new(),
            ring_trigger: "i2c-0 SN3236 (0x3c Active)".to_string(),
            clock_segments: BTreeMap::new(),
            amplifiers: Vec::new(),
            ultrasound: None,
            doa_angle: 0,
            tof_mm: 8190,
            als_lux: 240,
            temp_c: 40.0,
            volume_level: 7,
            master_volume: 7000,
            mics_muted: false,
            forced_led_pattern: String::new(),
            mic_rms: 60,
            mic_burst_active: false,
            speaker_rms: 0,
            captured_audio_bytes: 0,
            alice_state: "IDLE".to_string(),
            aliced_hijack_count: 0,
            aliced_say_count: 0,
            ipc_error_count: 0,
            led_trigger_conflict_count: 0,
            last_spoken_text: String::new(),
            recent_logs: Vec::new(),
            maind_logs: Vec::new(),
            available_led_patterns: Vec::new(),
            audio_config: AudioConfig::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum HostCommand {
    RequestState,
    PressButton {
        button: String,
        action: String,
    },
    SetVolume {
        volume_level: u32,
    },
    SetLedPattern {
        pattern: String,
    },
    SetClockDisplay {
        segments: Option<BTreeMap<String, u8>>,
        replace: Option<bool>,
        pixels: Option<Vec<u8>>,
        led_frame: Option<String>,
        brightness: Option<u8>,
        scale_brightness: Option<u8>,
        text: Option<String>,
    },
    SetSensor {
        doa_angle: Option<u32>,
        mic_rms: Option<u32>,
        als_lux: Option<u32>,
        temp_c: Option<f32>,
        tof_mm: Option<u32>,
        swipe: Option<bool>,
        #[serde(default)]
        swipe_direction: Option<String>,
    },
    InjectVoice {
        phrase: String,
        doa_angle: Option<u32>,
        rms: Option<u32>,
        wake_only: bool,
    },
    SetAudioConfig {
        config: AudioConfig,
    },
    IpcTest {
        cmd: String,
        text: Option<String>,
    },
    PlayTestTone,
    ClearAudio,
    PowerOff,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandAckPayload {
    pub ok: bool,
    pub error: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum GuestPacket {
    StateSnapshot(BridgeStateSnapshot),
    PcmAudioChunk(Vec<u8>),
    CommandAck {
        ok: bool,
        error: Option<String>,
        message: Option<String>,
    },
}

fn write_raw_frame<W: Write>(writer: &mut W, kind: u8, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("frame payload too large: {} bytes", payload.len()),
        ));
    }
    let mut header = [0u8; 5];
    header[0] = kind;
    header[1..5].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    writer.write_all(&header)?;
    writer.write_all(payload)?;
    writer.flush()
}

fn read_raw_frame<R: Read>(reader: &mut R) -> io::Result<(u8, Vec<u8>)> {
    let mut header = [0u8; 5];
    reader.read_exact(&mut header)?;
    let kind = header[0];
    let len = u32::from_le_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if len > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame length {} exceeds MAX_FRAME_LEN", len),
        ));
    }
    let mut payload = vec![0u8; len];
    if len > 0 {
        reader.read_exact(&mut payload)?;
    }
    Ok((kind, payload))
}

pub fn write_guest_packet<W: Write>(writer: &mut W, packet: &GuestPacket) -> io::Result<()> {
    match packet {
        GuestPacket::StateSnapshot(state) => {
            let bytes = serde_json::to_vec(state)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            write_raw_frame(writer, KIND_STATE_SNAPSHOT, &bytes)
        }
        GuestPacket::PcmAudioChunk(pcm) => write_raw_frame(writer, KIND_PCM_AUDIO_CHUNK, pcm),
        GuestPacket::CommandAck { ok, error, message } => {
            let ack = CommandAckPayload {
                ok: *ok,
                error: error.clone(),
                message: message.clone(),
            };
            let bytes = serde_json::to_vec(&ack)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            write_raw_frame(writer, KIND_COMMAND_ACK, &bytes)
        }
    }
}

pub fn read_guest_packet<R: Read>(reader: &mut R) -> io::Result<GuestPacket> {
    let (kind, payload) = read_raw_frame(reader)?;
    match kind {
        KIND_STATE_SNAPSHOT => {
            let state: BridgeStateSnapshot = serde_json::from_slice(&payload)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            Ok(GuestPacket::StateSnapshot(state))
        }
        KIND_PCM_AUDIO_CHUNK => Ok(GuestPacket::PcmAudioChunk(payload)),
        KIND_COMMAND_ACK => {
            let ack: CommandAckPayload = serde_json::from_slice(&payload)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            Ok(GuestPacket::CommandAck {
                ok: ack.ok,
                error: ack.error,
                message: ack.message,
            })
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown GuestPacket kind: {}", other),
        )),
    }
}

pub fn write_host_command<W: Write>(writer: &mut W, cmd: &HostCommand) -> io::Result<()> {
    let bytes =
        serde_json::to_vec(cmd).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_raw_frame(writer, KIND_HOST_COMMAND, &bytes)
}

pub fn read_host_command<R: Read>(reader: &mut R) -> io::Result<HostCommand> {
    let (kind, payload) = read_raw_frame(reader)?;
    if kind != KIND_HOST_COMMAND {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected HostCommand kind: {}", kind),
        ));
    }
    serde_json::from_slice(&payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_host_command_roundtrip_all_variants() {
        let mut segs = BTreeMap::new();
        segs.insert("a1".to_string(), 255);
        segs.insert("deg".to_string(), 128);

        let commands = vec![
            HostCommand::RequestState,
            HostCommand::PressButton {
                button: "mute".to_string(),
                action: "single".to_string(),
            },
            HostCommand::SetVolume { volume_level: 8 },
            HostCommand::SetLedPattern {
                pattern: "alice_listening.led".to_string(),
            },
            HostCommand::SetClockDisplay {
                segments: Some(segs),
                replace: Some(false),
                pixels: Some(vec![0, 128, 255]),
                led_frame: None,
                brightness: Some(255),
                scale_brightness: None,
                text: Some("19:30°".to_string()),
            },
            HostCommand::SetSensor {
                doa_angle: Some(90),
                mic_rms: Some(1800),
                als_lux: Some(640),
                temp_c: Some(51.4),
                tof_mm: Some(165),
                swipe: Some(false),
                swipe_direction: Some("left".to_string()),
            },
            HostCommand::InjectVoice {
                phrase: "включи ночной режим".to_string(),
                doa_angle: Some(90),
                rms: Some(2400),
                wake_only: false,
            },
            HostCommand::SetAudioConfig {
                config: AudioConfig {
                    mic_sample_rate: 8_000,
                    mic_frame_ms: 20,
                    mic_send_interval_ms: 25,
                    mic_lag_cap: false,
                    ..AudioConfig::default()
                },
            },
            HostCommand::IpcTest {
                cmd: "vocalize".to_string(),
                text: Some("Привет!".to_string()),
            },
            HostCommand::PlayTestTone,
            HostCommand::ClearAudio,
            HostCommand::PowerOff,
        ];

        let mut buf = Vec::new();
        for cmd in &commands {
            write_host_command(&mut buf, cmd).expect("write_host_command should succeed");
        }

        let mut cursor = Cursor::new(buf);
        for expected in &commands {
            let decoded = read_host_command(&mut cursor).expect("read_host_command should succeed");
            assert_eq!(&decoded, expected);
        }
    }

    #[test]
    fn test_guest_packet_roundtrip_all_variants() {
        let mut state = BridgeStateSnapshot {
            board_slug: "orion".to_string(),
            board_name: "Яндекс Станция 3 (YNDX-00055 / orion)".to_string(),
            ring_leds: 36,
            ambilight_leds: 12,
            aux_light_kind: "ambilight".to_string(),
            display_kind: "matrix_21x12".to_string(),
            ring_pixels: vec![[107, 15, 255]; 36],
            logo_pixels: vec![[200, 110, 255]; 12],
            audio_config: AudioConfig {
                mic_sample_rate: 24_000,
                mic_frame_ms: 20,
                mic_send_interval_ms: 20,
                hijack_window_ms: 2000,
                ..AudioConfig::default()
            },
            ..Default::default()
        };
        state.amplifiers.push(AmplifierSlotState {
            i2c_device: "3-002d".to_string(),
            role: "woofer".to_string(),
            alsa_prefix: "wf Acm8625".to_string(),
            settings_file: "orion-acm8625-woofer.cfg".to_string(),
            gain_db: -20.0,
            ramp_db: -6.0,
            muted: false,
        });
        state.ultrasound = Some(UltrasoundState {
            enabled: true,
            tdm_device: "hw:0,0".to_string(),
            sample_rate_hz: 96000,
            carrier_freq_hz: 38400,
            bandwidth_hz: 600,
            channels: 4,
            active: true,
            rms: 420,
        });

        let packets = vec![
            GuestPacket::StateSnapshot(state),
            GuestPacket::PcmAudioChunk(vec![0x01, 0x02, 0xfe, 0xff, 0x00, 0x80]),
            GuestPacket::CommandAck {
                ok: true,
                error: None,
                message: Some("applied".to_string()),
            },
            GuestPacket::CommandAck {
                ok: false,
                error: Some("mics_muted".to_string()),
                message: None,
            },
        ];

        let mut buf = Vec::new();
        for pkt in &packets {
            write_guest_packet(&mut buf, pkt).expect("write_guest_packet should succeed");
        }

        let mut cursor = Cursor::new(buf);
        for expected in &packets {
            let decoded = read_guest_packet(&mut cursor).expect("read_guest_packet should succeed");
            assert_eq!(&decoded, expected);
        }
    }

    #[test]
    fn test_build_wav_bytes_header() {
        let pcm = vec![0u8; 3840];
        let wav = build_wav_bytes(&pcm, 48_000, 2);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), 44 + 3840);
    }

    #[test]
    fn test_audio_config_defaults_and_sanitization() {
        let def = AudioConfig::default();
        assert_eq!(def.mic_sample_rate, 16_000);
        assert_eq!(def.mic_frame_ms, 40);
        assert_eq!(def.mic_send_interval_ms, 40);
        assert!(def.mic_lag_cap);
        assert_eq!(def.mic_idle_rms, 60);
        assert_eq!(def.mic_burst_rms, 2400);
        assert_eq!(def.recognition_delay_ms, 300);
        assert_eq!(def.burst_duration_ms, 650);
        assert_eq!(def.hijack_window_ms, 1680);
        assert_eq!(def.capture_sample_rate, 48_000);
        assert_eq!(def.capture_chunk_ms, 20);
        assert_eq!(def.silence_gate_rms, 5);
        assert_eq!(def.hangover_ms, 400);
        assert_eq!(def.host_max_inflight_chunks, 24);
        assert_eq!(def.mic_frame_samples(), 640);
        assert_eq!(def.capture_chunk_bytes(), 3840);

        let raw = AudioConfig {
            mic_sample_rate: 1_000,
            mic_frame_ms: 1,
            mic_send_interval_ms: 9999,
            mic_lag_cap: false,
            mic_idle_rms: 99_999,
            mic_burst_rms: 5,
            recognition_delay_ms: 99_999,
            burst_duration_ms: 1,
            hijack_window_ms: 10,
            capture_sample_rate: 200_000,
            capture_chunk_ms: 1,
            silence_gate_rms: 50_000,
            hangover_ms: 50_000,
            host_max_inflight_chunks: 1,
        };
        let clean = raw.sanitized();
        assert_eq!(clean.mic_sample_rate, 8_000);
        assert_eq!(clean.mic_frame_ms, 5);
        assert_eq!(clean.mic_send_interval_ms, 500);
        assert!(!clean.mic_lag_cap);
        assert_eq!(clean.mic_idle_rms, 10_000);
        assert_eq!(clean.mic_burst_rms, 100);
        assert_eq!(clean.recognition_delay_ms, 5_000);
        assert_eq!(clean.burst_duration_ms, 50);
        assert_eq!(clean.hijack_window_ms, 100);
        assert_eq!(clean.capture_sample_rate, 96_000);
        assert_eq!(clean.capture_chunk_ms, 5);
        assert_eq!(clean.silence_gate_rms, 5_000);
        assert_eq!(clean.hangover_ms, 5_000);
        assert_eq!(clean.host_max_inflight_chunks, 2);
        assert_eq!(clean.mic_frame_samples(), 40);
        assert_eq!(clean.capture_chunk_bytes(), 480 * 4);
    }

    #[test]
    fn test_normalize_board_slug() {
        assert_eq!(normalize_board_slug("mini1"), "yandexmini");
        assert_eq!(normalize_board_slug("mini2"), "yandexmini2");
        assert_eq!(normalize_board_slug("mini2nc"), "yandexmini2nc");
        assert_eq!(normalize_board_slug("mini3"), "bergamot");
        assert_eq!(normalize_board_slug("yandexmini3"), "bergamot");
        assert_eq!(normalize_board_slug("mini3pro"), "pickle");
        assert_eq!(normalize_board_slug("yandexmini3pro"), "pickle");
        assert_eq!(normalize_board_slug("lite1"), "yandexmicro");
        assert_eq!(normalize_board_slug("lite2"), "plum");
        assert_eq!(normalize_board_slug("prune"), "plumnc");
        assert_eq!(normalize_board_slug("station-midi"), "station2");
        assert_eq!(normalize_board_slug("midi"), "cucumber");
        assert_eq!(normalize_board_slug("station3"), "orion");

        assert_eq!(
            detect_board_hint_from_filename("bergamot-0.373.1.13-rootfs.tar.gz").as_deref(),
            Some("bergamot")
        );
        assert_eq!(
            detect_board_hint_from_filename("pickle-mini3pro-kuyase-sanitized-20261104.zip")
                .as_deref(),
            Some("pickle")
        );
        assert_eq!(
            detect_board_hint_from_filename("station-lite-rootfs-2026-10-24.tgz").as_deref(),
            Some("yandexmicro")
        );
        assert_eq!(
            detect_board_hint_from_filename("Yandex-Station3-Orion-factory-13").as_deref(),
            Some("orion")
        );
        assert_eq!(
            detect_board_hint_from_filename("yandex-mini2-backup.zip").as_deref(),
            Some("yandexmini2")
        );
        assert_eq!(detect_board_hint_from_filename("rootfs.tar.gz"), None);

        assert_eq!(detect_board_slug_from_probe("mini2", ""), "yandexmini2");
        assert_eq!(
            detect_board_slug_from_probe("plum", "yandex,prune-evt2 yandex,prune"),
            "plumnc"
        );
        assert_eq!(
            detect_board_slug_from_probe("plum", "yandex,plum-evt2 yandex,plum"),
            "plum"
        );
        assert_eq!(
            detect_board_slug_from_probe("yandexmidi", "yandex,station-midi amlogic, axg"),
            "station2"
        );
        assert_eq!(
            detect_board_slug_from_probe("yandexmidi", "amlogic, a5"),
            "cucumber"
        );
        assert_eq!(
            detect_board_slug_from_probe("", "yandex,cucumber-dvt2 amlogic,a5"),
            "cucumber"
        );
        assert_eq!(
            detect_board_slug_from_probe("station3", "yandex,orion"),
            "orion"
        );
        assert_eq!(
            detect_board_slug_from_probe("", "amlogic, axg Amlogic"),
            "yandexmini"
        );
        assert_eq!(detect_board_slug_from_probe("lite1", ""), "yandexmicro");
        assert_eq!(
            detect_board_slug_from_probe("", "yandex,bergamot amlogic,axg"),
            "bergamot"
        );
        assert_eq!(
            detect_board_slug_from_probe("", "yandex,mango amlogic,a1 Yandex Station Mango DVT"),
            "mango"
        );
        assert_eq!(
            detect_board_slug_from_probe("", "yandex,pickle amlogic,a4 Yandex Pickle"),
            "pickle"
        );
    }
}
