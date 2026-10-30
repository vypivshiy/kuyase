use crate::gui::state::GuiAppState;
use eframe::egui::{self, Color32, FontId, RichText};

pub fn render_terminal_pane(ui: &mut egui::Ui, app: &mut GuiAppState) {
    ui.horizontal(|ui| {
        ui.heading("VM Console & Logs");
        ui.separator();
        if ui
            .selectable_label(
                app.config.active_terminal_tab == "serial",
                "ttyAMA0 Serial Console",
            )
            .clicked()
        {
            app.config.active_terminal_tab = "serial".to_string();
            let _ = app.save_config();
        }
        if ui
            .selectable_label(
                app.config.active_terminal_tab == "bridge",
                "Hardware Bridge Logs",
            )
            .clicked()
        {
            app.config.active_terminal_tab = "bridge".to_string();
            let _ = app.save_config();
        }
        if ui
            .selectable_label(app.config.active_terminal_tab == "maind", "maind.log")
            .clicked()
        {
            app.config.active_terminal_tab = "maind".to_string();
            let _ = app.save_config();
        }
        ui.separator();
        ui.checkbox(&mut app.terminal_autoscroll, "Auto-scroll");
        if app.config.active_terminal_tab == "serial" {
            if let Some(ref sess) = app.session {
                if ui.button("Clear").clicked() {
                    sess.clear_serial_lines();
                }
            }
        }
    });

    ui.separator();

    let lines: Vec<String> = match app.config.active_terminal_tab.as_str() {
        "bridge" => app
            .latest_state
            .as_ref()
            .map(|s| s.recent_logs.clone())
            .unwrap_or_default(),
        "maind" => app
            .latest_state
            .as_ref()
            .map(|s| s.maind_logs.clone())
            .unwrap_or_default(),
        _ => app
            .session
            .as_ref()
            .map(|s| s.serial_lines())
            .unwrap_or_default(),
    };

    let footer_height = if app.config.active_terminal_tab == "serial" {
        56.0
    } else {
        8.0
    };
    let scroll_height = (ui.available_height() - footer_height).max(80.0);

    egui::Frame::canvas(ui.style())
        .fill(Color32::from_rgb(14, 17, 23))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(scroll_height)
                .stick_to_bottom(app.terminal_autoscroll)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    if lines.is_empty() {
                        ui.label(
                            RichText::new(if app.session.is_some() {
                                "Waiting for output..."
                            } else {
                                "VM is stopped. Start VM to open interactive ttyAMA0 root console."
                            })
                            .font(FontId::monospace(12.5))
                            .color(Color32::from_rgb(130, 140, 155)),
                        );
                    } else {
                        for line in &lines {
                            let color = if line.contains("WARN") || line.contains("ERR") {
                                Color32::from_rgb(255, 170, 90)
                            } else if line.contains("HIJACK") || line.contains("VOICE") {
                                Color32::from_rgb(120, 230, 180)
                            } else {
                                Color32::from_rgb(215, 225, 235)
                            };
                            ui.label(
                                RichText::new(line)
                                    .font(FontId::monospace(12.0))
                                    .color(color),
                            );
                        }
                    }
                });
        });

    if app.config.active_terminal_tab == "serial" {
        ui.add_space(4.0);
        let vm_running = app.session.is_some();
        ui.horizontal(|ui| {
            ui.label(RichText::new("ttyAMA0 #").font(FontId::monospace(13.0)));
            let resp = ui.add_enabled(
                vm_running,
                egui::TextEdit::singleline(&mut app.serial_input)
                    .font(FontId::monospace(13.0))
                    .desired_width(ui.available_width() - 75.0)
                    .hint_text("Type root shell command and press Enter..."),
            );
            let enter_pressed =
                resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if enter_pressed || ui.add_enabled(vm_running, egui::Button::new("Send")).clicked() {
                let cmd = app.serial_input.clone();
                app.serial_input.clear();
                app.send_serial_command(&cmd);
                resp.request_focus();
            }
        });

        ui.horizontal(|ui| {
            ui.small("Quick:");
            for (label, cmd) in [
                ("Enter", ""),
                ("ls -la /", "ls -la /"),
                ("ps", "ps"),
                ("hw.json", "cat /tmp/quasar-emu-hw.json"),
                ("uname -a", "uname -a"),
            ] {
                if ui.add_enabled(vm_running, egui::Button::new(label)).clicked() {
                    app.send_serial_command(cmd);
                }
            }
        });
    }
}
