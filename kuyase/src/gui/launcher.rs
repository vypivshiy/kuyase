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

    egui::ScrollArea::vertical()
        .id_salt("images_adb_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("Статус: {}", app.status_message))
                        .color(Color32::from_rgb(150, 210, 255)),
                );
                if is_busy || bg_status != "Idle" {
                    ui.separator();
                    if is_busy {
                        ui.spinner();
                    }
                    ui.label(
                        RichText::new(format!("Задача: {}", bg_status))
                            .color(Color32::from_rgb(255, 215, 100)),
                    );
                }
            });
            ui.separator();

            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Папка образов (.ext4):").strong());
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut app.config.custom_images_dir)
                            .desired_width(240.0)
                            .hint_text("./work (по умолчанию)"),
                    )
                    .changed()
                {
                    app.refresh_images_and_dumps();
                    let _ = app.save_config();
                }
                if ui.button("Выбрать папку...").clicked() {
                    app.open_native_folder_dialog();
                }
                if ui.button("Обновить список").clicked() {
                    app.refresh_images_and_dumps();
                }
                if ui
                    .add_enabled(!is_busy, egui::Button::new("Собрать все из backups/"))
                    .clicked()
                {
                    spawn_prepare_task(app, None, None);
                }
            });

            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Исходный дамп (rootfs.tar.gz / ubi / папка):").strong());
                if !app.firmware_dumps.is_empty() {
                    egui::ComboBox::from_id_salt("dump_selector")
                        .selected_text(&app.config.selected_dump_path)
                        .width(260.0)
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
                ui.add(
                    egui::TextEdit::singleline(&mut app.config.selected_dump_path)
                        .desired_width(220.0),
                );
                if ui
                    .add_enabled(
                        !is_busy && !app.config.selected_dump_path.trim().is_empty(),
                        egui::Button::new("Пересобрать .ext4 из дампа"),
                    )
                    .clicked()
                {
                    let dump_p = PathBuf::from(app.config.selected_dump_path.trim());
                    spawn_prepare_task(app, None, Some(dump_p));
                }
            });

            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Снятие дампа по ADB (:5555):").strong());
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut app.config.last_adb_target)
                            .desired_width(160.0),
                    )
                    .changed()
                {
                    let _ = app.save_config();
                }
                if ui
                    .add_enabled(
                        !is_busy && !app.config.last_adb_target.trim().is_empty(),
                        egui::Button::new("Скачать чистый rootfs.tar.gz по ADB"),
                    )
                    .clicked()
                {
                    let target = app.config.last_adb_target.trim().to_string();
                    spawn_adb_dump_task(app, target);
                }

                ui.separator();
                ui.label("Порт моста:");
                if ui
                    .add(egui::DragValue::new(&mut app.config.bridge_port).range(1024..=65535))
                    .changed()
                {
                    let _ = app.save_config();
                }
                ui.label("Порт ADB:");
                if ui
                    .add(egui::DragValue::new(&mut app.config.adb_port).range(1024..=65535))
                    .changed()
                {
                    let _ = app.save_config();
                }
            });

            ui.horizontal_wrapped(|ui| {
                let qemu_ok = app.qemu_path.is_some();
                let kernel_ok = app.kernel_path.is_some();
                ui.label(
                    RichText::new(format!(
                        "QEMU: {}",
                        app.qemu_path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "НЕ НАЙДЕН".to_string())
                    ))
                    .small()
                    .color(if qemu_ok {
                        Color32::from_rgb(110, 230, 130)
                    } else {
                        Color32::from_rgb(255, 110, 110)
                    }),
                );
                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "Ядро Image: {}",
                        app.kernel_path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "НЕ НАЙДЕНО".to_string())
                    ))
                    .small()
                    .color(if kernel_ok {
                        Color32::from_rgb(110, 230, 130)
                    } else {
                        Color32::from_rgb(255, 110, 110)
                    }),
                );
                if ui.small_button("Проверить QEMU и ядро").clicked() {
                    app.refresh_dependencies();
                }
                if ui
                    .add_enabled(
                        !is_busy,
                        egui::Button::new("Подготовить ./bin и ./kernel"),
                    )
                    .clicked()
                {
                    spawn_dependency_download_task(app);
                }
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
    *status.lock().unwrap() = "Сборка образа .ext4 в userspace...".to_string();

    thread::spawn(move || {
        let res = match dump {
            Some(ref dp) => {
                build_guest_image_from_dump(&root, board.as_deref(), dp, None).map(|r| vec![r])
            }
            None => prepare_images(&root, board.as_deref(), None),
        };
        match res {
            Ok(built) => {
                let names: Vec<String> = built.into_iter().map(|(b, _)| b).collect();
                *status.lock().unwrap() = format!("Собраны образы: {}", names.join(", "));
            }
            Err(e) => {
                *status.lock().unwrap() = format!("Ошибка сборки образа: {}", e);
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
    *status.lock().unwrap() = format!("Снятие дампа по ADB с {}...", target);

    thread::spawn(move || {
        match dump_from_adb(&root, &target, None) {
            Ok(res) => {
                *status.lock().unwrap() = format!(
                    "Дамп сохранён: {} ({} байт)",
                    res.out_dir.display(),
                    res.archive_bytes
                );
            }
            Err(e) => {
                *status.lock().unwrap() = format!("Ошибка снятия дампа ADB: {}", e);
            }
        }
        flag.store(false, Ordering::SeqCst);
    });
}

pub fn provision_dependencies_into_workspace(root: &std::path::Path) -> (bool, bool) {
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
            let url = std::env::var("KUYASE_QEMU_URL").unwrap_or_else(|_| default_url.to_string());
            let _ = Command::new("curl")
                .args(["-fL", &url, "-o", &target_qemu.display().to_string()])
                .status();
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
    *status.lock().unwrap() = "Проверка и подготовка ./bin и ./kernel...".to_string();

    thread::spawn(move || {
        let (q_ok, k_ok) = provision_dependencies_into_workspace(&root);
        *status.lock().unwrap() = format!(
            "Проверка завершена (QEMU={}, Kernel={})",
            if q_ok { "OK" } else { "MISSING" },
            if k_ok { "OK" } else { "MISSING" }
        );
        flag.store(false, Ordering::SeqCst);
    });
}
