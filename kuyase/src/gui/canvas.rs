use crate::gui::state::GuiAppState;
use eframe::egui;

pub fn render_twin_canvas(ui: &mut egui::Ui, app: &mut GuiAppState) {
    ui.vertical_centered(|ui| {
        ui.add_space(12.0);
        let board_title = app
            .latest_state
            .as_ref()
            .map(|s| format!("{} ({})", s.board_name, s.board_slug))
            .unwrap_or_else(|| format!("Board Profile: {}", app.config.selected_board));
        ui.heading(board_title);
        ui.add_space(6.0);

        if let Some(ref st) = app.latest_state {
            ui.label(format!(
                "LED Ring: {} LEDs | Trigger: {} | Conflicts: {} | Display: {} | Aux: {}",
                st.ring_leds,
                st.ring_trigger,
                st.led_trigger_conflict_count,
                st.display_kind,
                st.aux_light_kind
            ));
            ui.label(format!(
                "Alice: {} | Volume: {}/10 | Mics Muted: {} | DOA: {}° | ToF: {} mm | ALS: {} lux | Temp: {:.1} °C",
                st.alice_state,
                st.volume_level,
                st.mics_muted,
                st.doa_angle,
                st.tof_mm,
                st.als_lux,
                st.temp_c
            ));
        } else {
            ui.label("Virtual Speaker Twin canvas ready. Start the VM to stream 60 FPS hardware telemetry.");
        }
    });
}
