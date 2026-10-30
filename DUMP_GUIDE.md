# Снятие дампа прошивки для `KUYASE`

Инструкция по снятию системного дампа с колонки Яндекса по сетевому ADB (`<IP>:5555`).

>[!warning]
> **Особенности сетевого ADB на колонках Яндекса**
> * Выполняйте команды строго последовательно — параллельные обращения к `<IP>:5555` вызывают `protocol fault` на старом демоне `adbd`.
> * Скачивайте бинарные архивы только через `adb pull` — перенаправление `adb shell "tar ..." > file.tar.gz` повреждает архив из-за замены `\n` (`0x0A`) на `\r\n` (`0x0D 0x0A`) в `stdout`.

---

## Состав дампа

| Файл | Содержимое |
| :--- | :--- |
| `rootfs.tar.gz` | Архив системных каталогов корня (`bin sbin lib lib32 lib64 usr etc system boot linuxrc`) без временных сетевых конфигов и хешей паролей |

>[!note]
> Разделы `/data`, `/bkupgrade`, `/nv`, `/env`, `/ota` и `/tee` (токены аккаунта Яндекса, пароли Wi-Fi, серийные номера и ключи устройства) в дамп **не входят**.

### Требования к свободной памяти на устройстве и ПК

Так как `adb pull` скачивает обычный файл с файловой системы колонки, перед передачей по сети архив временно собирается на самом устройстве (в ОЗУ `/tmp` либо во флеш-памяти `/bkupgrade` / `/data`), а после скачивания автоматически удаляется:

| Архитектура и модели | Формат дампа | Требуется на колонке (временно) | Размер на ПК (дампа / образа `.ext4`) |
| :--- | :--- | :--- | :--- |
| **32-bit `armhf` (NAND UBIFS)**<br>`yandexmini`, `yandexmini2`, `yandexmicro`, `plum`, `bergamot`, `station2`, `mango`, `saturn` | `rootfs.tar.gz`<br>`ubi0_0_rootfs.img`<br>`mtd3_system.bin` | **45 – 55 МиБ** (в ОЗУ `/tmp` или NAND `/bkupgrade`)<br>**72 – 76 МиБ** (в ОЗУ `/tmp`)<br>**80 – 128 МиБ** (в ОЗУ `/tmp`) | ~45–55 МиБ / **256 МиБ**<br>~72–76 МиБ / **256 МиБ**<br>~80–128 МиБ / **256 МиБ** |
| **64-bit `aarch64` (eMMC / NAND)**<br>`cucumber` (Миди), `orion` (Станция 3), `pickle` (Мини 3 Про) | `rootfs.tar.gz`<br>`system.img.gz`<br>`system.img` | **150 – 300 МиБ** (в `/data`, `/bkupgrade` или `/tmp`)<br>**150 – 300 МиБ** (в `/data`)<br>**512 – 1024 МиБ** (в `/data`) | ~150–300 МиБ / **1024 МиБ**<br>~150–300 МиБ / **1024 МиБ**<br>~1024 МиБ / **1024 МиБ** |

>[!tip]
> На колонках с `>= 256 МиБ` ОЗУ (например, `yandexmini`, где в `tmpfs /tmp` свободно ~117 МиБ) скрипт создаёт временный архив прямо в ОЗУ (`/tmp`), не расходуя ресурс перезаписи NAND-флеш. На 64-битных колонках с крупным системным разделом (`150–300 МиБ` в сжатом виде) скрипт автоматически выбирает раздел `/data` или `/bkupgrade`, если архив не помещается в `/tmp`.

---

## Вариант А — Скриптом из репозитория

Укажите IP-адрес колонки с открытым портом `5555`:

```bash
# Linux / WSL2 / macOS:
./scripts/dump-from-adb.sh 10.1.1.56
```

```powershell
# Windows PowerShell:
.\scripts\dump-from-adb.ps1 10.1.1.56
```

Скрипт автоматически определит модель колонки, выберет раздел со свободным местом для временного архива (`/bkupgrade`, `/data` или `/tmp`), скачает файлы в `backups/dump-<модель>-<дата>/` и очистит временные файлы на устройстве.

---

## Вариант Б — Вручную через `adb` (без клонирования репозитория)

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

Для колонок с NAND-памятью (например, `yandexmini`, `yandexmini2`, `yandexmicro`) вместо `rootfs.tar.gz` можно снять образ тома UBIFS `/dev/ubi0_0` или раздела `/dev/mtd3`:

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

## Детекция и очистка чувствительных данных (`sanitize-rootfs`)

Чтобы не перегружать первичный дамп на самом устройстве, фильтрация сетевых настроек (`wpa_supplicant.conf`, DHCP-лизов, локальных IP в `/etc/hosts`), ключей `/tee` и сторонних модификаций (`dropbear`, `S99alinad`, `S100privacy`) вынесена в отдельный скрипт на ПК (`scripts/sanitize-rootfs.sh` / `scripts/sanitize-rootfs.ps1`). Сборщик `scripts/prepare-rootfs.sh` вызывает его автоматически, но вы также можете запускать его отдельно для аудита (`--check`) или очистки (`--clean`) папок дампа и готовых `.ext4` образов:

```bash
# Linux / WSL2 — аудит всех образов в work/*.ext4 или конкретной папки дампа:
sudo ./scripts/sanitize-rootfs.sh --check
sudo ./scripts/sanitize-rootfs.sh --check backups/dump-yandexmini

# Очистка папки дампа или пересборка образов .ext4 с обнулением свободных блоков:
sudo ./scripts/sanitize-rootfs.sh --clean backups/dump-yandexmini
sudo ./scripts/sanitize-rootfs.sh --clean
```

```powershell
# Windows PowerShell:
.\scripts\sanitize-rootfs.ps1 -Check
.\scripts\sanitize-rootfs.ps1 -Check backups\dump-yandexmini
.\scripts\sanitize-rootfs.ps1 -Clean backups\dump-yandexmini
```

---

>[!note]
> **Поддерживаемые форматы дампов и запуск**
> Помимо папки с `rootfs.tar.gz`, сборщик `scripts/prepare-rootfs.sh` напрямую принимает образы NAND UBIFS (`ubi0_0_rootfs.img`, `mtd3_system.bin`) и eMMC (`system.img`, `system.img.gz`). Инструкция по сборке и запуску эмулятора приведена в [`README.md`](./README.md).
