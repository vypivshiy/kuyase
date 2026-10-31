use crate::dumper::dump_from_adb;
use crate::gui::state::GuiAppState;
use crate::image_builder::{
    build_guest_image_from_dump, ensure_persistent_data_image, prepare_images,
};
use eframe::egui::{self, Color32, Margin, RichText, Stroke};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::thread;

const MODAL_CARD_WIDTH: f32 = 760.0;
const LABEL_COL_WIDTH: f32 = 155.0;
const PATH_COL_WIDTH: f32 = 345.0;
const PATH_ACTIONS_COL_WIDTH: f32 = 240.0;

fn section_card_frame(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: &str,
    add_body: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::none()
        .fill(Color32::from_rgb(35, 37, 43))
        .stroke(Stroke::new(1.0, Color32::from_rgb(62, 68, 78)))
        .rounding(8.0)
        .inner_margin(Margin::symmetric(12.0, 10.0))
        .show(ui, |ui| {
            ui.set_min_width(MODAL_CARD_WIDTH);
            ui.set_max_width(MODAL_CARD_WIDTH);
            ui.label(RichText::new(title).strong().size(14.8));
            ui.label(
                RichText::new(subtitle)
                    .small()
                    .color(Color32::from_rgb(155, 165, 180)),
            );
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(6.0);
            add_body(ui);
        });
}

fn render_row_label(ui: &mut egui::Ui, text: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(LABEL_COL_WIDTH, 20.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(LABEL_COL_WIDTH);
            ui.set_max_width(LABEL_COL_WIDTH);
            ui.label(RichText::new(text).small().strong());
        },
    );
}

fn render_path_cell(
    ui: &mut egui::Ui,
    dot_color: Color32,
    text: &str,
    text_color: Color32,
    hover: &str,
) {
    ui.allocate_ui_with_layout(
        egui::vec2(PATH_COL_WIDTH, 20.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(PATH_COL_WIDTH);
            ui.set_max_width(PATH_COL_WIDTH);
            ui.label(RichText::new("●").small().color(dot_color));
            ui.add(
                egui::Label::new(RichText::new(text).small().monospace().color(text_color))
                    .truncate(),
            )
            .on_hover_text(hover);
        },
    );
}

pub fn render_image_manager_modal(ctx: &egui::Context, app: &mut GuiAppState) {
    if !app.show_image_manager_modal {
        return;
    }

    let mut open = app.show_image_manager_modal;
    egui::Window::new("Образы (.ext4), ADB и настройки QEMU")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .auto_sized()
        .default_width(MODAL_CARD_WIDTH + 24.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            render_launcher_pane(ui, app);
        });
    app.show_image_manager_modal = open;
}

pub fn render_launcher_pane(ui: &mut egui::Ui, app: &mut GuiAppState) {
    let is_busy = app.bg_task_running.load(Ordering::Relaxed);
    let bg_status = app.bg_task_status.lock().unwrap().clone();

    if is_busy || bg_status != "Idle" {
        egui::Frame::none()
            .fill(Color32::from_rgb(48, 42, 22))
            .stroke(Stroke::new(1.0, Color32::from_rgb(220, 165, 20)))
            .rounding(7.0)
            .inner_margin(Margin::symmetric(12.0, 6.0))
            .show(ui, |ui| {
                ui.set_min_width(MODAL_CARD_WIDTH);
                ui.set_max_width(MODAL_CARD_WIDTH);
                ui.horizontal(|ui| {
                    if is_busy {
                        ui.spinner();
                    }
                    ui.label(
                        RichText::new(format!("Операция: {}", bg_status))
                            .small()
                            .strong()
                            .color(Color32::from_rgb(255, 220, 110)),
                    );
                });
            });
        ui.add_space(8.0);
    }

    // Карточка 1: Сборка образа колонки (.ext4)
    section_card_frame(
        ui,
        "Сборка образа колонки (.ext4)",
        "Создание загрузочного образа work/<board>-system.ext4 с физической колонки по ADB или из файла дампа",
        |ui| {
            let is_file_mode = app.config.build_source_mode == "file";

            // Переключатель способа сборки
            egui::Grid::new("image_build_mode_switch_grid")
                .num_columns(2)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    render_row_label(ui, "Способ сборки:");
                    ui.horizontal(|ui| {
                        let adb_tab_btn = egui::Button::new(
                            RichText::new("С колонки по ADB (:5555)")
                                .small()
                                .strong()
                                .color(if !is_file_mode {
                                    Color32::WHITE
                                } else {
                                    Color32::from_rgb(190, 200, 215)
                                }),
                        )
                        .fill(if !is_file_mode {
                            Color32::from_rgb(53, 132, 228)
                        } else {
                            Color32::from_rgb(48, 52, 60)
                        });
                        if ui.add(adb_tab_btn).clicked() && is_file_mode {
                            app.config.build_source_mode = "adb".to_string();
                            let _ = app.save_config();
                        }

                        let file_tab_btn = egui::Button::new(
                            RichText::new("Из файла дампа (.tar.gz / .zip / .ubi / .img)")
                                .small()
                                .strong()
                                .color(if is_file_mode {
                                    Color32::WHITE
                                } else {
                                    Color32::from_rgb(190, 200, 215)
                                }),
                        )
                        .fill(if is_file_mode {
                            Color32::from_rgb(53, 132, 228)
                        } else {
                            Color32::from_rgb(48, 52, 60)
                        });
                        if ui.add(file_tab_btn).clicked() && !is_file_mode {
                            app.config.build_source_mode = "file".to_string();
                            let _ = app.save_config();
                        }
                    });
                    ui.end_row();
                });

            ui.add_space(6.0);

            if !is_file_mode {
                // Вариант 1: По сети ADB (:5555)
                egui::Grid::new("image_build_adb_grid")
                    .num_columns(2)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        render_row_label(ui, "Адрес колонки (IP:5555):");
                        ui.horizontal(|ui| {
                            if ui
                                .add_sized(
                                    [215.0, 22.0],
                                    egui::TextEdit::singleline(&mut app.config.last_adb_target)
                                        .desired_width(215.0)
                                        .hint_text("192.168.1.100:5555"),
                                )
                                .changed()
                            {
                                let _ = app.save_config();
                            }
                            let adb_build_btn = egui::Button::new(
                                RichText::new("Снять дамп и собрать .ext4")
                                    .strong()
                                    .color(Color32::WHITE),
                            )
                            .fill(Color32::from_rgb(38, 162, 105));
                            if ui
                                .add_enabled(
                                    !is_busy && !app.config.last_adb_target.trim().is_empty(),
                                    adb_build_btn,
                                )
                                .on_hover_text(
                                    "Скачать системный раздел по ADB (:5555) в backups/ и сразу собрать готовый к запуску образ work/<board>-system.ext4",
                                )
                                .clicked()
                            {
                                let target = app.config.last_adb_target.trim().to_string();
                                spawn_adb_dump_and_build_task(app, target, true);
                            }
                            if ui
                                .add_enabled(
                                    !is_busy && !app.config.last_adb_target.trim().is_empty(),
                                    egui::Button::new("Только скачать .tar.gz"),
                                )
                                .clicked()
                            {
                                let target = app.config.last_adb_target.trim().to_string();
                                spawn_adb_dump_and_build_task(app, target, false);
                            }
                        });
                        ui.end_row();
                    });
            } else {
                // Вариант 2: Из файла дампа (в том же стиле, что и настройки путей)
                let dump_trimmed = app.config.selected_dump_path.trim().to_string();
                let dump_exists = !dump_trimmed.is_empty() && Path::new(&dump_trimmed).is_file();
                let (dump_dot_col, dump_display, dump_text_col) = if dump_exists {
                    (
                        Color32::from_rgb(100, 230, 135),
                        dump_trimmed.clone(),
                        Color32::from_rgb(215, 222, 232),
                    )
                } else {
                    (
                        Color32::from_rgb(245, 190, 90),
                        "Не выбран (выберите .tar.gz / .zip / .ubi / .img)".to_string(),
                        Color32::from_rgb(245, 190, 90),
                    )
                };

                egui::Grid::new("image_build_file_grid")
                    .num_columns(3)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        render_row_label(ui, "Файл дампа:");
                        render_path_cell(
                            ui,
                            dump_dot_col,
                            &dump_display,
                            dump_text_col,
                            &dump_display,
                        );
                        ui.allocate_ui_with_layout(
                            egui::vec2(PATH_ACTIONS_COL_WIDTH, 20.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_min_width(PATH_ACTIONS_COL_WIDTH);
                                ui.set_max_width(PATH_ACTIONS_COL_WIDTH);
                                if ui.small_button("Выбрать файл...").clicked() {
                                    app.open_native_file_dialog();
                                }
                                let build_btn = egui::Button::new(
                                    RichText::new("Собрать .ext4")
                                        .small()
                                        .strong()
                                        .color(Color32::WHITE),
                                )
                                .fill(Color32::from_rgb(38, 162, 105));
                                if ui
                                    .add_enabled(!is_busy && dump_exists, build_btn)
                                    .on_hover_text(
                                        "Собрать загрузочный образ work/<board>-system.ext4 из выбранного файла дампа",
                                    )
                                    .clicked()
                                {
                                    let dump_p = PathBuf::from(&dump_trimmed);
                                    spawn_prepare_task(app, None, Some(dump_p));
                                }
                            },
                        );
                        ui.end_row();
                    });
            }

            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);

            ui.label(
                RichText::new(
                    "Примечание: автоматический санитайзер удаляет известные токены, ключи и конфиги Wi-Fi, но не гарантирует 100% очистку чувствительных данных. Если планируется публично выкладывать дамп или образ, обязательно перепроверьте его содержимое вручную.",
                )
                .small()
                .color(Color32::from_rgb(235, 195, 105)),
            );
        },
    );

    ui.add_space(10.0);

    // Карточка 2: Пути и сеть виртуальной машины
    section_card_frame(
        ui,
        "Пути и сеть виртуальной машины",
        "Текущие пути к папке образов, эмулятору QEMU и ядру Linux",
        |ui| {
            let images_dir_display = if app.config.custom_images_dir.trim().is_empty() {
                format!("{} (авто)", app.root_dir.join("work").display())
            } else {
                app.config.custom_images_dir.trim().to_string()
            };

            let (qemu_ok, qemu_display) = match app.qemu_path.as_ref() {
                Some(p) => {
                    let suffix = if app.config.custom_qemu_path.trim().is_empty() {
                        " (авто)"
                    } else {
                        ""
                    };
                    (true, format!("{}{}", p.display(), suffix))
                }
                None => (false, "Не найден (выберите qemu-system-aarch64)".to_string()),
            };

            let (kernel_ok, kernel_display) = match app.kernel_path.as_ref() {
                Some(p) => {
                    let suffix = if app.config.custom_kernel_path.trim().is_empty() {
                        " (авто)"
                    } else {
                        ""
                    };
                    (true, format!("{}{}", p.display(), suffix))
                }
                None => (false, "Не найдено (выберите файл Image)".to_string()),
            };

            egui::Grid::new("vm_paths_settings_grid")
                .num_columns(3)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    // 1. Папка образов (.ext4)
                    render_row_label(ui, "Папка образов (.ext4):");
                    let img_count = app.guest_images.len();
                    let img_col = if img_count > 0 {
                        Color32::from_rgb(100, 230, 135)
                    } else {
                        Color32::from_rgb(245, 190, 90)
                    };
                    let img_text = format!("{} ({} обр.)", images_dir_display, img_count);
                    render_path_cell(
                        ui,
                        img_col,
                        &img_text,
                        Color32::from_rgb(215, 222, 232),
                        &img_text,
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(PATH_ACTIONS_COL_WIDTH, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_min_width(PATH_ACTIONS_COL_WIDTH);
                            ui.set_max_width(PATH_ACTIONS_COL_WIDTH);
                            if ui.small_button("Выбрать папку...").clicked() {
                                app.open_native_folder_dialog();
                            }
                            if !app.config.custom_images_dir.trim().is_empty()
                                && ui
                                    .small_button("Авто")
                                    .on_hover_text("Вернуть папку ./work по умолчанию")
                                    .clicked()
                            {
                                app.config.custom_images_dir.clear();
                                app.refresh_images_and_dumps();
                                let _ = app.save_config();
                            }
                            if ui
                                .small_button("↻")
                                .on_hover_text("Пересканировать папку образов")
                                .clicked()
                            {
                                app.refresh_images_and_dumps();
                            }
                        },
                    );
                    ui.end_row();

                    // 2. Эмулятор QEMU
                    render_row_label(ui, "Эмулятор QEMU:");
                    let q_col = if qemu_ok {
                        Color32::from_rgb(100, 230, 135)
                    } else {
                        Color32::from_rgb(255, 110, 110)
                    };
                    render_path_cell(
                        ui,
                        q_col,
                        &qemu_display,
                        if qemu_ok {
                            Color32::from_rgb(215, 222, 232)
                        } else {
                            q_col
                        },
                        &qemu_display,
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(PATH_ACTIONS_COL_WIDTH, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_min_width(PATH_ACTIONS_COL_WIDTH);
                            ui.set_max_width(PATH_ACTIONS_COL_WIDTH);
                            if ui.small_button("Выбрать файл...").clicked() {
                                app.open_qemu_file_dialog();
                            }
                            if !app.config.custom_qemu_path.trim().is_empty()
                                && ui
                                    .small_button("Авто")
                                    .on_hover_text("Вернуть автопоиск QEMU")
                                    .clicked()
                            {
                                app.config.custom_qemu_path.clear();
                                app.refresh_dependencies();
                                let _ = app.save_config();
                            }
                        },
                    );
                    ui.end_row();

                    // 3. Ядро Linux (Image)
                    render_row_label(ui, "Ядро Linux (Image):");
                    let k_col = if kernel_ok {
                        Color32::from_rgb(100, 230, 135)
                    } else {
                        Color32::from_rgb(255, 110, 110)
                    };
                    render_path_cell(
                        ui,
                        k_col,
                        &kernel_display,
                        if kernel_ok {
                            Color32::from_rgb(215, 222, 232)
                        } else {
                            k_col
                        },
                        &kernel_display,
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(PATH_ACTIONS_COL_WIDTH, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_min_width(PATH_ACTIONS_COL_WIDTH);
                            ui.set_max_width(PATH_ACTIONS_COL_WIDTH);
                            if ui.small_button("Выбрать файл...").clicked() {
                                app.open_kernel_file_dialog();
                            }
                            if !app.config.custom_kernel_path.trim().is_empty()
                                && ui
                                    .small_button("Авто")
                                    .on_hover_text("Вернуть автопоиск ядра Image")
                                    .clicked()
                            {
                                app.config.custom_kernel_path.clear();
                                app.refresh_dependencies();
                                let _ = app.save_config();
                            }
                        },
                    );
                    ui.end_row();

                    // 4. Персистентный диск данных (/data)
                    render_row_label(ui, "Диск данных (/data):");
                    let data_img_path = app.selected_board_data_image_path();
                    let data_img_exists = data_img_path.is_file();
                    let data_active = app.config.persist_data_disk || data_img_exists;
                    let (d_col, d_display) = if data_img_exists {
                        let mb = fs::metadata(&data_img_path)
                            .map(|m| m.len() / (1024 * 1024))
                            .unwrap_or(512);
                        (
                            Color32::from_rgb(100, 230, 135),
                            format!("{} ({} МБ)", data_img_path.display(), mb),
                        )
                    } else if app.config.persist_data_disk {
                        (
                            Color32::from_rgb(100, 230, 135),
                            format!("{} (создастся при старте, 512 МБ)", data_img_path.display()),
                        )
                    } else if !app.config.clean_boot_snapshot {
                        (
                            Color32::from_rgb(245, 190, 90),
                            "На корневом диске system.ext4 (Snapshot выкл.)".to_string(),
                        )
                    } else {
                        (
                            Color32::from_rgb(155, 165, 180),
                            "Отключён (tmpfs в ОЗУ)".to_string(),
                        )
                    };
                    render_path_cell(
                        ui,
                        d_col,
                        &d_display,
                        if data_active {
                            Color32::from_rgb(215, 222, 232)
                        } else {
                            d_col
                        },
                        "Отдельный образ work/<board>-data.ext4 (quasar.data=persist) сохраняет установленные пакеты YAM / Luna / AirPlay между перезапусками и пересборками системного образа, даже при включённом Snapshot.",
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(PATH_ACTIONS_COL_WIDTH, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_min_width(PATH_ACTIONS_COL_WIDTH);
                            ui.set_max_width(PATH_ACTIONS_COL_WIDTH);
                            let mut use_disk = data_active;
                            if ui
                                .checkbox(&mut use_disk, "Диск")
                                .on_hover_text("Подключать отдельный диск work/<board>-data.ext4 (512 МБ) как /data (с учётом симлинка /data -> /bkupgrade/data)")
                                .changed()
                            {
                                app.config.persist_data_disk = use_disk;
                                if !use_disk && data_img_exists {
                                    let _ = fs::remove_file(&data_img_path);
                                }
                                let _ = app.save_config();
                            }
                            let create_label = if data_img_exists {
                                "Очистить"
                            } else {
                                "Создать 512 МБ"
                            };
                            if ui
                                .add_enabled(!is_busy, egui::Button::new(RichText::new(create_label).small()))
                                .on_hover_text("Создать или пересоздать чистый образ work/<board>-data.ext4 (512 МБ) в userspace")
                                .clicked()
                            {
                                app.config.persist_data_disk = true;
                                let _ = app.save_config();
                                spawn_create_data_disk_task(app);
                            }
                        },
                    );
                    ui.end_row();
                });

            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                ui.label(RichText::new("Порт моста (:8091 -> ПК):").small());
                if ui
                    .add(
                        egui::DragValue::new(&mut app.config.bridge_port)
                            .range(1024..=65535),
                    )
                    .changed()
                {
                    let _ = app.save_config();
                }
                ui.separator();
                ui.label(RichText::new("Порт гостевого ADB (:5555 -> ПК):").small());
                if ui
                    .add(
                        egui::DragValue::new(&mut app.config.adb_port).range(1024..=65535),
                    )
                    .changed()
                {
                    let _ = app.save_config();
                }
                ui.separator();
                if ui
                    .checkbox(
                        &mut app.config.bind_all_interfaces,
                        RichText::new("Доступ из локальной сети (0.0.0.0)").small(),
                    )
                    .on_hover_text(
                        "Разрешить подключение к проброшенным портам моста (:18091), ADB (:15555) и SDK (:18090) с других устройств в локальной сети (hostfwd на 0.0.0.0 вместо 127.0.0.1).",
                    )
                    .changed()
                {
                    let _ = app.save_config();
                }
            });

            ui.add_space(4.0);

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("LAN TAP-мост (wlan0):")
                        .small()
                        .strong(),
                )
                .on_hover_text(
                    "Опциональный второй сетевой адаптер virtio-net-pci, подключённый к TAP-интерфейсу на хосте.\nГостевая ОС переименовывает адаптер с MAC 52:4b:59:* в wlan0 и получает IP по DHCP от роутера (для обнаружения колонки по AirPlay / mDNS с телефона).",
                );
                if ui
                    .add_sized(
                        [130.0, 20.0],
                        egui::TextEdit::singleline(&mut app.config.lan_tap_interface)
                            .desired_width(130.0)
                            .hint_text("выкл. (напр. kytap0)"),
                    )
                    .on_hover_text("Имя предварительно созданного TAP-интерфейса на хосте (оставьте пустым для отключения второго адаптера)")
                    .changed()
                {
                    let _ = app.save_config();
                }
                ui.separator();
                ui.label(RichText::new("MAC (52:4b:59:*):").small());
                if ui
                    .add_sized(
                        [145.0, 20.0],
                        egui::TextEdit::singleline(&mut app.config.lan_mac_address)
                            .desired_width(145.0)
                            .hint_text(crate::session::DEFAULT_LAN_MAC_BASE),
                    )
                    .on_hover_text("MAC-адрес второго адаптера (должен начинаться с 52:4b:59:, чтобы гостевой init переименовал его в wlan0)")
                    .changed()
                {
                    let _ = app.save_config();
                }
                if app.config.lan_mac_address.trim() != crate::session::DEFAULT_LAN_MAC_BASE
                    && ui
                        .small_button("Сброс MAC")
                        .on_hover_text("Вернуть стандартный MAC 52:4b:59:00:00:01")
                        .clicked()
                {
                    app.config.lan_mac_address = crate::session::DEFAULT_LAN_MAC_BASE.to_string();
                    let _ = app.save_config();
                }
            });
        },
    );
}

fn spawn_create_data_disk_task(app: &mut GuiAppState) {
    if app.bg_task_running.swap(true, Ordering::SeqCst) {
        return;
    }
    let images_dir = app.active_images_dir();
    let board = app.config.selected_board.clone();
    let flag = app.bg_task_running.clone();
    let status = app.bg_task_status.clone();
    *status.lock().unwrap() = format!("Создание образа {}-data.ext4 (512 МБ)...", board);

    thread::spawn(move || {
        match ensure_persistent_data_image(&images_dir, &board, true) {
            Ok(path) => {
                *status.lock().unwrap() =
                    format!("Готов персистентный диск /data: {}", path.display());
            }
            Err(e) => {
                *status.lock().unwrap() = format!("Ошибка создания data.ext4: {}", e);
            }
        }
        flag.store(false, Ordering::SeqCst);
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

fn spawn_adb_dump_and_build_task(app: &mut GuiAppState, target: String, build_ext4: bool) {
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
                if build_ext4 {
                    *status.lock().unwrap() = format!(
                        "Дамп {} снят ({} байт). Сборка образа .ext4...",
                        res.board_slug, res.archive_bytes
                    );
                    match build_guest_image_from_dump(
                        &root,
                        Some(&res.board_slug),
                        &res.archive_path,
                        None,
                    ) {
                        Ok((board, out)) => {
                            *status.lock().unwrap() = format!(
                                "Готов образ {} из ADB: {}",
                                board,
                                out.display()
                            );
                        }
                        Err(e) => {
                            *status.lock().unwrap() =
                                format!("Дамп снят, но ошибка сборки .ext4: {}", e);
                        }
                    }
                } else {
                    *status.lock().unwrap() = format!(
                        "Дамп сохранён: {} ({} байт)",
                        res.out_dir.display(),
                        res.archive_bytes
                    );
                }
            }
            Err(e) => {
                *status.lock().unwrap() = format!("Ошибка снятия дампа ADB: {}", e);
            }
        }
        flag.store(false, Ordering::SeqCst);
    });
}
