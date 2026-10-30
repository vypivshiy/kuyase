use crate::state::{now_millis, AliceState, BridgeState};
use std::fs;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub fn parse_led_pattern_str(raw: &str, ring_leds: usize) -> Vec<(Vec<[u8; 3]>, u32)> {
    let mut frames = Vec::new();
    for line in raw.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let Ok(dur_ms) = parts.last().unwrap().parse::<u32>() else {
            continue;
        };
        let hex_tokens = &parts[..parts.len() - 1];
        let mut parsed = Vec::with_capacity(hex_tokens.len());
        for tok in hex_tokens {
            if tok.len() == 6 {
                if let Ok(rgb) = u32::from_str_radix(tok, 16) {
                    parsed.push([
                        ((rgb >> 16) & 0xff) as u8,
                        ((rgb >> 8) & 0xff) as u8,
                        (rgb & 0xff) as u8,
                    ]);
                }
            }
        }
        if !parsed.is_empty() && ring_leds > 0 {
            let mut resized = Vec::with_capacity(ring_leds);
            for i in 0..ring_leds {
                resized.push(parsed[i % parsed.len()]);
            }
            frames.push((resized, dur_ms.clamp(15, 250)));
        }
    }
    frames
}

pub fn load_led_pattern(kind: &str, ring_leds: usize) -> Vec<(Vec<[u8; 3]>, u32)> {
    let candidates = [
        format!("/system/vendor/quasar/ledpatterns/ring/{}.led", kind),
        format!("/system/vendor/quasar/ledpatterns/oknix/{}.led", kind),
        format!("/system/vendor/quasar/ledpatterns/{}.led", kind),
        format!("/system/vendor/quasar/ledpatterns/ring/alice_{}.led", kind),
        format!("/system/vendor/quasar/ledpatterns/oknix/alice_{}.led", kind),
        format!("/system/vendor/quasar/ledpatterns/alice_{}.led", kind),
        format!("/system/vendor/quasar/ledpatterns/alice_pro_{}.led", kind),
    ];
    for path in &candidates {
        if let Ok(raw) = fs::read_to_string(path) {
            let frames = parse_led_pattern_str(&raw, ring_leds);
            if !frames.is_empty() {
                return frames;
            }
        }
    }

    if kind == "mute_mics" {
        return vec![(vec![[255, 0, 0]; ring_leds], 100)];
    }
    if kind == "error" {
        return vec![(vec![[255, 30, 0]; ring_leds], 80)];
    }

    let mut fallback = Vec::new();
    for f in 0..20 {
        let mut px = Vec::with_capacity(ring_leds);
        for i in 0..ring_leds {
            let phase = ((i + f) % ring_leds.max(1)) as f32 / (ring_leds.max(1) as f32);
            let r = (107.0 * (0.4 + 0.6 * phase)) as u8;
            let g = (15.0 + 90.0 * (1.0 - phase)) as u8;
            let b = 255u8;
            px.push([r, g, b]);
        }
        fallback.push((px, 30));
    }
    fallback
}

pub fn load_ambilight_pattern(kind: &str, zones: usize) -> Vec<(Vec<[u8; 3]>, u32)> {
    if zones == 0 {
        return Vec::new();
    }
    let candidates = [
        format!("/system/vendor/quasar/ledpatterns/ambilight/{}.led", kind),
        "/system/vendor/quasar/ledpatterns/ambilight/violet.led".to_string(),
        "/system/vendor/quasar/ledpatterns/ambilight/boot.led".to_string(),
    ];
    for path in &candidates {
        if let Ok(raw) = fs::read_to_string(path) {
            let frames = parse_led_pattern_str(&raw, zones);
            if !frames.is_empty() {
                return frames;
            }
        }
    }
    vec![(vec![[107, 15, 255]; zones], 40)]
}

pub fn build_volume_led_frame(
    volume_frames: &[(Vec<[u8; 3]>, u32)],
    vol_level: u32,
    ring_leds: usize,
) -> Vec<[u8; 3]> {
    let clamped = vol_level.min(10);
    if !volume_frames.is_empty() {
        let idx = ((clamped as usize) * (volume_frames.len() - 1)) / 10;
        return volume_frames[idx].0.clone();
    }
    let lit = ((clamped as usize) * ring_leds).div_ceil(10);
    let color = match clamped {
        0 => [0, 0, 0],
        1..=3 => [0, 255, 69],
        4..=6 => [255, 219, 49],
        7..=8 => [255, 129, 0],
        _ => [225, 0, 0],
    };
    let mut px = vec![[0u8; 3]; ring_leds];
    for item in px.iter_mut().take(lit.min(ring_leds)) {
        *item = color;
    }
    px
}

fn apply_native_frames(state: &BridgeState, ring_px: Vec<[u8; 3]>, aux_px: Option<Vec<[u8; 3]>>) {
    state.peripherals.ring.write_native_frame(&ring_px);
    *state.native_led_frame.lock().unwrap() = ring_px;

    let aux_zones = state.peripherals.aux_light.zone_count();
    if aux_zones > 0 {
        if let Some(zones) = aux_px {
            state.peripherals.aux_light.write_native_frame(&zones);
            *state.native_aux_frame.lock().unwrap() = zones;
        } else {
            *state.native_aux_frame.lock().unwrap() = vec![[0u8; 3]; aux_zones];
        }
    }
}

pub fn sync_immediate_led_state(state: &BridgeState) {
    let ring_leds = state.ring_leds;
    let aux_zones = state.peripherals.aux_light.zone_count();
    let now = now_millis();
    let vol_until = state.volume_overlay_until_ms.load(Ordering::Relaxed);
    let forced = state.forced_led_pattern.lock().unwrap().clone();

    if now < vol_until {
        let lvl = state.volume_level.load(Ordering::Relaxed);
        let volume_frames = load_led_pattern("volume", ring_leds);
        let px = build_volume_led_frame(&volume_frames, lvl, ring_leds);
        apply_native_frames(state, px, None);
        state.peripherals.front_display.render_volume_overlay(lvl, true);
        return;
    }

    if !forced.is_empty() {
        if let Some(hex) = forced.strip_prefix("rgb:") {
            let clean = hex.trim_start_matches('#');
            let rgb = u32::from_str_radix(clean, 16).unwrap_or(0);
            let c = [
                ((rgb >> 16) & 0xff) as u8,
                ((rgb >> 8) & 0xff) as u8,
                (rgb & 0xff) as u8,
            ];
            let aux = if aux_zones > 0 {
                Some(vec![c; aux_zones])
            } else {
                None
            };
            apply_native_frames(state, vec![c; ring_leds], aux);
            return;
        }
        let frames = load_led_pattern(&forced, ring_leds);
        if let Some((px, _)) = frames.first() {
            apply_native_frames(state, px.clone(), None);
        }
        return;
    }

    if state.mics_muted.load(Ordering::Relaxed) {
        let mute_frames = load_led_pattern("mute_mics", ring_leds);
        let (ref px, _) = mute_frames[0];
        let aux = if aux_zones > 0 {
            Some(vec![[180, 0, 0]; aux_zones])
        } else {
            None
        };
        apply_native_frames(state, px.clone(), aux);
        return;
    }

    if state.alice_state() == AliceState::Idle && state.forced_led_pattern.lock().unwrap().is_empty()
    {
        let aux = if aux_zones > 0 {
            Some(vec![[0u8; 3]; aux_zones])
        } else {
            None
        };
        apply_native_frames(state, vec![[0u8; 3]; ring_leds], aux);
    }
}

pub fn spawn_native_led_animator(state: Arc<BridgeState>) {
    thread::spawn(move || {
        let ring_leds = state.ring_leds;
        let aux_zones = state.peripherals.aux_light.zone_count();
        let listening_frames = load_led_pattern("listening", ring_leds);
        let speaking_frames = load_led_pattern("speaking", ring_leds);
        let thinking_frames = load_led_pattern("thinking", ring_leds);
        let mute_frames = load_led_pattern("mute_mics", ring_leds);
        let volume_frames = load_led_pattern("volume", ring_leds);
        let timer_frames = load_led_pattern("timer", ring_leds);
        let error_frames = load_led_pattern("error", ring_leds);
        let bt_frames = load_led_pattern("bluetooth_playing", ring_leds);
        let notif_frames = load_led_pattern("notification_available", ring_leds);
        let ambilight_violet = load_ambilight_pattern("violet", aux_zones);

        let mut frame_idx = 0usize;
        let mut was_active = false;

        loop {
            let now = now_millis();
            let vol_until = state.volume_overlay_until_ms.load(Ordering::Relaxed);
            let forced = state.forced_led_pattern.lock().unwrap().clone();

            // 1. Оверлей громкости (1.5 с: шкала на кольце + кадр громкости на матрице 21×12)
            if now < vol_until {
                was_active = true;
                let lvl = state.volume_level.load(Ordering::Relaxed);
                let px = build_volume_led_frame(&volume_frames, lvl, ring_leds);
                apply_native_frames(&state, px, None);
                state.peripherals.front_display.render_volume_overlay(lvl, true);
                thread::sleep(Duration::from_millis(45));
                continue;
            } else {
                state.peripherals.front_display.render_volume_overlay(0, false);
            }

            // 2. Ручной выбор паттерна / цвета из UI
            if !forced.is_empty() {
                was_active = true;
                if let Some(hex) = forced.strip_prefix("rgb:") {
                    let clean = hex.trim_start_matches('#');
                    let rgb = u32::from_str_radix(clean, 16).unwrap_or(0);
                    let c = [
                        ((rgb >> 16) & 0xff) as u8,
                        ((rgb >> 8) & 0xff) as u8,
                        (rgb & 0xff) as u8,
                    ];
                    let aux = if aux_zones > 0 {
                        Some(vec![c; aux_zones])
                    } else {
                        None
                    };
                    apply_native_frames(&state, vec![c; ring_leds], aux);
                    thread::sleep(Duration::from_millis(60));
                    continue;
                }
                let frames = match forced.as_str() {
                    "listening" | "alice_listening" => &listening_frames,
                    "speaking" | "alice_speaking" => &speaking_frames,
                    "thinking" | "alice_thinking" => &thinking_frames,
                    "mute_mics" => &mute_frames,
                    "volume" => &volume_frames,
                    "timer" => &timer_frames,
                    "error" => &error_frames,
                    "bluetooth_playing" => &bt_frames,
                    "notification_available" => &notif_frames,
                    _ => &listening_frames,
                };
                let (ref px, dur_ms) = frames[frame_idx % frames.len()];
                let aux = if aux_zones > 0 && !ambilight_violet.is_empty() {
                    Some(ambilight_violet[frame_idx % ambilight_violet.len()].0.clone())
                } else {
                    None
                };
                apply_native_frames(&state, px.clone(), aux);
                frame_idx = frame_idx.wrapping_add(1);
                thread::sleep(Duration::from_millis(dur_ms as u64));
                continue;
            }

            // 3. Активный диалог Алисы (LISTENING / SPEAKING)
            match state.alice_state() {
                AliceState::Listening | AliceState::Speaking => {
                    was_active = true;
                    let frames = if state.alice_state() == AliceState::Listening {
                        &listening_frames
                    } else {
                        &speaking_frames
                    };
                    let (ref px, dur_ms) = frames[frame_idx % frames.len()];
                    let aux = if aux_zones > 0 && !ambilight_violet.is_empty() {
                        Some(ambilight_violet[frame_idx % ambilight_violet.len()].0.clone())
                    } else {
                        None
                    };
                    apply_native_frames(&state, px.clone(), aux);
                    frame_idx = frame_idx.wrapping_add(1);
                    thread::sleep(Duration::from_millis(dur_ms as u64));
                    continue;
                }
                AliceState::Idle => {}
            }

            // 4. Отключённые микрофоны (MUTE MIC -> красное кольцо)
            if state.mics_muted.load(Ordering::Relaxed) {
                was_active = true;
                let (ref px, _) = mute_frames[0];
                let aux = if aux_zones > 0 {
                    Some(vec![[180, 0, 0]; aux_zones])
                } else {
                    None
                };
                apply_native_frames(&state, px.clone(), aux);
                thread::sleep(Duration::from_millis(80));
                continue;
            }

            // 5. Состояние покоя (IDLE)
            if was_active {
                was_active = false;
                frame_idx = 0;
                let aux = if aux_zones > 0 {
                    Some(vec![[0u8; 3]; aux_zones])
                } else {
                    None
                };
                apply_native_frames(&state, vec![[0u8; 3]; ring_leds], aux);
            }
            thread::sleep(Duration::from_millis(40));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_led_pattern_str() {
        let sample = "loop\n6b0fff 6b0fff 6b0fff 6b0fff 6b0fff 6b0fff 20\n00ff80 00ff80 35\n";
        let frames = parse_led_pattern_str(sample, 6);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].0.len(), 6);
        assert_eq!(frames[0].0[0], [0x6b, 0x0f, 0xff]);
        assert_eq!(frames[0].1, 20);
        assert_eq!(frames[1].0[0], [0x00, 0xff, 0x80]);
        assert_eq!(frames[1].1, 35);
    }

    #[test]
    fn test_volume_led_arc_fallback() {
        let px0 = build_volume_led_frame(&[], 0, 6);
        assert_eq!(px0, vec![[0, 0, 0]; 6]);
        let px5 = build_volume_led_frame(&[], 5, 6);
        assert_eq!(px5[0], [255, 219, 49]);
        assert_eq!(px5[2], [255, 219, 49]);
        assert_eq!(px5[3], [0, 0, 0]);
        let px10 = build_volume_led_frame(&[], 10, 6);
        assert_eq!(px10, vec![[225, 0, 0]; 6]);
    }
}
