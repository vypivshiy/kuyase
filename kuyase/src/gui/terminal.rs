use crate::gui::state::{GuiAppState, SignalDirection, SignalFilter};
use crate::session::strip_ansi_and_control_chars;
use eframe::egui::{self, Color32, FontId, Margin, RichText, Stroke};

const SERIAL_SNIPPETS: &[(&str, &str)] = &[
    (
        "Профиль железа (/tmp/quasar-emu-hw.json)",
        "cat /tmp/quasar-emu-hw.json",
    ),
    ("Список процессов (ps)", "ps"),
    ("Сетевые сокеты (netstat -tlpn)", "netstat -tlpn"),
    (
        "Фреймбуферы (/sys/class/graphics/fb*/name)",
        "head -n 1 /sys/class/graphics/fb*/name",
    ),
    (
        "LED: Ручной режим sysfs (trigger=none + Красный)",
        "for d in /sys/class/leds/*:ring*; do echo none > \"$d/trigger\"; done; for d in /sys/class/leds/*:red:ring*; do echo 220 > \"$d/brightness\"; done",
    ),
    (
        "LED: Возврат триггера Quasar (ledfb-ring)",
        "for d in /sys/class/leds/*:ring*; do echo 0 > \"$d/brightness\"; echo ledfb-ring > \"$d/trigger\"; done",
    ),
    (
        "LED: Демо конфликта триггера (запись при [ledfb-ring])",
        "for d in /sys/class/leds/*:red:ring*; do echo ledfb-ring > \"$d/trigger\"; echo 200 > \"$d/brightness\"; break; done",
    ),
];

fn set_text_edit_cursor_to_end(ctx: &egui::Context, id: egui::Id, text: &str) {
    if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
        let ccursor = egui::text::CCursor::new(text.chars().count());
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(ccursor)));
        state.store(ctx, id);
    }
}

pub fn render_bottom_dock(ui: &mut egui::Ui, app: &mut GuiAppState) {
    ui.horizontal_wrapped(|ui| {
        let collapse_label = if app.config.dock_collapsed {
            "Развернуть панель логов [+]"
        } else {
            "Свернуть панель логов [-]"
        };
        if ui
            .button(RichText::new(collapse_label).strong())
            .on_hover_text("Свернуть или развернуть нижнюю панель. Высоту открытой панели можно плавно менять перетаскиванием её верхней границы с зажатой ЛКМ.")
            .clicked()
        {
            app.config.dock_collapsed = !app.config.dock_collapsed;
            let _ = app.save_config();
        }

        ui.separator();

        for (tab_id, label) in [
            ("signals", "Лог сигналов (TX / RX)"),
            ("serial", "Интерактивный терминал ttyAMA0"),
            ("maind", "Лог демона maind.log"),
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
                let vm_running = app.session.is_some();
                ui.menu_button("Быстрые команды ▾", |ui| {
                    ui.set_min_width(320.0);
                    ui.label(
                        RichText::new("Готовые диагностические сниппеты для ttyAMA0:")
                            .small()
                            .color(Color32::from_rgb(160, 170, 185)),
                    );
                    ui.separator();
                    for &(label, cmd) in SERIAL_SNIPPETS {
                        if ui
                            .add_enabled(vm_running, egui::Button::new(label))
                            .on_hover_text(cmd)
                            .clicked()
                        {
                            app.send_serial_command(cmd);
                            ui.close_menu();
                        }
                    }
                });
                if ui
                    .add_enabled(vm_running, egui::Button::new("Ctrl+C"))
                    .on_hover_text("Отправить сигнал прерывания SIGINT (0x03) в консоль ttyAMA0")
                    .clicked()
                {
                    app.send_serial_command("\x03");
                }
                if ui
                    .button("Копировать")
                    .on_hover_text("Скопировать весь вывод консоли ttyAMA0 в буфер обмена")
                    .clicked()
                {
                    if let Some(ref sess) = app.session {
                        let joined = sess.serial_lines().join("\n");
                        ui.ctx().copy_text(joined);
                    }
                }
                if ui
                    .button("Очистить")
                    .on_hover_text("Очистить вывод терминала (горячая клавиша: Ctrl+L)")
                    .clicked()
                {
                    if let Some(ref sess) = app.session {
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
                            ui.add(
                                egui::Label::new(
                                    RichText::new(line)
                                        .font(FontId::monospace(13.2))
                                        .color(color),
                                )
                                .selectable(true),
                            );
                        }
                    }
                });
        });
}

fn classify_serial_line_color(clean: &str) -> Color32 {
    if clean.starts_with("===") {
        Color32::from_rgb(87, 227, 137)
    } else if clean.starts_with("~ #") || clean.starts_with("/ #") || clean.starts_with("root@") {
        Color32::from_rgb(130, 200, 255)
    } else if clean.contains("error")
        || clean.contains("Error")
        || clean.contains("FAIL")
        || clean.contains("panic")
    {
        Color32::from_rgb(255, 120, 120)
    } else if clean.starts_with("[qemu]") || clean.contains("can't access tty") {
        Color32::from_rgb(165, 175, 190)
    } else {
        Color32::from_rgb(222, 230, 240)
    }
}

fn render_serial_console_tab(ui: &mut egui::Ui, app: &mut GuiAppState) {
    let vm_running = app.session.is_some();
    let board = app
        .latest_state
        .as_ref()
        .map(|s| s.board_slug.clone())
        .unwrap_or_else(|| app.config.selected_board.clone());
    let lines = app
        .session
        .as_ref()
        .map(|s| s.serial_lines())
        .unwrap_or_default();

    let scroll_height = ui.available_height().max(60.0);
    let prompt_id = ui.make_persistent_id("ide_serial_inline_prompt");

    egui::Frame::canvas(ui.style())
        .fill(Color32::from_rgb(14, 16, 20))
        .stroke(Stroke::new(1.0, Color32::from_rgb(45, 52, 64)))
        .inner_margin(Margin::symmetric(10.0, 6.0))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("tty_serial_scroll")
                .auto_shrink([false, false])
                .max_height(scroll_height)
                .stick_to_bottom(app.terminal_autoscroll)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 2.0;

                    if !vm_running && lines.is_empty() {
                        ui.label(
                            RichText::new(
                                "Терминал ttyAMA0 не активен (ВМ остановлена). Нажмите «Запустить образ (Старт ВМ)» в верхней панели.",
                            )
                            .font(FontId::monospace(13.2))
                            .color(Color32::from_rgb(145, 155, 170)),
                        );
                    } else {
                        for line in &lines {
                            let clean = strip_ansi_and_control_chars(line);
                            if clean.trim() == "~ #" {
                                continue;
                            }
                            let col = classify_serial_line_color(&clean);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&clean)
                                        .font(FontId::monospace(13.2))
                                        .color(col),
                                )
                                .selectable(true),
                            );
                        }
                    }

                    ui.add_space(2.0);
                    let mut prompt_resp = None;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let cwd_display = if app.serial_cwd.is_empty() {
                            "/"
                        } else {
                            &app.serial_cwd
                        };
                        let prompt_prefix = format!("root@{}:{}#", board, cwd_display);
                        let prefix_col = if vm_running {
                            Color32::from_rgb(87, 227, 137)
                        } else {
                            Color32::from_rgb(120, 130, 145)
                        };
                        ui.label(
                            RichText::new(prompt_prefix)
                                .font(FontId::monospace(13.5))
                                .strong()
                                .color(prefix_col),
                        );

                        let has_prompt_focus = ui.ctx().memory(|m| m.has_focus(prompt_id));
                        let mut move_cursor_to_end = false;

                        if has_prompt_focus && vm_running {
                            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab))
                            {
                                app.tab_complete_serial_input();
                                app.terminal_autoscroll = true;
                                move_cursor_to_end = true;
                            }
                            if ui.input_mut(|i| {
                                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                            }) && app.history_up()
                            {
                                move_cursor_to_end = true;
                            }
                            if ui.input_mut(|i| {
                                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                            }) && app.history_down()
                            {
                                move_cursor_to_end = true;
                            }
                            if ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::L))
                            {
                                if let Some(ref sess) = app.session {
                                    sess.clear_serial_lines();
                                }
                            }
                            if app.serial_input.is_empty()
                                && ui.input_mut(|i| {
                                    i.consume_key(egui::Modifiers::CTRL, egui::Key::C)
                                })
                            {
                                app.send_serial_command("\x03");
                            }
                        }

                        if move_cursor_to_end {
                            set_text_edit_cursor_to_end(ui.ctx(), prompt_id, &app.serial_input);
                        }

                        let resp = ui.add_enabled(
                            vm_running,
                            egui::TextEdit::singleline(&mut app.serial_input)
                                .id(prompt_id)
                                .frame(false)
                                .lock_focus(true)
                                .font(FontId::monospace(13.5))
                                .text_color(Color32::from_rgb(242, 247, 255))
                                .desired_width(ui.available_width().max(120.0)),
                        );

                        if app.serial_input.contains('\t') {
                            app.tab_complete_serial_input();
                            app.terminal_autoscroll = true;
                            set_text_edit_cursor_to_end(ui.ctx(), resp.id, &app.serial_input);
                        } else if move_cursor_to_end {
                            set_text_edit_cursor_to_end(ui.ctx(), resp.id, &app.serial_input);
                        }

                        let enter_pressed = (resp.has_focus() || resp.lost_focus())
                            && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if enter_pressed && vm_running {
                            let cmd = app.serial_input.clone();
                            app.serial_input.clear();
                            app.send_serial_command(&cmd);
                            app.terminal_autoscroll = true;
                            resp.request_focus();
                        }

                        prompt_resp = Some(resp);
                    });

                    // Clicking empty terminal space focuses the inline prompt like in VS Code / JetBrains
                    let rem_h = ui.available_height().max(18.0);
                    let (_, empty_resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), rem_h),
                        egui::Sense::click(),
                    );
                    if empty_resp.clicked() {
                        if let Some(resp) = prompt_resp {
                            resp.request_focus();
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
                            ui.add(
                                egui::Label::new(
                                    RichText::new(clean)
                                        .font(FontId::monospace(13.2))
                                        .color(Color32::from_rgb(215, 225, 240)),
                                )
                                .selectable(true),
                            );
                        }
                    }
                });
        });
}
