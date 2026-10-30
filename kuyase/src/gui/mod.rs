pub mod canvas;
pub mod docks;
pub mod host_audio;
pub mod launcher;
pub mod state;
pub mod terminal;

use crate::gui::state::GuiAppState;
use eframe::egui::{self, Color32, RichText};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Launcher,
    TwinCanvas,
    TerminalLogs,
    AudioAmplifiers,
    VoiceIpc,
}

struct WorkspaceBehavior<'a> {
    app: &'a mut GuiAppState,
}

impl<'a> egui_tiles::Behavior<Pane> for WorkspaceBehavior<'a> {
    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        match pane {
            Pane::Launcher => "Board & Image Manager".into(),
            Pane::TwinCanvas => "Virtual Speaker Twin (60 FPS)".into(),
            Pane::TerminalLogs => "VM Terminal (ttyAMA0) & Logs".into(),
            Pane::AudioAmplifiers => "Audio, Waveform & Amplifiers".into(),
            Pane::VoiceIpc => "Voice Injection & Quasar IPC".into(),
        }
    }

    fn pane_ui(
        &mut self,
        ui: &mut egui::Ui,
        _tile_id: egui_tiles::TileId,
        pane: &mut Pane,
    ) -> egui_tiles::UiResponse {
        egui::Frame::none()
            .inner_margin(egui::Margin::same(8.0))
            .show(ui, |ui| match pane {
                Pane::Launcher => launcher::render_launcher_pane(ui, self.app),
                Pane::TwinCanvas => canvas::render_twin_canvas(ui, self.app),
                Pane::TerminalLogs => terminal::render_terminal_pane(ui, self.app),
                Pane::AudioAmplifiers => docks::render_audio_dock(ui, self.app),
                Pane::VoiceIpc => docks::render_voice_ipc_dock(ui, self.app),
            });
        egui_tiles::UiResponse::None
    }
}

pub fn build_tree_for_preset(preset: &str) -> egui_tiles::Tree<Pane> {
    let mut tiles = egui_tiles::Tiles::default();
    if preset == "compact" {
        let canvas = tiles.insert_pane(Pane::TwinCanvas);
        return egui_tiles::Tree::new("kuyase_compact_tree", canvas, tiles);
    }

    let launcher = tiles.insert_pane(Pane::Launcher);
    let canvas = tiles.insert_pane(Pane::TwinCanvas);
    let term = tiles.insert_pane(Pane::TerminalLogs);
    let audio = tiles.insert_pane(Pane::AudioAmplifiers);
    let voice = tiles.insert_pane(Pane::VoiceIpc);

    let center = tiles.insert_vertical_tile(vec![canvas, term]);
    let right = tiles.insert_vertical_tile(vec![audio, voice]);
    let root = tiles.insert_horizontal_tile(vec![launcher, center, right]);
    egui_tiles::Tree::new("kuyase_workbench_tree", root, tiles)
}

pub struct KuyaseGuiApp {
    pub state: GuiAppState,
    pub tree: egui_tiles::Tree<Pane>,
    pub active_preset: String,
}

impl KuyaseGuiApp {
    pub fn new(root_dir: &Path) -> Self {
        let state = GuiAppState::new(root_dir);
        let active_preset = state.config.layout_preset.clone();
        let tree = build_tree_for_preset(&active_preset);
        Self {
            state,
            tree,
            active_preset,
        }
    }

    fn render_top_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("KUYASE")
                    .strong()
                    .color(Color32::from_rgb(110, 210, 255)),
            );
            ui.separator();

            let vm_running = self.state.session.is_some();
            let bridge_online = self
                .state
                .bridge
                .as_ref()
                .map(|b| b.is_connected())
                .unwrap_or(false);

            let (badge_text, badge_color) = if bridge_online {
                (
                    format!(
                        "● ONLINE ({} :{})",
                        self.state.config.selected_board, self.state.config.bridge_port
                    ),
                    Color32::from_rgb(90, 230, 130),
                )
            } else if vm_running {
                (
                    format!("◐ BOOTING ({})...", self.state.config.selected_board),
                    Color32::from_rgb(255, 205, 90),
                )
            } else {
                (
                    format!("○ STOPPED ({})", self.state.config.selected_board),
                    Color32::from_rgb(160, 170, 185),
                )
            };
            ui.label(RichText::new(badge_text).color(badge_color).strong());

            ui.separator();
            let busy = self.state.bg_task_running.load(Ordering::Relaxed);
            if ui
                .add_enabled(!vm_running && !busy, egui::Button::new("▶ Start"))
                .clicked()
            {
                self.state.start_vm();
            }
            if ui
                .add_enabled(vm_running, egui::Button::new("⏹ Stop"))
                .clicked()
            {
                self.state.stop_vm();
            }
            if ui
                .add_enabled(!busy, egui::Button::new("↻ Restart"))
                .clicked()
            {
                self.state.restart_vm();
            }

            ui.separator();
            if ui
                .checkbox(&mut self.state.config.clean_boot_snapshot, "Clean Boot (-snapshot)")
                .changed()
            {
                let _ = self.state.save_config();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let is_workbench = self.state.config.layout_preset != "compact";
                if ui
                    .selectable_label(is_workbench, "Full Workbench")
                    .clicked()
                {
                    self.state.set_layout_preset("workbench");
                }
                if ui
                    .selectable_label(!is_workbench, "Compact Remote")
                    .clicked()
                {
                    self.state.set_layout_preset("compact");
                }
                ui.label("Layout:");
            });
        });
    }
}

impl eframe::App for KuyaseGuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.state.poll_runtime();

        if self.state.config.layout_preset != self.active_preset {
            self.active_preset = self.state.config.layout_preset.clone();
            self.tree = build_tree_for_preset(&self.active_preset);
        }

        if self.state.session.is_some()
            || self.state.bg_task_running.load(Ordering::Relaxed)
            || self.state.connecting_bridge.load(Ordering::Relaxed)
        {
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        egui::TopBottomPanel::top("kuyase_top_toolbar").show(ctx, |ui| {
            self.render_top_toolbar(ui);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let mut behavior = WorkspaceBehavior {
                app: &mut self.state,
            };
            self.tree.ui(&mut behavior, ui);
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.state.stop_vm();
        let _ = self.state.save_config();
    }
}

pub fn run_gui(root_dir: &Path) -> Result<(), String> {
    let root = root_dir.to_path_buf();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("KUYASE — Smart Speaker Hardware & Quasar Stack Emulator")
            .with_inner_size([1380.0, 860.0])
            .with_min_inner_size([640.0, 480.0]),
        ..Default::default()
    };

    eframe::run_native(
        "kuyase",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(KuyaseGuiApp::new(&root)))
        }),
    )
    .map_err(|e| format!("GUI error: {}", e))
}
