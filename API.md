# HTTP API аппаратного моста (`qemu-hw-bridge`)

Демон `qemu-hw-bridge` внутри виртуальной машины слушает порт `:8091`, который по умолчанию пробрасывается на хост как `http://127.0.0.1:18091` (переопределяется флагом `--bridge-port` / `-BridgePort`).

>[!note]
> Все `POST`-запросы принимают JSON (`Content-Type: application/json`) и возвращают JSON с полем `"ok": true|false`. На всех ответах включены заголовки CORS (`Access-Control-Allow-Origin: *`).

---

## Сводная таблица методов

| Метод | Путь | Тело запроса (JSON) | Описание |
| :--- | :--- | :--- | :--- |
| `GET` | `/` | — | Веб-интерфейс `Virtual Speaker Twin` (`text/html`) |
| `GET` | `/api/state` | — | Полный снимок состояния железа, датчиков, звука, IPC и логов |
| `GET` | `/api/audio/output.wav` | — | Захваченный из `snd-aloop` звук динамика (`audio/wav`, 48 кГц, 16-bit stereo) |
| `POST` | `/api/hw/button` | `{"button": "...", "action": "..."}` | Инжекция нажатия физической кнопки через `/dev/uinput` |
| `POST` | `/api/hw/volume` | `{"volume_level": 0..10}` | Установка громкости (`0..10`) и `Master` в ALSA (`0..10000`) |
| `POST` | `/api/hw/led` | `{"pattern": "..."}` | Принудительный запуск анимации `.led` (пустая строка — возврат в авторежим) |
| `POST` | `/api/hw/clock` | `{"segments": {...}}` / `{"pixels": [...]}` / `{"led_frame": "..."}` / `{"text": "..."}` | Управление сегментами часов (`/sys/class/leds/*:clock-*`) и LED-матрицей `21×12` (`/dev/fb1`) |
| `POST` | `/api/hw/sensor` | `{"temp_c", "als_lux", "tof_mm", "doa_angle", "mic_rms", "swipe"}` | Установка значений датчиков (`sysfs`, `iio`, `/dev/uinput`) |
| `POST` | `/api/voice/inject` | `{"phrase": "...", "doa_angle": 90, "wake_only": false}` | Инжекция голосовой команды / wake-word и старт окна перехвата (1680 мс) |
| `POST` | `/api/ipc/test` | `{"cmd": "cancel_dialog"\|"stop_tts"\|"vocalize", "text": "..."}` | Отправка тестового Protobuf-пакета в локальный сокет `aliced` (`:9875`) |
| `POST` | `/api/audio/test_tone` | `{}` | Воспроизведение тестового синусоидального тона `440 Гц` (350 мс) в ALSA |
| `POST` | `/api/audio/clear` | `{}` | Очистка буфера захваченного аудиовыхода (`output.wav`) |
| `POST` | `/api/hw/poweroff` | `{}` | Завершение работы виртуальной машины (`poweroff -f`) |

---

## Чтение состояния и звука

### `GET /api/state`

Возвращает текущее состояние эмулируемой периферии и демонов.

Пример ответа:

```json
{
  "ok": true,
  "board_slug": "cucumber",
  "board_name": "Яндекс Станция Миди (YNDX-00054 / cucumber)",
  "ring_leds": 24,
  "logo_leds": 6,
  "ambilight_leds": 0,
  "aux_light_kind": "logo",
  "has_clock": true,
  "display_kind": "segment_clock",
  "screen_width": 21,
  "screen_height": 12,
  "screen_pixels": [0, 0, 0],
  "ring_pixels": [[0, 0, 0], [0, 0, 0]],
  "logo_pixels": [[0, 0, 0], [0, 0, 0]],
  "ring_trigger": "ledfb-ring",
  "clock_segments": {"a1": 255, "b1": 255, "deg": 0},
  "amplifiers": [],
  "ultrasound": null,
  "doa_angle": 90,
  "tof_mm": 8190,
  "als_lux": 320,
  "temp_c": 42.5,
  "volume_level": 5,
  "master_volume": 5000,
  "mics_muted": false,
  "forced_led_pattern": "",
  "mic_rms": 60,
  "mic_burst_active": false,
  "speaker_rms": 0,
  "captured_audio_bytes": 38400,
  "alice_state": "IDLE",
  "aliced_hijack_count": 1,
  "aliced_say_count": 2,
  "ipc_error_count": 0,
  "led_trigger_conflict_count": 0,
  "last_spoken_text": "Проверка синтеза",
  "recent_logs": ["..."],
  "maind_logs": ["..."]
}
```

Ключевые поля:
* `ring_pixels` / `logo_pixels` — массивы `[R, G, B]` (`0..255`) для каждого светодиода кольца и логотипа/эмбиента (вычитываются через трейты `LedRingDevice` и `AuxLightDevice` из `I2C 0x3c`, `/dev/fb0..2` или `/sys/class/leds/*`).
* `aux_light_kind` — тип дополнительной подсветки (`"none"`, `"logo"`, `"ambilight"`).
* `display_kind` — тип фронтального дисплея (`"none"`, `"segment_clock"`, `"matrix_21x12"`).
* `screen_width`, `screen_height`, `screen_pixels` — геометрия и массив яркости пикселей (`252` байта `0..255` для монохромной матрицы `21×12` на `/dev/fb1` в `orion`).
* `amplifiers` — массив состояний аппаратных I2C-усилителей (на `orion` — 4× `ACM8625S`: `3-002d` `woofer`, `3-002e` `lr`, `3-002c` `center`, `2-002c` `us`, включая загруженный профиль `settings_file`, усиление в дБ по кривой `hardvold` и статус `muted`).
* `ultrasound` — статус ультразвукового TDM-тракта на `orion` (`hw:0,0` / `pcm.ultrasound`, `96000 Hz`, несущая `38400 Hz`, 4 канала, `rms`).
* `ring_trigger` — текущий активный триггер в `/sys/class/leds/*:ring0/trigger` (`ledfb-ring` или `none`).
* `led_trigger_conflict_count` — счётчик попыток прямой записи в `brightness` при неснятом триггере `[ledfb-ring]`.
* `clock_segments` — карта яркости сегментов 7-сегментных часов (`0..255`).
* `alice_state` — состояние диалогового автомата (`IDLE`, `LISTENING`, `SPEAKING`).
* `aliced_hijack_count` — число успешных перехватов диалога (`CANCEL_DIALOG_AND_CLEAR_QUEUE` / `STOP_TTS`) на сокете `:9875`.
* `aliced_say_count` — число вызовов `START_VOCALIZE` на сокете `:9875`.
* `ipc_error_count` — число некорректных Protobuf-фреймов, полученных на портах `:9875`, `:9850`, `:9903`.

---

### `GET /api/audio/output.wav`

Отдаёт бинарный WAV-файл (`48000 Hz`, `S16_LE`, 2 канала), записанный с петлевого захвата `snd-aloop` (`hw:0,1`). Поддерживает необязательный параметр `?from=<byte_offset>` для потоковой подгрузки только новых PCM-фреймов в веб-интерфейс `Virtual Speaker Twin`.

```bash
curl -s http://127.0.0.1:18091/api/audio/output.wav -o output.wav
```

---

## Управление периферией и событиями

### `POST /api/hw/button`

Генерирует события `EV_KEY` в виртуальном устройстве `/dev/uinput` и обновляет внутреннее состояние (громкость, отключение микрофонов, сброс диалога):

* `button`:
  * `"action"` — центральная кнопка активации (`KEY_VOICECOMMAND = 582`)
  * `"mute"` — кнопка микрофонов (`KEY_MUTE = 113`), переключает `mics_muted` и включает красное кольцо `mute_mics.led`
  * `"volume_up"` — увеличение громкости (`KEY_VOLUMEUP = 115`)
  * `"volume_down"` — уменьшение громкости (`KEY_VOLUMEDOWN = 114`)
* `action`: `"single"` (по умолчанию, 50 мс), `"double"`, `"triple"`, `"long"` (удержание 900 мс).

```bash
curl -X POST http://127.0.0.1:18091/api/hw/button \
  -H "Content-Type: application/json" \
  -d '{"button": "mute", "action": "single"}'
```

---

### `POST /api/hw/volume`

Устанавливает уровень громкости от `0` до `10`, обновляет `Master` в ALSA и запускает анимацию дуги громкости (`volume.led`):

```bash
curl -X POST http://127.0.0.1:18091/api/hw/volume \
  -H "Content-Type: application/json" \
  -d '{"volume_level": 8}'
```

---

### `POST /api/hw/led`

Принудительно включает проигрывание штатного паттерна из `/system/vendor/quasar/ledpatterns/*.led` (передайте `""` для возврата к автоматическому режиму по состоянию Алисы):

```bash
curl -X POST http://127.0.0.1:18091/api/hw/led \
  -H "Content-Type: application/json" \
  -d '{"pattern": "alice_listening.led"}'
```

---

### `POST /api/hw/clock`

Управляет фронтальным дисплеем (7-сегментными часами `/sys/class/leds/<prefix>:white:clock-*` или монохромной матрицей `21×12` `/dev/fb1`). Поддерживает следующие варианты JSON-тела:

* `{"segments": {"a1": 255, "g2": 128, "deg": 64}, "replace": false}` — прямая установка яркости (`0..255`) произвольных сегментов (`a1..g4`, `p3..q7`, `h1`, `h2`, `m1`, `m2`, `l1`, `l2`, `deg`). Если `"replace": true`, не указанные сегменты гасятся в `0`.
* `{"pixels": [0, 255, 128, ...]}` — прямая запись массива из `252` байт яркости (`0..255`) в матрицу `21×12` (`/dev/fb1` `YNDX LED screen`).
* `{"led_frame": "ffffff 000000 ... 40", "brightness": 255}` — загрузка кадра в формате Quasar `.led` (`252` hex-токена для матрицы `21×12` или `45` hex-токенов для 7-сегментного циферблата).
* `{"scale_brightness": 80}` — масштабирование яркости всех активных (`> 0`) сегментов или пикселей до заданного уровня ШИМ (`0..255`).
* `{"text": "19:30°", "brightness": 255}` — вывод текста/времени, встроенного паттерна (`"pattern:joy"`) или текстового перечисления сегментов (`"a1,b1,deg=128"`).

```bash
# Точечная установка сегментов и ШИМ-яркости:
curl -X POST http://127.0.0.1:18091/api/hw/clock \
  -H "Content-Type: application/json" \
  -d '{"segments": {"a1": 255, "f1": 128, "deg": 64}, "replace": true}'

# Вывод строки времени с заданной яркостью:
curl -X POST http://127.0.0.1:18091/api/hw/clock \
  -H "Content-Type: application/json" \
  -d '{"text": "19:30°", "brightness": 255}'
```

---

### `POST /api/hw/sensor`

Обновляет показания виртуальных датчиков (все поля опциональны):

* `temp_c` (`float`) — температура SoC в градусах Цельсия (записывается в `/sys/class/thermal/thermal_zone0/temp` в миллиградусах и пересчитывается в `in_voltage6_mean_raw`).
* `als_lux` (`int`) — освещённость в люксах (`/sys/bus/iio/devices/iio:device0/in_illuminance_raw`).
* `tof_mm` (`int`) — расстояние дальномера `VL53L0X` в миллиметрах (генерирует `EV_ABS` в `/dev/uinput`).
* `swipe` (`bool`) — если `true`, симулирует взмах руки над датчиком `ToF` (`140 мм -> 8190 мм`).
* `doa_angle` (`0..359`) — угол направления на источник звука (`Direction of Arrival`), передаваемый подписчикам аудиопотока `:9850`.
* `mic_rms` (`int`) — уровень сигнала микрофонной матрицы.

```bash
curl -X POST http://127.0.0.1:18091/api/hw/sensor \
  -H "Content-Type: application/json" \
  -d '{"temp_c": 51.4, "als_lux": 640, "tof_mm": 165, "doa_angle": 90}'
```

---

### `POST /api/voice/inject`

Симулирует распознавание голосовой фразы (или только срабатывание споттера при `"wake_only": true`):

1. Переводит состояние Алисы в `LISTENING`, включает всплеск в 4-канальном микрофонном потоке `:9850` и записывает строку `[sdk] onRequestStateChanged: LISTENING, recognizedPhrase='...'` в `/tmp/daemons_logs/maind.log`.
2. Открывает окно перехвата (`1680 мс`): если за это время внешний демон не пришлёт `CANCEL_DIALOG_AND_CLEAR_QUEUE` (`6`) или `STOP_TTS` (`14`) в сокет `aliced` (`:9875`), срабатывает штатный ответ Алисы (`SPEAKING` + формантный синтез речи в ALSA).
3. Если микрофоны выключены (`mics_muted == true`), возвращает `{"ok": false, "error": "mics_muted"}`.

```bash
curl -X POST http://127.0.0.1:18091/api/voice/inject \
  -H "Content-Type: application/json" \
  -d '{"phrase": "включи ночной режим", "doa_angle": 90}'
```

---

### `POST /api/ipc/test`

Формирует и отправляет валидный Protobuf-фрейм `QuasarMessage.remoting (210)` (`AliceCapability`) напрямую в локальный TCP-сокет `127.0.0.1:9875` (`aliced`):

* `cmd`:
  * `"cancel_dialog"` — `CANCEL_DIALOG_AND_CLEAR_QUEUE = 6`
  * `"stop_tts"` — `STOP_TTS = 14`
  * `"vocalize"` — `START_VOCALIZE = 18` с передачей текста из поля `text`

```bash
curl -X POST http://127.0.0.1:18091/api/ipc/test \
  -H "Content-Type: application/json" \
  -d '{"cmd": "vocalize", "text": "Проверка локального синтеза"}'
```

---

### `POST /api/audio/test_tone` и `POST /api/audio/clear`

Управление проверкой звукового тракта `ALSA -> snd-aloop`:

```bash
# Проиграть тестовый тон 440 Гц в ALSA default:
curl -X POST http://127.0.0.1:18091/api/audio/test_tone

# Очистить накопленный буфер /api/audio/output.wav:
curl -X POST http://127.0.0.1:18091/api/audio/clear
```

---

### `POST /api/hw/poweroff`

Немедленно выключает гостевую систему (`poweroff -f`) и завершает процесс `qemu-system-aarch64`:

```bash
curl -X POST http://127.0.0.1:18091/api/hw/poweroff
```
