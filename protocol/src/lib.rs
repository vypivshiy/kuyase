use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};

pub const KIND_STATE_SNAPSHOT: u8 = 1;
pub const KIND_PCM_AUDIO_CHUNK: u8 = 2;
pub const KIND_COMMAND_ACK: u8 = 3;
pub const KIND_HOST_COMMAND: u8 = 1;

pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

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
}

pub type BridgeState = BridgeStateSnapshot;

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

pub fn normalize_board_slug(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "station-lite" | "lite" | "lite1" | "micro" | "yandexmicro" => "yandexmicro".to_string(),
        "mini" | "mini1" | "yandexmini" => "yandexmini".to_string(),
        "mini2" | "yandexmini_2" | "yandexmini2" => "yandexmini2".to_string(),
        "mini2nc" | "yandexmini_2_nc" | "yandexmini2nc" => "yandexmini2nc".to_string(),
        "mini3" | "bergamot" => "bergamot".to_string(),
        "mini3pro" | "pickle" => "pickle".to_string(),
        "lite2" | "plum" => "plum".to_string(),
        "prune" | "lite2nc" | "plumnc" => "plumnc".to_string(),
        "midi" | "cucumber" => "cucumber".to_string(),
        "station3" | "orion" => "orion".to_string(),
        "station_2" | "station-midi" | "yandexmidi" | "station2" => "station2".to_string(),
        "street" | "mango" => "mango".to_string(),
        "hub" | "saturn" => "saturn".to_string(),
        other => other.to_string(),
    }
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
        let mut state = BridgeStateSnapshot::default();
        state.board_slug = "orion".to_string();
        state.board_name = "Яндекс Станция 3 (YNDX-00055 / orion)".to_string();
        state.ring_leds = 36;
        state.ambilight_leds = 12;
        state.aux_light_kind = "ambilight".to_string();
        state.display_kind = "matrix_21x12".to_string();
        state.ring_pixels = vec![[107, 15, 255]; 36];
        state.logo_pixels = vec![[200, 110, 255]; 12];
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
    fn test_normalize_board_slug() {
        assert_eq!(normalize_board_slug("mini1"), "yandexmini");
        assert_eq!(normalize_board_slug("mini2"), "yandexmini2");
        assert_eq!(normalize_board_slug("mini2nc"), "yandexmini2nc");
        assert_eq!(normalize_board_slug("lite1"), "yandexmicro");
        assert_eq!(normalize_board_slug("lite2"), "plum");
        assert_eq!(normalize_board_slug("prune"), "plumnc");
        assert_eq!(normalize_board_slug("station-midi"), "station2");
        assert_eq!(normalize_board_slug("midi"), "cucumber");
        assert_eq!(normalize_board_slug("station3"), "orion");
    }
}
