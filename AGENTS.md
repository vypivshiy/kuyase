# AGENTS.md

## Context Pointers

- [`CONTEXT.md`](./CONTEXT.md): Канонический глоссарий доменных терминов (`Board Profile`, `Guest Image`, `Hardware Bridge`, `Register Latch`, `Trigger Conflict`, `Hijack Window`). Сверяйся при именовании переменных, полей JSON, флагов CLI и написании документации.
- [`API.md`](./API.md): Контракты всех 12 HTTP-эндпоинтов `qemu-hw-bridge` (`:8091` в госте / `:18091` на хосте), формат JSON-состояния `/api/state` и коды Protobuf-команд `aliced`. Читай при изменении `hw-bridge/src/http.rs`, `hw-bridge/src/dashboard.html` или интеграционных тестов.
- [`DUMP_GUIDE.md`](./DUMP_GUIDE.md): Снятие `rootfs.tar.gz` по сетевому ADB (`:5555`). Читай при изменении `scripts/dump-from-adb.*` или разборе форматов дампов в `scripts/prepare-rootfs.sh`.
- [`README.md`](./README.md): Матрица эмулируемой периферии, таблицы протестированных и автопрофилируемых плат и команды сборки из исходников.

## Пайплайн сборки и инжекции (критично)

При запуске через `scripts/run-qemu.sh`, `scripts/run-qemu.ps1` или `tests/test_qemu_multi_board.py` бинарник `qemu-hw-bridge` с хоста пробрасывается в виртуальную машину вторым `virtio-blk` диском (`/dev/vdb`) и автоматически обновляет `/usr/local/bin/qemu-hw-bridge` при каждом старте QEMU без пересборки `.ext4`. Пересборка `work/<board>-system.ext4` через `scripts/prepare-rootfs.sh` требуется только при первоначальном создании образа или при изменении `guest-init/equasar-qemu-init.sh`:

1. **Кросс-компиляция `qemu-hw-bridge`**:
   ```bash
   (cd hw-bridge && cargo build --release --target armv7-unknown-linux-musleabihf)
   cp -f hw-bridge/target/armv7-unknown-linux-musleabihf/release/qemu-hw-bridge guest-init/bin/qemu-hw-bridge
   ```
   Бинарник всегда собирается статически под 32-битный таргет `armv7-unknown-linux-musleabihf` (линкер `rust-lld` задан в `hw-bridge/.cargo/config.toml`), чтобы один исполняемый файл работал и на 32-битных (`yandexmini`, `yandexmini2`, `yandexmicro`), и на 64-битных (`cucumber`, `orion`) образах через ядерный `CONFIG_COMPAT=y`.
2. **Переинжекция в гостевой образ (только при изменении `guest-init/equasar-qemu-init.sh` или сборке нового образа)**:
   ```bash
   # Linux / WSL2 (требуется root для mount -o loop):
   sudo bash ./scripts/prepare-rootfs.sh <board>
   ```
   При запуске из Windows через WSL2 перед вызовом скриптов нормализуй переводы строк `CRLF -> LF` в `guest-init/equasar-qemu-init.sh` и `scripts/*.sh` (`sed -i 's/\r$//' ...`), иначе ядро Linux завершится с `Kernel panic - not syncing: Attempted to kill init!`.
3. **Проверка интеграционным тестом**:
   ```bash
   python3 ./tests/test_qemu_multi_board.py --board <board>
   ```
   Критерий завершения: тест проходит целиком для затронутых профилей плат при `ipc_error_count == 0`.

## Инварианты архитектуры и эмуляции

- **Единый канал доступа в гостевую ОС**: В гостевых образах используется только штатный демон `adbd` на TCP `:5555` (проброшен на хост как `127.0.0.1:15555`). Не добавляй сторонние SSH-серверы (`dropbear`), ключи или бэкдоры в `guest-init/` и `scripts/`.
- **Чистота образов и дампов**: В `work/*.ext4`, `backups/` и коммитах не должно быть содержимого разделов `/data`, `/bkupgrade`, `/nv`, `/env`, `/tee`, учётных данных Wi-Fi (`wpa_supplicant.conf`) и серийных номеров/ключей устройств.
- **Контроллер `SN3236` (`yandexmini`, I2C `0x3c`, `hw-bridge/src/leds.rs`)**:
  - Порядок каналов в регистрах `0x01..0x12` и `0x26..0x37` — строго **BGR** (`B = base+0`, `G = base+1`, `R = base+2`).
  - Запись в `0x01..0x12` и `0x26..0x37` меняет только теневой буфер; перенос в активное свечение светодиодов происходит только при записи любого байта в регистр защёлки `0x25` (`PWM Update`).
  - В регистрах `0x26..0x37` бит `0` (`OUT_EN`) включает канал, биты `2:1` (`SL`) делят яркость на `1`, `2`, `3` или `4`. Запись в `0x4a` сбрасывает все регистры чипа в `0x00`.
- **Декомпозиция периферии через трейты (`hw-bridge/src/devices.rs`)**:
  - Все аппаратные различия плат инкапсулированы в 4 доменных трейтах внутри `BoardPeripherals`: `LedRingDevice` (`Sn3236I2cRing`, `SysfsFbLedRing`), `AuxLightDevice` (`OrionAmbilightDevice`, `LogoSysfsFbDevice`, `NullAuxLight`), `FrontDisplayDevice` (`SegmentClockSysfsDisplay`, `OrionMatrix21x12Display`, `NullFrontDisplay`) и `AudioSubsystemDevice` (`OrionQuadAcm8625Audio`, `GenericAlsaAudio`). Не добавляй платформенные ветвления `if board_slug == ...` в `http.rs` или `leds.rs`.
- **Светодиоды `sysfs` и `framebuffer` (`yandexmini2`, `yandexmicro`, `cucumber`, `orion`)**:
  - Запись в `/sys/class/leds/<prefix>:*:ring*/brightness` при активном триггере `[ledfb-ring]` должна отклоняться и инкрементировать `led_trigger_conflict_count`. Для прямого управления через `sysfs` триггер предварительно переключается в `none`.
  - На `orion` (Яндекс Станция 3) раскладка фреймбуферов определяется по `/sys/class/graphics/fb*/name`: `/dev/fb0` — `ambilight` (`12×1`, `48 bpp` + 96 каналов `orion:{red,green,blue,white}:ambilight{0..11}{a,b}`), `/dev/fb1` — `YNDX LED screen` (`21×12`, `24 bpp`, `756 Б` монохромной матрицы `gowin_led_device`), `/dev/fb2` — `ring` (`36×1`, `24 bpp`). Четыре усилителя `ACM8625S` (`2-002c` `us`, `3-002c` `center`, `3-002d` `woofer`, `3-002e` `lr`) конфигурируются через `settings_file`, а ультразвук выводится на отдельный TDM `pcm.ultrasound` (`96 кГц`, несущая `38.4 кГц`).
- **Локальный IPC стека Quasar (`hw-bridge/src/ipc.rs`, `hw-bridge/src/proto.rs`)**:
  - Все сокеты (`aliced` `:9875`, `yio_audio` `:9850`, `iohub_services` `:9903`) используют фрейминг `[u32_le_len][QuasarMessage protobuf]` без внешних protobuf-зависимостей.
  - Инжекция голоса (`/api/voice/inject`) открывает окно перехвата `1680 мс`: получение `CANCEL_DIALOG_AND_CLEAR_QUEUE` (`6`) или `STOP_TTS` (`14`) на `:9875` подавляет штатный ответ Алисы и инкрементирует `aliced_hijack_count`.
- **Стиль документации**: Документация репозитория (`README.md`, `API.md`, `DUMP_GUIDE.md`, `CONTEXT.md`, `AGENTS.md`, `llm.txt`) пишется технически лаконично, без эмодзи, с использованием блоков GitHub Alerts (`>[!note]`, `>[!warning]`, `>[!tip]`).

## Agent skills

### Issue tracker

Local markdown files under `.scratch/<feature>/` (ignored by git). See `docs/agents/issue-tracker.md`.

### Triage labels

Default five-role vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context layout (`CONTEXT.md` and `docs/adr/` at the repo root). See `docs/agents/domain.md`.

