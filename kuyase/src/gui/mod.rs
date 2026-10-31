pub mod canvas;
pub mod docks;
pub mod host_audio;
pub mod launcher;
pub mod state;
pub mod terminal;

use crate::gui::state::GuiAppState;
use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, Margin, RichText, Stroke, TextStyle};
use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

pub const BOARD_LABELS: &[(&str, &str)] = &[
    ("cucumber", "cucumber — Станция Миди (24 LED + Часы + 6 LED Лого)"),
    ("orion", "orion — Яндекс Станция 3 (36 LED + Ambilight + Матрица 21x12)"),
    ("yandexmini2", "yandexmini2 — Станция Мини 2 с часами (6 LED + Часы)"),
    ("yandexmini2nc", "yandexmini2nc — Станция Мини 2 без часов (6 LED)"),
    ("plum", "plum — Станция Лайт 2 с часами (3 LED + Часы)"),
    ("plumnc", "plumnc — Станция Лайт 2 без часов (3 LED)"),
    ("station2", "station2 — Яндекс Станция 2 (88 LED световая панель)"),
    ("yandexmini", "yandexmini — Станция Мини 1 (6 LED SN3236 BGR + ToF)"),
    ("yandexmicro", "yandexmicro — Станция Лайт 1 (3 LED sysfs)"),
    ("bergamot", "bergamot — Станция Мини 3 (6 LED + Часы)"),
    ("pickle", "pickle — Станция Мини 3 Про (16 LED + Часы + 6 LED Лого)"),
    ("mango", "mango — Станция Стрит (4 LED + 3 LED Лого)"),
    ("saturn", "saturn — Яндекс Хаб (1 LED)"),
];

pub fn board_display_label(slug: &str) -> String {
    BOARD_LABELS
        .iter()
        .find(|&&(k, _)| k == slug)
        .map(|&(_, v)| v.to_string())
        .unwrap_or_else(|| slug.to_string())
}

pub struct KuyaseGuiApp {
    pub state: GuiAppState,
}

impl KuyaseGuiApp {
    pub fn new(root_dir: &Path) -> Self {
        Self {
            state: GuiAppState::new(root_dir),
        }
    }

    fn render_headerbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("KUYASE")
                    .strong()
                    .size(16.0)
                    .color(Color32::from_rgb(120, 185, 255)),
            );
            ui.separator();

            let combo_w = (ui.available_width() * 0.26).clamp(190.0, 340.0);
            let selected_text = if self.state.guest_images.is_empty() {
                "Нет готовых образов .ext4".to_string()
            } else {
                board_display_label(&self.state.config.selected_board)
            };
            egui::ComboBox::from_id_salt("header_board_selector")
                .selected_text(selected_text)
                .width(combo_w)
                .show_ui(ui, |ui| {
                    if self.state.guest_images.is_empty() {
                        ui.label(
                            RichText::new("Готовые образы (.ext4) не найдены в ./work")
                                .small()
                                .color(Color32::from_rgb(245, 190, 110)),
                        );
                        if ui.button("Открыть «Образы и QEMU...»").clicked() {
                            self.state.show_image_manager_modal = true;
                            ui.close_menu();
                        }
                    } else {
                        ui.label(
                            RichText::new("Найденные образы (.ext4):")
                                .small()
                                .color(Color32::from_rgb(165, 175, 190)),
                        );
                        for (slug, path, sz) in self.state.guest_images.clone() {
                            let fname = path
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or(&slug);
                            let label = format!(
                                "{} [{} — {:.0} МБ]",
                                board_display_label(&slug),
                                fname,
                                sz as f64 / (1024.0 * 1024.0)
                            );
                            if ui
                                .selectable_value(
                                    &mut self.state.config.selected_board,
                                    slug.clone(),
                                    label,
                                )
                                .changed()
                            {
                                let _ = self.state.save_config();
                            }
                        }
                    }
                });

            let vm_running = self.state.session.is_some();
            let busy = self.state.bg_task_running.load(Ordering::Relaxed);

            let start_btn = egui::Button::new(
                RichText::new(if vm_running {
                    "Перезапустить образ"
                } else {
                    "Запустить образ (Старт ВМ)"
                })
                .strong()
                .color(Color32::WHITE),
            )
            .fill(if vm_running {
                Color32::from_rgb(53, 132, 228)
            } else {
                Color32::from_rgb(38, 162, 105)
            });
            if ui.add_enabled(!busy, start_btn).clicked() {
                if vm_running {
                    self.state.restart_vm();
                } else {
                    self.state.start_vm();
                }
            }

            if vm_running {
                let stop_btn = egui::Button::new(
                    RichText::new("Остановить ВМ")
                        .strong()
                        .color(Color32::from_rgb(255, 220, 220)),
                )
                .fill(Color32::from_rgb(135, 38, 42));
                if ui.add(stop_btn).clicked() {
                    self.state.stop_vm();
                }
            }

            let modal_btn = egui::Button::new(
                RichText::new("Образы и QEMU...")
                    .strong()
                    .color(if self.state.show_image_manager_modal {
                        Color32::from_rgb(130, 195, 255)
                    } else {
                        Color32::from_rgb(225, 232, 240)
                    }),
            );
            if ui
                .add(modal_btn)
                .on_hover_text("Открыть окно создания образов .ext4 (по ADB или из дампа), выбора папки образов и настроек QEMU")
                .clicked()
            {
                self.state.show_image_manager_modal = !self.state.show_image_manager_modal;
            }

            let bridge_online = self
                .state
                .bridge
                .as_ref()
                .map(|b| b.is_connected())
                .unwrap_or(false);

            let (badge_text, badge_fg, badge_bg) = if bridge_online {
                (
                    format!(
                        "ONLINE (:{} / {})",
                        self.state.config.bridge_port, self.state.config.selected_board
                    ),
                    Color32::from_rgb(87, 227, 137),
                    Color32::from_rgb(24, 58, 38),
                )
            } else if vm_running {
                (
                    format!("BOOTING ({})...", self.state.config.selected_board),
                    Color32::from_rgb(248, 228, 92),
                    Color32::from_rgb(64, 52, 20),
                )
            } else {
                (
                    format!("ОСТАНОВЛЕНА ({})", self.state.config.selected_board),
                    Color32::from_rgb(245, 190, 110),
                    Color32::from_rgb(62, 46, 26),
                )
            };

            ui.add(
                egui::Button::new(RichText::new(badge_text).small().strong().color(badge_fg))
                    .fill(badge_bg)
                    .rounding(99.0)
                    .sense(egui::Sense::hover()),
            );

            ui.separator();

            let mut mode_changed = false;
            egui::ComboBox::from_id_salt("header_mode_selector")
                .selected_text(if self.state.config.mode == "full" {
                    "Режим: Full"
                } else {
                    "Режим: Stub"
                })
                .width(120.0)
                .show_ui(ui, |ui| {
                    ui.label(
                        RichText::new(if vm_running {
                            "Смена режима автоматически перезапустит запущенную ВМ:"
                        } else {
                            "Режим запуска гостевого стека (при старте ВМ):"
                        })
                        .small()
                        .color(Color32::from_rgb(245, 200, 110)),
                    );
                    ui.separator();
                    if ui
                        .selectable_value(
                            &mut self.state.config.mode,
                            "stub".to_string(),
                            "Stub — автономный мок-сервер IPC (по умолчанию)",
                        )
                        .on_hover_text(
                            "Оригинальный maind отключён. Гостевой демон qemu-hw-bridge сам поднимает локальные сокеты Quasar IPC (:9875/:9850/:9903), эмулирует периферию, синтезатор речи и окно перехвата (1680 мс) без облака Яндекса.",
                        )
                        .changed()
                    {
                        mode_changed = true;
                    }
                    if ui
                        .selectable_value(
                            &mut self.state.config.mode,
                            "full".to_string(),
                            "Full — штатный maind из прошивки (экспериментальный)",
                        )
                        .on_hover_text(
                            "Запускается штатный /system/vendor/quasar/onboot.sh и оригинальный maind (мост уступает ему порты :9850/:9875/:9903).\nПрактически не тестировался: на очищенных образах без ключей и токенов авторизации Яндекса полноценно работать, скорее всего, не будет.",
                        )
                        .changed()
                    {
                        mode_changed = true;
                    }
                })
                .response
                .on_hover_text(
                    "Режим работы гостевого стека прошивки:\n\
                    • Stub (по умолчанию): оригинальный maind отключён; демон qemu-hw-bridge сам поднимает сокеты Quasar IPC (:9875/:9850/:9903), синтезатор речи и окно перехвата (1680 мс) без привязки к облаку Яндекса.\n\
                    • Full (экспериментальный): запускается штатный /system/vendor/quasar/onboot.sh и оригинальный maind (на очищенных образах без токенов авторизации Яндекса полноценно работать, скорее всего, не будет).\n\n\
                    При переключении режима во время работы ВМ виртуальная машина перезапускается автоматически.",
                );

            if mode_changed {
                let _ = self.state.save_config();
                if vm_running {
                    self.state.restart_vm();
                }
            }

            if ui
                .checkbox(&mut self.state.config.clean_boot_snapshot, "Snapshot")
                .on_hover_text(
                    "Режим чистого запуска системного образа QEMU (snapshot):\n\
                    • Включён (по умолчанию): корневой образ work/<board>-system.ext4 защищён временным оверлеем в ОЗУ и не изменяется (но если в «Образы и QEMU...» подключён отдельный диск work/<board>-data.ext4, раздел /data сохраняется между запусками).\n\
                    • Выключен: изменения (включая /data и /bkupgrade при отсутствии отдельного data.ext4) записываются напрямую в work/<board>-system.ext4 и сохраняются между запусками.",
                )
                .changed()
            {
                let _ = self.state.save_config();
            }

            let mut sound_on = !self.state.config.audio_muted;
            if ui
                .checkbox(&mut sound_on, "Звук на ПК")
                .on_hover_text(
                    "Вывод звука колонки на динамики ПК в реальном времени:\n\
                    Транслирует PCM-аудиопоток (48 кГц, 16-бит, стерео), захватываемый с виртуальной звуковой карты snd-aloop внутри гостевой ОС (синтезатор речи, тестовый тон 440 Гц и звуки приложений).",
                )
                .changed()
            {
                self.state.config.audio_muted = !sound_on;
                self.state
                    .audio_player
                    .set_muted(self.state.config.audio_muted);
                let _ = self.state.save_config();
            }
        });
    }
}

impl eframe::App for KuyaseGuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.state.poll_runtime();

        if self.state.session.is_some()
            || self.state.bg_task_running.load(Ordering::Relaxed)
            || self.state.connecting_bridge.load(Ordering::Relaxed)
            || self.state.tof_wave_reset_at.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        egui::TopBottomPanel::top("gnome_headerbar")
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(42, 42, 42))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(62, 62, 62)))
                    .inner_margin(Margin::symmetric(12.0, 8.0)),
            )
            .show(ctx, |ui| {
                self.render_headerbar(ui);
            });

        let collapsed = self.state.config.dock_collapsed;
        let dock_id = if collapsed {
            "gnome_bottom_dock_collapsed"
        } else {
            "gnome_bottom_dock_expanded"
        };

        let mut dock_panel = egui::TopBottomPanel::bottom(dock_id).frame(
            egui::Frame::none()
                .fill(Color32::from_rgb(24, 24, 24))
                .stroke(Stroke::new(1.0, Color32::from_rgb(62, 62, 62)))
                .inner_margin(Margin::same(8.0)),
        );
        if collapsed {
            dock_panel = dock_panel.resizable(false).exact_height(42.0);
        } else {
            let avail_win_h = ctx.screen_rect().height();
            let max_h = (avail_win_h - 180.0).clamp(180.0, 720.0);
            let min_h = 120.0_f32.min(max_h);
            let def_h = 260.0_f32.min(max_h);
            dock_panel = dock_panel
                .resizable(true)
                .default_height(def_h)
                .min_height(min_h)
                .max_height(max_h);
        }
        dock_panel.show(ctx, |ui| {
            terminal::render_bottom_dock(ui, &mut self.state);
            if !collapsed {
                ui.expand_to_include_y(ui.max_rect().bottom());
            }
        });

        egui::SidePanel::left("gnome_device_stage")
            .resizable(true)
            .default_width(440.0)
            .min_width(380.0)
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(30, 30, 30))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(56, 56, 56)))
                    .inner_margin(Margin::same(10.0)),
            )
            .show(ctx, |ui| {
                canvas::render_twin_canvas(ui, &mut self.state);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(30, 30, 30))
                    .inner_margin(Margin::same(12.0)),
            )
            .show(ctx, |ui| {
                docks::render_gnome_action_cards(ui, &mut self.state);
            });

        launcher::render_image_manager_modal(ctx, &mut self.state);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.state.stop_vm();
        let _ = self.state.save_config();
    }
}

fn configure_fonts_and_style(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    let prop_candidates = [
        r"C:\Windows\Fonts\segoeui.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    ];
    for path in prop_candidates {
        if let Ok(bytes) = fs::read(path) {
            fonts
                .font_data
                .insert("sys_proportional".to_owned(), FontData::from_owned(bytes).into());
            if let Some(fam) = fonts.families.get_mut(&FontFamily::Proportional) {
                fam.insert(0, "sys_proportional".to_owned());
            }
            break;
        }
    }

    let mono_candidates = [
        r"C:\Windows\Fonts\consola.ttf",
        r"C:\Windows\Fonts\lucon.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
    ];
    for path in mono_candidates {
        if let Ok(bytes) = fs::read(path) {
            fonts
                .font_data
                .insert("sys_monospace".to_owned(), FontData::from_owned(bytes).into());
            if let Some(fam) = fonts.families.get_mut(&FontFamily::Monospace) {
                fam.insert(0, "sys_monospace".to_owned());
            }
            if let Some(fam) = fonts.families.get_mut(&FontFamily::Proportional) {
                fam.push("sys_monospace".to_owned());
            }
            break;
        }
    }

    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (TextStyle::Heading, FontId::new(17.5, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(14.5, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(13.5, FontFamily::Monospace)),
        (TextStyle::Small, FontId::new(12.5, FontFamily::Proportional)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    ctx.set_style(style);

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(30, 30, 30);
    visuals.window_fill = Color32::from_rgb(40, 40, 40);
    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(43, 43, 43);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(56, 56, 56);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(70, 70, 70);
    visuals.widgets.active.bg_fill = Color32::from_rgb(53, 132, 228);
    ctx.set_visuals(visuals);
}

pub fn run_gui(root_dir: &Path) -> Result<(), String> {
    let root = root_dir.to_path_buf();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("KUYASE — Smart Speaker Hardware & Quasar Stack Emulator")
            .with_inner_size([1480.0, 920.0])
            .with_min_inner_size([960.0, 640.0]),
        ..Default::default()
    };

    eframe::run_native(
        "kuyase",
        options,
        Box::new(move |cc| {
            configure_fonts_and_style(&cc.egui_ctx);
            Ok(Box::new(KuyaseGuiApp::new(&root)))
        }),
    )
    .map_err(|e| format!("GUI error: {}", e))
}
