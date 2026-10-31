use crate::gui::launcher;
use crate::gui::state::{GuiAppState, SignalDirection, SignalFilter};
use crate::session::strip_ansi_and_control_chars;
use eframe::egui::{self, Color32, FontId, Margin, RichText};

pub fn render_bottom_dock(ui: &mut egui::Ui, app: &mut GuiAppState) {
    ui.horizontal_wrapped(|ui| {
        let collapse_label = if app.config.dock_collapsed {
            "Развернуть панель логов [+]"
        } else {
            "Свернуть панель логов [-]"
        };
        if ui.button(RichText::new(collapse_label).strong()).clicked() {
            app.config.dock_collapsed = !app.config.dock_collapsed;
            let _ = app.save_config();
        }

        if !app.config.dock_collapsed {
            let size_label = if app.config.dock_large {
                "Высота: Обычная"
            } else {
                "Высота: Большая"
            };
            if ui.button(size_label).clicked() {
                app.config.dock_large = !app.config.dock_large;
                let _ = app.save_config();
            }
        }

        ui.separator();

        for (tab_id, label) in [
            ("signals", "Лог сигналов (TX / RX)"),
            ("serial", "Интерактивный терминал ttyAMA0"),
            ("maind", "Лог демона maind.log"),
            ("images_adb", "Образы, ADB и QEMU"),
        ] {
            let active = app.config.active_terminal_tab == tab_id && !app.config.dock_collapsed;
            if ui
                .selectable_label(active, RichText::new(label).strong())
                .clicked()
            {
                app.config.active_terminal_tab = tab_id.to_string();
                app.config.dock_collapsed = false;
                let _ = app.save_config();
            }
        }

        ui.separator();
        let ipc_errs = app
            .latest_state
            .as_ref()
            .map(|s| s.ipc_error_count)
            .unwrap_or(0);
        let pill_col = if ipc_errs == 0 {
            Color32::from_rgb(155, 205, 255)
        } else {
            Color32::from_rgb(255, 115, 115)
        };
        ui.add(
            egui::Button::new(
                RichText::new(format!(
                    "Событий: {} | Ошибок IPC: {}",
                    app.signal_logs.len(),
                    ipc_errs
                ))
                .small()
                .strong()
                .color(pill_col),
            )
            .fill(Color32::from_rgb(38, 50, 68))
            .rounding(5.0)
            .sense(egui::Sense::hover()),
        );

        if !app.config.dock_collapsed {
            ui.separator();
            if app.config.active_terminal_tab == "signals" {
                for (flt, title) in [
                    (SignalFilter::All, "Все"),
                    (SignalFilter::TofButtons, "ToF / Кнопки"),
                    (SignalFilter::TtsIpc, "TTS / IPC"),
                    (SignalFilter::LedDisplay, "LED / Экран"),
                    (SignalFilter::Audio, "Аудио"),
                ] {
                    if ui
                        .selectable_label(app.signal_filter == flt, title)
                        .clicked()
                    {
                        app.signal_filter = flt;
                    }
                }
                if ui.button("Очистить").clicked() {
                    app.clear_signals();
                }
            } else if app.config.active_terminal_tab == "serial" {
                if let Some(ref sess) = app.session {
                    if ui.button("Очистить консоль").clicked() {
                        sess.clear_serial_lines();
                    }
                }
                ui.checkbox(&mut app.terminal_autoscroll, "Автопрокрутка");
            } else if app.config.active_terminal_tab == "maind" {
                ui.checkbox(&mut app.terminal_autoscroll, "Автопрокрутка");
            }
        }
    });

    if app.config.dock_collapsed {
        return;
    }

    ui.separator();

    match app.config.active_terminal_tab.as_str() {
        "serial" => render_serial_console_tab(ui, app),
        "maind" => render_maind_log_tab(ui, app),
        "images_adb" => launcher::render_launcher_pane(ui, app),
        _ => render_signal_log_tab(ui, app),
    }
}

fn render_signal_log_tab(ui: &mut egui::Ui, app: &mut GuiAppState) {
    let entries = app.filtered_signals();
    let avail_h = ui.available_height().max(60.0);

    egui::Frame::canvas(ui.style())
        .fill(Color32::from_rgb(18, 19, 22))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("signal_monitor_scroll")
                .auto_shrink([false, false])
                .max_height(avail_h)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    if entries.is_empty() {
                        ui.label(
                            RichText::new("Нет записей в логе сигналов для выбранного фильтра.")
                                .font(FontId::monospace(13.5))
                                .color(Color32::from_rgb(145, 155, 170)),
                        );
                    } else {
                        for entry in &entries {
                            let color = match entry.direction {
                                SignalDirection::Tx => Color32::from_rgb(120, 180, 245),
                                SignalDirection::Rx => Color32::from_rgb(87, 227, 137),
                            };
                            let line = format!(
                                "[{}] [{} {}] {}",
                                entry.timestamp,
                                entry.direction.as_str(),
                                entry.category.as_str(),
                                strip_ansi_and_control_chars(&entry.message)
                            );
                            ui.label(
                                RichText::new(line)
                                    .font(FontId::monospace(13.2))
                                    .color(color),
                            );
                        }
                    }
                });
        });
}

fn render_serial_console_tab(ui: &mut egui::Ui, app: &mut GuiAppState) {
    let lines = app
        .session
        .as_ref()
        .map(|s| s.serial_lines())
        .unwrap_or_default();

    egui::TopBottomPanel::bottom("serial_input_bar_inner")
        .frame(egui::Frame::none().inner_margin(Margin::symmetric(0.0, 4.0)))
        .show_inside(ui, |ui| {
            let vm_running = app.session.is_some();
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("ttyAMA0 #").font(FontId::monospace(13.5)));
                let input_w = (ui.available_width() * 0.38).clamp(160.0, 420.0);
                let resp = ui.add_enabled(
                    vm_running,
                    egui::TextEdit::singleline(&mut app.serial_input)
                        .font(FontId::monospace(13.5))
                        .desired_width(input_w)
                        .hint_text("Введите root-команду и нажмите Enter..."),
                );
                let enter_pressed =
                    resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if enter_pressed
                    || ui
                        .add_enabled(vm_running, egui::Button::new("Отправить"))
                        .clicked()
                {
                    let cmd = app.serial_input.clone();
                    app.serial_input.clear();
                    app.send_serial_command(&cmd);
                    resp.request_focus();
                }
                for (label, cmd) in [
                    ("Ctrl+C", "\x03"),
                    ("hw.json", "cat /tmp/quasar-emu-hw.json"),
                    (
                        "LED sysfs (none + Красный)",
                        "for d in /sys/class/leds/*:ring*; do echo none > \"$d/trigger\"; done; for d in /sys/class/leds/*:red:ring*; do echo 220 > \"$d/brightness\"; done",
                    ),
                    (
                        "LED возврат ledfb-ring",
                        "for d in /sys/class/leds/*:ring*; do echo ledfb-ring > \"$d/trigger\"; done",
                    ),
                    ("ps", "ps"),
                ] {
                    if ui
                        .add_enabled(vm_running, egui::Button::new(label))
                        .clicked()
                    {
                        app.send_serial_command(cmd);
                    }
                }
            });
        });

    let scroll_height = ui.available_height().max(50.0);
    egui::Frame::canvas(ui.style())
        .fill(Color32::from_rgb(16, 18, 22))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("tty_serial_scroll")
                .auto_shrink([false, false])
                .max_height(scroll_height)
                .stick_to_bottom(app.terminal_autoscroll)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    if lines.is_empty() {
                        ui.label(
                            RichText::new(if app.session.is_some() {
                                "Ожидание вывода консоли ttyAMA0..."
                            } else {
                                "ВМ остановлена. Нажмите 'Запустить образ (Старт ВМ)' в верхней панели для запуска ttyAMA0."
                            })
                            .font(FontId::monospace(13.5))
                            .color(Color32::from_rgb(145, 155, 170)),
                        );
                    } else {
                        for line in &lines {
                            let clean = strip_ansi_and_control_chars(line);
                            ui.label(
                                RichText::new(clean)
                                    .font(FontId::monospace(13.2))
                                    .color(Color32::from_rgb(220, 230, 240)),
                            );
                        }
                    }
                });
        });
}

fn render_maind_log_tab(ui: &mut egui::Ui, app: &mut GuiAppState) {
    let lines = app
        .latest_state
        .as_ref()
        .map(|s| s.maind_logs.clone())
        .unwrap_or_default();
    let avail_h = ui.available_height().max(60.0);

    egui::Frame::canvas(ui.style())
        .fill(Color32::from_rgb(16, 18, 22))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("maind_log_scroll")
                .auto_shrink([false, false])
                .max_height(avail_h)
                .stick_to_bottom(app.terminal_autoscroll)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    if lines.is_empty() {
                        ui.label(
                            RichText::new("Лог /tmp/daemons_logs/maind.log пока пуст.")
                                .font(FontId::monospace(13.5))
                                .color(Color32::from_rgb(145, 155, 170)),
                        );
                    } else {
                        for line in &lines {
                            let clean = strip_ansi_and_control_chars(line);
                            ui.label(
                                RichText::new(clean)
                                    .font(FontId::monospace(13.2))
                                    .color(Color32::from_rgb(215, 225, 240)),
                            );
                        }
                    }
                });
        });
}

pub fn render_terminal_pane(ui: &mut egui::Ui, app: &mut GuiAppState) {
    render_bottom_dock(ui, app);
}
