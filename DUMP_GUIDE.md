# Снятие дампа прошивки для `KUYASE`

Инструкция по снятию системного дампа (`Firmware Dump`) с колонки Яндекса по сетевому ADB (`<IP>:5555`).

>[!warning]
> **Особенности сетевого ADB на колонках Яндекса**
> * Выполняйте обращения к `<IP>:5555` строго последовательно — параллельные TCP-сессии вызывают `protocol fault` на старом демоне `adbd`.
> * При ручном снятии скачивайте бинарные архивы только через `adb pull` (протокол `sync:`) — перенаправление `adb shell "tar ..." > file.tar.gz` повреждает архив из-за замены `\n` (`0x0A`) на `\r\n` (`0x0D 0x0A`) в потоке `shell:`.

---

## Состав дампа

| Файл | Содержимое |
| :--- | :--- |
| `rootfs.tar.gz` | Архив системных каталогов корня (`bin sbin lib lib32 lib64 usr etc system boot linuxrc`) без пользовательских разделов, сетевых конфигов и хешей паролей |
| `dump-info.json` | Метаданные снятого дампа (`board_slug`, `device_type_cfg`, `dt_model`, таблица разделов `df -k`, создаётся при снятии через `kuyase dump`) |

>[!note]
> Разделы `/data`, `/bkupgrade`, `/nv`, `/env`, `/ota` и `/tee` (токены аккаунта Яндекса, пароли Wi-Fi, серийные номера и ключи устройства) в дамп **не входят**.

### Требования к свободной памяти на устройстве и ПК

Перед передачей по сети архив временно собирается на самом устройстве (в ОЗУ `/tmp` либо во флеш-памяти `/bkupgrade` / `/data`), а сразу после скачивания автоматически удаляется:

| Архитектура и модели | Формат дампа | Требуется на колонке (временно) | Размер на ПК (дампа / образа `.ext4`) |
| :--- | :--- | :--- | :--- |
| **32-bit `armhf` (NAND UBIFS)**<br>`yandexmini`, `yandexmini2`, `yandexmini2nc`, `yandexmicro`, `plum`, `plumnc`, `station2`, `bergamot`, `mango`, `saturn` | `rootfs.tar.gz`<br>`ubi0_0_rootfs.img`<br>`mtd3_system.bin` | **45 – 55 МиБ** (в ОЗУ `/tmp` или NAND `/bkupgrade`)<br>**72 – 76 МиБ** (в ОЗУ `/tmp`)<br>**80 – 128 МиБ** (в ОЗУ `/tmp`) | ~45–55 МиБ / **256 МиБ**<br>~72–76 МиБ / **256 МиБ**<br>~80–128 МиБ / **256 МиБ** |
| **64-bit `aarch64` (eMMC / NAND)**<br>`cucumber` (Миди), `orion` (Станция 3), `pickle` (Мини 3 Про) | `rootfs.tar.gz`<br>`system.img.gz`<br>`system.img` | **150 – 300 МиБ** (в `/data`, `/bkupgrade` или `/tmp`)<br>**150 – 300 МиБ** (в `/data`)<br>**512 – 1024 МиБ** (в `/data`) | ~150–300 МиБ / **1024 МиБ**<br>~150–300 МиБ / **1024 МиБ**<br>~1024 МиБ / **1024 МиБ** |

>[!tip]
> На колонках с `>= 256 МиБ` ОЗУ (например, `yandexmini`, где в `tmpfs /tmp` свободно ~117 МиБ) временный архив создаётся прямо в ОЗУ (`/tmp`), не расходуя ресурс перезаписи NAND-флеш. На 64-битных колонках с крупным системным разделом (`150–300 МиБ` в сжатом виде) автоматически выбирается раздел `/bkupgrade` или `/data`, если архив не помещается в `/tmp`.

---

## Вариант А — Автономным скриптом (без установки эмулятора)

Если нужно только снять дамп с колонки (например, чтобы поделиться архивом прошивки) и не требуется устанавливать сам эмулятор `kuyase`, достаточно стандартной утилиты `adb` и одного скрипта из папки `scripts/`:

```bash
# Linux / macOS / WSL:
./scripts/dump-from-adb.sh 10.1.1.56
```

```powershell
# Windows PowerShell:
.\scripts\dump-from-adb.ps1 10.1.1.56
```

Скрипт автоматически определит модель колонки, выберет раздел со свободным местом для временного архива (`/tmp`, `/bkupgrade` или `/data`), скачает `rootfs.tar.gz` в `backups/dump-<модель>-<дата>/` и удалит временный файл на колонке.

---

## Вариант Б — Вручную через `adb` (без скриптов и клонирования репозитория)

### 1. Подключение

```bash
S="10.1.1.56:5555"   # В PowerShell: $S = "10.1.1.56:5555"
adb connect $S
mkdir -p speaker-dump
```

### 2. Сборка и скачивание `rootfs.tar.gz`

```bash
adb -s $S shell '
OUT="/tmp/rootfs.tar.gz"
if [ $(df -k /tmp | awk "NR==2 {print \$4}") -lt 70000 ]; then
  if [ -d /bkupgrade ] && [ $(df -k /bkupgrade | awk "NR==2 {print \$4}") -gt 70000 ]; then
    OUT="/bkupgrade/rootfs.tar.gz"
  elif [ -d /data ] && [ $(df -k /data | awk "NR==2 {print \$4}") -gt 70000 ]; then
    OUT="/data/rootfs.tar.gz"
  fi
fi
rm -f "$OUT" /tmp/rootfs_pull.tar.gz
cd /
DIRS=""
for d in bin sbin lib lib32 lib64 usr etc system boot linuxrc; do [ -e "$d" ] && DIRS="$DIRS $d"; done
tar -cf - $DIRS 2>/dev/null | gzip -1 > "$OUT"
[ "$OUT" != "/tmp/rootfs_pull.tar.gz" ] && ln -sf "$OUT" /tmp/rootfs_pull.tar.gz
ls -lh "$OUT"
'

adb -s $S pull /tmp/rootfs_pull.tar.gz speaker-dump/rootfs.tar.gz
adb -s $S shell "rm -f /bkupgrade/rootfs.tar.gz /data/rootfs.tar.gz /tmp/rootfs.tar.gz /tmp/rootfs_pull.tar.gz"
```

### 3. Альтернатива: снятие сырого образа UBIFS (`ubi0_0_rootfs.img`) или раздела NAND (`mtd3_system.bin`)

```bash
# Вариант с томом UBIFS (/dev/ubi0_0):
adb -s $S shell "dd if=/dev/ubi0_0 of=/tmp/ubi0_0_rootfs.img bs=126976"
adb -s $S pull /tmp/ubi0_0_rootfs.img speaker-dump/ubi0_0_rootfs.img
adb -s $S shell "rm -f /tmp/ubi0_0_rootfs.img"

# Или вариант с разделом NAND (/dev/mtd3):
adb -s $S shell "dd if=/dev/mtd3 of=/tmp/mtd3_system.bin bs=131072"
adb -s $S pull /tmp/mtd3_system.bin speaker-dump/mtd3_system.bin
adb -s $S shell "rm -f /tmp/mtd3_system.bin"
```

---

## Вариант В — Встроенным клиентом `kuyase` (без установки `adb`)

В сам эмулятор `kuyase` уже встроен собственный TCP ADB-клиент (`kuyase/src/adb.rs`, `kuyase/src/dumper.rs`), поэтому внешняя утилита `adb` не нужна.

### Из графического интерфейса (`kuyase`):

1. Нажмите в верхней панели кнопку **«Образы и QEMU...»**.
2. Введите адрес колонки (`10.1.1.56:5555`) и нажмите **«Снять дамп и собрать .ext4»** — программа скачает `rootfs.tar.gz` в `backups/dump-<board>-<timestamp>/`, очистит его от секретов и сразу соберёт образ `work/<board>-system.ext4`.

### Из командной строки (Windows / Linux):

```bash
# 1. Снять чистый дамп в backups/dump-<board>-<timestamp>/:
kuyase dump 10.1.1.56

# 2. Собрать загрузочный образ work/<board>-system.ext4 из снятого дампа:
kuyase prepare --dump ./backups/dump-yandexmini-20261029_120000
```

---

## Очистка чувствительных данных (`sanitize`)

>[!warning]
> **Проверяйте дамп и образ вручную перед публикацией**
>
> Автоматические алгоритмы очистки (`kuyase dump`, `kuyase prepare` и `scripts/sanitize-rootfs.*`) работают по известным путям и шаблонам и **не гарантируют 100% удаления всех личных данных** (например, если токены, ключи, пароли или кастомные конфиги были сохранены в нестандартных директориях). Перед передачей или публикацией `rootfs.tar.gz` / `.ext4` обязательно проверьте содержимое архива или готового образа самостоятельно.

При снятии дампа через `kuyase dump` и при сборке образа через `kuyase prepare` (`kuyase/src/image_builder.rs`) архив автоматически проходит очистку в оперативной памяти:
* Очищается содержимое каталогов `/data/*`, `/bkupgrade/*`, `/tee/*`, `/nv/*`, `/env/*`, `/ota/*`, `/tmp/*`, `/run/*`, `/var/run/*`, `/var/db/dhcpcd/*`, `/var/lib/chrony/*`.
* Удаляются учётные данные Wi-Fi и ключи (`/etc/wpa_supplicant.conf`, `/etc/hostapd_temp.conf`, `/etc/wifi/ap_name`, `/etc/shadow`, `/root/.ssh`, `/root/.ash_history`, базы `*.db` и `*.sqlite*` в `/etc`, `/var`, `/root`).
* Очищаются ключи SSH (`dropbear`) и пользовательские init-патчи постоянного доступа (`/etc/dropbear`, `/usr/local/bin/dropbear`, `/etc/init.d/S99zdropbear`, `/etc/init.d/S97adb`, `/etc/init.d/S89usbgadget`, `/etc/init.d/S99alinad`, `/etc/init.d/S100privacy`), чтобы они не конфликтовали с инициализацией `adbd` в QEMU и не содержали личных ключей владельца.

Если дамп снимался автономным скриптом или вручную (без `kuyase`), проверить и вычистить архив `rootfs.tar.gz` перед публикацией можно также отдельным скриптом `scripts/sanitize-rootfs.sh` (или `.ps1`):

```bash
# Аудит папки дампа или архива без изменений:
sudo ./scripts/sanitize-rootfs.sh --check ./backups/dump-yandexmini

# Очистка и перепаковка архива rootfs.tar.gz:
sudo ./scripts/sanitize-rootfs.sh --clean ./backups/dump-yandexmini
```

```powershell
# Windows PowerShell (через WSL2):
.\scripts\sanitize-rootfs.ps1 -Check .\backups\dump-yandexmini
.\scripts\sanitize-rootfs.ps1 -Clean .\backups\dump-yandexmini
```

>[!note]
> **Поддерживаемые входные форматы `kuyase prepare`**
> Сборщик образов напрямую принимает архивы `rootfs.tar.gz` / `*.tgz` / `*.tar` / `*.zip`, распакованные директории, а также образы разделов `system.img[.gz]`, `ubi0_0_rootfs.img` и `mtd3_system.bin`. Инструкция по запуску эмулятора приведена в [`README.md`](./README.md).
