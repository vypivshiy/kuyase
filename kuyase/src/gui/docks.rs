use crate::gui::state::GuiAppState;
use eframe::egui;
use protocol::HostCommand;

pub fn render_audio_dock(ui: &mut egui::Ui, app: &mut GuiAppState) {
    ui.heading("Audio, Waveform & Amplifiers");
    ui.separator();
    let mut muted = app.config.audio_muted;
    if ui.checkbox(&mut muted, "Mute Host Audio Output").changed() {
        app.config.audio_muted = muted;
        app.audio_player.set_muted(muted);
        let _ = app.save_config();
    }
    if let Some(ref st) = app.latest_state {
        ui.label(format!(
            "Speaker RMS: {} | Mic RMS: {} | Captured PCM: {} bytes",
            st.speaker_rms, st.mic_rms, st.captured_audio_bytes
        ));
    }
    ui.horizontal(|ui| {
        if ui.button("Play 440 Hz Tone").clicked() {
            app.send_command(HostCommand::PlayTestTone);
        }
        if ui.button("Clear Audio Buffer").clicked() {
            app.send_command(HostCommand::ClearAudio);
            app.audio_player.clear_waveform();
        }
    });
}

pub fn render_voice_ipc_dock(ui: &mut egui::Ui, app: &mut GuiAppState) {
    ui.heading("Voice Injection & Quasar IPC");
    ui.separator();
    if let Some(ref st) = app.latest_state {
        ui.label(format!(
            "AliceState: {} | Hijacks: {} | Say: {} | IPC Errors: {}",
            st.alice_state, st.aliced_hijack_count, st.aliced_say_count, st.ipc_error_count
        ));
        if !st.last_spoken_text.is_empty() {
            ui.label(format!("Last TTS: {}", st.last_spoken_text));
        }
    } else {
        ui.label("Connect VM bridge to inspect Quasar IPC (:9875, :9850, :9903).");
    }
}
