pub mod board_cards;
pub mod sensors_tof;
pub mod voice_audio;

use crate::gui::state::GuiAppState;
use eframe::egui::{self, Color32, Margin, Pos2, RichText, Stroke, Vec2};
use std::sync::atomic::Ordering;

pub const LED_PATTERNS: &[(&str, &str)] = &[
    ("", "Авто (по состоянию Quasar)"),
    ("rgb:#6B0FFF", "Фиолетовый (#6B0FFF)"),
    ("alice_pro_listening", "Алиса Про (#00FF80)"),
    ("rgb:#00E5FF", "Бирюзовый (#00E5FF)"),
    ("mute_mics", "Красный Mute (#FF0000)"),
];

pub(crate) fn gnome_preferences_group(
    ui: &mut egui::Ui,
    title: &str,
    description: &str,
    add_rows: impl FnOnce(&mut egui::Ui),
) {
    ui.label(RichText::new(title).strong().size(15.0));
    ui.label(
        RichText::new(description)
            .small()
            .color(Color32::from_rgb(160, 160, 156)),
    );
    ui.add_space(4.0);
    egui::Frame::none()
        .fill(Color32::from_rgb(43, 43, 43))
        .stroke(Stroke::new(1.0, Color32::from_rgb(62, 62, 62)))
        .rounding(10.0)
        .inner_margin(Margin::symmetric(12.0, 10.0))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add_rows(ui);
        });
    ui.add_space(12.0);
}

pub(crate) fn gnome_action_row(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: &str,
    is_last: bool,
    add_controls: impl FnOnce(&mut egui::Ui),
) {
    ui.vertical(|ui| {
        ui.label(RichText::new(title).strong());
        ui.label(
            RichText::new(subtitle)
                .small()
                .color(Color32::from_rgb(156, 155, 152)),
        );
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            add_controls(ui);
        });
    });
    if !is_last {
        ui.add_space(5.0);
        ui.separator();
        ui.add_space(5.0);
    }
}

pub fn render_gnome_action_cards(ui: &mut egui::Ui, app: &mut GuiAppState) {
    egui::ScrollArea::vertical()
        .id_salt("gnome_action_cards_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            render_vm_connection_banner(ui, app);
            voice_audio::render_tts_audio_voice_card(ui, app);
            sensors_tof::render_tof_and_sensors_card(ui, app);
            board_cards::render_board_specific_card(ui, app);
        });
}

fn render_vm_connection_banner(ui: &mut egui::Ui, app: &mut GuiAppState) {
    let vm_running = app.session.is_some();
    let bridge_online = app
        .bridge
        .as_ref()
        .map(|b| b.is_connected())
        .unwrap_or(false);
    if bridge_online {
        return;
    }

    let busy = app.bg_task_running.load(Ordering::Relaxed);
    let sub = if vm_running {
        "Пожалуйста, подождите 5-10 секунд, пока загрузится гостевое ядро Linux и поднимется демон qemu-hw-bridge."
    } else if protocol::normalize_board_slug(&app.config.selected_board) == "yandexmini" {
        "Нажмите кнопку ниже, чтобы запустить образ в QEMU и подключить реальное выполнение команд TTS, ToF (VL53L0X), кнопок и дисплея."
    } else {
        "Нажмите кнопку ниже, чтобы запустить образ в QEMU и подключить реальное выполнение команд TTS, датчиков среды, кнопок и дисплея."
    };
    let (bg, border, title) = if vm_running {
        (
            Color32::from_rgb(48, 42, 22),
            Color32::from_rgb(229, 165, 10),
            format!(
                "Загрузка образа '{}' в QEMU и ожидание моста :{}...",
                app.config.selected_board, app.config.bridge_port
            ),
        )
    } else {
        (
            Color32::from_rgb(26, 48, 72),
            Color32::from_rgb(53, 132, 228),
            format!(
                "Виртуальная колонка '{}' сейчас не запущена (режим предпросмотра макета)",
                app.config.selected_board
            ),
        )
    };

    egui::Frame::none()
        .fill(bg)
        .stroke(Stroke::new(1.5, border))
        .rounding(10.0)
        .inner_margin(Margin::symmetric(14.0, 10.0))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(title)
                        .strong()
                        .color(Color32::from_rgb(240, 246, 255)),
                );
                ui.label(
                    RichText::new(sub)
                        .small()
                        .color(Color32::from_rgb(195, 210, 230)),
                );
                ui.add_space(4.0);
                if vm_running {
                    ui.spinner();
                } else {
                    let btn = egui::Button::new(
                        RichText::new(format!(
                            "Запустить образ '{}' в QEMU",
                            app.config.selected_board
                        ))
                        .strong()
                        .color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(38, 162, 105));
                    if ui.add_enabled(!busy, btn).clicked() {
                        app.start_vm();
                    }
                }
            });
        });
    ui.add_space(12.0);
}

pub(crate) fn render_mini_oscilloscope(ui: &mut egui::Ui, app: &GuiAppState) {
    let desired = Vec2::new(195.0, 30.0);
    let (rect, _resp) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 4.0, Color32::from_rgb(15, 19, 25));
    painter.rect_stroke(rect, 4.0, Stroke::new(1.0, Color32::from_rgb(48, 60, 78)));

    let mid_y = rect.center().y;
    let samples = app.audio_player.recent_waveform(128);
    if samples.len() >= 2 {
        let n = samples.len();
        let w = rect.width() - 6.0;
        let half_h = (rect.height() - 6.0) * 0.5;
        let pts: Vec<Pos2> = samples
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                let x = rect.left() + 3.0 + (i as f32 / (n - 1) as f32) * w;
                let y = mid_y - (s.clamp(-1.0, 1.0) * half_h);
                Pos2::new(x, y)
            })
            .collect();
        painter.add(egui::Shape::line(
            pts,
            Stroke::new(1.6, Color32::from_rgb(87, 227, 137)),
        ));
    } else {
        painter.line_segment(
            [
                Pos2::new(rect.left() + 3.0, mid_y),
                Pos2::new(rect.right() - 3.0, mid_y),
            ],
            Stroke::new(1.2, Color32::from_rgb(55, 110, 100)),
        );
    }
}
