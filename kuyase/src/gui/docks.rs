use crate::gui::state::GuiAppState;
use eframe::egui::{self, Color32, Pos2, RichText, Stroke, Vec2};
use protocol::HostCommand;
use std::time::{Duration, Instant};

pub const HIJACK_WINDOW_MS: u64 = 1680;

pub fn compute_hijack_window_remaining_ms(last_inject_at: Option<Instant>, now: Instant) -> u64 {
    let Some(start) = last_inject_at else {
        return 0;
    };
    let elapsed_ms = now.saturating_duration_since(start).as_millis() as u64;
    HIJACK_WINDOW_MS.saturating_sub(elapsed_ms)
}

pub fn render_audio_dock(ui: &mut egui::Ui, app: &mut GuiAppState) {
    egui::ScrollArea::vertical()
        .id_salt("audio_dock_scroll")
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Audio, Waveform & Amplifiers");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (tag, col) = if app.audio_player.is_muted() {
                        ("HOST MUTED", Color32::from_rgb(240, 175, 75))
                    } else if app.audio_player.is_backend_active() {
                        ("48 kHz STEREO LIVE", Color32::from_rgb(85, 225, 145))
                    } else {
                        ("HEADLESS CAPTURE", Color32::from_rgb(145, 165, 190))
                    };
                    ui.label(RichText::new(tag).small().strong().color(col));
                });
            });
            ui.separator();

            ui.horizontal_wrapped(|ui| {
                let mut muted = app.config.audio_muted;
                if ui.checkbox(&mut muted, "Mute Host Audio").changed() {
                    app.config.audio_muted = muted;
                    app.audio_player.set_muted(muted);
                    let _ = app.save_config();
                }
                if ui.button("Play 440 Hz Test Tone").clicked() {
                    app.send_command(HostCommand::PlayTestTone);
                }
                if ui.button("Clear Audio Buffer").clicked() {
                    app.send_command(HostCommand::ClearAudio);
                    app.audio_player.clear_waveform();
                    if let Some(ref b) = app.bridge {
                        b.clear_captured_pcm();
                    }
                    app.status_message = "Cleared guest and host PCM audio buffers.".to_string();
                }
                if ui.button("Export Captured .WAV").clicked() {
                    let board = app
                        .latest_state
                        .as_ref()
                        .map(|s| s.board_slug.clone())
                        .unwrap_or_else(|| app.config.selected_board.clone());
                    if app.audio_player.captured_bytes_len() == 0 {
                        if let Some(ref b) = app.bridge {
                            let fallback = b.captured_pcm_bytes();
                            if !fallback.is_empty() {
                                app.audio_player.feed_pcm_chunk(&fallback);
                            }
                        }
                    }
                    let wav_path = app
                        .root_dir
                        .join("work")
                        .join(format!("{}-captured.wav", board));
                    match app.audio_player.export_wav(&wav_path) {
                        Ok(bytes) => {
                            app.status_message = format!(
                                "Exported {} bytes to {}",
                                bytes,
                                wav_path.display()
                            );
                        }
                        Err(e) => {
                            app.status_message = format!("WAV export failed: {}", e);
                        }
                    }
                }
            });

            ui.add_space(4.0);
            render_oscilloscope_canvas(ui, app);
            ui.add_space(6.0);

            let (spk_rms, mic_rms, mic_burst, mics_muted, guest_cap_bytes) =
                if let Some(ref st) = app.latest_state {
                    (
                        st.speaker_rms.max(app.audio_player.live_rms()),
                        st.mic_rms,
                        st.mic_burst_active,
                        st.mics_muted,
                        st.captured_audio_bytes.max(app.audio_player.captured_bytes_len()),
                    )
                } else {
                    (
                        app.audio_player.live_rms(),
                        0,
                        false,
                        false,
                        app.audio_player.captured_bytes_len(),
                    )
                };

            egui::Grid::new("audio_rms_bars_grid")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label("Speaker RMS:");
                    let spk_frac = (spk_rms as f32 / 12_000.0).clamp(0.0, 1.0);
                    ui.add(
                        egui::ProgressBar::new(spk_frac)
                            .text(format!("{} (Peak {:.2})", spk_rms, app.audio_player.live_peak()))
                            .fill(Color32::from_rgb(60, 195, 140)),
                    );
                    ui.end_row();

                    ui.label("Mic Array RMS:");
                    let mic_frac = (mic_rms as f32 / 4_000.0).clamp(0.0, 1.0);
                    let mic_label = if mics_muted {
                        format!("{} [MICS MUTED]", mic_rms)
                    } else if mic_burst {
                        format!("{} [VOICE BURST]", mic_rms)
                    } else {
                        format!("{} [IDLE]", mic_rms)
                    };
                    let mic_color = if mics_muted {
                        Color32::from_rgb(215, 65, 65)
                    } else if mic_burst {
                        Color32::from_rgb(75, 185, 250)
                    } else {
                        Color32::from_rgb(90, 125, 165)
                    };
                    ui.add(egui::ProgressBar::new(mic_frac).text(mic_label).fill(mic_color));
                    ui.end_row();
                });

            ui.label(
                RichText::new(format!(
                    "Captured PCM Buffer: {} bytes ({:.1} s @ 48 kHz S16_LE 2ch)",
                    guest_cap_bytes,
                    guest_cap_bytes as f32 / (48_000.0 * 4.0)
                ))
                .small()
                .color(Color32::from_rgb(165, 180, 200)),
            );

            if let Some(ref st) = app.latest_state {
                if !st.amplifiers.is_empty() || st.board_slug == "orion" {
                    ui.add_space(6.0);
                    ui.separator();
                    ui.label(
                        RichText::new("Orion 4× TI ACM8625S I2C Amplifiers")
                            .strong()
                            .color(Color32::from_rgb(130, 210, 255)),
                    );
                    egui::Grid::new("orion_amplifiers_table")
                        .striped(true)
                        .num_columns(6)
                        .spacing([10.0, 4.0])
                        .show(ui, |ui| {
                            for hdr in ["I2C", "Role", "ALSA Prefix", "Settings File", "Gain / Ramp", "State"] {
                                ui.label(RichText::new(hdr).small().strong());
                            }
                            ui.end_row();

                            for amp in &st.amplifiers {
                                ui.monospace(&amp.i2c_device);
                                ui.label(RichText::new(&amp.role).strong());
                                ui.monospace(&amp.alsa_prefix);
                                ui.monospace(&amp.settings_file);
                                ui.monospace(format!("{:+.1} dB / {:+.1} dB", amp.gain_db, amp.ramp_db));
                                let (st_txt, st_col) = if amp.muted {
                                    ("MUTED", Color32::from_rgb(245, 95, 95))
                                } else {
                                    ("ACTIVE", Color32::from_rgb(85, 225, 140))
                                };
                                ui.label(RichText::new(st_txt).small().strong().color(st_col));
                                ui.end_row();
                            }
                        });
                }

                if let Some(ref us) = st.ultrasound {
                    ui.add_space(6.0);
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Ultrasound TDM-A (pcm.ultrasound)")
                                .strong()
                                .color(Color32::from_rgb(190, 155, 255)),
                        );
                        let badge = if us.active { "ACTIVE" } else { "STANDBY" };
                        let col = if us.active {
                            Color32::from_rgb(110, 235, 175)
                        } else {
                            Color32::from_rgb(155, 165, 180)
                        };
                        ui.label(RichText::new(badge).small().strong().color(col));
                    });
                    ui.label(
                        RichText::new(format!(
                            "Device: {} | Rate: {} Hz | Carrier: {} Hz | BW: {} Hz | Ch: {}",
                            us.tdm_device,
                            us.sample_rate_hz,
                            us.carrier_freq_hz,
                            us.bandwidth_hz,
                            us.channels
                        ))
                        .small()
                        .monospace(),
                    );
                    let us_frac = (us.rms as f32 / 2_000.0).clamp(0.0, 1.0);
                    ui.add(
                        egui::ProgressBar::new(us_frac)
                            .text(format!("Ultrasound RMS: {}", us.rms))
                            .fill(Color32::from_rgb(150, 110, 240)),
                    );
                }
            }
        });
}

fn render_oscilloscope_canvas(ui: &mut egui::Ui, app: &GuiAppState) {
    let desired = Vec2::new(ui.available_width().max(220.0), 92.0);
    let (rect, _resp) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 4.0, Color32::from_rgb(13, 17, 23));
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0, Color32::from_rgb(42, 54, 72)),
    );

    let mid_y = rect.center().y;
    let q1_y = rect.top() + rect.height() * 0.25;
    let q3_y = rect.top() + rect.height() * 0.75;
    let grid_stroke = Stroke::new(1.0, Color32::from_rgb(26, 35, 48));
    painter.line_segment([Pos2::new(rect.left(), q1_y), Pos2::new(rect.right(), q1_y)], grid_stroke);
    painter.line_segment([Pos2::new(rect.left(), q3_y), Pos2::new(rect.right(), q3_y)], grid_stroke);
    painter.line_segment(
        [Pos2::new(rect.left(), mid_y), Pos2::new(rect.right(), mid_y)],
        Stroke::new(1.0, Color32::from_rgb(48, 66, 90)),
    );

    let samples = app.audio_player.recent_waveform(256);
    if samples.len() >= 2 {
        let n = samples.len();
        let w = rect.width() - 8.0;
        let half_h = (rect.height() - 12.0) * 0.5;
        let pts: Vec<Pos2> = samples
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                let x = rect.left() + 4.0 + (i as f32 / (n - 1) as f32) * w;
                let y = mid_y - (s.clamp(-1.0, 1.0) * half_h);
                Pos2::new(x, y)
            })
            .collect();
        painter.add(egui::Shape::line(
            pts,
            Stroke::new(1.6, Color32::from_rgb(75, 235, 175)),
        ));
    } else {
        painter.line_segment(
            [
                Pos2::new(rect.left() + 4.0, mid_y),
                Pos2::new(rect.right() - 4.0, mid_y),
            ],
            Stroke::new(1.2, Color32::from_rgb(55, 120, 110)),
        );
    }

    painter.text(
        Pos2::new(rect.left() + 6.0, rect.top() + 5.0),
        egui::Align2::LEFT_TOP,
        "PCM Oscilloscope (snd-aloop 48kHz S16_LE)",
        egui::FontId::monospace(10.5),
        Color32::from_rgb(135, 160, 185),
    );
}

pub fn render_voice_ipc_dock(ui: &mut egui::Ui, app: &mut GuiAppState) {
    egui::ScrollArea::vertical()
        .id_salt("voice_ipc_dock_scroll")
        .show(ui, |ui| {
            let (alice_state, hijacks, says, ipc_errs, last_spoken) =
                if let Some(ref st) = app.latest_state {
                    (
                        st.alice_state.as_str(),
                        st.aliced_hijack_count,
                        st.aliced_say_count,
                        st.ipc_error_count,
                        st.last_spoken_text.as_str(),
                    )
                } else {
                    ("IDLE", 0, 0, 0, "")
                };

            ui.horizontal(|ui| {
                ui.heading("Voice Injection & Quasar IPC");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (badge_col, badge_bg) = match alice_state {
                        "LISTENING" => (
                            Color32::from_rgb(80, 215, 255),
                            Color32::from_rgb(18, 52, 74),
                        ),
                        "SPEAKING" => (
                            Color32::from_rgb(205, 145, 255),
                            Color32::from_rgb(52, 26, 78),
                        ),
                        _ => (
                            Color32::from_rgb(165, 175, 190),
                            Color32::from_rgb(34, 40, 50),
                        ),
                    };
                    egui::Frame::none()
                        .fill(badge_bg)
                        .rounding(4.0)
                        .inner_margin(egui::Margin::symmetric(8.0, 3.0))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(format!("AliceState: {}", alice_state))
                                    .strong()
                                    .color(badge_col),
                            );
                        });
                });
            });
            ui.separator();

            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("Hijacks: {}", hijacks))
                        .strong()
                        .color(Color32::from_rgb(100, 215, 255)),
                );
                ui.separator();
                ui.label(
                    RichText::new(format!("Say / TTS: {}", says))
                        .strong()
                        .color(Color32::from_rgb(185, 145, 255)),
                );
                ui.separator();
                let err_col = if ipc_errs == 0 {
                    Color32::from_rgb(85, 225, 140)
                } else {
                    Color32::from_rgb(255, 95, 95)
                };
                ui.label(RichText::new(format!("IPC Errors: {}", ipc_errs)).strong().color(err_col));
            });

            let tts_text = if last_spoken.is_empty() {
                "— (no speech synthesized yet)"
            } else {
                last_spoken
            };
            ui.label(
                RichText::new(format!("Last Spoken Text: {}", tts_text))
                    .small()
                    .color(Color32::from_rgb(195, 210, 230)),
            );

            ui.add_space(4.0);
            let rem_ms = compute_hijack_window_remaining_ms(app.last_voice_inject_at, Instant::now());
            let hijack_active = rem_ms > 0 && alice_state == "LISTENING";
            if hijack_active {
                ui.ctx().request_repaint_after(Duration::from_millis(16));
                egui::Frame::none()
                    .fill(Color32::from_rgb(48, 34, 14))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(245, 175, 55)))
                    .rounding(4.0)
                    .inner_margin(egui::Margin::same(6.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let frac = (rem_ms as f32 / HIJACK_WINDOW_MS as f32).clamp(0.0, 1.0);
                            ui.add(
                                egui::ProgressBar::new(frac)
                                    .desired_width(180.0)
                                    .text(format!("Hijack Window: {} ms left", rem_ms))
                                    .fill(Color32::from_rgb(235, 155, 35)),
                            );
                            if ui
                                .button(
                                    RichText::new("HIJACK NOW (Cancel Dialog)")
                                        .strong()
                                        .color(Color32::from_rgb(255, 225, 130)),
                                )
                                .clicked()
                            {
                                app.send_command(HostCommand::IpcTest {
                                    cmd: "cancel_dialog".to_string(),
                                    text: None,
                                });
                            }
                        });
                    });
            } else {
                ui.label(
                    RichText::new("1680 ms Hijack Window: Ready (triggers on Voice Injection)")
                        .small()
                        .color(Color32::from_rgb(130, 145, 165)),
                );
            }

            ui.add_space(6.0);
            ui.separator();
            ui.label(RichText::new("Voice Injection (:9850 yio_audio & maind)").strong());

            ui.horizontal(|ui| {
                ui.label("Phrase:");
                ui.add(
                    egui::TextEdit::singleline(&mut app.voice_phrase)
                        .desired_width(ui.available_width().max(120.0)),
                );
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Presets:").small());
                for preset in ["включи ночной режим", "громкость 3", "какая погода"] {
                    if ui.small_button(preset).clicked() {
                        app.voice_phrase = preset.to_string();
                    }
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("DOA:");
                ui.add(egui::Slider::new(&mut app.voice_doa, 0..=359).suffix("°"));
                ui.label("Mic RMS:");
                ui.add(egui::Slider::new(&mut app.voice_rms, 100..=6000));
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut app.voice_wake_only, "Wake-word only");
                if ui.button(RichText::new("Inject Voice").strong()).clicked() {
                    app.last_voice_inject_at = Some(Instant::now());
                    let cmd = HostCommand::InjectVoice {
                        phrase: app.voice_phrase.clone(),
                        doa_angle: Some(app.voice_doa),
                        rms: Some(app.voice_rms),
                        wake_only: app.voice_wake_only,
                    };
                    app.send_command(cmd);
                }
            });

            ui.add_space(6.0);
            ui.separator();
            ui.label(RichText::new("Quasar IPC (aliced :9875 Remoting)").strong());
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button("CANCEL_DIALOG_AND_CLEAR_QUEUE (6)")
                    .clicked()
                {
                    app.send_command(HostCommand::IpcTest {
                        cmd: "cancel_dialog".to_string(),
                        text: None,
                    });
                }
                if ui.button("STOP_TTS (14)").clicked() {
                    app.send_command(HostCommand::IpcTest {
                        cmd: "stop_tts".to_string(),
                        text: None,
                    });
                }
            });
            ui.horizontal(|ui| {
                ui.label("TTS Text:");
                ui.add(
                    egui::TextEdit::singleline(&mut app.ipc_vocalize_text)
                        .desired_width(ui.available_width().max(120.0)),
                );
            });
            if ui
                .button("START_VOCALIZE (18) — Formant TTS")
                .clicked()
            {
                let text = app.ipc_vocalize_text.clone();
                app.send_command(HostCommand::IpcTest {
                    cmd: "vocalize".to_string(),
                    text: Some(text),
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hijack_window_countdown_calculation() {
        let now = Instant::now();
        assert_eq!(compute_hijack_window_remaining_ms(None, now), 0);
        assert_eq!(compute_hijack_window_remaining_ms(Some(now), now), 1680);

        let after_500 = now + Duration::from_millis(500);
        assert_eq!(compute_hijack_window_remaining_ms(Some(now), after_500), 1180);

        let after_1680 = now + Duration::from_millis(1680);
        assert_eq!(compute_hijack_window_remaining_ms(Some(now), after_1680), 0);

        let after_2500 = now + Duration::from_millis(2500);
        assert_eq!(compute_hijack_window_remaining_ms(Some(now), after_2500), 0);
    }
}
