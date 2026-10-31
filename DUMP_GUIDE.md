# Снятие дампа прошивки для `KUYASE`

Инструкция по снятию системного дампа (`Firmware Dump`) с колонки Яндекса по сетевому ADB (`<IP>:5555`) и сборке загрузочного образа в userspace.

>[!warning]
> **Особенности сетевого ADB на колонках Яндекса**
> * Выполняйте обращения к `<IP>:5555` строго последовательно — параллельные TCP-сессии вызывают `protocol fault` на старом демоне `adbd`. Встроенный клиент `kuyase dump` соблюдает это ограничение автоматически.
> * При ручном снятии скачивайте бинарные архивы только через `adb pull` (протокол `sync:`) — перенаправление `adb shell "tar ..." > file.tar.gz` повреждает архив из-за замены `\n` (`0x0A`) на `\r\n` (`0x0D 0x0A`) в потоке `shell:`.

---

## Состав дампа

| Файл | Содержимое |
| :--- | :--- |
| `rootfs.tar.gz` | Архив системных каталогов корня (`bin sbin lib lib32 lib64 usr etc system boot linuxrc`) без пользовательских разделов, сетевых конфигов и хешей паролей |
| `dump-info.json` | Метаданные снятого дампа (`board_slug`, `device_type_cfg`, `dt_model`, таблица разделов `df -k`) |

>[!note]
> Разделы `/data`, `/bkupgrade`, `/nv`, `/env`, `/ota` и `/tee` (токены аккаунта Яндекса, пароли Wi-Fi, серийные номера и ключи устройства) в дамп **не входят**.

### Требования к свободной памяти на устройстве и ПК

Перед передачей по ADB `sync:` архив временно собирается на самом устройстве (в ОЗУ `/tmp` либо во флеш-памяти `/bkupgrade` / `/data`), а сразу после скачивания автоматически удаляется:

| Архитектура и модели | Формат дампа | Требуется на колонке (временно) | Размер на ПК (дампа / образа `.ext4`) |
| :--- | :--- | :--- | :--- |
| **32-bit `armhf` (NAND UBIFS)**<br>`yandexmini`, `yandexmini2`, `yandexmini2nc`, `yandexmicro`, `plum`, `plumnc`, `station2`, `bergamot`, `mango`, `saturn` | `rootfs.tar.gz`<br>`ubi0_0_rootfs.img`<br>`mtd3_system.bin` | **45 – 55 МиБ** (в ОЗУ `/tmp` или NAND `/bkupgrade`)<br>**72 – 76 МиБ** (в ОЗУ `/tmp`)<br>**80 – 128 МиБ** (в ОЗУ `/tmp`) | ~45–55 МиБ / **256 МиБ**<br>~72–76 МиБ / **256 МиБ**<br>~80–128 МиБ / **256 МиБ** |
| **64-bit `aarch64` (eMMC / NAND)**<br>`cucumber` (Миди), `orion` (Станция 3), `pickle` (Мини 3 Про) | `rootfs.tar.gz`<br>`system.img.gz`<br>`system.img` | **150 – 300 МиБ** (в `/data`, `/bkupgrade` или `/tmp`)<br>**150 – 300 МиБ** (в `/data`)<br>**512 – 1024 МиБ** (в `/data`) | ~150–300 МиБ / **1024 МиБ**<br>~150–300 МиБ / **1024 МиБ**<br>~1024 МиБ / **1024 МиБ** |

>[!tip]
> На колонках с `>= 256 МиБ` ОЗУ (например, `yandexmini`, где в `tmpfs /tmp` свободно ~117 МиБ) `kuyase dump` создаёт временный архив прямо в ОЗУ (`/tmp`), не расходуя ресурс перезаписи NAND-флеш. На 64-битных колонках с крупным системным разделом (`150–300 МиБ` в сжатом виде) автоматически выбирается раздел `/bkupgrade` или `/data`, если архив не помещается в `/tmp`.

---

## Вариант А — Встроенным клиентом `kuyase` (CLI или GUI, без `platform-tools`)

Приложение `kuyase` содержит собственную реализацию протокола ADB TCP (`CNXN`, `OPEN`, `WRTE`, `CLSE` и `sync:` `RECV`/`DATA`) на Rust (`kuyase/src/adb.rs`, `kuyase/src/dumper.rs`), поэтому установка внешней утилиты `adb` не требуется.

### Из командной строки (Windows / Linux):

```bash
# Снять дамп в backups/dump-<board>-<timestamp>/:
cargo run --release -p kuyase -- dump 10.1.1.56

# С указанием нестандартного порта или целевой папки:
cargo run --release -p kuyase -- dump 10.1.1.56:5555 --out ./backups/my-speaker-dump
```

### Из графического интерфейса (`kuyase`):

1. Откройте панель **Board & Image Manager** -> секцию **3. Network ADB Firmware Dumper (:5555)**.
2. Введите адрес колонки (например, `10.1.1.56:5555`) и нажмите **Pull Clean rootfs.tar.gz via ADB**.
3. После завершения скачивания нажмите **Build All from backups/** (или выполните `cargo run --release -p kuyase -- prepare`) для сборки `work/<board>-system.ext4`.

---

## Вариант Б — Вручную через внешнюю утилиту `adb`

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

## Автоматическая очистка чувствительных данных и userspace-сборка `.ext4`

И встроенный дампер `kuyase dump`, и сборщик образов `kuyase prepare` (`kuyase/src/image_builder.rs`) автоматически пропускают дерево файлов через фильтр очистки (`sanitize_tree`) в оперативной памяти:
* Очищается содержимое каталогов `/data/*`, `/bkupgrade/*`, `/tee/*`, `/nv/*`, `/env/*`, `/ota/*`, `/tmp/*`, `/run/*`, `/var/run/*`, `/var/db/dhcpcd/*`, `/var/lib/chrony/*`.
* Удаляются учётные данные Wi-Fi и ключи (`/etc/wpa_supplicant.conf`, `/etc/hostapd_temp.conf`, `/etc/wifi/ap_name`, `/etc/shadow`, `/root/.ssh`, `/root/.ash_history`, базы `*.db` и `*.sqlite*` в `/etc`, `/var`, `/root`).
* Удаляются сторонние SSH-серверы и модификации (`/etc/dropbear`, `/usr/local/bin/dropbear`, `/usr/local/bin/equasar-daemon`, `/etc/init.d/S99zdropbear`, `/etc/init.d/S99alinad`, `/etc/init.d/S100privacy`).

После очистки сборщик нормализует окончания строк `CRLF -> LF` в `/quasar-qemu-init.sh`, инжектирует `/quasar-qemu-init.sh` (`0755`) и `/usr/local/bin/qemu-hw-bridge` (`0755`) и формирует чистый образ `work/<board>-system.ext4` напрямую в userspace (без `sudo mount -o loop` и без WSL2):

```bash
cargo run --release -p kuyase -- prepare --dump ./speaker-dump/rootfs.tar.gz
```

>[!note]
> **Поддерживаемые входные форматы `kuyase prepare`**
> Сборщик напрямую принимает архивы `rootfs.tar.gz` / `*.tgz` / `*.tar` / `*.zip`, распакованные директории, а также образы разделов `system.img[.gz]`, `ubi0_0_rootfs.img` и `mtd3_system.bin`. Инструкция по запуску и тестированию эмулятора приведена в [`README.md`](./README.md).
