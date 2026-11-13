use super::{gnome_action_row, gnome_preferences_group, render_mini_oscilloscope};
use crate::gui::state::GuiAppState;
use eframe::egui::{self, Color32, RichText};
use protocol::HostCommand;
use std::time::{Duration, Instant};

const MIC_SAMPLE_RATES: &[(u32, &str)] = &[
    (8_000, "8000 Гц"),
    (16_000, "16000 Гц (Luna)"),
    (24_000, "24000 Гц"),
    (32_000, "32000 Гц"),
    (48_000, "48000 Гц"),
];

const CAPTURE_SAMPLE_RATES: &[(u32, &str)] = &[
    (16_000, "16000 Гц"),
    (24_000, "24000 Гц"),
    (32_000, "32000 Гц"),
    (44_100, "44100 Гц"),
    (48_000, "48000 Гц (По умолч.)"),
];

pub fn render_tts_audio_voice_card(ui: &mut egui::Ui, app: &mut GuiAppState) {
    let (spk_rms, guest_cap_bytes, last_spoken, hijacks, says) =
        if let Some(ref st) = app.latest_state {
            (
                st.speaker_rms.max(app.audio_player.live_rms()),
                st.captured_audio_bytes.max(app.audio_player.captured_bytes_len()),
                st.last_spoken_text.clone(),
                st.aliced_hijack_count,
                st.aliced_say_count,
            )
        } else {
            (
                app.audio_player.live_rms(),
                app.audio_player.captured_bytes_len(),
                String::new(),
                0,
                0,
            )
        };

    let cap_rate = app.config.audio_config.capture_sample_rate.max(8_000);
    let hijack_window_ms = app.config.audio_config.hijack_window_ms as u64;

    gnome_preferences_group(
        ui,
        "1. Кастомный TTS, Прослушивание звука и Голосовая команда",
        "Синтез произвольной речи на колонке (:9875), живая осциллограмма потока и отправка фраз в микрофоны (:9850)",
        |ui| {
            let tts_sub = if last_spoken.is_empty() {
                format!(
                    "START_VOCALIZE (18) в aliced (:9875) -> поток {} кГц на динамики ПК | Озвучено: {}",
                    cap_rate / 1000,
                    says
                )
            } else {
                format!("Последняя фраза: \"{}\" | Озвучено: {}", last_spoken, says)
            };
            gnome_action_row(
                ui,
                "Озвучить кастомный текст (TTS)",
                &tts_sub,
                false,
                |ui| {
                    let text_w = (ui.available_width() * 0.42).clamp(170.0, 320.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut app.ipc_vocalize_text)
                            .desired_width(text_w)
                            .hint_text("Введите текст для озвучки..."),
                    );
                    let tts_btn = egui::Button::new(
                        RichText::new("Озвучить и Прослушать")
                            .strong()
                            .color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(53, 132, 228));
                    if ui.add(tts_btn).clicked() {
                        if app.config.audio_muted {
                            app.config.audio_muted = false;
                            app.audio_player.set_muted(false);
                            let _ = app.save_config();
                        }
                        let text = app.ipc_vocalize_text.clone();
                        app.send_command(HostCommand::IpcTest {
                            cmd: "vocalize".to_string(),
                            text: Some(text),
                        });
                    }
                    if ui.button("Стоп TTS").clicked() {
                        app.send_command(HostCommand::IpcTest {
                            cmd: "stop_tts".to_string(),
                            text: None,
                        });
                    }
                    for (label, phrase) in [
                        (
                            "Погода",
                            "Привет! Сегодня в Москве плюс 21 градус, без осадков.",
                        ),
                        (
                            "Панграмма",
                            "Съешь этих мягких французских булок да выпей чаю",
                        ),
                    ] {
                        if ui.button(label).on_hover_text(phrase).clicked() {
                            app.ipc_vocalize_text = phrase.to_string();
                        }
                    }
                },
            );

            let audio_sub = format!(
                "Поток S16_LE 2 канала ({} Гц, чанк {} мс) | RMS: {} | Захвачено: {:.1} КБ ({:.1} с)",
                cap_rate,
                app.config.audio_config.capture_chunk_ms,
                spk_rms,
                guest_cap_bytes as f64 / 1024.0,
                guest_cap_bytes as f64 / ((cap_rate as f64) * 4.0)
            );
            let audio_title = format!("Звуковой выход колонки (snd-aloop {} кГц)", cap_rate / 1000);
            gnome_action_row(
                ui,
                &audio_title,
                &audio_sub,
                false,
                |ui| {
                    render_mini_oscilloscope(ui, app);
                    if ui.button("Тон 440 Гц").clicked() {
                        app.send_command(HostCommand::PlayTestTone);
                    }
                    if ui.button("Очистить").clicked() {
                        app.send_command(HostCommand::ClearAudio);
                        app.audio_player.clear_waveform();
                    }
                    if ui.button("Сохранить .WAV").clicked() {
                        let board = app.active_board_slug();
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
                                    "Сохранён файл {} ({} байт)",
                                    wav_path.display(),
                                    bytes
                                );
                            }
                            Err(e) => {
                                app.status_message = format!("Ошибка сохранения WAV: {}", e);
                            }
                        }
                    }
                },
            );

            let rem_ms = app
                .last_voice_inject_at
                .map(|start| {
                    let elapsed = Instant::now().saturating_duration_since(start).as_millis() as u64;
                    hijack_window_ms.saturating_sub(elapsed)
                })
                .unwrap_or(0);
            if rem_ms > 0 {
                ui.ctx().request_repaint_after(Duration::from_millis(16));
            }
            let voice_sub = if rem_ms > 0 {
                format!(
                    "Окно перехвата активно: осталось {} мс | Перехватов: {}",
                    rem_ms, hijacks
                )
            } else {
                format!(
                    "Инжекция фразы в yio_audio (:9850, {} Гц / {} мс) и окно перехвата {} мс | Перехватов: {}",
                    app.config.audio_config.mic_sample_rate,
                    app.config.audio_config.mic_frame_ms,
                    hijack_window_ms,
                    hijacks
                )
            };
            gnome_action_row(
                ui,
                "Сказать фразу Алисе (Voice Injection)",
                &voice_sub,
                false,
                |ui| {
                    let voice_w = (ui.available_width() * 0.30).clamp(140.0, 240.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut app.voice_phrase)
                            .desired_width(voice_w)
                            .hint_text("Фраза для Алисы..."),
                    );
                    let send_btn = egui::Button::new(
                        RichText::new("Послать фразу")
                            .strong()
                            .color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(53, 132, 228));
                    if ui.add(send_btn).clicked() {
                        let cmd = HostCommand::InjectVoice {
                            phrase: app.voice_phrase.clone(),
                            doa_angle: Some(app.doa_slider),
                            rms: Some(app.voice_rms),
                            wake_only: app.voice_wake_only,
                        };
                        app.send_command(cmd);
                    }
                    let hijack_lbl = if rem_ms > 0 {
                        format!("Перехватить ({} мс)", rem_ms)
                    } else {
                        format!("Перехватить ({} мс)", hijack_window_ms)
                    };
                    if ui.button(hijack_lbl).clicked() {
                        app.send_command(HostCommand::IpcTest {
                            cmd: "cancel_dialog".to_string(),
                            text: None,
                        });
                    }
                    if ui
                        .add(
                            egui::Slider::new(&mut app.doa_slider, 0..=359)
                                .text("DOA")
                                .suffix("°"),
                        )
                        .changed()
                    {
                        app.send_command(HostCommand::SetSensor {
                            doa_angle: Some(app.doa_slider),
                            mic_rms: None,
                            als_lux: None,
                            temp_c: None,
                            tof_mm: None,
                            swipe: None,
                            swipe_direction: None,
                        });
                    }
                    ui.add(egui::Slider::new(&mut app.voice_rms, 100..=28_000).text("RMS"));
                    ui.checkbox(&mut app.voice_wake_only, "Только Wake");
                    for preset in ["включи ночной режим", "громкость 3", "какая погода"] {
                        if ui.button(preset).clicked() {
                            app.voice_phrase = preset.to_string();
                        }
                    }
                },
            );

            render_advanced_audio_settings(ui, app);
        },
    );
}

fn slider_committed(resp: &egui::Response) -> bool {
    resp.drag_stopped() || (resp.changed() && !resp.dragged())
}

fn render_advanced_audio_settings(ui: &mut egui::Ui, app: &mut GuiAppState) {
    let mic_samples = app.config.audio_config.mic_frame_samples();
    let header_title = format!(
        "Тонкая настройка микрофона (:9850, {} Гц / {} сэмплов), таймингов и передачи аудио",
        app.config.audio_config.mic_sample_rate, mic_samples
    );
    egui::CollapsingHeader::new(RichText::new(header_title).strong())
        .id_salt("audio_config_collapsing")
        .default_open(false)
        .show(ui, |ui| {
            let mut changed = false;

            ui.add_space(4.0);
            ui.label(
                RichText::new("Виртуальная микрофонная решётка (yio_audio :9850)")
                    .strong()
                    .color(Color32::from_rgb(210, 220, 235)),
            );
            ui.horizontal_wrapped(|ui| {
                let cur_mic_rate = app.config.audio_config.mic_sample_rate;
                let mic_rate_label = MIC_SAMPLE_RATES
                    .iter()
                    .find(|&&(r, _)| r == cur_mic_rate)
                    .map(|&(_, l)| l.to_string())
                    .unwrap_or_else(|| format!("{} Гц", cur_mic_rate));
                egui::ComboBox::from_id_salt("mic_sample_rate_combo")
                    .selected_text(format!("Частота: {}", mic_rate_label))
                    .width(175.0)
                    .show_ui(ui, |ui| {
                        for &(rate, label) in MIC_SAMPLE_RATES {
                            if ui.selectable_label(cur_mic_rate == rate, label).clicked() {
                                app.config.audio_config.mic_sample_rate = rate;
                                changed = true;
                            }
                        }
                    });

                let frame_resp = ui.add(
                    egui::Slider::new(&mut app.config.audio_config.mic_frame_ms, 5..=200)
                        .text("Кадр")
                        .suffix(" мс"),
                );
                if frame_resp.changed() && app.config.mic_lock_interval_to_frame {
                    app.config.audio_config.mic_send_interval_ms =
                        app.config.audio_config.mic_frame_ms;
                }
                if slider_committed(&frame_resp) {
                    changed = true;
                }
                ui.label(
                    RichText::new(format!(
                        "({} сэмплов)",
                        app.config.audio_config.mic_frame_samples()
                    ))
                    .small()
                    .color(Color32::from_rgb(160, 165, 175)),
                );

                if ui
                    .button("40 мс (640, Luna)")
                    .on_hover_text("Стандарт PR #9 для экосистемы Luna: 16 кГц, 40 мс (640 сэмплов)")
                    .clicked()
                {
                    app.config.audio_config.mic_sample_rate = protocol::DEFAULT_MIC_SAMPLE_RATE;
                    app.config.audio_config.mic_frame_ms = protocol::DEFAULT_MIC_FRAME_MS;
                    app.config.audio_config.mic_send_interval_ms =
                        protocol::DEFAULT_MIC_SEND_INTERVAL_MS;
                    app.config.mic_lock_interval_to_frame = true;
                    app.config.audio_config.mic_lag_cap = protocol::DEFAULT_MIC_LAG_CAP;
                    changed = true;
                }
                if ui
                    .button("16 мс / 40 мс (256, Legacy)")
                    .on_hover_text("До PR #9: 256 сэмплов (16 мс при 16 кГц) с отправкой раз в 40 мс")
                    .clicked()
                {
                    app.config.audio_config.mic_sample_rate = protocol::DEFAULT_MIC_SAMPLE_RATE;
                    app.config.audio_config.mic_frame_ms = 16;
                    app.config.audio_config.mic_send_interval_ms =
                        protocol::DEFAULT_MIC_SEND_INTERVAL_MS;
                    app.config.mic_lock_interval_to_frame = false;
                    changed = true;
                }
            });

            ui.horizontal_wrapped(|ui| {
                if ui
                    .checkbox(
                        &mut app.config.mic_lock_interval_to_frame,
                        "Интервал = кадру (Real-time)",
                    )
                    .changed()
                {
                    if app.config.mic_lock_interval_to_frame {
                        app.config.audio_config.mic_send_interval_ms =
                            app.config.audio_config.mic_frame_ms;
                    }
                    changed = true;
                }
                let int_resp = ui.add_enabled(
                    !app.config.mic_lock_interval_to_frame,
                    egui::Slider::new(
                        &mut app.config.audio_config.mic_send_interval_ms,
                        5..=500,
                    )
                    .text("Интервал отправки")
                    .suffix(" мс"),
                );
                if slider_committed(&int_resp) {
                    changed = true;
                }
                if ui
                    .checkbox(&mut app.config.audio_config.mic_lag_cap, "Lag Cap")
                    .on_hover_text("Сбрасывать дедлайн при отставании свыше 1 кадра (исключает залповую досылку пакетов)")
                    .changed()
                {
                    changed = true;
                }
                let idle_resp = ui.add(
                    egui::Slider::new(&mut app.config.audio_config.mic_idle_rms, 0..=10_000)
                        .text("Фоновый RMS"),
                );
                if slider_committed(&idle_resp) {
                    changed = true;
                }
                let burst_resp = ui.add(
                    egui::Slider::new(&mut app.config.audio_config.mic_burst_rms, 100..=28_000)
                        .text("Бурст RMS"),
                );
                if burst_resp.changed() {
                    app.voice_rms = app.config.audio_config.mic_burst_rms;
                }
                if slider_committed(&burst_resp) {
                    changed = true;
                }
            });

            ui.add_space(4.0);
            ui.separator();
            ui.label(
                RichText::new("Тайминги голосовой инжекции и окна перехвата (aliced :9875)")
                    .strong()
                    .color(Color32::from_rgb(210, 220, 235)),
            );
            ui.horizontal_wrapped(|ui| {
                let rec_resp = ui.add(
                    egui::Slider::new(
                        &mut app.config.audio_config.recognition_delay_ms,
                        0..=5_000,
                    )
                    .text("Задержка распознавания")
                    .suffix(" мс"),
                );
                if slider_committed(&rec_resp) {
                    changed = true;
                }
                let bdur_resp = ui.add(
                    egui::Slider::new(
                        &mut app.config.audio_config.burst_duration_ms,
                        50..=10_000,
                    )
                    .text("Длительность бурста")
                    .suffix(" мс"),
                );
                if slider_committed(&bdur_resp) {
                    changed = true;
                }
                let hj_resp = ui.add(
                    egui::Slider::new(
                        &mut app.config.audio_config.hijack_window_ms,
                        100..=15_000,
                    )
                    .text("Hijack Window")
                    .suffix(" мс"),
                );
                if slider_committed(&hj_resp) {
                    changed = true;
                }
            });

            ui.add_space(4.0);
            ui.separator();
            ui.label(
                RichText::new("Захват и передача звука колонки (snd-aloop -> Хост)")
                    .strong()
                    .color(Color32::from_rgb(210, 220, 235)),
            );
            ui.horizontal_wrapped(|ui| {
                let cur_cap_rate = app.config.audio_config.capture_sample_rate;
                let cap_rate_label = CAPTURE_SAMPLE_RATES
                    .iter()
                    .find(|&&(r, _)| r == cur_cap_rate)
                    .map(|&(_, l)| l.to_string())
                    .unwrap_or_else(|| format!("{} Гц", cur_cap_rate));
                egui::ComboBox::from_id_salt("capture_sample_rate_combo")
                    .selected_text(format!("Частота aloop: {}", cap_rate_label))
                    .width(205.0)
                    .show_ui(ui, |ui| {
                        for &(rate, label) in CAPTURE_SAMPLE_RATES {
                            if ui.selectable_label(cur_cap_rate == rate, label).clicked() {
                                app.config.audio_config.capture_sample_rate = rate;
                                changed = true;
                            }
                        }
                    });

                let chunk_resp = ui.add(
                    egui::Slider::new(&mut app.config.audio_config.capture_chunk_ms, 5..=100)
                        .text("Чанк")
                        .suffix(" мс"),
                );
                if slider_committed(&chunk_resp) {
                    changed = true;
                }
                let gate_resp = ui
                    .add(
                        egui::Slider::new(&mut app.config.audio_config.silence_gate_rms, 0..=5_000)
                            .text("Шумогейт RMS"),
                    )
                    .on_hover_text("0 = непрерывная передача потока без отсечения тишины");
                if slider_committed(&gate_resp) {
                    changed = true;
                }
                let hang_resp = ui.add(
                    egui::Slider::new(&mut app.config.audio_config.hangover_ms, 0..=5_000)
                        .text("Hangover")
                        .suffix(" мс"),
                );
                if slider_committed(&hang_resp) {
                    changed = true;
                }
                let inf_resp = ui.add(
                    egui::Slider::new(
                        &mut app.config.audio_config.host_max_inflight_chunks,
                        protocol::MIN_HOST_INFLIGHT_CHUNKS..=protocol::MAX_HOST_INFLIGHT_CHUNKS,
                    )
                    .text("Буфер хоста")
                    .suffix(" чанков"),
                );
                if slider_committed(&inf_resp) {
                    changed = true;
                }

                if ui
                    .button("Сбросить по умолчанию (Luna)")
                    .on_hover_text("Вернуть все параметры микрофона, таймингов и аудиопотока к дефолтам Luna")
                    .clicked()
                {
                    app.reset_audio_config_to_defaults();
                }
            });

            if changed {
                app.apply_audio_config();
            }
        });
}
