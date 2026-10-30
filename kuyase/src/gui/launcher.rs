use crate::dumper::dump_from_adb;
use crate::gui::state::GuiAppState;
use crate::image_builder::{build_guest_image_from_dump, prepare_images};
use crate::session::{resolve_kernel_image, resolve_qemu_binary};
use eframe::egui::{self, Color32, RichText};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::thread;

pub fn render_launcher_pane(ui: &mut egui::Ui, app: &mut GuiAppState) {
    let is_busy = app.bg_task_running.load(Ordering::Relaxed);
    let bg_status = app.bg_task_status.lock().unwrap().clone();

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.heading("Board & Image Manager");
        ui.label(
            RichText::new(format!("Status: {}", app.status_message))
                .color(Color32::from_rgb(150, 210, 255)),
        );
        if is_busy || bg_status != "Idle" {
            ui.horizontal(|ui| {
                if is_busy {
                    ui.spinner();
                }
                ui.label(
                    RichText::new(format!("Task: {}", bg_status))
                        .color(Color32::from_rgb(255, 215, 100)),
                );
            });
        }
        ui.separator();

        // 1. VM Lifecycle & Boot Mode Controls
        ui.collapsing("1. VM Lifecycle & Boot Mode", |ui| {
            ui.horizontal(|ui| {
                ui.label("Board Profile:");
                egui::ComboBox::from_id_salt("board_selector")
                    .selected_text(&app.config.selected_board)
                    .show_ui(ui, |ui| {
                        let known = [
                            "yandexmini",
                            "yandexmini2",
                            "yandexmicro",
                            "cucumber",
                            "orion",
                            "bergamot",
                            "pickle",
                            "plum",
                            "station2",
                            "mango",
                            "saturn",
                        ];
                        for b in known {
                            if ui
                                .selectable_value(&mut app.config.selected_board, b.to_string(), b)
                                .changed()
                            {
                                let _ = app.save_config();
                            }
                        }
                    });
            });

            ui.horizontal(|ui| {
                ui.label("Quasar Mode:");
                if ui
                    .radio_value(&mut app.config.mode, "stub".to_string(), "Stub Mode (quasar.mode=stub)")
                    .changed()
                {
                    let _ = app.save_config();
                }
                if ui
                    .radio_value(&mut app.config.mode, "full".to_string(), "Full Mode (quasar.mode=full)")
                    .changed()
                {
                    let _ = app.save_config();
                }
            });

            if ui
                .checkbox(
                    &mut app.config.clean_boot_snapshot,
                    "Clean Boot (Ephemeral Snapshot / -snapshot)",
                )
                .changed()
            {
                let _ = app.save_config();
            }

            ui.horizontal(|ui| {
                ui.label("Bridge Port:");
                if ui
                    .add(egui::DragValue::new(&mut app.config.bridge_port).range(1024..=65535))
                    .changed()
                {
                    let _ = app.save_config();
                }
                ui.label("ADB Port:");
                if ui
                    .add(egui::DragValue::new(&mut app.config.adb_port).range(1024..=65535))
                    .changed()
                {
                    let _ = app.save_config();
                }
            });

            ui.horizontal(|ui| {
                let running = app.session.is_some();
                if ui
                    .add_enabled(!running && !is_busy, egui::Button::new("▶ Start VM"))
                    .clicked()
                {
                    app.start_vm();
                }
                if ui
                    .add_enabled(running, egui::Button::new("⏹ Stop VM"))
                    .clicked()
                {
                    app.stop_vm();
                }
                if ui
                    .add_enabled(!is_busy, egui::Button::new("↻ Restart VM"))
                    .clicked()
                {
                    app.restart_vm();
                }
            });
        });

        ui.separator();

        // 2. Discovered Guest Images & Userspace Builder
        ui.collapsing("2. Guest Images (work/*-system.ext4) & Userspace Builder", |ui| {
            ui.horizontal(|ui| {
                if ui.button("↻ Refresh List").clicked() {
                    app.refresh_images_and_dumps();
                }
                if ui
                    .add_enabled(!is_busy, egui::Button::new("⚙ Build All from backups/"))
                    .clicked()
                {
                    spawn_prepare_task(app, None, None);
                }
            });

            if app.guest_images.is_empty() {
                ui.label(
                    RichText::new("No Guest Images found in work/*.ext4 yet.")
                        .color(Color32::from_rgb(240, 160, 90)),
                );
            } else {
                for (slug, path, sz) in app.guest_images.clone() {
                    ui.horizontal(|ui| {
                        let selected = app.config.selected_board == slug;
                        if ui.selectable_label(selected, &slug).clicked() {
                            app.config.selected_board = slug.clone();
                            let _ = app.save_config();
                        }
                        ui.small(format!(
                            "{} ({:.1} MB)",
                            path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
                            sz as f64 / (1024.0 * 1024.0)
                        ));
                    });
                }
            }

            ui.add_space(4.0);
            ui.label("Firmware Dump Source (rootfs.tar.gz / ubi / squashfs / folder):");
            if !app.firmware_dumps.is_empty() {
                egui::ComboBox::from_id_salt("dump_selector")
                    .selected_text(&app.config.selected_dump_path)
                    .show_ui(ui, |ui| {
                        for (name, path) in &app.firmware_dumps {
                            let p_str = path.display().to_string();
                            if ui
                                .selectable_value(
                                    &mut app.config.selected_dump_path,
                                    p_str.clone(),
                                    format!("{} ({})", name, p_str),
                                )
                                .changed()
                            {
                                let _ = app.save_config();
                            }
                        }
                    });
            }
            ui.text_edit_singleline(&mut app.config.selected_dump_path);

            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !is_busy && !app.config.selected_dump_path.trim().is_empty(),
                        egui::Button::new("🛠 Rebuild Image from Selected Dump"),
                    )
                    .clicked()
                {
                    let dump_p = PathBuf::from(app.config.selected_dump_path.trim());
                    spawn_prepare_task(app, None, Some(dump_p));
                }
            });
        });

        ui.separator();

        // 3. Built-in Network ADB Firmware Dumper (:5555)
        ui.collapsing("3. Network ADB Firmware Dumper (:5555)", |ui| {
            ui.label("Speaker IP[:port] (built-in Rust TCP ADB client):");
            if ui
                .text_edit_singleline(&mut app.config.last_adb_target)
                .changed()
            {
                let _ = app.save_config();
            }
            if ui
                .add_enabled(
                    !is_busy && !app.config.last_adb_target.trim().is_empty(),
                    egui::Button::new("⬇ Pull Clean rootfs.tar.gz via ADB"),
                )
                .clicked()
            {
                let target = app.config.last_adb_target.trim().to_string();
                spawn_adb_dump_task(app, target);
            }
        });

        ui.separator();

        // 4. Dependency Manager (QEMU & Guest Kernel)
        ui.collapsing("4. Dependency Manager (QEMU & Kernel)", |ui| {
            let qemu_ok = app.qemu_path.is_some();
            let kernel_ok = app.kernel_path.is_some();

            ui.label(
                RichText::new(format!(
                    "qemu-system-aarch64: {}",
                    app.qemu_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "MISSING (checked ./bin, ./Releases, PATH)".to_string())
                ))
                .color(if qemu_ok {
                    Color32::from_rgb(110, 230, 130)
                } else {
                    Color32::from_rgb(255, 110, 110)
                }),
            );
            ui.horizontal(|ui| {
                ui.label("Custom QEMU path:");
                if ui
                    .text_edit_singleline(&mut app.config.custom_qemu_path)
                    .changed()
                {
                    app.refresh_dependencies();
                    let _ = app.save_config();
                }
            });

            ui.label(
                RichText::new(format!(
                    "Guest Kernel Image: {}",
                    app.kernel_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| {
                            "MISSING (checked ./kernel/Image, ./Releases/Image, ./bin/Image)"
                                .to_string()
                        })
                ))
                .color(if kernel_ok {
                    Color32::from_rgb(110, 230, 130)
                } else {
                    Color32::from_rgb(255, 110, 110)
                }),
            );
            ui.horizontal(|ui| {
                ui.label("Custom Kernel path:");
                if ui
                    .text_edit_singleline(&mut app.config.custom_kernel_path)
                    .changed()
                {
                    app.refresh_dependencies();
                    let _ = app.save_config();
                }
            });

            ui.horizontal(|ui| {
                if ui.button("🔍 Re-check Dependencies").clicked() {
                    app.refresh_dependencies();
                }
                if ui
                    .add_enabled(
                        !is_busy,
                        egui::Button::new("⬇ One-Click Download / Provision into ./bin & ./kernel"),
                    )
                    .clicked()
                {
                    spawn_dependency_download_task(app);
                }
            });
        });
    });
}

fn spawn_prepare_task(app: &mut GuiAppState, board: Option<String>, dump: Option<PathBuf>) {
    if app.bg_task_running.swap(true, Ordering::SeqCst) {
        return;
    }
    let root = app.root_dir.clone();
    let flag = app.bg_task_running.clone();
    let status = app.bg_task_status.clone();
    *status.lock().unwrap() = "Building userspace .ext4 Guest Image...".to_string();

    thread::spawn(move || {
        let res = match dump {
            Some(ref dp) => build_guest_image_from_dump(&root, board.as_deref(), dp, None)
                .map(|r| vec![r]),
            None => prepare_images(&root, board.as_deref(), None),
        };
        match res {
            Ok(built) => {
                let names: Vec<String> = built.into_iter().map(|(b, _)| b).collect();
                *status.lock().unwrap() =
                    format!("Built Guest Image(s): {}", names.join(", "));
            }
            Err(e) => {
                *status.lock().unwrap() = format!("Image build failed: {}", e);
            }
        }
        flag.store(false, Ordering::SeqCst);
    });
}

fn spawn_adb_dump_task(app: &mut GuiAppState, target: String) {
    if app.bg_task_running.swap(true, Ordering::SeqCst) {
        return;
    }
    let root = app.root_dir.clone();
    let flag = app.bg_task_running.clone();
    let status = app.bg_task_status.clone();
    *status.lock().unwrap() = format!("Pulling ADB dump from {}...", target);

    thread::spawn(move || {
        match dump_from_adb(&root, &target, None) {
            Ok(res) => {
                *status.lock().unwrap() = format!(
                    "Dump complete: {} ({} bytes)",
                    res.out_dir.display(),
                    res.archive_bytes
                );
            }
            Err(e) => {
                *status.lock().unwrap() = format!("ADB dump failed: {}", e);
            }
        }
        flag.store(false, Ordering::SeqCst);
    });
}

pub fn provision_dependencies_into_workspace(root: & std::path::Path) -> (bool, bool) {
    let bin_dir = root.join("bin");
    let kernel_dir = root.join("kernel");
    let _ = fs::create_dir_all(&bin_dir);
    let _ = fs::create_dir_all(&kernel_dir);

    let qemu_bin_name = if cfg!(windows) {
        "qemu-system-aarch64.exe"
    } else {
        "qemu-system-aarch64"
    };
    let target_qemu = bin_dir.join(qemu_bin_name);
    if !target_qemu.is_file() {
        if let Ok(found_q) = resolve_qemu_binary(root) {
            if found_q != target_qemu {
                let _ = fs::copy(&found_q, &target_qemu);
            }
        } else {
            let default_url = if cfg!(windows) {
                "https://github.com/kuyase/kuyase/releases/latest/download/qemu-system-aarch64.exe"
            } else {
                "https://github.com/kuyase/kuyase/releases/latest/download/qemu-system-aarch64"
            };
            let url =
                std::env::var("KUYASE_QEMU_URL").unwrap_or_else(|_| default_url.to_string());
            let _ = Command::new("curl")
                .args(["-fL", &url, "-o", &target_qemu.display().to_string()])
                .status();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = fs::metadata(&target_qemu) {
                    let mut perms = meta.permissions();
                    perms.set_mode(0o755);
                    let _ = fs::set_permissions(&target_qemu, perms);
                }
            }
        }
    }

    let target_kernel = kernel_dir.join("Image");
    if !target_kernel.is_file() {
        if let Ok(found_k) = resolve_kernel_image(root) {
            if found_k != target_kernel {
                let _ = fs::copy(&found_k, &target_kernel);
            }
        } else {
            let url = std::env::var("KUYASE_KERNEL_URL").unwrap_or_else(|_| {
                "https://github.com/kuyase/kuyase/releases/latest/download/Image".to_string()
            });
            let _ = Command::new("curl")
                .args(["-fL", &url, "-o", &target_kernel.display().to_string()])
                .status();
        }
    }

    let q_ok = resolve_qemu_binary(root).is_ok();
    let k_ok = resolve_kernel_image(root).is_ok();
    (q_ok, k_ok)
}

fn spawn_dependency_download_task(app: &mut GuiAppState) {
    if app.bg_task_running.swap(true, Ordering::SeqCst) {
        return;
    }
    let root = app.root_dir.clone();
    let flag = app.bg_task_running.clone();
    let status = app.bg_task_status.clone();
    *status.lock().unwrap() = "Provisioning ./bin and ./kernel dependencies...".to_string();

    thread::spawn(move || {
        let (q_ok, k_ok) = provision_dependencies_into_workspace(&root);
        *status.lock().unwrap() = format!(
            "Dependency check complete (QEMU={}, Kernel={})",
            if q_ok { "OK" } else { "MISSING" },
            if k_ok { "OK" } else { "MISSING" }
        );
        flag.store(false, Ordering::SeqCst);
    });
}
