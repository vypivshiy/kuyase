use crate::gui::state::GuiAppState;
use eframe::egui::{self, Color32, Margin, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use protocol::{BridgeStateSnapshot, HostCommand};
use std::collections::BTreeMap;
use std::f32::consts::{FRAC_PI_2, TAU};
use std::time::Duration;

pub fn led_ring_position(center: Pos2, radius: f32, index: usize, count: usize) -> Pos2 {
    if count <= 1 {
        return center;
    }
    let angle = -FRAC_PI_2 + TAU * (index as f32) / (count as f32);
    Pos2::new(
        center.x + radius * angle.cos(),
        center.y + radius * angle.sin(),
    )
}

pub fn doa_angle_from_pointer(center: Pos2, pointer: Pos2) -> u32 {
    let dx = pointer.x - center.x;
    let dy = pointer.y - center.y;
    let deg = dy.atan2(dx).to_degrees() + 90.0;
    (deg.rem_euclid(360.0).round() as u32) % 360
}

pub fn segment_brightness(segments: &BTreeMap<String, u8>, key: &str) -> u8 {
    segments.get(key).copied().unwrap_or(0)
}

pub fn default_board_ring_leds(slug: &str) -> usize {
    match slug {
        "cucumber" => 24,
        "orion" => 36,
        "yandexmini" | "yandexmini2" | "yandexmini2nc" | "bergamot" => 6,
        "pickle" => 16,
        "plum" | "plumnc" | "yandexmicro" => 3,
        "station2" => 88,
        "mango" => 4,
        "saturn" => 1,
        _ => 6,
    }
}

pub fn board_has_segment_clock(slug: &str, st: &BridgeStateSnapshot) -> bool {
    st.display_kind == "segment_clock"
        || st.has_clock
        || matches!(
            slug,
            "cucumber" | "yandexmini2" | "plum" | "bergamot" | "pickle"
        )
}

pub fn board_has_matrix_21x12(slug: &str, st: &BridgeStateSnapshot) -> bool {
    st.display_kind == "matrix_21x12" || slug == "orion"
}

pub fn clock_segs_for_char(c: char) -> &'static [&'static str] {
    match c.to_ascii_uppercase() {
        '0' | 'O' => &["a", "b", "c", "d", "e", "f"],
        '1' | 'I' => &["b", "c"],
        '2' => &["a", "b", "d", "e", "g"],
        '3' => &["a", "b", "c", "d", "g"],
        '4' => &["b", "c", "f", "g"],
        '5' | 'S' => &["a", "c", "d", "f", "g"],
        '6' => &["a", "c", "d", "e", "f", "g"],
        '7' => &["a", "b", "c"],
        '8' => &["a", "b", "c", "d", "e", "f", "g"],
        '9' => &["a", "b", "c", "d", "f", "g"],
        '-' => &["g"],
        'E' => &["a", "d", "e", "f", "g"],
        'R' => &["e", "g"],
        'C' => &["a", "d", "e", "f"],
        'P' => &["a", "b", "e", "f", "g"],
        _ => &[],
    }
}

pub fn build_local_segments_from_text(text: &str, brightness: u8) -> BTreeMap<String, u8> {
    let mut map = BTreeMap::new();
    if text.trim().is_empty() || brightness == 0 {
        return map;
    }
    let mut chars = Vec::new();
    let mut colon = false;
    let mut degree = false;
    let mut plus = false;
    for ch in text.chars() {
        match ch {
            ':' => colon = true,
            '°' | '*' => degree = true,
            '+' => plus = true,
            _ => {
                if chars.len() < 4 {
                    chars.push(ch);
                }
            }
        }
    }
    for (idx, &ch) in chars.iter().take(4).enumerate() {
        let pos = idx + 1;
        for &part in clock_segs_for_char(ch) {
            map.insert(format!("{}{}", part, pos), brightness);
        }
    }
    if colon {
        map.insert("p3".to_string(), brightness);
        map.insert("q3".to_string(), brightness);
    }
    if degree {
        map.insert("deg".to_string(), brightness);
    }
    if plus {
        map.insert("h1".to_string(), brightness);
        map.insert("m1".to_string(), brightness);
    }
    map
}

fn glyph_4x7(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        '0' | 'O' => [0b0110, 0b1001, 0b1001, 0b1001, 0b1001, 0b1001, 0b0110],
        '1' | 'I' => [0b0010, 0b0110, 0b0010, 0b0010, 0b0010, 0b0010, 0b0111],
        '2' => [0b0110, 0b1001, 0b0001, 0b0010, 0b0100, 0b1000, 0b1111],
        '3' => [0b1110, 0b0001, 0b0001, 0b0110, 0b0001, 0b0001, 0b1110],
        '4' => [0b1001, 0b1001, 0b1001, 0b1111, 0b0001, 0b0001, 0b0001],
        '5' | 'S' => [0b1111, 0b1000, 0b1000, 0b1110, 0b0001, 0b0001, 0b1110],
        '6' => [0b0111, 0b1000, 0b1000, 0b1110, 0b1001, 0b1001, 0b0110],
        '7' => [0b1111, 0b0001, 0b0010, 0b0010, 0b0100, 0b0100, 0b0100],
        '8' => [0b0110, 0b1001, 0b1001, 0b0110, 0b1001, 0b1001, 0b0110],
        '9' => [0b0110, 0b1001, 0b1001, 0b0111, 0b0001, 0b0001, 0b1110],
        '-' => [0b0000, 0b0000, 0b0000, 0b1111, 0b0000, 0b0000, 0b0000],
        '+' => [0b0000, 0b0100, 0b0100, 0b1110, 0b0100, 0b0100, 0b0000],
        _ => [0; 7],
    }
}

pub fn render_text_to_matrix_21x12(text: &str, brightness: u8) -> Vec<u8> {
    let mut grid = vec![0u8; 252];
    let trimmed = text.trim();
    if trimmed.is_empty() || brightness == 0 {
        return grid;
    }

    let mut chars = Vec::new();
    let mut has_colon = false;
    let mut has_degree = false;
    for ch in trimmed.chars() {
        match ch {
            ':' => has_colon = true,
            '°' | '*' => has_degree = true,
            _ => {
                if chars.len() < 4 {
                    chars.push(ch);
                }
            }
        }
    }

    let y0 = 2usize;
    let put_px = |grid: &mut [u8], x: usize, y: usize, val: u8| {
        if x < 21 && y < 12 {
            grid[y * 21 + x] = val;
        }
    };

    let x_positions: &[usize] = match (chars.len(), has_colon) {
        (4, true) => &[0, 5, 12, 17],
        (4, false) => &[0, 5, 10, 15],
        (3, _) => &[1, 6, 11],
        (2, _) => &[4, 10],
        (1, _) => &[8],
        _ => &[],
    };

    for (idx, &ch) in chars.iter().enumerate() {
        let Some(&x0) = x_positions.get(idx) else {
            break;
        };
        let rows = glyph_4x7(ch);
        for (ry, &bits) in rows.iter().enumerate() {
            for rx in 0..4 {
                if (bits & (1 << (3 - rx))) != 0 {
                    put_px(&mut grid, x0 + rx, y0 + ry, brightness);
                }
            }
        }
    }

    if has_colon {
        put_px(&mut grid, 10, y0 + 2, brightness);
        put_px(&mut grid, 10, y0 + 4, brightness);
    }

    if has_degree {
        if chars.len() >= 4 {
            put_px(&mut grid, 20, 0, brightness);
        } else {
            let deg_x = x_positions
                .last()
                .map(|&x| (x + 5).min(18))
                .unwrap_or(16);
            put_px(&mut grid, deg_x + 1, y0, brightness);
            put_px(&mut grid, deg_x, y0 + 1, brightness);
            put_px(&mut grid, deg_x + 2, y0 + 1, brightness);
            put_px(&mut grid, deg_x + 1, y0 + 2, brightness);
        }
    }

    grid
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let ap = p - a;
    let len_sq = ab.length_sq();
    if len_sq <= 1e-5 {
        return ap.length();
    }
    let t = (ap.dot(ab) / len_sq).clamp(0.0, 1.0);
    let proj = a + ab * t;
    p.distance(proj)
}

fn clock_segment_lines(rect: Rect) -> Vec<(String, Pos2, Pos2)> {
    let digit_w = 24.0;
    let digit_h = 40.0;
    let start_x = rect.center().x - 66.0;
    let top_y = rect.center().y - digit_h * 0.5;
    let ym = top_y + digit_h * 0.5;

    let mut out = Vec::with_capacity(30);
    let plus_cx = start_x - 16.0;
    out.push((
        "h1".to_string(),
        Pos2::new(plus_cx, ym - 7.0),
        Pos2::new(plus_cx, ym + 7.0),
    ));
    out.push((
        "m1".to_string(),
        Pos2::new(plus_cx - 7.0, ym),
        Pos2::new(plus_cx + 7.0, ym),
    ));

    for idx in 0..4 {
        let pos = idx + 1;
        let x0 = start_x + (idx as f32) * 34.0 + if idx >= 2 { 12.0 } else { 0.0 };
        let x1 = x0 + digit_w;
        let y1 = top_y + digit_h;
        let coords = [
            ("a", Pos2::new(x0 + 2.5, top_y), Pos2::new(x1 - 2.5, top_y)),
            ("b", Pos2::new(x1, top_y + 2.5), Pos2::new(x1, ym - 1.5)),
            ("c", Pos2::new(x1, ym + 1.5), Pos2::new(x1, y1 - 2.5)),
            ("d", Pos2::new(x0 + 2.5, y1), Pos2::new(x1 - 2.5, y1)),
            ("e", Pos2::new(x0, ym + 1.5), Pos2::new(x0, y1 - 2.5)),
            ("f", Pos2::new(x0, top_y + 2.5), Pos2::new(x0, ym - 1.5)),
            ("g", Pos2::new(x0 + 2.5, ym), Pos2::new(x1 - 2.5, ym)),
        ];
        for (part, p0, p1) in coords {
            out.push((format!("{}{}", part, pos), p0, p1));
        }
    }
    out
}

pub fn hit_test_clock_segment(rect: Rect, pointer: Pos2) -> Option<String> {
    if !rect.contains(pointer) {
        return None;
    }
    let digit_h = 40.0;
    let start_x = rect.center().x - 66.0;
    let top_y = rect.center().y - digit_h * 0.5;
    let colon_x = start_x + 2.0 * 34.0 - 1.0;

    let p3_pos = Pos2::new(colon_x, top_y + 13.0);
    if pointer.distance(p3_pos) <= 6.5 {
        return Some("p3".to_string());
    }
    let q3_pos = Pos2::new(colon_x, top_y + 27.0);
    if pointer.distance(q3_pos) <= 6.5 {
        return Some("q3".to_string());
    }
    let deg_pos = Pos2::new(start_x + 4.0 * 34.0 + 14.0, top_y + 6.0);
    if pointer.distance(deg_pos) <= 7.5 {
        return Some("deg".to_string());
    }

    let mut best: Option<(String, f32)> = None;
    for (name, p0, p1) in clock_segment_lines(rect) {
        let d = dist_to_segment(pointer, p0, p1);
        if d <= 6.0 && best.as_ref().map(|(_, b)| d < *b).unwrap_or(true) {
            best = Some((name, d));
        }
    }
    best.map(|(n, _)| n)
}

pub fn hit_test_matrix_pixel(rect: Rect, cell: f32, pointer: Pos2) -> Option<usize> {
    if !rect.contains(pointer) {
        return None;
    }
    let col = ((pointer.x - rect.left() - 5.0) / cell).floor() as isize;
    let row = ((pointer.y - rect.top() - 5.0) / cell).floor() as isize;
    if (0..21).contains(&col) && (0..12).contains(&row) {
        Some((row as usize) * 21 + (col as usize))
    } else {
        None
    }
}

fn draw_ring_led(painter: &egui::Painter, pos: Pos2, radius: f32, rgb: [u8; 3]) {
    let [r, g, b] = rgb;
    if r > 0 || g > 0 || b > 0 {
        painter.circle_filled(
            pos,
            radius * 1.55,
            Color32::from_rgba_unmultiplied(r, g, b, 52),
        );
        painter.circle_filled(pos, radius, Color32::from_rgb(r, g, b));
        painter.circle_stroke(
            pos,
            radius + 0.5,
            Stroke::new(0.8, Color32::from_rgba_unmultiplied(15, 18, 24, 180)),
        );
    } else {
        painter.circle_filled(pos, radius, Color32::from_rgb(28, 32, 40));
        painter.circle_stroke(pos, radius, Stroke::new(1.0, Color32::from_rgb(56, 64, 78)));
    }
}

fn draw_matrix_dot(painter: &egui::Painter, pos: Pos2, radius: f32, val: u8) {
    if val > 0 {
        let alpha_glow = (val / 4).max(16);
        painter.circle_filled(
            pos,
            radius * 1.22,
            Color32::from_rgba_unmultiplied(140, 205, 255, alpha_glow),
        );
        let c = val.max(48);
        painter.circle_filled(pos, radius, Color32::from_rgb(c, c, (c as u16 + 15).min(255) as u8));
    } else {
        painter.circle_filled(pos, radius * 0.88, Color32::from_rgb(26, 30, 38));
        painter.circle_stroke(
            pos,
            radius * 0.88,
            Stroke::new(0.8, Color32::from_rgb(44, 50, 62)),
        );
    }
}

fn draw_segment_clock(painter: &egui::Painter, rect: Rect, segments: &BTreeMap<String, u8>) {
    painter.rect_filled(rect, 6.0, Color32::from_rgb(13, 16, 21));
    painter.rect_stroke(rect, 6.0, Stroke::new(1.0, Color32::from_rgb(51, 59, 72)));

    let seg_color = |br: u8| {
        if br > 0 {
            Color32::from_rgba_unmultiplied(232, 246, 255, br.max(55))
        } else {
            Color32::from_rgb(37, 43, 54)
        }
    };

    for (name, p0, p1) in clock_segment_lines(rect) {
        let br = segment_brightness(segments, &name);
        painter.line_segment([p0, p1], Stroke::new(3.6, seg_color(br)));
    }

    let digit_h = 40.0;
    let start_x = rect.center().x - 66.0;
    let top_y = rect.center().y - digit_h * 0.5;
    let colon_x = start_x + 2.0 * 34.0 - 1.0;

    let p3 = segment_brightness(segments, "p3");
    let q3 = segment_brightness(segments, "q3");
    painter.circle_filled(Pos2::new(colon_x, top_y + 13.0), 2.8, seg_color(p3));
    painter.circle_filled(Pos2::new(colon_x, top_y + 27.0), 2.8, seg_color(q3));

    let deg_br = segment_brightness(segments, "deg");
    painter.circle_stroke(
        Pos2::new(start_x + 4.0 * 34.0 + 14.0, top_y + 6.0),
        4.2,
        Stroke::new(2.4, seg_color(deg_br)),
    );
}

fn stage_box_frame(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(Color32::from_rgb(23, 25, 30))
        .stroke(Stroke::new(1.0, Color32::from_rgb(56, 62, 74)))
        .rounding(8.0)
        .inner_margin(Margin::same(10.0))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add_contents(ui);
        });
}

pub fn render_twin_canvas(ui: &mut egui::Ui, app: &mut GuiAppState) {
    let bridge_online = app
        .bridge
        .as_ref()
        .map(|b| b.is_connected())
        .unwrap_or(false);
    if bridge_online {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }

    let board_slug = app
        .latest_state
        .as_ref()
        .map(|s| s.board_slug.clone())
        .unwrap_or_else(|| app.config.selected_board.clone());

    let mut st = app.latest_state.clone().unwrap_or_else(|| {
        let mut def = BridgeStateSnapshot::default();
        def.board_slug = board_slug.clone();
        def.board_name = crate::gui::board_display_label(&board_slug);
        def.ring_leds = default_board_ring_leds(&board_slug);
        def.tof_mm = app.tof_slider;
        def.doa_angle = app.doa_slider;
        def.volume_level = app.volume_slider;
        let mut segs = BTreeMap::new();
        for (k, &on) in &app.clock_active_mask {
            if on && app.clock_brightness > 0 {
                segs.insert(k.clone(), app.clock_brightness);
            }
        }
        def.clock_segments = segs;
        def.screen_pixels = app.matrix_custom_pixels.clone();
        def
    });
    if !bridge_online {
        st.tof_mm = app.tof_slider;
        st.doa_angle = app.doa_slider;
        st.volume_level = app.volume_slider;
    }

    egui::ScrollArea::vertical()
        .id_salt("device_stage_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::none()
                .fill(Color32::from_rgb(43, 43, 43))
                .stroke(Stroke::new(1.0, Color32::from_rgb(60, 60, 60)))
                .rounding(10.0)
                .inner_margin(Margin::same(10.0))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!("{} ({})", st.board_name, st.board_slug))
                                .strong(),
                        );
                        ui.add(
                            egui::Button::new(
                                RichText::new(format!("Alice: {}", st.alice_state))
                                    .small()
                                    .strong()
                                    .color(Color32::from_rgb(130, 190, 255)),
                            )
                            .fill(Color32::from_rgb(28, 52, 82))
                            .rounding(99.0)
                            .sense(Sense::hover()),
                        );
                    });

                    ui.add_space(6.0);

                    // 1. Top Enclosure Box (Spacious LED Ring + Ambilight + DOA + ToF Hand)
                    stage_box_frame(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            let trig_txt = if st.ring_trigger.is_empty() {
                                format!("Драйвер LED: Кольцо {} LED", st.ring_leds)
                            } else {
                                format!("Драйвер LED: {}", st.ring_trigger)
                            };
                            let trig_col = if st.ring_trigger.contains("none") {
                                Color32::from_rgb(120, 210, 255)
                            } else if st.ring_trigger.contains("Stealth") {
                                Color32::from_rgb(255, 175, 70)
                            } else {
                                Color32::from_rgb(165, 175, 190)
                            };
                            ui.label(RichText::new(trig_txt).small().color(trig_col))
                                .on_hover_text(
                                    "Режим драйвера светового кольца:\n\
                                     • ledfb-ring (Quasar Native) — штатная анимация прошивки через фреймбуфер\n\
                                     • none (Custom Override) — прямое ручное управление яркостью через /sys/class/leds/*:ring*/brightness\n\
                                     • SN3236 (0x3c) — аппаратный I2C-контроллер на yandexmini",
                                );

                            if board_slug == "yandexmini" {
                                let tof_near = st.tof_mm < 400;
                                let (tof_fg, tof_bg) = if tof_near {
                                    (
                                        Color32::from_rgb(255, 225, 100),
                                        Color32::from_rgb(66, 52, 18),
                                    )
                                } else {
                                    (
                                        Color32::from_rgb(155, 205, 250),
                                        Color32::from_rgb(28, 42, 60),
                                    )
                                };
                                let tof_label = if tof_near {
                                    format!("Датчик ToF (VL53L0X): {} мм (РУКА)", st.tof_mm)
                                } else {
                                    format!("Датчик ToF (VL53L0X): {} мм", st.tof_mm)
                                };
                                ui.add(
                                    egui::Button::new(
                                        RichText::new(tof_label).small().monospace().color(tof_fg),
                                    )
                                    .fill(tof_bg)
                                    .rounding(99.0)
                                    .sense(Sense::hover()),
                                )
                                .on_hover_text(
                                    "Оптический лазерный дальномер приближения STM VL53L0X (устанавливался только в Яндекс Станцию Мини 1 / yandexmini).\n\
                                     Дистанция < 400 мм означает поднесённую ладонь над крышкой колонки (жест управления громкостью / остановки).\n\
                                     Дистанция 8190 мм — препятствий над колонкой нет.",
                                );
                            } else if board_slug == "orion" {
                                let us_near = st.tof_mm < 1500;
                                let (us_fg, us_bg) = if us_near {
                                    (
                                        Color32::from_rgb(255, 225, 100),
                                        Color32::from_rgb(66, 52, 18),
                                    )
                                } else {
                                    (
                                        Color32::from_rgb(155, 205, 250),
                                        Color32::from_rgb(28, 42, 60),
                                    )
                                };
                                let us_label = format!("Ультразвук 38.4 кГц: эхо {} мм", st.tof_mm);
                                ui.add(
                                    egui::Button::new(
                                        RichText::new(us_label).small().monospace().color(us_fg),
                                    )
                                    .fill(us_bg)
                                    .rounding(99.0)
                                    .sense(Sense::hover()),
                                )
                                .on_hover_text(
                                    "На Станции 3 (orion) нет оптического датчика VL53L0X (ToF).\n\
                                     Вместо него используется отдельный ультразвуковой TDM-тракт pcm.ultrasound (96 кГц, несущая 38.4 кГц) через четвёртый усилитель ACM8625S (2-002c) и микрофонную матрицу.",
                                );
                            } else {
                                let als_label =
                                    format!("Датчик света (ALS): {} лк", st.als_lux);
                                ui.add(
                                    egui::Button::new(
                                        RichText::new(als_label)
                                            .small()
                                            .monospace()
                                            .color(Color32::from_rgb(155, 205, 250)),
                                    )
                                    .fill(Color32::from_rgb(28, 42, 60))
                                    .rounding(99.0)
                                    .sense(Sense::hover()),
                                )
                                .on_hover_text(
                                    "Датчик внешней освещённости ALS (ltr308als01 в /sys/bus/iio/devices/iio:device0/in_illuminance_raw).\n\
                                     Используется прошивкой для автояркости светового кольца и дисплея часов (оптического датчика жестов VL53L0X на этой модели нет).",
                                );
                            }
                        });

                        if st.led_trigger_conflict_count > 0 {
                            ui.add_space(2.0);
                            ui.horizontal_wrapped(|ui| {
                                let conflict_badge = egui::Button::new(
                                    RichText::new(format!(
                                        "Конфликт записи sysfs LED: {} (?)",
                                        st.led_trigger_conflict_count
                                    ))
                                    .small()
                                    .strong()
                                    .color(Color32::from_rgb(255, 130, 130)),
                                )
                                .fill(Color32::from_rgb(68, 26, 30))
                                .stroke(Stroke::new(1.0, Color32::from_rgb(150, 55, 60)))
                                .rounding(6.0)
                                .sense(Sense::hover());

                                ui.add(conflict_badge).on_hover_text(
                                    "Что означает «Конфликт записи sysfs LED» (Trigger Conflict):\n\
                                     Была выполнена прямая запись яркости в /sys/class/leds/*:ring*/brightness,\n\
                                     пока световое кольцо находилось под управлением триггера драйвера [ledfb-ring].\n\n\
                                     При активном [ledfb-ring] драйвер отклоняет прямую запись в sysfs и сбрасывает brightness в 0.\n\
                                     Чтобы управлять светодиодами вручную через sysfs, сначала переключите триггер в режим 'none':\n\
                                     echo none > /sys/class/leds/*:ring*/trigger",
                                );

                                if app.session.is_some() {
                                    let is_custom = st.ring_trigger.contains("none");
                                    if !is_custom
                                        && ui
                                            .small_button("Переключить в sysfs (none)")
                                            .on_hover_text("Отключить триггер ledfb-ring для прямой записи яркости через sysfs")
                                            .clicked()
                                    {
                                        app.send_serial_command(
                                            "for d in /sys/class/leds/*:ring*; do echo none > \"$d/trigger\"; done",
                                        );
                                    }
                                }
                            });
                        }

                        let ring_box_h = if board_slug == "orion" { 265.0 } else { 225.0 };
                        let canvas_size = Vec2::new(ui.available_width().max(260.0), ring_box_h);
                        let (rect, response) =
                            ui.allocate_exact_size(canvas_size, Sense::click_and_drag());
                        let painter = ui.painter_at(rect);
                        let center = rect.center();
                        let outer_r = (rect.height().min(rect.width()) * 0.40).clamp(80.0, 105.0);

                        if response.clicked() || response.dragged() {
                            if let Some(ptr) = response.interact_pointer_pos() {
                                let dist = ptr.distance(center);
                                if dist >= outer_r * 0.50 && dist <= outer_r * 1.32 {
                                    let angle = doa_angle_from_pointer(center, ptr);
                                    app.doa_slider = angle;
                                    app.send_command(HostCommand::SetSensor {
                                        doa_angle: Some(angle),
                                        mic_rms: None,
                                        als_lux: None,
                                        temp_c: None,
                                        tof_mm: None,
                                        swipe: None,
                                        swipe_direction: None,
                                    });
                                }
                            }
                        }

                        // Ambilight 12x2 RGBW outer halo (orion)
                        if st.aux_light_kind == "ambilight" || board_slug == "orion" {
                            let zones = st.logo_pixels.len().max(12);
                            painter.circle_stroke(
                                center,
                                outer_r + 16.0,
                                Stroke::new(1.0, Color32::from_rgb(46, 52, 66)),
                            );
                            for i in 0..zones {
                                let rgb = st
                                    .logo_pixels
                                    .get(i)
                                    .copied()
                                    .unwrap_or([90, 45, 130]);
                                let p = led_ring_position(center, outer_r + 16.0, i, zones);
                                draw_ring_led(&painter, p, 4.6, rgb);
                            }
                        }

                        painter.circle_filled(center, outer_r, Color32::from_rgb(24, 28, 36));
                        painter.circle_stroke(
                            center,
                            outer_r,
                            Stroke::new(2.0, Color32::from_rgb(66, 74, 90)),
                        );
                        painter.circle_filled(
                            center,
                            outer_r * 0.65,
                            Color32::from_rgb(16, 19, 25),
                        );

                        let ring_count = st
                            .ring_leds
                            .max(st.ring_pixels.len())
                            .max(default_board_ring_leds(&board_slug))
                            .max(1);
                        let ring_r = if ring_count == 1 { 0.0 } else { outer_r * 0.85 };
                        let led_dot_r = match ring_count {
                            1..=6 => 6.4,
                            7..=16 => 5.2,
                            17..=24 => 4.2,
                            25..=36 => 3.3,
                            _ => 1.9,
                        };
                        for idx in 0..ring_count {
                            let rgb = st.ring_pixels.get(idx).copied().unwrap_or([0, 165, 210]);
                            let pos = led_ring_position(center, ring_r, idx, ring_count);
                            draw_ring_led(&painter, pos, led_dot_r, rgb);
                        }

                        let doa_pos =
                            led_ring_position(center, outer_r + 4.0, st.doa_angle as usize, 360);
                        painter.line_segment(
                            [center, doa_pos],
                            Stroke::new(1.6, Color32::from_rgba_unmultiplied(0, 229, 255, 140)),
                        );
                        painter.circle_filled(doa_pos, 5.2, Color32::from_rgb(0, 229, 255));

                        if board_slug == "yandexmini" && st.tof_mm < 400 {
                            painter.circle_filled(
                                center,
                                outer_r * 0.52,
                                Color32::from_rgba_unmultiplied(229, 165, 10, 55),
                            );
                            painter.circle_stroke(
                                center,
                                outer_r * 0.52,
                                Stroke::new(1.5, Color32::from_rgb(235, 175, 25)),
                            );
                            painter.text(
                                center,
                                egui::Align2::CENTER_CENTER,
                                format!("РУКА: {} мм\nDOA {} град", st.tof_mm, st.doa_angle),
                                egui::FontId::monospace(12.0),
                                Color32::from_rgb(255, 230, 110),
                            );
                        } else {
                            painter.text(
                                center,
                                egui::Align2::CENTER_CENTER,
                                format!(
                                    "DOA {} град\n{} LED | Громк {}/10",
                                    st.doa_angle, ring_count, st.volume_level
                                ),
                                egui::FontId::monospace(12.0),
                                Color32::from_rgb(195, 210, 230),
                            );
                        }
                    });

                    ui.add_space(6.0);

                    // 2. Dynamic Front Faceplate Display (Segment Clock OR 21x12 Matrix)
                    stage_box_frame(ui, |ui| {
                        if board_has_matrix_21x12(&board_slug, &st) {
                            render_matrix_21x12_faceplate(ui, app, &st, bridge_online);
                        } else if board_has_segment_clock(&board_slug, &st) {
                            render_segment_clock_faceplate(ui, app, &board_slug, &st, bridge_online);
                        } else {
                            ui.label(
                                RichText::new(format!(
                                    "Фронтальный экран отсутствует на плате {}",
                                    board_slug
                                ))
                                .small()
                                .color(Color32::from_rgb(155, 165, 180)),
                            );
                            ui.label(
                                RichText::new(format!(
                                    "Индикация выполняется через верхнее кольцо ({} LED).",
                                    st.ring_leds.max(default_board_ring_leds(&board_slug))
                                ))
                                .small()
                                .color(Color32::from_rgb(135, 145, 160)),
                            );
                        }
                    });

                    ui.add_space(6.0);

                    // 3. Physical Enclosure Buttons (Linux uinput)
                    stage_box_frame(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new("Кнопки корпуса (uinput):")
                                    .small()
                                    .color(Color32::from_rgb(165, 175, 190)),
                            );
                            egui::ComboBox::from_id_salt("enclosure_press_mode")
                                .selected_text(match app.button_press_action.as_str() {
                                    "double" => "Двойное (double)",
                                    "triple" => "Тройное (triple)",
                                    "long" => "Удержание (long)",
                                    _ => "Одиночное (single)",
                                })
                                .width(155.0)
                                .show_ui(ui, |ui| {
                                    for (val, lbl) in [
                                        ("single", "Одиночное (single)"),
                                        ("double", "Двойное (double)"),
                                        ("triple", "Тройное (triple)"),
                                        ("long", "Удержание (long)"),
                                    ] {
                                        ui.selectable_value(
                                            &mut app.button_press_action,
                                            val.to_string(),
                                            lbl,
                                        );
                                    }
                                });
                        });
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            let mute_lbl = if st.mics_muted {
                                "Вкл. Микрофоны"
                            } else {
                                "Mute (Микрофоны)"
                            };
                            for (label, btn) in [
                                ("Action (Алиса)", "action"),
                                (mute_lbl, "mute"),
                                ("Громк -", "volume_down"),
                                ("Громк +", "volume_up"),
                            ] {
                                if ui.button(label).clicked() {
                                    app.send_command(HostCommand::PressButton {
                                        button: btn.to_string(),
                                        action: app.button_press_action.clone(),
                                    });
                                }
                            }
                        });
                    });
                });
        });
}

fn render_segment_clock_faceplate(
    ui: &mut egui::Ui,
    app: &mut GuiAppState,
    board_slug: &str,
    st: &BridgeStateSnapshot,
    bridge_online: bool,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new("7-сегментный дисплей (45 сегментов)")
                .small()
                .color(Color32::from_rgb(165, 175, 190)),
        );
        ui.separator();
        ui.label(
            RichText::new(format!("PWM: {}", app.clock_brightness))
                .small()
                .monospace()
                .color(Color32::from_rgb(155, 175, 200)),
        );
    });

    if bridge_online && st.clock_segments.values().any(|&v| v > 0) {
        app.clock_active_mask.clear();
        for (k, &v) in &st.clock_segments {
            if v > 0 {
                app.clock_active_mask.insert(k.clone(), true);
            }
        }
    }

    let mut active_segs = BTreeMap::new();
    if bridge_online && !st.clock_segments.is_empty() {
        active_segs = st.clock_segments.clone();
    } else {
        for (k, &on) in &app.clock_active_mask {
            if on && app.clock_brightness > 0 {
                active_segs.insert(k.clone(), app.clock_brightness);
            }
        }
    }

    let (crect, cresp) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(240.0), 64.0),
        Sense::click(),
    );
    let cp = ui.painter_at(crect);
    draw_segment_clock(&cp, crect, &active_segs);

    if cresp.clicked() {
        if let Some(ptr) = cresp.interact_pointer_pos() {
            if let Some(seg_key) = hit_test_clock_segment(crect, ptr) {
                let is_on = app.clock_active_mask.get(&seg_key).copied().unwrap_or(false)
                    && segment_brightness(&active_segs, &seg_key) > 0;
                let next_val = if is_on {
                    app.clock_active_mask.remove(&seg_key);
                    0
                } else {
                    if app.clock_brightness == 0 {
                        app.clock_brightness = 255;
                    }
                    app.clock_active_mask.insert(seg_key.clone(), true);
                    app.clock_brightness
                };
                if let Some(ref mut latest) = app.latest_state {
                    latest.clock_segments.insert(seg_key.clone(), next_val);
                }
                let mut map = BTreeMap::new();
                map.insert(seg_key, next_val);
                app.send_command(HostCommand::SetClockDisplay {
                    segments: Some(map),
                    replace: Some(false),
                    pixels: None,
                    led_frame: None,
                    brightness: Some(app.clock_brightness),
                    scale_brightness: None,
                    text: None,
                });
            }
        }
    }

    ui.horizontal_wrapped(|ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(&mut app.clock_text_input)
                .desired_width(82.0)
                .hint_text("19:30"),
        );
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if enter || ui.button("Вывести").clicked() {
            let txt = app.clock_text_input.clone();
            if !txt.is_empty() && app.clock_brightness == 0 {
                app.clock_brightness = 255;
            }
            let built = build_local_segments_from_text(&txt, 255);
            app.clock_active_mask = built
                .into_iter()
                .filter(|(_, v)| *v > 0)
                .map(|(k, _)| (k, true))
                .collect();
            if let Some(ref mut latest) = app.latest_state {
                latest.clock_segments =
                    build_local_segments_from_text(&txt, app.clock_brightness);
            }
            app.send_command(HostCommand::SetClockDisplay {
                segments: None,
                replace: None,
                pixels: None,
                led_frame: None,
                brightness: Some(app.clock_brightness),
                scale_brightness: None,
                text: Some(txt),
            });
        }
        for preset in ["19:30", "+24°", "-05°", ""] {
            let lbl = if preset.is_empty() { "Погасить" } else { preset };
            if ui.button(lbl).clicked() {
                app.clock_text_input = preset.to_string();
                let br = if preset.is_empty() {
                    app.clock_active_mask.clear();
                    0
                } else {
                    if app.clock_brightness == 0 {
                        app.clock_brightness = 255;
                    }
                    let built = build_local_segments_from_text(preset, 255);
                    app.clock_active_mask = built
                        .into_iter()
                        .filter(|(_, v)| *v > 0)
                        .map(|(k, _)| (k, true))
                        .collect();
                    app.clock_brightness
                };
                if let Some(ref mut latest) = app.latest_state {
                    latest.clock_segments = build_local_segments_from_text(preset, br);
                }
                app.send_command(HostCommand::SetClockDisplay {
                    segments: None,
                    replace: None,
                    pixels: None,
                    led_frame: None,
                    brightness: Some(br),
                    scale_brightness: None,
                    text: Some(preset.to_string()),
                });
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label(RichText::new("Яркость PWM:").small());
        if ui
            .add(egui::Slider::new(&mut app.clock_brightness, 0..=255))
            .changed()
        {
            if let Some(ref mut latest) = app.latest_state {
                for (k, v) in latest.clock_segments.iter_mut() {
                    if app.clock_active_mask.get(k).copied().unwrap_or(false) {
                        *v = app.clock_brightness;
                    } else {
                        *v = 0;
                    }
                }
            }
            app.send_command(HostCommand::SetClockDisplay {
                segments: None,
                replace: None,
                pixels: None,
                led_frame: None,
                brightness: None,
                scale_brightness: Some(app.clock_brightness),
                text: None,
            });
        }
    });

    if st.aux_light_kind == "logo" || matches!(board_slug, "cucumber" | "pickle" | "mango") {
        let count = if !st.logo_pixels.is_empty() {
            st.logo_pixels.len()
        } else if board_slug == "mango" {
            3
        } else {
            6
        };
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Логотип (/dev/fb1, {} LED):", count))
                    .small()
                    .color(Color32::from_rgb(160, 170, 185)),
            );
            let (lrect, _) = ui.allocate_exact_size(Vec2::new(150.0, 20.0), Sense::hover());
            let lp = ui.painter_at(lrect);
            for i in 0..count {
                let rgb = st
                    .logo_pixels
                    .get(i)
                    .copied()
                    .unwrap_or([0, 185, 235]);
                let x = lrect.left() + 10.0 + (i as f32) * 20.0;
                draw_ring_led(&lp, Pos2::new(x, lrect.center().y), 4.6, rgb);
            }
        });
    }
}

fn render_matrix_21x12_faceplate(
    ui: &mut egui::Ui,
    app: &mut GuiAppState,
    st: &BridgeStateSnapshot,
    bridge_online: bool,
) {
    ui.label(
        RichText::new("Матрица 21x12 LED (/dev/fb1 — ЛКМ: рисовать, ПКМ: стирать)")
            .small()
            .color(Color32::from_rgb(165, 175, 190)),
    );

    if app.matrix_custom_pixels.len() < 252 {
        app.matrix_custom_pixels.resize(252, 0);
    }
    if app.matrix_active_mask.len() < 252 {
        app.matrix_active_mask.resize(252, false);
    }

    if bridge_online && st.screen_pixels.len() == 252 && app.clock_brightness > 0 {
        app.matrix_custom_pixels = st.screen_pixels.clone();
        if st.screen_pixels.iter().any(|&v| v > 0) {
            app.matrix_active_mask = st.screen_pixels.iter().map(|&v| v > 0).collect();
        }
    }

    let cell = 13.0;
    let msize = Vec2::new(21.0 * cell + 10.0, 12.0 * cell + 10.0);
    let (mrect, mresp) = ui.allocate_exact_size(msize, Sense::click_and_drag());
    let mp = ui.painter_at(mrect);
    mp.rect_filled(mrect, 6.0, Color32::from_rgb(13, 15, 20));
    mp.rect_stroke(mrect, 6.0, Stroke::new(1.0, Color32::from_rgb(48, 56, 70)));

    let lmb_down = ui.input(|i| i.pointer.primary_down());
    let rmb_down = ui.input(|i| i.pointer.secondary_down());
    if (lmb_down || rmb_down) && (mresp.hovered() || mresp.dragged() || mresp.clicked()) {
        if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
            if let Some(idx) = hit_test_matrix_pixel(mrect, cell, p) {
                let draw_mode = lmb_down && !rmb_down;
                if draw_mode && app.clock_brightness == 0 {
                    app.clock_brightness = 255;
                }
                let next_v = if draw_mode {
                    app.clock_brightness.max(1)
                } else {
                    0
                };
                if app.matrix_custom_pixels[idx] != next_v
                    || app.matrix_active_mask[idx] != draw_mode
                {
                    app.matrix_active_mask[idx] = draw_mode;
                    app.matrix_custom_pixels[idx] = next_v;
                    if let Some(ref mut latest) = app.latest_state {
                        if latest.screen_pixels.len() < 252 {
                            latest.screen_pixels.resize(252, 0);
                        }
                        latest.screen_pixels[idx] = next_v;
                    }
                    app.send_command(HostCommand::SetClockDisplay {
                        segments: None,
                        replace: None,
                        pixels: Some(app.matrix_custom_pixels.clone()),
                        led_frame: None,
                        brightness: Some(app.clock_brightness),
                        scale_brightness: None,
                        text: None,
                    });
                }
            }
        }
    }

    for row in 0..12 {
        for col in 0..21 {
            let idx = row * 21 + col;
            let v = if app.clock_brightness == 0 {
                0
            } else {
                app.matrix_custom_pixels.get(idx).copied().unwrap_or(0)
            };
            let dot_pos = Pos2::new(
                mrect.left() + 5.0 + (col as f32 + 0.5) * cell,
                mrect.top() + 5.0 + (row as f32 + 0.5) * cell,
            );
            draw_matrix_dot(&mp, dot_pos, 4.2, v);
        }
    }

    ui.horizontal_wrapped(|ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(&mut app.matrix_text_input)
                .desired_width(82.0)
                .hint_text("19:30"),
        );
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if enter || ui.button("Вывести").clicked() {
            let txt = app.matrix_text_input.clone();
            if !txt.is_empty() && app.clock_brightness == 0 {
                app.clock_brightness = 255;
            }
            let mask_px = render_text_to_matrix_21x12(&txt, 255);
            app.matrix_active_mask = mask_px.iter().map(|&v| v > 0).collect();
            app.matrix_custom_pixels = render_text_to_matrix_21x12(&txt, app.clock_brightness);
            if let Some(ref mut latest) = app.latest_state {
                latest.screen_pixels = app.matrix_custom_pixels.clone();
            }
            app.send_command(HostCommand::SetClockDisplay {
                segments: None,
                replace: None,
                pixels: None,
                led_frame: None,
                brightness: Some(app.clock_brightness),
                scale_brightness: None,
                text: Some(txt),
            });
        }
        for preset in ["19:30", "+24°", "-05°", ""] {
            let lbl = if preset.is_empty() { "Погасить" } else { preset };
            if ui.button(lbl).clicked() {
                app.matrix_text_input = preset.to_string();
                let br = if preset.is_empty() {
                    app.matrix_active_mask.fill(false);
                    app.matrix_custom_pixels.fill(0);
                    0
                } else {
                    if app.clock_brightness == 0 {
                        app.clock_brightness = 255;
                    }
                    let mask_px = render_text_to_matrix_21x12(preset, 255);
                    app.matrix_active_mask = mask_px.iter().map(|&v| v > 0).collect();
                    app.matrix_custom_pixels =
                        render_text_to_matrix_21x12(preset, app.clock_brightness);
                    app.clock_brightness
                };
                if let Some(ref mut latest) = app.latest_state {
                    latest.screen_pixels = app.matrix_custom_pixels.clone();
                }
                app.send_command(HostCommand::SetClockDisplay {
                    segments: None,
                    replace: None,
                    pixels: None,
                    led_frame: None,
                    brightness: Some(br),
                    scale_brightness: None,
                    text: Some(preset.to_string()),
                });
            }
        }
        if ui.button("Волна").clicked() {
            if app.clock_brightness == 0 {
                app.clock_brightness = 255;
            }
            app.matrix_active_mask.fill(false);
            app.matrix_custom_pixels.fill(0);
            for c in 0..21 {
                let h = (3.0 + ((c as f32) * 0.7).sin() * 3.0).round() as usize;
                for r in (11usize.saturating_sub(h))..11 {
                    let idx = r * 21 + c;
                    app.matrix_active_mask[idx] = true;
                    app.matrix_custom_pixels[idx] = app.clock_brightness;
                }
            }
            if let Some(ref mut latest) = app.latest_state {
                latest.screen_pixels = app.matrix_custom_pixels.clone();
            }
            app.send_command(HostCommand::SetClockDisplay {
                segments: None,
                replace: None,
                pixels: Some(app.matrix_custom_pixels.clone()),
                led_frame: None,
                brightness: Some(app.clock_brightness),
                scale_brightness: None,
                text: None,
            });
        }
    });

    ui.horizontal(|ui| {
        ui.label(RichText::new("Яркость PWM:").small());
        if ui
            .add(egui::Slider::new(&mut app.clock_brightness, 0..=255))
            .changed()
        {
            for idx in 0..252 {
                app.matrix_custom_pixels[idx] = if app.matrix_active_mask[idx] {
                    app.clock_brightness
                } else {
                    0
                };
            }
            if let Some(ref mut latest) = app.latest_state {
                latest.screen_pixels = app.matrix_custom_pixels.clone();
            }
            app.send_command(HostCommand::SetClockDisplay {
                segments: None,
                replace: None,
                pixels: None,
                led_frame: None,
                brightness: None,
                scale_brightness: Some(app.clock_brightness),
                text: None,
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_led_ring_geometry_and_doa_azimuth() {
        let c = Pos2::new(100.0, 100.0);
        let top = led_ring_position(c, 50.0, 0, 4);
        let right = led_ring_position(c, 50.0, 1, 4);
        assert!((top.x - 100.0).abs() < 1e-3 && (top.y - 50.0).abs() < 1e-3);
        assert!((right.x - 150.0).abs() < 1e-3 && (right.y - 100.0).abs() < 1e-3);
        assert_eq!(doa_angle_from_pointer(c, top), 0);
        assert_eq!(doa_angle_from_pointer(c, right), 90);
        assert_eq!(doa_angle_from_pointer(c, Pos2::new(100.0, 150.0)), 180);
        assert_eq!(doa_angle_from_pointer(c, Pos2::new(50.0, 100.0)), 270);
    }

    #[test]
    fn test_segment_clock_text_decoding_and_hit_testing() {
        let map_time = build_local_segments_from_text("19:30°", 255);
        assert_eq!(segment_brightness(&map_time, "b1"), 255);
        assert_eq!(segment_brightness(&map_time, "p3"), 255);
        assert_eq!(segment_brightness(&map_time, "deg"), 255);
        assert_eq!(segment_brightness(&map_time, "h1"), 0);

        let map_temp = build_local_segments_from_text("+24°", 200);
        assert_eq!(segment_brightness(&map_temp, "h1"), 200);
        assert_eq!(segment_brightness(&map_temp, "m1"), 200);
        assert_eq!(segment_brightness(&map_temp, "deg"), 200);

        let rect = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(260.0, 64.0));
        let lines = clock_segment_lines(rect);
        let a1_line = lines.iter().find(|(k, _, _)| k == "a1").unwrap();
        let mid_a1 = Pos2::new(
            (a1_line.1.x + a1_line.2.x) * 0.5,
            (a1_line.1.y + a1_line.2.y) * 0.5,
        );
        assert_eq!(
            hit_test_clock_segment(rect, mid_a1),
            Some("a1".to_string())
        );

        let mrect = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(283.0, 166.0));
        assert_eq!(
            hit_test_matrix_pixel(mrect, 13.0, Pos2::new(17.0, 17.0)),
            Some(0)
        );
        assert_eq!(
            hit_test_matrix_pixel(
                mrect,
                13.0,
                Pos2::new(15.0 + 20.5 * 13.0, 15.0 + 11.5 * 13.0)
            ),
            Some(251)
        );

        let m_1930 = render_text_to_matrix_21x12("19:30°", 255);
        assert_eq!(m_1930.len(), 252);
        assert_eq!(m_1930[20], 255);
    }
}
