pub fn push_varint(buf: &mut Vec<u8>, mut val: u64) {
    loop {
        let mut byte = (val & 0x7f) as u8;
        val >>= 7;
        if val != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if val == 0 {
            break;
        }
    }
}

pub fn push_len_delimited(buf: &mut Vec<u8>, field_num: u32, payload: &[u8]) {
    let tag = ((field_num as u64) << 3) | 2;
    push_varint(buf, tag);
    push_varint(buf, payload.len() as u64);
    buf.extend_from_slice(payload);
}

pub fn build_yio_audio_frame(doa_deg: f32, target_rms: u16, seq: u64) -> Vec<u8> {
    let n_samples = 256usize;
    let mut base_samples = vec![0i16; n_samples + 16];
    let amp = ((target_rms as f32) * 1.414).min(28_000.0);
    for i in 0..base_samples.len() {
        let t = (seq * 256 + i as u64) as f32;
        // Broadband multi-tone signal with rich high-frequency slope for TDOA cross-correlation
        let v = (t * 0.42).sin() * 0.55 + (t * 1.13).cos() * 0.30 + (t * 2.31).sin() * 0.15;
        base_samples[i] = (v * amp) as i16;
    }

    let rad = doa_deg.to_radians();
    let cos_a = rad.cos();
    let sin_a = rad.sin();

    // TDOA sample shifts at step=2 (lag_samples = lag * 2 in [-4..+4])
    let shift_02 = (cos_a * 4.0).round() as isize;
    let shift_13 = (sin_a * 4.0).round() as isize;

    let gains = [
        (1.0 + 0.45 * cos_a).max(0.2),
        (1.0 + 0.45 * sin_a).max(0.2),
        (1.0 - 0.45 * cos_a).max(0.2),
        (1.0 - 0.45 * sin_a).max(0.2),
    ];
    let offsets: [isize; 4] = [
        8,
        8,
        (8 - shift_02).clamp(2, 14),
        (8 - shift_13).clamp(2, 14),
    ];

    let mut channels_pb = Vec::new();
    let names = [
        b"raw_mic_0" as &[u8],
        b"raw_mic_1",
        b"raw_mic_2",
        b"raw_mic_3",
    ];

    for m in 0..4 {
        let mut pcm = Vec::with_capacity(n_samples * 2);
        let off = offsets[m] as usize;
        for i in 0..n_samples {
            let s = ((base_samples[off + i] as f32) * gains[m]).clamp(-32767.0, 32767.0) as i16;
            pcm.extend_from_slice(&s.to_le_bytes());
        }
        // IOAudioChannelData (yandex_io/protos/yandex_io.proto):
        // type = 1 (RAW_MIC = 1), name = 2, data = 3, is_for_recognition = 4 (false), sample_rate = 7 (16000), format = 8 (PCM = 1)
        let mut ch_msg = Vec::with_capacity(pcm.len() + 32);
        ch_msg.extend_from_slice(&[0x08, 0x01]); // field 1: RAW_MIC (1)
        push_len_delimited(&mut ch_msg, 2, names[m]);
        push_len_delimited(&mut ch_msg, 3, &pcm);
        ch_msg.extend_from_slice(&[0x20, 0x00]); // field 4: is_for_recognition = false
        ch_msg.extend_from_slice(&[0x38, 0x80, 0x7d]); // field 7: sample_rate = 16000
        ch_msg.extend_from_slice(&[0x40, 0x01]); // field 8: format = PCM (1)
        push_len_delimited(&mut channels_pb, 1, &ch_msg);
    }

    // Add vqe_0 channel (IOAudioChannelData):
    // type = 1 (VQE_OMNI = 6), name = 2 ("vqe_0"), data = 3, is_for_recognition = 4 (true),
    // doa_angle = 6 (double, wire type 1) + legacy field 5 (float, wire type 5), sample_rate = 7 (16000), format = 8 (PCM = 1)
    let mut vqe_pcm = Vec::with_capacity(n_samples * 2);
    for i in 0..n_samples {
        vqe_pcm.extend_from_slice(&base_samples[8 + i].to_le_bytes());
    }
    let mut vqe_msg = Vec::with_capacity(vqe_pcm.len() + 48);
    vqe_msg.extend_from_slice(&[0x08, 0x06]); // field 1: VQE_OMNI (6)
    push_len_delimited(&mut vqe_msg, 2, b"vqe_0");
    push_len_delimited(&mut vqe_msg, 3, &vqe_pcm);
    vqe_msg.extend_from_slice(&[0x20, 0x01]); // field 4: is_for_recognition = true
    vqe_msg.push((5 << 3) | 5); // legacy field 5 (float)
    vqe_msg.extend_from_slice(&doa_deg.to_le_bytes());
    vqe_msg.push((6 << 3) | 1); // formal field 6: optional double doa_angle = 6
    vqe_msg.extend_from_slice(&(doa_deg as f64).to_le_bytes());
    vqe_msg.extend_from_slice(&[0x38, 0x80, 0x7d]); // field 7: sample_rate = 16000
    vqe_msg.extend_from_slice(&[0x40, 0x01]); // field 8: format = PCM (1)
    push_len_delimited(&mut channels_pb, 1, &vqe_msg);

    let mut qmsg = Vec::with_capacity(channels_pb.len() + 16);
    push_len_delimited(&mut qmsg, 158, &channels_pb);

    let mut framed = Vec::with_capacity(4 + qmsg.len());
    framed.extend_from_slice(&(qmsg.len() as u32).to_le_bytes());
    framed.extend_from_slice(&qmsg);
    framed
}

enum PbField<'a> {
    Varint(u64),
    Fixed64,
    Bytes(&'a [u8]),
    Fixed32,
}

fn read_pb_varint(buf: &[u8], pos: &mut usize) -> Result<u64, &'static str> {
    let mut val = 0u64;
    let mut shift = 0u32;
    while *pos < buf.len() {
        let b = buf[*pos];
        *pos += 1;
        val |= ((b & 0x7f) as u64) << shift;
        if (b & 0x80) == 0 {
            return Ok(val);
        }
        shift += 7;
        if shift >= 64 {
            return Err("varint overflow");
        }
    }
    Err("truncated varint")
}

fn next_pb_field<'a>(
    buf: &'a [u8],
    pos: &mut usize,
) -> Result<Option<(u32, PbField<'a>)>, &'static str> {
    if *pos >= buf.len() {
        return Ok(None);
    }
    let tag = read_pb_varint(buf, pos)?;
    let field_num = (tag >> 3) as u32;
    let wire_type = (tag & 0x07) as u8;
    if field_num == 0 {
        return Err("invalid field number 0");
    }
    match wire_type {
        0 => Ok(Some((field_num, PbField::Varint(read_pb_varint(buf, pos)?)))),
        1 => {
            if *pos + 8 > buf.len() {
                return Err("truncated fixed64");
            }
            *pos += 8;
            Ok(Some((field_num, PbField::Fixed64)))
        }
        2 => {
            let len = read_pb_varint(buf, pos)? as usize;
            if *pos + len > buf.len() {
                return Err("truncated length-delimited field");
            }
            let slice = &buf[*pos..*pos + len];
            *pos += len;
            Ok(Some((field_num, PbField::Bytes(slice))))
        }
        5 => {
            if *pos + 4 > buf.len() {
                return Err("truncated fixed32");
            }
            *pos += 4;
            Ok(Some((field_num, PbField::Fixed32)))
        }
        _ => Err("unsupported wire type"),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecodedRemoting {
    CancelDialog,
    StopTts,
    StartVocalize(String),
    AliceMethod(u64),
    PlaybackControl(u64),
    DeviceState { mics_muted: Option<bool> },
    OtherCapability { object_id: String, method_field: u32 },
}

// Strict decoder for QuasarMessage { remoting = 210 } (v3rt3p/quasar-protos: quasar_proto.proto & remoting.proto)
pub fn decode_quasar_remoting_packet(payload: &[u8]) -> Result<DecodedRemoting, String> {
    let mut pos = 0usize;
    let mut remoting_bytes: Option<&[u8]> = None;
    while let Some((fnum, val)) = next_pb_field(payload, &mut pos).map_err(|e| e.to_string())? {
        if fnum == 210 {
            if let PbField::Bytes(b) = val {
                remoting_bytes = Some(b);
            } else {
                return Err("QuasarMessage.remoting (210) must be wire_type 2".to_string());
            }
        }
    }
    let rem_buf =
        remoting_bytes.ok_or_else(|| "missing QuasarMessage.remoting (field 210)".to_string())?;

    let mut rpos = 0usize;
    let mut remote_obj_id = String::new();
    let mut alice_method_bytes: Option<&[u8]> = None;
    let mut playback_method_bytes: Option<&[u8]> = None;
    let mut device_state_bytes: Option<&[u8]> = None;
    let mut any_valid_method_field: Option<u32> = None;

    while let Some((fnum, val)) = next_pb_field(rem_buf, &mut rpos).map_err(|e| e.to_string())? {
        match (fnum, val) {
            (7, PbField::Bytes(b)) => {
                remote_obj_id = String::from_utf8_lossy(b).into_owned();
            }
            (8, PbField::Bytes(b)) => {
                alice_method_bytes = Some(b);
                any_valid_method_field = Some(8);
            }
            (13, PbField::Bytes(b)) => {
                playback_method_bytes = Some(b);
                any_valid_method_field = Some(13);
            }
            (16, PbField::Bytes(b)) => {
                device_state_bytes = Some(b);
                any_valid_method_field = Some(16);
            }
            // Any other method field in quasar.proto.Remoting (remoting.proto)
            (fnum, _) if fnum != 7 && fnum != 25 => {
                any_valid_method_field = Some(fnum);
            }
            _ => {}
        }
    }

    if remote_obj_id == "PlaybackControlCapability" {
        if let Some(pb_buf) = playback_method_bytes {
            let mut mpos = 0usize;
            let mut method_id: Option<u64> = None;
            while let Some((fnum, val)) =
                next_pb_field(pb_buf, &mut mpos).map_err(|e| e.to_string())?
            {
                if fnum == 1 {
                    if let PbField::Varint(v) = val {
                        method_id = Some(v);
                    }
                }
            }
            return Ok(DecodedRemoting::PlaybackControl(method_id.unwrap_or(0)));
        }
        if let Some(mf) = any_valid_method_field {
            return Ok(DecodedRemoting::OtherCapability {
                object_id: remote_obj_id,
                method_field: mf,
            });
        }
        return Err(
            "PlaybackControlCapability missing playback_control_capability_method (field 13)"
                .to_string(),
        );
    }

    if remote_obj_id == "DeviceStateCapability" {
        if let Some(ds_buf) = device_state_bytes {
            let mut dpos = 0usize;
            let mut mics_muted = None;
            while let Some((fnum, val)) =
                next_pb_field(ds_buf, &mut dpos).map_err(|e| e.to_string())?
            {
                if fnum == 21 {
                    if let PbField::Varint(v) = val {
                        mics_muted = Some(v != 0);
                    }
                }
            }
            return Ok(DecodedRemoting::DeviceState { mics_muted });
        }
        if let Some(mf) = any_valid_method_field {
            return Ok(DecodedRemoting::OtherCapability {
                object_id: remote_obj_id,
                method_field: mf,
            });
        }
    }

    if remote_obj_id == "AliceCapability" {
        if let Some(alice_buf) = alice_method_bytes {
            let mut apos = 0usize;
            let mut method_id: Option<u64> = None;
            let mut vocalize_text = String::new();

            while let Some((fnum, val)) =
                next_pb_field(alice_buf, &mut apos).map_err(|e| e.to_string())?
            {
                match (fnum, val) {
                    (1, PbField::Varint(v)) => method_id = Some(v),
                    (13, PbField::Bytes(vdir)) => {
                        let mut vpos = 0usize;
                        while let Some((vf, vval)) =
                            next_pb_field(vdir, &mut vpos).map_err(|e| e.to_string())?
                        {
                            if vf == 1 {
                                if let PbField::Bytes(tb) = vval {
                                    vocalize_text = String::from_utf8_lossy(tb).into_owned();
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }

            return match method_id {
                Some(6) => Ok(DecodedRemoting::CancelDialog),
                Some(14) => Ok(DecodedRemoting::StopTts),
                Some(18) => {
                    if vocalize_text.is_empty() {
                        Err(
                            "START_VOCALIZE (18) missing vocalize_directive.Text (field 13.1)"
                                .to_string(),
                        )
                    } else {
                        Ok(DecodedRemoting::StartVocalize(vocalize_text))
                    }
                }
                Some(m) => Ok(DecodedRemoting::AliceMethod(m)),
                None => Err("AliceCapabilityMethod missing method enum (field 1)".to_string()),
            };
        }
        if let Some(mf) = any_valid_method_field {
            return Ok(DecodedRemoting::OtherCapability {
                object_id: remote_obj_id,
                method_field: mf,
            });
        }
        return Err("AliceCapability missing alice_capability_method (field 8)".to_string());
    }

    // Other valid Remoting objects (EndpointStorage_, FilePlayerCapability, SpotterCapability, etc.)
    if !remote_obj_id.is_empty() {
        if let Some(mf) = any_valid_method_field {
            return Ok(DecodedRemoting::OtherCapability {
                object_id: remote_obj_id,
                method_field: mf,
            });
        }
    }

    Err(format!(
        "unsupported or missing Remoting.remote_object_id (field 7): '{}'",
        remote_obj_id
    ))
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecodedIoHubPacket {
    ExternalDirectives(String),
    IoControlField(u32),
    RetrieveStateHandshake,
    ServiceReady(String),
    IoEvent,
}

// Strict decoder for QuasarMessage on iohub_services (:9903):
// Supports io_control (88), io_hub_message (183), service_name_ready (157), io_event (85)
pub fn decode_quasar_io_control_packet(payload: &[u8]) -> Result<DecodedIoHubPacket, String> {
    let mut pos = 0usize;
    let mut io_ctrl_bytes: Option<&[u8]> = None;
    let mut has_io_hub_msg = false;
    let mut service_ready: Option<String> = None;
    let mut has_io_event = false;

    while let Some((fnum, val)) = next_pb_field(payload, &mut pos).map_err(|e| e.to_string())? {
        match (fnum, val) {
            (88, PbField::Bytes(b)) => io_ctrl_bytes = Some(b),
            (183, PbField::Bytes(_)) => has_io_hub_msg = true,
            (157, PbField::Bytes(b)) => {
                service_ready = Some(String::from_utf8_lossy(b).into_owned())
            }
            (1..=300, _) => has_io_event = true,
            _ => {}
        }
    }

    if let Some(ctrl_buf) = io_ctrl_bytes {
        let mut cpos = 0usize;
        let mut any_field: Option<u32> = None;
        while let Some((fnum, val)) =
            next_pb_field(ctrl_buf, &mut cpos).map_err(|e| e.to_string())?
        {
            any_field = Some(fnum);
            if fnum == 95 {
                if let PbField::Bytes(b) = val {
                    return Ok(DecodedIoHubPacket::ExternalDirectives(
                        String::from_utf8_lossy(b).into_owned(),
                    ));
                }
            }
        }
        if let Some(f) = any_field {
            return Ok(DecodedIoHubPacket::IoControlField(f));
        }
        return Err(
            "IOControlCommand (field 88) is empty (missing external_directives field 95)"
                .to_string(),
        );
    }

    if has_io_hub_msg {
        return Ok(DecodedIoHubPacket::RetrieveStateHandshake);
    }
    if let Some(s) = service_ready {
        return Ok(DecodedIoHubPacket::ServiceReady(s));
    }
    if has_io_event {
        return Ok(DecodedIoHubPacket::IoEvent);
    }

    Err("missing QuasarMessage.io_control (88) or io_hub_message (183)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_cancel_dialog_and_vocalize() {
        let mut alice_method = Vec::new();
        alice_method.extend_from_slice(&[0x08, 0x06]); // CANCEL_DIALOG_AND_CLEAR_QUEUE (6)
        let mut remoting = Vec::new();
        push_len_delimited(&mut remoting, 7, b"AliceCapability");
        push_len_delimited(&mut remoting, 8, &alice_method);
        let mut qmsg = Vec::new();
        push_len_delimited(&mut qmsg, 210, &remoting);

        assert_eq!(
            decode_quasar_remoting_packet(&qmsg),
            Ok(DecodedRemoting::CancelDialog)
        );

        let mut voc_dir = Vec::new();
        push_len_delimited(&mut voc_dir, 1, "Привет".as_bytes());
        let mut voc_method = Vec::new();
        voc_method.extend_from_slice(&[0x08, 18]);
        push_len_delimited(&mut voc_method, 13, &voc_dir);
        let mut remoting2 = Vec::new();
        push_len_delimited(&mut remoting2, 7, b"AliceCapability");
        push_len_delimited(&mut remoting2, 8, &voc_method);
        let mut qmsg2 = Vec::new();
        push_len_delimited(&mut qmsg2, 210, &remoting2);

        assert_eq!(
            decode_quasar_remoting_packet(&qmsg2),
            Ok(DecodedRemoting::StartVocalize("Привет".to_string()))
        );
    }

    #[test]
    fn test_accept_native_endpoint_storage_and_device_state() {
        let mut remoting = Vec::new();
        push_len_delimited(&mut remoting, 7, b"EndpointStorage_");
        push_len_delimited(&mut remoting, 1, &[0x10, 0x02]);
        let mut qmsg = Vec::new();
        push_len_delimited(&mut qmsg, 210, &remoting);
        assert_eq!(
            decode_quasar_remoting_packet(&qmsg),
            Ok(DecodedRemoting::OtherCapability {
                object_id: "EndpointStorage_".to_string(),
                method_field: 1
            })
        );
    }

    #[test]
    fn test_decode_io_control_packet() {
        let mut io_ctrl = Vec::new();
        push_len_delimited(&mut io_ctrl, 95, br#"{"directives":[]}"#);
        let mut qmsg = Vec::new();
        push_len_delimited(&mut qmsg, 88, &io_ctrl);

        assert_eq!(
            decode_quasar_io_control_packet(&qmsg),
            Ok(DecodedIoHubPacket::ExternalDirectives(
                r#"{"directives":[]}"#.to_string()
            ))
        );
    }
}
