use crate::gui::state::GuiAppState;
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use protocol::{BridgeStateSnapshot, HostCommand};
use std::collections::BTreeMap;
use std::f32::consts::{FRAC_PI_2, TAU};
use std::time::Duration;

const LED_PATTERNS: &[(&str, &str)] = &[
    ("", "Auto (Quasar State)"),
    ("listening", "listening.led"),
    ("speaking", "speaking.led"),
    ("thinking", "thinking.led"),
    ("mute_mics", "mute_mics.led"),
    ("volume", "volume.led"),
    ("timer", "timer.led"),
    ("error", "error.led"),
    ("bluetooth_playing", "bluetooth_playing.led"),
    ("notification_available", "notification_available.led"),
    ("rgb:#00E5FF", "Solid Cyan (rgb:#00E5FF)"),
];

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

fn draw_led_with_glow(painter: &egui::Painter, pos: Pos2, radius: f32, rgb: [u8; 3]) {
    let [r, g, b] = rgb;
    if r > 0 || g > 0 || b > 0 {
        painter.circle_filled(
            pos,
            radius * 2.5,
            Color32::from_rgba_unmultiplied(r, g, b, 34),
        );
        painter.circle_filled(
            pos,
            radius * 1.65,
            Color32::from_rgba_unmultiplied(r, g, b, 92),
        );
        painter.circle_filled(pos, radius, Color32::from_rgb(r, g, b));
    } else {
        painter.circle_filled(pos, radius, Color32::from_rgb(30, 34, 42));
        painter.circle_stroke(pos, radius, Stroke::new(1.0, Color32::from_rgb(56, 64, 78)));
    }
}

fn draw_segment_clock(painter: &egui::Painter, rect: Rect, segments: &BTreeMap<String, u8>) {
    painter.rect_filled(rect, 6.0, Color32::from_rgb(16, 19, 25));
    painter.rect_stroke(rect, 6.0, Stroke::new(1.0, Color32::from_rgb(48, 56, 70)));

    let digit_w = 22.0;
    let digit_h = 38.0;
    let start_x = rect.center().x - 68.0;
    let top_y = rect.center().y - digit_h * 0.5;
    let seg_color = |br: u8| {
        if br > 0 {
            Color32::from_rgba_unmultiplied(235, 248, 255, br.max(45))
        } else {
            Color32::from_rgb(34, 40, 50)
        }
    };

    for idx in 0..4 {
        let pos = idx + 1;
        let x0 = start_x + (idx as f32) * 32.0 + if idx >= 2 { 10.0 } else { 0.0 };
        let x1 = x0 + digit_w;
        let ym = top_y + digit_h * 0.5;
        let y1 = top_y + digit_h;
        let coords = [
            ("a", Pos2::new(x0 + 2.0, top_y), Pos2::new(x1 - 2.0, top_y)),
            ("b", Pos2::new(x1, top_y + 2.0), Pos2::new(x1, ym - 1.5)),
            ("c", Pos2::new(x1, ym + 1.5), Pos2::new(x1, y1 - 2.0)),
            ("d", Pos2::new(x0 + 2.0, y1), Pos2::new(x1 - 2.0, y1)),
            ("e", Pos2::new(x0, ym + 1.5), Pos2::new(x0, y1 - 2.0)),
            ("f", Pos2::new(x0, top_y + 2.0), Pos2::new(x0, ym - 1.5)),
            ("g", Pos2::new(x0 + 2.0, ym), Pos2::new(x1 - 2.0, ym)),
        ];
        for (part, p0, p1) in coords {
            let br = segment_brightness(segments, &format!("{}{}", part, pos));
            painter.line_segment([p0, p1], Stroke::new(3.2, seg_color(br)));
        }
    }

    let colon_x = start_x + 2.0 * 32.0 - 1.0;
    let p3 = segment_brightness(segments, "p3").max(segment_brightness(segments, "h1"));
    let q3 = segment_brightness(segments, "q3").max(segment_brightness(segments, "h2"));
    painter.circle_filled(Pos2::new(colon_x, top_y + 12.0), 2.5, seg_color(p3));
    painter.circle_filled(Pos2::new(colon_x, top_y + 26.0), 2.5, seg_color(q3));

    let deg_br = segment_brightness(segments, "deg");
    painter.circle_stroke(
        Pos2::new(start_x + 4.0 * 32.0 + 14.0, top_y + 6.0),
        3.5,
        Stroke::new(2.2, seg_color(deg_br)),
    );
}

pub fn render_twin_canvas(ui: &mut egui::Ui, app: &mut GuiAppState) {
    let bridge_online = app.bridge.as_ref().map(|b| b.is_connected()).unwrap_or(false);
    if bridge_online {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
    let st = app.latest_state.clone().unwrap_or_else(|| {
        let mut def = BridgeStateSnapshot::default();
        def.board_slug = app.config.selected_board.clone();
        def
    });

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.heading(format!("{} ({})", st.board_name, st.board_slug));
            let stealth = st.ring_trigger.contains("Stealth");
            let trig_color = if stealth {
                Color32::from_rgb(255, 175, 60)
            } else if st.ring_trigger.contains("none") {
                Color32::from_rgb(120, 210, 255)
            } else {
                Color32::from_rgb(110, 230, 150)
            };
            ui.label(RichText::new(format!("[Trigger: {}]", st.ring_trigger)).color(trig_color));
            if stealth {
                ui.label(
                    RichText::new("STEALTH LOCK")
                        .strong()
                        .color(Color32::from_rgb(255, 120, 70)),
                );
            }
            let conflict_color = if st.led_trigger_conflict_count > 0 {
                Color32::from_rgb(255, 95, 95)
            } else {
                Color32::from_rgb(150, 160, 175)
            };
            ui.label(
                RichText::new(format!("Conflicts: {}", st.led_trigger_conflict_count))
                    .color(conflict_color),
            );
            ui.label(
                RichText::new(format!("Alice: {}", st.alice_state))
                    .strong()
                    .color(Color32::from_rgb(185, 145, 255)),
            );
            let next_preset = if app.config.layout_preset == "compact" {
                ("Full Workbench", "workbench")
            } else {
                ("Compact Remote", "compact")
            };
            if ui.small_button(format!("Switch to {}", next_preset.0)).clicked() {
                app.set_layout_preset(next_preset.1);
            }
        });
        ui.separator();

        // 2D Speaker Top Enclosure + LED Ring + Ambilight Halo + DOA Radar
        let canvas_size = Vec2::new(ui.available_width().max(260.0), 250.0);
        let (rect, response) = ui.allocate_exact_size(canvas_size, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let center = Pos2::new(rect.center().x, rect.center().y);
        let outer_r = (rect.height().min(rect.width()) * 0.43).clamp(75.0, 108.0);

        if response.clicked() || response.dragged() {
            if let Some(ptr) = response.interact_pointer_pos() {
                let dist = ptr.distance(center);
                if dist >= outer_r * 0.72 && dist <= outer_r * 1.28 {
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
        if st.aux_light_kind == "ambilight" && !st.logo_pixels.is_empty() {
            let zones = st.logo_pixels.len();
            for (i, &rgb) in st.logo_pixels.iter().enumerate() {
                let p = led_ring_position(center, outer_r + 12.0, i, zones);
                draw_led_with_glow(&painter, p, 5.0, rgb);
            }
        }

        painter.circle_filled(center, outer_r, Color32::from_rgb(22, 25, 32));
        painter.circle_stroke(center, outer_r, Stroke::new(2.0, Color32::from_rgb(58, 66, 82)));
        painter.circle_filled(center, outer_r * 0.62, Color32::from_rgb(16, 18, 24));

        // Active LED Ring (1..64 LEDs)
        let ring_count = st.ring_leds.max(st.ring_pixels.len()).max(1);
        let ring_r = if ring_count == 1 { 0.0 } else { outer_r * 0.80 };
        let led_dot_r = match ring_count {
            1..=6 => 7.0,
            7..=16 => 5.5,
            17..=24 => 4.5,
            25..=36 => 3.6,
            _ => 2.6,
        };
        for idx in 0..ring_count {
            let rgb = st.ring_pixels.get(idx).copied().unwrap_or([0, 0, 0]);
            let pos = led_ring_position(center, ring_r, idx, ring_count);
            draw_led_with_glow(&painter, pos, led_dot_r, rgb);
        }

        // DOA Azimuth Vector & Handle
        let doa_pos = led_ring_position(center, outer_r + 4.0, st.doa_angle as usize, 360);
        painter.line_segment(
            [center, doa_pos],
            Stroke::new(1.5, Color32::from_rgba_unmultiplied(0, 229, 255, 110)),
        );
        painter.circle_filled(doa_pos, 5.5, Color32::from_rgb(0, 229, 255));
        painter.text(
            Pos2::new(center.x, center.y),
            egui::Align2::CENTER_CENTER,
            format!("DOA {}°\nVol {}/10", st.doa_angle, st.volume_level),
            egui::FontId::monospace(11.5),
            Color32::from_rgb(190, 205, 225),
        );

        // Auxiliary Logo LEDs (cucumber / pickle / mango)
        if st.aux_light_kind == "logo" && !st.logo_pixels.is_empty() {
            ui.horizontal(|ui| {
                ui.label("Logo LEDs:");
                let (lrect, _) =
                    ui.allocate_exact_size(Vec2::new(160.0, 22.0), Sense::hover());
                let lp = ui.painter_at(lrect);
                for (i, &rgb) in st.logo_pixels.iter().enumerate() {
                    let x = lrect.left() + 14.0 + (i as f32) * 22.0;
                    draw_led_with_glow(&lp, Pos2::new(x, lrect.center().y), 5.5, rgb);
                }
            });
        }

        // Front Display: Segment Clock or 21x12 Monochrome Matrix
        if st.display_kind == "matrix_21x12" || st.board_slug == "orion" {
            ui.label(RichText::new("21×12 Monochrome LED Matrix (/dev/fb1 — click dots to toggle):").strong());
            let cell = 9.5;
            let msize = Vec2::new(21.0 * cell + 8.0, 12.0 * cell + 8.0);
            let (mrect, mresp) = ui.allocate_exact_size(msize, Sense::click());
            let mp = ui.painter_at(mrect);
            mp.rect_filled(mrect, 4.0, Color32::from_rgb(14, 16, 22));
            if mresp.clicked() {
                if let Some(p) = mresp.interact_pointer_pos() {
                    let col = ((p.x - mrect.left() - 4.0) / cell).floor() as isize;
                    let row = ((p.y - mrect.top() - 4.0) / cell).floor() as isize;
                    if (0..21).contains(&col) && (0..12).contains(&row) {
                        let idx = (row as usize) * 21 + (col as usize);
                        if app.matrix_custom_pixels.len() < 252 {
                            app.matrix_custom_pixels.resize(252, 0);
                        }
                        app.matrix_custom_pixels[idx] =
                            if app.matrix_custom_pixels[idx] > 0 { 0 } else { 255 };
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
            for row in 0..12 {
                for col in 0..21 {
                    let idx = row * 21 + col;
                    let v = st.screen_pixels.get(idx).copied().unwrap_or(0);
                    let dot_pos = Pos2::new(
                        mrect.left() + 4.0 + (col as f32 + 0.5) * cell,
                        mrect.top() + 4.0 + (row as f32 + 0.5) * cell,
                    );
                    draw_led_with_glow(&mp, dot_pos, 3.0, [v, v, v]);
                }
            }
        } else if st.display_kind == "segment_clock" || st.has_clock {
            let (crect, _) = ui.allocate_exact_size(Vec2::new(200.0, 54.0), Sense::hover());
            let cp = ui.painter_at(crect);
            draw_segment_clock(&cp, crect, &st.clock_segments);
        }

        ui.separator();
        render_hardware_controls(ui, app, &st);
    });
}

fn render_hardware_controls(ui: &mut egui::Ui, app: &mut GuiAppState, st: &BridgeStateSnapshot) {
    // 1. Physical Buttons & Swipes
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Press:").strong());
        for mode in ["single", "double", "triple", "long"] {
            if ui
                .selectable_label(app.button_press_action == mode, mode)
                .clicked()
            {
                app.button_press_action = mode.to_string();
            }
        }
        ui.separator();
        for (label, btn) in [
            ("Action", "action"),
            (if st.mics_muted { "Unmute Mics" } else { "Mute Mics" }, "mute"),
            ("Vol -", "volume_down"),
            ("Vol +", "volume_up"),
        ] {
            if ui.button(label).clicked() {
                app.send_command(HostCommand::PressButton {
                    button: btn.to_string(),
                    action: app.button_press_action.clone(),
                });
            }
        }
        for (swipe_lbl, dir) in [
            ("Swipe Left", "left"),
            ("Swipe Right", "right"),
            ("Swipe Up", "up"),
            ("Swipe Down", "down"),
        ] {
            if ui.small_button(swipe_lbl).clicked() {
                app.send_command(HostCommand::SetSensor {
                    doa_angle: None,
                    mic_rms: None,
                    als_lux: None,
                    temp_c: None,
                    tof_mm: None,
                    swipe: Some(true),
                    swipe_direction: Some(dir.to_string()),
                });
            }
        }
    });

    // 2. Master Volume & Forced LED Pattern
    ui.horizontal_wrapped(|ui| {
        ui.label("Volume:");
        if ui
            .add(egui::Slider::new(&mut app.volume_slider, 0..=10))
            .changed()
        {
            app.send_command(HostCommand::SetVolume {
                volume_level: app.volume_slider,
            });
        }
        ui.separator();
        ui.label("LED Pattern:");
        egui::ComboBox::from_id_salt("led_pattern_combo")
            .selected_text(if app.selected_led_pattern.is_empty() {
                "Auto (Quasar State)"
            } else {
                &app.selected_led_pattern
            })
            .show_ui(ui, |ui| {
                for &(val, label) in LED_PATTERNS {
                    if ui
                        .selectable_label(app.selected_led_pattern == val, label)
                        .clicked()
                    {
                        app.selected_led_pattern = val.to_string();
                        app.send_command(HostCommand::SetLedPattern {
                            pattern: app.selected_led_pattern.clone(),
                        });
                    }
                }
            });
    });

    // 3. Clock / Matrix Controls
    ui.horizontal_wrapped(|ui| {
        ui.label("Display:");
        ui.add(egui::TextEdit::singleline(&mut app.clock_text_input).desired_width(90.0));
        ui.add(egui::Slider::new(&mut app.clock_brightness, 0..=255).text("PWM"));
        if ui.button("Apply Text").clicked() {
            app.send_command(HostCommand::SetClockDisplay {
                segments: None,
                replace: None,
                pixels: None,
                led_frame: None,
                brightness: Some(app.clock_brightness),
                scale_brightness: None,
                text: Some(app.clock_text_input.clone()),
            });
        }
        for preset in ["19:30°", "+24°", ""] {
            let lbl = if preset.is_empty() { "CLEAR" } else { preset };
            if ui.small_button(lbl).clicked() {
                app.clock_text_input = preset.to_string();
                app.send_command(HostCommand::SetClockDisplay {
                    segments: None,
                    replace: None,
                    pixels: None,
                    led_frame: None,
                    brightness: Some(if preset.is_empty() { 0 } else { app.clock_brightness }),
                    scale_brightness: None,
                    text: Some(preset.to_string()),
                });
            }
        }
        if (st.display_kind == "matrix_21x12" || st.board_slug == "orion")
            && ui.small_button("Send Custom 21x12 Matrix").clicked()
        {
            if app.matrix_custom_pixels.len() < 252 {
                app.matrix_custom_pixels.resize(252, 0);
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

    // 4. Environmental Sensors (DOA, ToF, ALS, Temp)
    ui.horizontal_wrapped(|ui| {
        if ui
            .add(egui::Slider::new(&mut app.doa_slider, 0..=359).text("DOA °"))
            .changed()
        {
            app.send_command(HostCommand::SetSensor {
                doa_angle: Some(app.doa_slider),
                mic_rms: None,
                als_lux: None,
                temp_c: None,
                tof_mm: None,
                swipe: None,
                swipe_direction: None,
            });
        }
        if ui
            .add(egui::Slider::new(&mut app.tof_slider, 40..=8190).text("ToF mm"))
            .changed()
        {
            app.send_command(HostCommand::SetSensor {
                doa_angle: None,
                mic_rms: None,
                als_lux: None,
                temp_c: None,
                tof_mm: Some(app.tof_slider),
                swipe: None,
                swipe_direction: None,
            });
        }
        if ui
            .add(egui::Slider::new(&mut app.als_slider, 0..=10000).text("ALS lux"))
            .changed()
        {
            app.send_command(HostCommand::SetSensor {
                doa_angle: None,
                mic_rms: None,
                als_lux: Some(app.als_slider),
                temp_c: None,
                tof_mm: None,
                swipe: None,
                swipe_direction: None,
            });
        }
        if ui
            .add(egui::Slider::new(&mut app.temp_slider, 20.0..=85.0).text("Temp °C"))
            .changed()
        {
            app.send_command(HostCommand::SetSensor {
                doa_angle: None,
                mic_rms: None,
                als_lux: None,
                temp_c: Some(app.temp_slider),
                tof_mm: None,
                swipe: None,
                swipe_direction: None,
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
    fn test_segment_brightness_lookup() {
        let mut segs = BTreeMap::new();
        segs.insert("b1".to_string(), 255);
        segs.insert("deg".to_string(), 128);
        assert_eq!(segment_brightness(&segs, "b1"), 255);
        assert_eq!(segment_brightness(&segs, "deg"), 128);
        assert_eq!(segment_brightness(&segs, "a4"), 0);
    }
}
