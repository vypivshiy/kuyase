use crate::gui::state::GuiAppState;
use eframe::egui::{self, Color32, Margin, Pos2, RichText, Stroke, Vec2};
use protocol::HostCommand;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

pub const HIJACK_WINDOW_MS: u64 = 1680;

const LED_PATTERNS: &[(&str, &str)] = &[
    ("", "Авто (по состоянию Quasar)"),
    ("listening", "listening.led (Голубой)"),
    ("speaking", "speaking.led (Фиолетовый)"),
    ("thinking", "thinking.led (Вращение)"),
    ("mute_mics", "mute_mics.led (Красный)"),
    ("volume", "volume.led (Зеленый)"),
    ("timer", "timer.led"),
    ("error", "error.led"),
    ("bluetooth_playing", "bluetooth_playing.led"),
    ("notification_available", "notification_available.led"),
    ("rgb:#00E5FF", "Сплошной Бирюзовый (rgb:#00E5FF)"),
];

pub fn compute_hijack_window_remaining_ms(last_inject_at: Option<Instant>, now: Instant) -> u64 {
    let Some(start) = last_inject_at else {
        return 0;
    };
    let elapsed_ms = now.saturating_duration_since(start).as_millis() as u64;
    HIJACK_WINDOW_MS.saturating_sub(elapsed_ms)
}

fn gnome_preferences_group(
    ui: &mut egui::Ui,
    title: &str,
    description: &str,
    add_rows: impl FnOnce(&mut egui::Ui),
) {
    ui.label(RichText::new(title).strong().size(15.0));
    ui.label(
        RichText::new(description)
            .small()
            .color(Color32::from_rgb(160, 160, 156)),
    );
    ui.add_space(4.0);
    egui::Frame::none()
        .fill(Color32::from_rgb(43, 43, 43))
        .stroke(Stroke::new(1.0, Color32::from_rgb(62, 62, 62)))
        .rounding(10.0)
        .inner_margin(Margin::symmetric(12.0, 10.0))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add_rows(ui);
        });
    ui.add_space(12.0);
}

fn gnome_action_row(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: &str,
    is_last: bool,
    add_controls: impl FnOnce(&mut egui::Ui),
) {
    ui.vertical(|ui| {
        ui.label(RichText::new(title).strong());
        ui.label(
            RichText::new(subtitle)
                .small()
                .color(Color32::from_rgb(156, 155, 152)),
        );
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            add_controls(ui);
        });
    });
    if !is_last {
        ui.add_space(5.0);
        ui.separator();
        ui.add_space(5.0);
    }
}

pub fn render_gnome_action_cards(ui: &mut egui::Ui, app: &mut GuiAppState) {
    egui::ScrollArea::vertical()
        .id_salt("gnome_action_cards_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            render_vm_connection_banner(ui, app);
            render_tts_audio_voice_card(ui, app);
            render_tof_and_sensors_card(ui, app);
            render_board_specific_card(ui, app);
        });
}

fn render_vm_connection_banner(ui: &mut egui::Ui, app: &mut GuiAppState) {
    let vm_running = app.session.is_some();
    let bridge_online = app
        .bridge
        .as_ref()
        .map(|b| b.is_connected())
        .unwrap_or(false);
    if bridge_online {
        return;
    }

    let busy = app.bg_task_running.load(Ordering::Relaxed);
    let (bg, border, title, sub) = if vm_running {
        (
            Color32::from_rgb(48, 42, 22),
            Color32::from_rgb(229, 165, 10),
            format!(
                "Загрузка образа '{}' в QEMU и ожидание моста :{}...",
                app.config.selected_board, app.config.bridge_port
            ),
            "Пожалуйста, подождите 5-10 секунд, пока загрузится гостевое ядро Linux и поднимется демон qemu-hw-bridge.".to_string(),
        )
    } else {
        (
            Color32::from_rgb(26, 48, 72),
            Color32::from_rgb(53, 132, 228),
            format!(
                "Виртуальная колонка '{}' сейчас не запущена (режим предпросмотра макета)",
                app.config.selected_board
            ),
            "Нажмите кнопку ниже, чтобы запустить образ в QEMU и подключить реальное выполнение команд TTS, ToF, кнопок и дисплея.".to_string(),
        )
    };

    egui::Frame::none()
        .fill(bg)
        .stroke(Stroke::new(1.5, border))
        .rounding(10.0)
        .inner_margin(Margin::symmetric(14.0, 10.0))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(title)
                        .strong()
                        .color(Color32::from_rgb(240, 246, 255)),
                );
                ui.label(
                    RichText::new(sub)
                        .small()
                        .color(Color32::from_rgb(195, 210, 230)),
                );
                ui.add_space(4.0);
                if vm_running {
                    ui.spinner();
                } else {
                    let btn = egui::Button::new(
                        RichText::new(format!(
                            "Запустить образ '{}' в QEMU",
                            app.config.selected_board
                        ))
                        .strong()
                        .color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(38, 162, 105));
                    if ui.add_enabled(!busy, btn).clicked() {
                        app.start_vm();
                    }
                }
            });
        });
    ui.add_space(12.0);
}

fn render_tts_audio_voice_card(ui: &mut egui::Ui, app: &mut GuiAppState) {
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

    gnome_preferences_group(
        ui,
        "1. Кастомный TTS, Прослушивание звука и Голосовая команда",
        "Синтез произвольной речи на колонке (:9875), живая осциллограмма потока 48 кГц и отправка фраз в микрофоны (:9850)",
        |ui| {
            let tts_sub = if last_spoken.is_empty() {
                format!(
                    "START_VOCALIZE (18) в aliced (:9875) -> поток 48 кГц на динамики ПК | Озвучено: {}",
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
                        if ui
                            .button(label)
                            .on_hover_text(phrase)
                            .clicked()
                        {
                            app.ipc_vocalize_text = phrase.to_string();
                        }
                    }
                },
            );

            let audio_sub = format!(
                "Поток S16_LE 2 канала | RMS: {} | Захвачено: {:.1} КБ ({:.1} с)",
                spk_rms,
                guest_cap_bytes as f64 / 1024.0,
                guest_cap_bytes as f64 / (48_000.0 * 4.0)
            );
            gnome_action_row(
                ui,
                "Звуковой выход колонки (snd-aloop 48 кГц)",
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
                        if let Some(ref b) = app.bridge {
                            b.clear_captured_pcm();
                        }
                    }
                    if ui.button("Сохранить .WAV").clicked() {
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

            let rem_ms = compute_hijack_window_remaining_ms(app.last_voice_inject_at, Instant::now());
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
                    "Инжекция фразы в yio_audio (:9850) и окно перехвата ответа 1680 мс | Перехватов: {}",
                    hijacks
                )
            };
            gnome_action_row(
                ui,
                "Сказать фразу Алисе (Voice Injection)",
                &voice_sub,
                true,
                |ui| {
                    let voice_w = (ui.available_width() * 0.34).clamp(150.0, 260.0);
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
                        app.last_voice_inject_at = Some(Instant::now());
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
                        "Перехватить (1680 мс)".to_string()
                    };
                    if ui.button(hijack_lbl).clicked() {
                        app.send_command(HostCommand::IpcTest {
                            cmd: "cancel_dialog".to_string(),
                            text: None,
                        });
                    }
                    for preset in ["включи ночной режим", "громкость 3", "какая погода"] {
                        if ui.button(preset).clicked() {
                            app.voice_phrase = preset.to_string();
                        }
                    }
                },
            );
        },
    );
}

fn render_tof_and_sensors_card(ui: &mut egui::Ui, app: &mut GuiAppState) {
    let board_slug = app
        .latest_state
        .as_ref()
        .map(|s| s.board_slug.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| app.config.selected_board.clone());

    let (card_title, card_subtitle) = if board_slug == "yandexmini" {
        (
            "2. Лазерный дальномер ToF (VL53L0X) и датчики среды",
            "Оптический датчик жестов руки VL53L0X (устанавливался только в Станцию Мини 1), освещённость ALS и температура SoC",
        )
    } else if board_slug == "orion" {
        (
            "2. Ультразвуковой тракт (38.4 кГц), освещённость ALS и датчики",
            "Дистанция акустического отражения для pcm.ultrasound (96 кГц / 38.4 кГц), датчик света light-sensor-top и температура SoC",
        )
    } else {
        (
            "2. Датчики среды (ALS, температура SoC) и световое кольцо",
            "Датчик внешней освещённости ltr308als01 (IIO), температура SoC и управление кольцом LED (оптического датчика VL53L0X на этой модели нет)",
        )
    };

    gnome_preferences_group(ui, card_title, card_subtitle, |ui| {
        if board_slug == "yandexmini" {
            gnome_action_row(
                ui,
                "Импульс датчика ToF (STM VL53L0X)",
                "Кратковременное приближение ладони (140 мм на 0.9 с с возвратом к прежней дистанции)",
                false,
                |ui| {
                    let wave_btn = egui::Button::new(
                        RichText::new("Взмах руки (Импульс 140 мм)")
                            .strong()
                            .color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(53, 132, 228));
                    if ui.add(wave_btn).clicked() {
                        app.trigger_tof_wave();
                    }
                    if ui.button("Поднести ладонь (120 мм)").clicked() {
                        app.tof_wave_reset_at = None;
                        app.tof_slider = 120;
                        app.tof_restore_distance = 120;
                        app.send_command(HostCommand::SetSensor {
                            doa_angle: None,
                            mic_rms: None,
                            als_lux: None,
                            temp_c: None,
                            tof_mm: Some(120),
                            swipe: None,
                            swipe_direction: None,
                        });
                    }
                    if ui.button("Убрать руку (8190 мм)").clicked() {
                        app.tof_wave_reset_at = None;
                        app.tof_slider = 8190;
                        app.tof_restore_distance = 8190;
                        app.send_command(HostCommand::SetSensor {
                            doa_angle: None,
                            mic_rms: None,
                            als_lux: None,
                            temp_c: None,
                            tof_mm: Some(8190),
                            swipe: None,
                            swipe_direction: None,
                        });
                    }
                },
            );

            gnome_action_row(
                ui,
                "Точная дистанция ToF (VL53L0X)",
                "Ручная установка расстояния от ладони до крышки (40..8190 мм) для регулировки громкости жестом",
                false,
                |ui| {
                    if ui
                        .add(egui::Slider::new(&mut app.tof_slider, 40..=8190).suffix(" мм"))
                        .changed()
                    {
                        app.tof_wave_reset_at = None;
                        app.tof_restore_distance = app.tof_slider;
                        app.send_command(HostCommand::SetSensor {
                            doa_angle: None,
                            mic_rms: None,
                            als_lux: None,
                            temp_c: None,
                            tof_mm: Some(app.tof_slider),
                            swipe: None,
                            swipe_direction: None,
                        });
                    }
                },
            );
        } else if board_slug == "orion" {
            gnome_action_row(
                ui,
                "Акустическое отражение ультразвука (pcm.ultrasound, 38.4 кГц)",
                "Дистанция до препятствия (40..8190 мм), определяющая амплитуду отражённого эха на усилителе ACM8625S (2-002c)",
                false,
                |ui| {
                    if ui
                        .add(egui::Slider::new(&mut app.tof_slider, 40..=8190).suffix(" мм"))
                        .changed()
                    {
                        app.tof_wave_reset_at = None;
                        app.tof_restore_distance = app.tof_slider;
                        app.send_command(HostCommand::SetSensor {
                            doa_angle: None,
                            mic_rms: None,
                            als_lux: None,
                            temp_c: None,
                            tof_mm: Some(app.tof_slider),
                            swipe: None,
                            swipe_direction: None,
                        });
                    }
                    if ui.button("Ближнее эхо (180 мм)").clicked() {
                        app.tof_wave_reset_at = None;
                        app.tof_slider = 180;
                        app.tof_restore_distance = 180;
                        app.send_command(HostCommand::SetSensor {
                            doa_angle: None,
                            mic_rms: None,
                            als_lux: None,
                            temp_c: None,
                            tof_mm: Some(180),
                            swipe: None,
                            swipe_direction: None,
                        });
                    }
                    if ui.button("Свободное поле (8190 мм)").clicked() {
                        app.tof_wave_reset_at = None;
                        app.tof_slider = 8190;
                        app.tof_restore_distance = 8190;
                        app.send_command(HostCommand::SetSensor {
                            doa_angle: None,
                            mic_rms: None,
                            als_lux: None,
                            temp_c: None,
                            tof_mm: Some(8190),
                            swipe: None,
                            swipe_direction: None,
                        });
                    }
                },
            );
        }

        gnome_action_row(
            ui,
            "Громкость, датчики среды и паттерн LED",
            "Громкость (0..10), освещённость ALS (лк), температура SoC (C) и принудительный .led паттерн",
            true,
            |ui| {
                if ui
                    .add(egui::Slider::new(&mut app.volume_slider, 0..=10).text("Громк"))
                    .changed()
                {
                    app.send_command(HostCommand::SetVolume {
                        volume_level: app.volume_slider,
                    });
                }
                if ui
                    .add(egui::Slider::new(&mut app.als_slider, 0..=10000).text("лк"))
                    .changed()
                {
                    app.send_command(HostCommand::SetSensor {
                        doa_angle: None,
                        mic_rms: None,
                        als_lux: Some(app.als_slider),
                        temp_c: None,
                        tof_mm: None,
                        swipe: None,
                        swipe_direction: None,
                    });
                }
                if ui
                    .add(
                        egui::Slider::new(&mut app.temp_slider, 20.0..=85.0)
                            .text("C")
                            .fixed_decimals(1),
                    )
                    .changed()
                {
                    app.send_command(HostCommand::SetSensor {
                        doa_angle: None,
                        mic_rms: None,
                        als_lux: None,
                        temp_c: Some(app.temp_slider),
                        tof_mm: None,
                        swipe: None,
                        swipe_direction: None,
                    });
                }

                egui::ComboBox::from_id_salt("gnome_led_pattern_combo")
                    .selected_text(if app.selected_led_pattern.is_empty() {
                        "LED: Авто (Quasar)"
                    } else {
                        &app.selected_led_pattern
                    })
                    .width(185.0)
                    .show_ui(ui, |ui| {
                        for &(val, label) in LED_PATTERNS {
                            if ui
                                .selectable_label(app.selected_led_pattern == val, label)
                                .clicked()
                            {
                                app.selected_led_pattern = val.to_string();
                                app.send_command(HostCommand::SetLedPattern {
                                    pattern: app.selected_led_pattern.clone(),
                                });
                            }
                        }
                    });
            },
        );
    });
}

fn render_board_specific_card(ui: &mut egui::Ui, app: &mut GuiAppState) {
    let board_slug = app
        .latest_state
        .as_ref()
        .map(|s| s.board_slug.clone())
        .unwrap_or_else(|| app.config.selected_board.clone());

    let title = format!("3. Специфичная периферия платы ({})", board_slug);
    gnome_preferences_group(
        ui,
        &title,
        "Аппаратные контроллеры, усилители и драйверы индикации выбранного профиля платы",
        |ui| {
            if board_slug == "orion" {
                let st_opt = app.latest_state.clone();
                gnome_action_row(
                    ui,
                    "Контур Ambilight 12x2 RGBW (/dev/fb0 + 96 каналов sysfs)",
                    "Фоновая подсветка корпуса Яндекс Станции 3 (orion)",
                    false,
                    |ui| {
                        for (lbl, pat) in [
                            ("Фиолетовый (speaking)", "speaking"),
                            ("Бирюзовый (rgb:#00E5FF)", "rgb:#00E5FF"),
                            ("Теплый (rgb:#FF9933)", "rgb:#FF9933"),
                        ] {
                            if ui.button(lbl).clicked() {
                                app.selected_led_pattern = pat.to_string();
                                app.send_command(HostCommand::SetLedPattern {
                                    pattern: pat.to_string(),
                                });
                            }
                        }
                    },
                );

                if let Some(ref st) = st_opt {
                    if !st.amplifiers.is_empty() {
                        ui.label(
                            RichText::new("4x Усилителя TI ACM8625S (I2C) + Тракт pcm.ultrasound (96 кГц / 38.4 кГц)")
                                .strong(),
                        );
                        egui::Grid::new("gnome_orion_amp_grid")
                            .striped(true)
                            .num_columns(6)
                            .spacing([12.0, 5.0])
                            .show(ui, |ui| {
                                for hdr in ["I2C", "Роль", "ALSA", "Профиль DSP", "Усиление", "Статус"] {
                                    ui.label(RichText::new(hdr).small().strong());
                                }
                                ui.end_row();
                                for amp in &st.amplifiers {
                                    ui.monospace(&amp.i2c_device);
                                    ui.label(RichText::new(&amp.role).strong());
                                    ui.monospace(&amp.alsa_prefix);
                                    ui.monospace(&amp.settings_file);
                                    ui.monospace(format!("{:+.1} / {:+.1} dB", amp.gain_db, amp.ramp_db));
                                    let (txt, col) = if amp.muted {
                                        ("MUTED", Color32::from_rgb(245, 95, 95))
                                    } else {
                                        ("ACTIVE", Color32::from_rgb(87, 227, 137))
                                    };
                                    ui.label(RichText::new(txt).small().strong().color(col));
                                    ui.end_row();
                                }
                            });
                        if let Some(ref us) = st.ultrasound {
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(format!(
                                    "Ultrasound TDM-A: {} | {} Гц | Несущая: {} Гц | Полоса: {} Гц | Каналов: {} | RMS: {}",
                                    us.tdm_device,
                                    us.sample_rate_hz,
                                    us.carrier_freq_hz,
                                    us.bandwidth_hz,
                                    us.channels,
                                    us.rms
                                ))
                                .small()
                                .monospace(),
                            );
                        }
                        return;
                    }
                }
                gnome_action_row(
                    ui,
                    "4x Усилителя TI ACM8625S (I2C) + Ультразвук TDM-A",
                    "3-002d (woofer), 3-002e (lr), 3-002c (center), 2-002c (us) | pcm.ultrasound 96 кГц / 38.4 кГц",
                    true,
                    |ui| {
                        ui.label(
                            RichText::new("4x ACM8625S + Ultrasound 38.4 кГц")
                                .small()
                                .strong()
                                .color(Color32::from_rgb(87, 227, 137)),
                        );
                    },
                );
            } else if board_slug == "yandexmini" {
                let trig = app
                    .latest_state
                    .as_ref()
                    .map(|s| s.ring_trigger.clone())
                    .unwrap_or_else(|| "i2c-0 SN3236 (0x3c Active)".to_string());
                gnome_action_row(
                    ui,
                    "I2C Контроллер SN3236 (адрес 0x3c, 18 каналов BGR)",
                    &format!(
                        "Запись в 0x01..0x12 меняет только теневой буфер; перенос в активное свечение происходит только при записи в защёлку 0x25 (Register Latch), 0x00=0 включает Stealth Lock | Статус: {}",
                        trig
                    ),
                    true,
                    |ui| {
                        if ui.button("Проверка BGR (rgb:#6B0FFF)").clicked() {
                            app.selected_led_pattern = "rgb:#6B0FFF".to_string();
                            app.send_command(HostCommand::SetLedPattern {
                                pattern: "rgb:#6B0FFF".to_string(),
                            });
                        }
                        if ui.button("Авто (Quasar)").clicked() {
                            app.selected_led_pattern.clear();
                            app.send_command(HostCommand::SetLedPattern {
                                pattern: String::new(),
                            });
                        }
                        if app.session.is_some() {
                            if ui
                                .button("Демо защёлки 0x25 (I2C)")
                                .on_hover_text("Записать BGR в теневые регистры 0x01..0x12, выждать 1 с (кольцо ещё не меняется) и защёлкнуть регистром 0x25")
                                .clicked()
                            {
                                app.send_serial_command(
                                    "i2cset -y 0 0x3c 0x00 0x01; for r in $(seq 1 3 16); do i2cset -y 0 0x3c $r 0xff; i2cset -y 0 0x3c $((r+1)) 0x40; i2cset -y 0 0x3c $((r+2)) 0x90; done; sleep 1; i2cset -y 0 0x3c 0x25 0x01",
                                );
                            }
                            if ui
                                .button("Stealth Lock (0x00=0)")
                                .on_hover_text("Программно погасить и заблокировать чип SN3236 записью 0x00 в регистр 0x00")
                                .clicked()
                            {
                                app.send_serial_command("i2cset -y 0 0x3c 0x00 0x00");
                            }
                        }
                    },
                );
            } else {
                let (trig, conflicts) = app
                    .latest_state
                    .as_ref()
                    .map(|s| (s.ring_trigger.clone(), s.led_trigger_conflict_count))
                    .unwrap_or_else(|| ("ledfb-ring (Quasar Native)".to_string(), 0));
                gnome_action_row(
                    ui,
                    "Драйвер светового кольца (sysfs / ledfb-ring)",
                    &format!(
                        "Активный триггер: {} | Отклонено записей sysfs при [ledfb-ring] (Trigger Conflict): {}",
                        trig, conflicts
                    ),
                    true,
                    |ui| {
                        if ui.button("Бирюзовый (#00E5FF)").clicked() {
                            app.selected_led_pattern = "rgb:#00E5FF".to_string();
                            app.send_command(HostCommand::SetLedPattern {
                                pattern: "rgb:#00E5FF".to_string(),
                            });
                        }
                        if ui.button("Авто (ledfb-ring)").clicked() {
                            app.selected_led_pattern.clear();
                            app.send_command(HostCommand::SetLedPattern {
                                pattern: String::new(),
                            });
                        }
                        if app.session.is_some() {
                            if ui
                                .button("Ручной sysfs (trigger=none)")
                                .on_hover_text("Переключить триггер кольца в 'none' для прямой записи в /sys/class/leds/*:ring*/brightness без конфликта")
                                .clicked()
                            {
                                app.send_serial_command(
                                    "for d in /sys/class/leds/*:ring*; do echo none > \"$d/trigger\"; done",
                                );
                            }
                            if ui
                                .button("Вернуть триггер ledfb-ring")
                                .on_hover_text("Обнулить brightness в sysfs и вернуть управление кольцом штатному триггеру драйвера ledfb-ring")
                                .clicked()
                            {
                                app.send_serial_command(
                                    "for d in /sys/class/leds/*:ring*; do echo 0 > \"$d/brightness\"; echo ledfb-ring > \"$d/trigger\"; done",
                                );
                            }
                        }
                    },
                );
            }
        },
    );
}

fn render_mini_oscilloscope(ui: &mut egui::Ui, app: &GuiAppState) {
    let desired = Vec2::new(195.0, 30.0);
    let (rect, _resp) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 4.0, Color32::from_rgb(15, 19, 25));
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0, Color32::from_rgb(48, 60, 78)),
    );

    let mid_y = rect.center().y;
    let samples = app.audio_player.recent_waveform(128);
    if samples.len() >= 2 {
        let n = samples.len();
        let w = rect.width() - 6.0;
        let half_h = (rect.height() - 6.0) * 0.5;
        let pts: Vec<Pos2> = samples
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                let x = rect.left() + 3.0 + (i as f32 / (n - 1) as f32) * w;
                let y = mid_y - (s.clamp(-1.0, 1.0) * half_h);
                Pos2::new(x, y)
            })
            .collect();
        painter.add(egui::Shape::line(
            pts,
            Stroke::new(1.6, Color32::from_rgb(87, 227, 137)),
        ));
    } else {
        painter.line_segment(
            [
                Pos2::new(rect.left() + 3.0, mid_y),
                Pos2::new(rect.right() - 3.0, mid_y),
            ],
            Stroke::new(1.2, Color32::from_rgb(55, 110, 100)),
        );
    }
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
    }
}
