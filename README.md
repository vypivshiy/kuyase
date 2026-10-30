# КУЯСЭ! (`KUYASE`) — (К)устарный (У)ниверсальный (Я)ндекс (С)танций (Э)мулятор

**КУЯСЭ** — кроссплатформенное нативное десктопное приложение (`eframe` / `egui` + `egui_tiles`) и CLI-стенд для запуска системных образов (`rootfs`) умных колонок Яндекса (Amlogic / Buildroot) в `qemu-system-aarch64` с программной эмуляцией периферии, доступом по **ADB (`:15555`)**, интерактивным 60 FPS визуализатором (`Virtual Speaker Twin`), дуплексным TCP-протоколом телеметрии и аудио (`:18091 -> :8091`), встроенным userspace-сборщиком образов `.ext4` и локальной IPC-заглушкой `maind`.

>[!tip]
> На **Windows x64** и **Linux x64** приложение `kuyase` работает автономно: снятие дампа по сетевому ADB (`kuyase dump`), очистка секретов и сборка загрузочного образа `work/<board>-system.ext4` (`kuyase prepare`) выполняются напрямую в userspace без WSL2, без `sudo mount -o loop`, без Python и без внешних утилит `adb`.

>[!warning]
> **Дампы прошивок не входят в репозиторий**
>
> Репозиторий содержит только исходный код Cargo Workspace (`protocol`, `hw-bridge`, `kuyase`), гостевой инициализатор и конфигурацию ядра Linux. Дамп прошивки необходимо снять со своей колонки по ADB (см. [`DUMP_GUIDE.md`](./DUMP_GUIDE.md)), собрать или взять из тематических ресурсов.

>[!note]
> Проект предназначен для **разработки, реверс-инжиниринга и автотестирования** (отладка собственных демонов, SDK, скриптов, LED-анимаций, голосового пайплайна). В собираемые образы `.ext4` не встраиваются сторонние SDK — подключение к эмулятору выполняется через ADB (`127.0.0.1:15555`) или встроенный терминал `ttyAMA0`.

Скриншоты панелей визуализации
![](assets/theme_dark.png)
![](assets/theme_light.png)

Видео демонстрация

Станция 3 (orion)

https://github.com/user-attachments/assets/6d5f3f82-f871-4222-8145-2aedb490f2d6

Станция мини (1 поколение)

https://github.com/user-attachments/assets/938ed4e6-effd-455e-b322-a2376517cb8e

---

## Архитектура Cargo Workspace

Репозиторий организован как единый Cargo Workspace из трёх крейтов:

| Крейт | Назначение |
| :--- | :--- |
| `protocol` | Общие структуры данных `BridgeStateSnapshot`, команд `HostCommand`, пакетов `GuestPacket` (`StateSnapshot`, `PcmAudioChunk`, `CommandAck`) и бинарный фрейминг `[u8 kind][u32_le len][payload]` поверх дуплексного TCP-соединения (`:18091 -> :8091`). |
| `hw-bridge` (`qemu-hw-bridge`) | Гостевой демон внутри виртуальной машины (статическая сборка под `armv7-unknown-linux-musleabihf` для совместимости с `32-bit armhf` и `64-bit aarch64` образами). Опрашивает полиморфные трейты периферии `BoardPeripherals`, обслуживает Protobuf IPC стека Quasar (`:9875`, `:9850`, `:9903`), перехватывает звук из `snd-aloop` и стримит состояния и PCM-аудио в хост-приложение. |
| `kuyase` | Нативное десктопное приложение (`eframe` / `egui` + `egui_tiles`) и CLI-инструмент (`run`, `test`, `dump`, `prepare`). Включает 60 FPS сцену `Virtual Speaker Twin`, воспроизведение гостевого звука на хосте с экспортом `.wav`, интерактивный терминал `ttyAMA0`, встроенный TCP ADB-клиент (`:5555` / `:15555`) и userspace-сборщик файловых систем `.ext4`. |

---

## Что эмулируется и какие заглушки применяются

| Подсистема | Программная заглушка в `KUYASE` |
| :--- | :--- |
| **Процессор, ядро и Device Tree** | Виртуальная машина `qemu-system-aarch64 -M virt -cpu cortex-a53` с ядром Linux `6.6.60` (`CONFIG_COMPAT=y` для одновременной поддержки `32-bit armhf` и `64-bit aarch64` userspace). Дерево `/proc/device-tree` (`model`, `compatible`, `amlogic-dt-id`) синтезируется и монтируется поверх `tmpfs` автоматически по профилю платы |
| **Светодиоды, часы и экран (`LED` / `FB` / `I2C`)** | Драйвер `i2c-stub` (`/dev/i2c-0` на адресах `0x3c`, `0x21`, `0x37`), виртуальные фреймбуферы `vfb` (`/dev/fb0..2`) и синтезированное дерево `/sys/class/leds/*` (с контролем триггера `ledfb-ring` и счётчиком `Trigger Conflict`). Состояние в реальном времени вычитывается демоном `qemu-hw-bridge` и рендерится в 60 FPS на холсте `Virtual Speaker Twin` |
| **Аудио, усилители и микрофонная матрица (`ALSA`)** | Звуковая петля `snd-aloop` (`Loopback`, `hw:0,0` <-> `hw:0,1`), программный регулятор громкости `softvol` (`Master`), эмуляция 4× усилителей `ACM8625S` и ультразвукового TDM-тракта `pcm.ultrasound` (`96 кГц` / `38.4 кГц` на `orion`). Поток `48 кГц` S16_LE PCM передаётся пачками `PcmAudioChunk` в `kuyase`, воспроизводится через аудиоустройство хоста, отображается на осциллограмме и сохраняется в `.wav` |
| **Синтез речи (`Formant TTS Synthesizer`)** | Процедурный формантный синтезатор речи на базе модели «источник Розенберга — фильтр Клаута» с правилами русской орфоэпии и интонационными контурами `ИК-1..3` (`hw-bridge/src/tts.rs`), озвучивающий директивы `START_VOCALIZE` и автоответы Алисы в `snd-aloop` с мгновенным прерыванием по `STOP_TTS` |
| **Кнопки и датчики (`ToF`, `ALS`, `Temp`, `DOA`)** | Инжекция физических нажатий через `/dev/uinput` (`KEY_VOLUMEUP`, `KEY_VOLUMEDOWN`, `KEY_MUTE`, `KEY_VOICECOMMAND`) и файловые стабы датчиков в `/sys/class/thermal/thermal_zone0/temp` и `/sys/bus/iio/devices/iio:device0/*`, управляемые интерактивными виджетами `Virtual Speaker Twin` |
| **Стек демонов Quasar (`maind`) и локальный IPC** | Вместо запуска оригинального монолита `maind` демон `qemu-hw-bridge` поднимает локальные Protobuf IPC-сокеты (`[u32_le_len][QuasarMessage]`): `aliced` (`:9875`), `yio_audio` (`:9850`, 4-канальный поток микрофонов `raw_mic_0..3` + `vqe_0` и угол `DOA`), `iohub_services` (`:9903`), поддерживает окно перехвата `1680 мс` (`Hijack Window`) и ведёт лог распознанных фраз в `/tmp/daemons_logs/maind.log` |

### Что не эмулируется

* **Облачный стек `maind` и сервисы Яндекса**: оригинальный бинарник `/system/vendor/quasar/maind` по умолчанию (`Stub Mode`) не запускается, так как требует заводские ключи, аппаратный TDM-поток и облачную авторизацию.
* **Беспроводные контроллеры**: чипы Wi-Fi/Bluetooth (`wlan0`, `hci0`) и Zigbee-координатор (`ttyS2` / `zigbee_control`) отсутствуют — сеть работает через виртуальный `virtio-net-pci` (`eth0`).
* **Аппаратные ускорители**: нейроускоритель `NPU` (`galcore`) и аппаратный аудио-DSP (`HiFi4`).
* **Криптохранилище и ключи устройства**: разделы `NAND nv`, `OTP efuse`, `unifykeys` и защищённая среда `OP-TEE` (`keymaster`).

---

## Поддерживаемые устройства

Модель колонки (`Board Profile`) и конфигурация эмулируемой периферии определяются автоматически по `/system/vendor/quasar/quasar.cfg`, `/boot/*.dtb`, `/etc/init.d/*` и `/system/vendor/quasar/ledpatterns/*.led`.

### Протестированные устройства

| Код | Модель | Userspace | Устройства |
| :--- | :--- | :--- | :--- |
| `yandexmini` | Станция Мини (`YNDX-0004`) | 32-bit `armhf` | 6-LED кольцо I2C `SN3236` (`0x3c`), лазерный дальномер `VL53L0X` (`ToF`), 4 микрофона |
| `yandexmini2` | Станция Мини 2 (`YNDX-00021`) | 32-bit `armhf` | 6-LED кольцо `mini2:*:ring*`, 7-сегментные часы `mini2:white:clock*`, датчик света `LTR-308` (`ALS`), 4 микрофона |
| `yandexmicro` | Станция Лайт (`YNDX-00025`) | 32-bit `armhf` | 3-LED кольцо `micro:*:ring*`, 4 микрофона |
| `cucumber` | Станция Миди (`YNDX-00054`) | 64-bit `aarch64` | 24-LED кольцо (`/dev/fb0`), 6-LED логотип (`/dev/fb1`), 7-сегментные часы `cucumber:white:clock-*`, датчик света `LTR-308` (`ALS`), 4 микрофона |
| `orion` | Станция 3 (`YNDX-00055`) | 64-bit `aarch64` | 36-LED кольцо (`/dev/fb2`), 12×2 RGBW эмбиент (`/dev/fb0`), монохромный LED-экран `21×12` (`/dev/fb1`, `gowin_led_device`), 4× усилителя `ACM8625S` (`2-002c`, `3-002c..2e`), ультразвук TDM (`hw:0,0`, `96 кГц` / `38.4 кГц`), датчик света `LTR-308` (`ALS`), 6 микрофонов |

### Не протестированные устройства

Заложен автопрофиль, требуется тестирование на реальных дампах:

| Код | Модель | Userspace | Устройства |
| :--- | :--- | :--- | :--- |
| `plum` | Станция Лайт 2 | 32-bit `armhf` | 6-LED кольцо, LED-часы, 4 микрофона |
| `bergamot` | Станция Мини 3 | 32-bit `armhf` | 6-LED кольцо (`/dev/fb0`), LED-часы, 4 микрофона |
| `pickle` | Станция Мини 3 Про | 64-bit `aarch64` | 16-LED кольцо, 6-LED логотип, 36-сегментные часы, датчик света `LTR-308` (`ALS`), 4 микрофона |
| `station2` | Станция 2 (`yandex,station-midi`) | 32-bit `armhf` | 64-LED панель (`/dev/fb0`), датчик света `LTR-308` (`ALS`), 4 микрофона |
| `mango` | Станция Стрит | 32-bit `armhf` | 3-LED логотип RGBW, 4-LED индикатор заряда |
| `saturn` | Умный хаб Яндекса | 32-bit `armhf` | 1 RGB-светодиод статуса (без аудио и микрофонов) |

>[!warning]
> **Устройства на базе Android (YaOS) не поддерживаются**
>
> Эмулятор рассчитан только на Linux-прошивки (Buildroot + нативный стек `maind`). Устройства на базе Android / YaOS (где стек Quasar поставляется в виде APK поверх Android Runtime с проверкой `AVB` / `dm-verity`) **не поддерживаются**:
> * Яндекс Станция (1-го поколения, `tulip-d1` / `YNDX-0001`, Android 6)
> * Яндекс Станция Макс (`yandexstation_2` / `YNDX-0008`, YaOS / Android 9)
> * Яндекс Станция Макс с Zigbee (`yandexstation_2` / `YNDX-00052`, YaOS / Android 9)
> * Яндекс Станция Дуо Макс (Android / YaOS)
> * Все модели Яндекс ТВ Станций и медиаплееров

---

## Сборка из исходников (Windows и Linux)

### 1. Требования

* **Rust** (`rustup`, `cargo`) с установленным таргетом `armv7-unknown-linux-musleabihf` для гостевого моста:
  ```bash
  rustup target add armv7-unknown-linux-musleabihf
  ```
* **QEMU** (`qemu-system-aarch64` в `./bin/`, `./Releases/` или системном `PATH`).
* **Гостевое ядро Linux** (`./kernel/Image` или `./Releases/Image`).

### 2. Сборка гостевого моста и десктопного приложения

На **Windows (PowerShell)** или **Linux (Bash)**:

```bash
# 1. Кросс-компиляция гостевого демона qemu-hw-bridge (статический musl ELF):
cargo build --release -p qemu-hw-bridge --target armv7-unknown-linux-musleabihf
cp -f target/armv7-unknown-linux-musleabihf/release/qemu-hw-bridge guest-init/bin/qemu-hw-bridge

# 2. Сборка нативного приложения и CLI-раннера kuyase:
cargo build --release -p kuyase
```

### 3. Пересборка гостевого ядра `kernel/Image` (опционально, Linux)

Если требуется пересобрать гостевое ядро Linux `6.6.60` (`arm64`, с `CONFIG_COMPAT=y`, патчами `i2c-stub` и тремя виртуальными фреймбуферами `vfb`):

```bash
sudo apt update && sudo apt install -y \
  git curl wget xz-utils build-essential gcc-aarch64-linux-gnu \
  flex bison bc libssl-dev libelf-dev dwarves

./kernel/build-kernel.sh
```

---

## Получение дампа, сборка образа и запуск

Все операции доступны как из графического интерфейса (`Board & Image Manager`), так и из командной строки.

### 1. Снятие дампа прошивки по сетевому ADB (`:5555`)

Используйте встроенный в `kuyase` Rust ADB-клиент (внешний `adb` не требуется, подробности в [`DUMP_GUIDE.md`](./DUMP_GUIDE.md)):

```bash
cargo run --release -p kuyase -- dump 10.1.1.56
```

Команда автоматически определит модель колонки, соберёт на устройстве временный архив системных разделов, скачает его в `backups/dump-<board>-<timestamp>/rootfs.tar.gz`, удалит временный файл на колонке и очистит архив от чувствительных данных (`wpa_supplicant.conf`, `/etc/shadow`, ключей и сторонних SSH-серверов).

### 2. Сборка гостевого образа `.ext4` в userspace

Поместите дамп (`rootfs.tar.gz`, `ubi0_0_rootfs.img`, `mtd3_system.bin`, `system.img[.gz]` или папку) в `backups/` или укажите путь явно:

```bash
# Собрать все дампы из папки backups/ в work/<board>-system.ext4:
cargo run --release -p kuyase -- prepare

# Либо собрать образ из конкретного дампа:
cargo run --release -p kuyase -- prepare --dump ./backups/dump-yandexmini-20261029_120000
```

### 3. Запуск нативного десктопного GUI

```bash
cargo run --release -p kuyase
```

В окне приложения доступны:
* Переключение раскладки между **Full Workbench** (доки `egui_tiles`: менеджер образов, 60 FPS `Virtual Speaker Twin`, терминал `ttyAMA0` + логи `maind.log`, осциллограмма звука и инспектор усилителей `ACM8625S`, инжекция голоса и тестирование Protobuf IPC) и **Compact Remote** (компактный пульт колонки). Настройки сохраняются портативно в `work/kuyase-ui.json`.
* Режим **Clean Boot (`-snapshot`)**, позволяющий откатывать все изменения гостевой ФС при выключении ВМ, и кнопка **Rebuild Image from Selected Dump**.

### 4. Headless-запуск и автотестирование (`kuyase test`)

```bash
# Быстрый headless-смоук-тест загрузки ВМ, дуплексного моста и ADB:
cargo run --release -p kuyase -- run --headless --board orion

# Полный интеграционный автотест периферии и IPC по всем собранным образам в work/:
cargo run --release -p kuyase -- test

# Интеграционный тест конкретной платы:
cargo run --release -p kuyase -- test --board yandexmini
```

---

## Сетевые порты виртуальной машины

* **Дуплексный поток `Hardware Bridge`**: `127.0.0.1:18091` -> гостевой `:8091` (флаг `--bridge-port`, бинарный протокол `[u8 kind][u32_le len][payload]`, описан в [`protocol/src/lib.rs`](./protocol/src/lib.rs)).
* **ADB (`127.0.0.1:15555` -> гостевой `:5555`, `root`)**: поддерживается как встроенным клиентом `kuyase`, так и стандартной утилитой `adb`:
  ```bash
  adb connect 127.0.0.1:15555
  adb -s 127.0.0.1:15555 shell
  ```
* **Проброшенный порт для пользовательских сервисов**: `127.0.0.1:18090` -> гостевой `:8090`.
