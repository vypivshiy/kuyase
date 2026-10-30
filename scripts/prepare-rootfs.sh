#!/usr/bin/env bash
# К.У.Я.С.Э. (KUYASE) — Сборщик загрузочных образов .ext4 из дампов прошивок (Linux / WSL)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="${REPO_ROOT}/work"
BACKUPS_DIR="${REPO_ROOT}/backups"
RELEASES_DIR="${REPO_ROOT}/Releases"

BRIDGE_BIN="${REPO_ROOT}/hw-bridge/target/armv7-unknown-linux-musleabihf/release/qemu-hw-bridge"
if [ ! -f "${BRIDGE_BIN}" ]; then
  if [ -f "${REPO_ROOT}/guest-init/bin/qemu-hw-bridge" ]; then
    BRIDGE_BIN="${REPO_ROOT}/guest-init/bin/qemu-hw-bridge"
  elif [ -f "${RELEASES_DIR}/qemu-hw-bridge" ]; then
    BRIDGE_BIN="${RELEASES_DIR}/qemu-hw-bridge"
  fi
fi

INIT_SH="${REPO_ROOT}/guest-init/equasar-qemu-init.sh"

usage() {
  cat << 'EOF'
Использование:
  ./scripts/prepare-rootfs.sh [опции] [board] [путь_к_дампу]

Опции:
  -b, --board <slug>   Код модели (yandexmini, yandexmini2, cucumber, orion, auto и др.)
  -d, --dump <путь>    Путь к папке дампа или файлу (rootfs.tar.gz, ubi0_0_rootfs.img, mtd3_system.bin, system.img[.gz])
  -h, --help           Показать эту справку

Примеры:
  # Автоопределение модели и сборка образа .ext4 из папки или файла дампа:
  sudo ./scripts/prepare-rootfs.sh --dump ./backups/dump-yandexmini-20261029_120000
  sudo ./scripts/prepare-rootfs.sh --dump ./speaker-dump/rootfs.tar.gz
  sudo ./scripts/prepare-rootfs.sh --board yandexmini2 --dump ./ubi0_0_rootfs.img

  # Собрать все дампы, лежащие в папке ./backups/:
  sudo ./scripts/prepare-rootfs.sh
EOF
}

TARGET_BOARD=""
TARGET_DUMP=""
ORIG_ARGS=("$@")

while [ $# -gt 0 ]; do
  case "$1" in
    -b|--board) TARGET_BOARD="$2"; shift 2 ;;
    -d|--dump)  TARGET_DUMP="$2"; shift 2 ;;
    -h|--help)  usage; exit 0 ;;
    -*)         echo "[prepare] Неизвестная опция: $1"; usage; exit 1 ;;
    *)
      if [ -z "${TARGET_BOARD}" ]; then
        TARGET_BOARD="$1"
      elif [ -z "${TARGET_DUMP}" ]; then
        TARGET_DUMP="$1"
      fi
      shift
      ;;
  esac
done

# Auto-elevate via sudo if not root (needed for loop-mounting ext4 images)
if [ "$(id -u)" -ne 0 ]; then
  if command -v sudo >/dev/null 2>&1; then
    exec sudo bash "$0" "${ORIG_ARGS[@]}"
  else
    echo "[prepare] Для монтирования loop-образов ext4 требуются права root (запустите через sudo)."
    exit 1
  fi
fi

mkdir -p "${WORK_DIR}"
sed -i 's/\r$//' "${INIT_SH}"

if [ ! -f "${BRIDGE_BIN}" ]; then
  echo "[prepare] Не найден бинарник qemu-hw-bridge."
  echo "Соберите мост вручную:"
  echo "  cd hw-bridge && cargo build --release --target armv7-unknown-linux-musleabihf"
  echo "  cp -f target/armv7-unknown-linux-musleabihf/release/qemu-hw-bridge ../guest-init/bin/qemu-hw-bridge"
  exit 1
fi

detect_board_from_mnt() {
  local mnt="$1"
  local dev_type=""
  if [ -f "${mnt}/system/vendor/quasar/quasar.cfg" ]; then
    dev_type="$(grep -o '"deviceType"[[:space:]]*:[[:space:]]*"[^"]*"' "${mnt}/system/vendor/quasar/quasar.cfg" 2>/dev/null | head -n 1 | sed 's/.*"deviceType"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/' || true)"
  fi
  case "${dev_type}" in
    yandexmini_2|mini2)             echo "yandexmini2" ;;
    station_2|station-midi|yandexmidi) echo "station2" ;;
    mini3)                          echo "bergamot" ;;
    mini3pro)                       echo "pickle" ;;
    lite2)                          echo "plum" ;;
    lite1|micro)                    echo "yandexmicro" ;;
    station3)                       echo "orion" ;;
    street)                         echo "mango" ;;
    hub)                            echo "saturn" ;;
    "")
      if [ -e "${mnt}/etc/init.d/S61leds_brightness" ]; then
        echo "yandexmini2"
      elif [ -e "${mnt}/boot/yandex-cucumber-i2ctop-overlay.dtb" ]; then
        echo "cucumber"
      elif [ -e "${mnt}/boot/yandex-orion-i2c_top-overlay.dtb" ]; then
        echo "orion"
      else
        echo "yandexmini"
      fi
      ;;
    *) echo "${dev_type}" ;;
  esac
}

resolve_dump_source() {
  local src="$1"
  if [ -d "${src}" ]; then
    if [ -f "${src}/rootfs.tar.gz" ]; then
      echo "${src}/rootfs.tar.gz"
    elif ls "${src}"/*rootfs*.tgz >/dev/null 2>&1; then
      ls "${src}"/*rootfs*.tgz | head -n 1
    elif ls "${src}"/*.tgz >/dev/null 2>&1; then
      ls "${src}"/*.tgz | head -n 1
    elif [ -f "${src}/system.img.gz" ]; then
      echo "${src}/system.img.gz"
    elif [ -f "${src}/system.img" ]; then
      echo "${src}/system.img"
      elif [ -f "${src}/ubivol/ubi0_0_rootfs.img" ]; then
      echo "${src}/ubivol/ubi0_0_rootfs.img"
    elif [ -f "${src}/ubi0_0_rootfs.img" ]; then
      echo "${src}/ubi0_0_rootfs.img"
    elif [ -f "${src}/ubi0_0_rootfs.img.gz" ]; then
      echo "${src}/ubi0_0_rootfs.img.gz"
    elif [ -f "${src}/mtd/mtd3_system.bin" ]; then
      echo "${src}/mtd/mtd3_system.bin"
    elif [ -f "${src}/mtd3_system.bin" ]; then
      echo "${src}/mtd3_system.bin"
    elif [ -f "${src}/mtd/mtd3_system.bin.gz" ]; then
      echo "${src}/mtd/mtd3_system.bin.gz"
    elif [ -f "${src}/mtd3_system.bin.gz" ]; then
      echo "${src}/mtd3_system.bin.gz"
    else
      echo ""
    fi
  elif [ -f "${src}" ]; then
    echo "${src}"
  else
    echo ""
  fi
}

sanitize_mounted_rootfs() {
  local mnt="$1"
  bash "${REPO_ROOT}/scripts/sanitize-rootfs.sh" --clean "${mnt}"
}

build_and_inject_from_dump() {
  local req_board="$1"
  local raw_src="$2"
  local src_dir=""
  local src=""
  local tmp_zip_dir=""

  case "${raw_src}" in
    *.zip)
      if [ -f "${raw_src}" ]; then
        echo "[prepare] Распаковка ZIP-архива дампа ${raw_src}..."
        tmp_zip_dir="/tmp/zip-dump-$$"
        rm -rf "${tmp_zip_dir}"
        mkdir -p "${tmp_zip_dir}"
        python3 - "${raw_src}" "${tmp_zip_dir}" << 'PYEOF'
import os, sys, zipfile
zip_path, out_dir = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(zip_path, "r") as zf:
    for info in zf.infolist():
        norm = info.filename.replace("\\", "/").lstrip("/")
        if not norm or ".." in norm.split("/"):
            continue
        dest = os.path.join(out_dir, norm)
        if info.is_dir() or info.filename.endswith(("/", "\\")):
            os.makedirs(dest, exist_ok=True)
        else:
            os.makedirs(os.path.dirname(dest), exist_ok=True)
            with zf.open(info) as src, open(dest, "wb") as dst:
                dst.write(src.read())
PYEOF
        raw_src="${tmp_zip_dir}"
      fi
      ;;
  esac

  if [ -d "${raw_src}" ]; then
    src_dir="${raw_src}"
  else
    src_dir="$(dirname "${raw_src}")"
  fi
  src="$(resolve_dump_source "${raw_src}")"

  if [ -z "${src}" ] || [ ! -f "${src}" ]; then
    [ -n "${tmp_zip_dir}" ] && rm -rf "${tmp_zip_dir}"
    echo "[prepare] Не найден поддерживаемый файл дампа (rootfs.tar.gz, ubi0_0_rootfs.img, mtd3_system.bin, system.img[.gz]) в: ${raw_src}"
    return 1
  fi

  local tmp_img="${WORK_DIR}/.tmp-build-orig.ext4"
  rm -f "${tmp_img}"

  local src_size_mb=0
  if [ -f "${src}" ]; then
    src_size_mb=$(( $(stat -c '%s' "${src}" 2>/dev/null || echo 0) / 1048576 ))
  fi
  local img_size_spec="256M"
  if [ "${src_size_mb}" -ge 85 ]; then
    img_size_spec="1024M"
  fi

  case "${src}" in
    *.tar.gz|*.tgz)
      if tar -tzf "${src}" 2>/dev/null | head -n 5 | grep -Eq '^[^/]+\.(ext4|img)$'; then
        echo "[prepare] Распаковка готового образа ext4 из архива ${src}..."
        tar -xOzf "${src}" > "${tmp_img}"
      else
        echo "[prepare] Создание образа ext4 (${img_size_spec}) из архива ${src}..."
        truncate -s "${img_size_spec}" "${tmp_img}"
        mke2fs -t ext4 -F -L system "${tmp_img}" >/dev/null
        mkdir -p /tmp/mnt-prep
        umount /tmp/mnt-prep 2>/dev/null || true
        mount -o loop "${tmp_img}" /tmp/mnt-prep
        tar -xzf "${src}" -C /tmp/mnt-prep
        rm -f /tmp/mnt-prep/root/.ssh/authorized_keys /tmp/mnt-prep/etc/shadow 2>/dev/null || true
        mkdir -p /tmp/mnt-prep/{proc,sys,dev,tmp,run,var,mnt,media,root,ota} /tmp/mnt-prep/bkupgrade/data/quasar/daemons_logs
        [ -L /tmp/mnt-prep/data ] || mkdir -p /tmp/mnt-prep/data
        chmod 1777 /tmp/mnt-prep/tmp
        umount /tmp/mnt-prep
      fi
      ;;
    *ubi0_0_rootfs.img|*mtd3_system.bin|*.ubi|*.ubifs|*ubi0_0_rootfs.img.gz|*mtd3_system.bin.gz|*.ubi.gz|*.ubifs.gz)
      echo "[prepare] Распаковка UBIFS/UBI образа ${src}..."
      rm -rf /tmp/ubi-extract /tmp/ubi-raw.bin
      mkdir -p /tmp/ubi-extract
      local ubi_src="${src}"
      case "${src}" in
        *.gz)
          zcat "${src}" > /tmp/ubi-raw.bin
          ubi_src="/tmp/ubi-raw.bin"
          ;;
      esac
      ubireader_extract_files -k -o /tmp/ubi-extract "${ubi_src}" >/dev/null
      rm -f /tmp/ubi-raw.bin
      local extracted_root="/tmp/ubi-extract"
      if [ ! -d "${extracted_root}/system" ]; then
        extracted_root="$(find /tmp/ubi-extract -maxdepth 3 -name 'system' -type d | head -n 1 | xargs dirname)"
      fi
      local extracted_mb
      extracted_mb="$(du -sm "${extracted_root}" 2>/dev/null | awk '{print $1}')"
      if [ "${extracted_mb:-0}" -ge 190 ]; then
        img_size_spec="1024M"
      fi
      truncate -s "${img_size_spec}" "${tmp_img}"
      mke2fs -t ext4 -F -L system "${tmp_img}" >/dev/null
      mkdir -p /tmp/mnt-prep
      umount /tmp/mnt-prep 2>/dev/null || true
      mount -o loop "${tmp_img}" /tmp/mnt-prep
      cp -a "${extracted_root}/." /tmp/mnt-prep/
      mkdir -p /tmp/mnt-prep/{proc,sys,dev,tmp,run,var,mnt,media,root,ota} /tmp/mnt-prep/bkupgrade/data/quasar/daemons_logs
      [ -L /tmp/mnt-prep/data ] || mkdir -p /tmp/mnt-prep/data
      chmod 1777 /tmp/mnt-prep/tmp
      umount /tmp/mnt-prep
      rm -rf /tmp/ubi-extract
      ;;
    *.img.gz)
      echo "[prepare] Распаковка сжатого образа ext4 ${src}..."
      zcat "${src}" > "${tmp_img}"
      ;;
    *.img|*.ext4)
      echo "[prepare] Копирование образа ext4 ${src}..."
      cp -f "${src}" "${tmp_img}"
      ;;
    *)
      echo "[prepare] Неподдерживаемый формат дампа: ${src}"
      return 1
      ;;
  esac

  e2fsck -fy "${tmp_img}" >/dev/null 2>&1 || true

  # Mount to inspect board type and sanitize rootfs
  mkdir -p /tmp/mnt-prep
  umount /tmp/mnt-prep 2>/dev/null || true
  mount -o loop "${tmp_img}" /tmp/mnt-prep

  local detected_board
  detected_board="$(detect_board_from_mnt /tmp/mnt-prep)"
  local board="${req_board}"
  if [ -z "${board}" ] || [ "${board}" = "auto" ]; then
    board="${detected_board}"
  fi

  # Sanitize any sensitive or runtime host artifacts across all dump formats (tar.gz, UBIFS, UBI, ext4)
  sanitize_mounted_rootfs /tmp/mnt-prep

  sync
  umount /tmp/mnt-prep
  [ -n "${tmp_zip_dir}" ] && rm -rf "${tmp_zip_dir}"

  local orig_img="${WORK_DIR}/${board}-system-orig.ext4"
  mv -f "${tmp_img}" "${orig_img}"
  inject_guest_tools "${board}"
  echo "${board}" > "${WORK_DIR}/.last-board"
}

inject_guest_tools() {
  local board="$1"
  local orig_img="${WORK_DIR}/${board}-system-orig.ext4"
  local img="${WORK_DIR}/${board}-system.ext4"

  if [ ! -s "${orig_img}" ] && [ -s "${img}" ]; then
    cp -f "${img}" "${orig_img}"
  fi

  if [ ! -s "${orig_img}" ]; then
    echo "[prepare] Пропуск ${board}: базовый образ ${orig_img} не найден."
    return 1
  fi

  cp -f "${orig_img}" "${img}"
  e2fsck -fy "${img}" >/dev/null 2>&1 || true

  echo "[prepare] Инжекция гостевого инициализатора, ADB и hw-bridge в ${img}..."
  mkdir -p /tmp/mnt-prep
  umount /tmp/mnt-prep 2>/dev/null || true
  mount -o loop "${img}" /tmp/mnt-prep
  sanitize_mounted_rootfs /tmp/mnt-prep
  mkdir -p /tmp/mnt-prep/usr/local/bin
  cp -f "${INIT_SH}" /tmp/mnt-prep/quasar-qemu-init.sh
  cp -f "${BRIDGE_BIN}" /tmp/mnt-prep/usr/local/bin/qemu-hw-bridge
  chmod 0755 /tmp/mnt-prep/quasar-qemu-init.sh /tmp/mnt-prep/usr/local/bin/qemu-hw-bridge
  chmod 0755 /tmp/mnt-prep/bin/busybox /tmp/mnt-prep/lib/ld-* /tmp/mnt-prep/usr/bin/* /tmp/mnt-prep/sbin/* /tmp/mnt-prep/system/vendor/quasar/maind /tmp/mnt-prep/system/vendor/quasar/*.sh /tmp/mnt-prep/system/workdir/bin/* 2>/dev/null || true

  sync
  umount /tmp/mnt-prep
  echo "[prepare] Готов загрузочный образ: ${img} (профиль платы: ${board})"
}

# If called with explicit dump or board
if [ -n "${TARGET_DUMP}" ]; then
  build_and_inject_from_dump "${TARGET_BOARD:-auto}" "${TARGET_DUMP}"
  exit 0
elif [ -n "${TARGET_BOARD}" ]; then
  # If user passed a path as first positional arg (e.g. ./scripts/prepare-rootfs.sh ./speaker-dump)
  if [ -e "${TARGET_BOARD}" ]; then
    build_and_inject_from_dump "auto" "${TARGET_BOARD}"
  else
    inject_guest_tools "${TARGET_BOARD}"
    echo "${TARGET_BOARD}" > "${WORK_DIR}/.last-board"
  fi
  exit 0
fi

# Otherwise scan backups/ for any user-provided dump folders or files
FOUND_ANY=0
LAST_DUMP_BOARD=""
if [ -d "${BACKUPS_DIR}" ]; then
  for entry in "${BACKUPS_DIR}"/*; do
    [ -e "${entry}" ] || continue
    [ "$(basename "${entry}")" = "README.md" ] && continue
    resolved="$(resolve_dump_source "${entry}")"
    if [ -n "${resolved}" ]; then
      FOUND_ANY=1
      build_and_inject_from_dump "auto" "${entry}"
      if [ -f "${WORK_DIR}/.last-board" ]; then
        LAST_DUMP_BOARD="$(cat "${WORK_DIR}/.last-board")"
      fi
    fi
  done
fi

# If work/*-system-orig.ext4 already exist, re-inject guest tools into them
for orig in "${WORK_DIR}"/*-system-orig.ext4; do
  [ -e "${orig}" ] || continue
  FOUND_ANY=1
  b="$(basename "${orig}" -system-orig.ext4)"
  inject_guest_tools "${b}"
  if [ -z "${LAST_DUMP_BOARD}" ]; then
    echo "${b}" > "${WORK_DIR}/.last-board"
  fi
done
if [ -n "${LAST_DUMP_BOARD}" ]; then
  echo "${LAST_DUMP_BOARD}" > "${WORK_DIR}/.last-board"
fi

if [ "${FOUND_ANY}" -eq 0 ]; then
  echo "=================================================================="
  echo "[prepare] Дампы прошивок не найдены в папке backups/."
  echo "Дампы проприетарных прошивок НЕ поставляются в комплекте с KUYASE."
  echo ""
  echo "Чтобы собрать образ .ext4 для эмулятора:"
  echo "  1) Снимите дамп со своей колонки по сетевому ADB (см. DUMP_GUIDE.md):"
  echo "     Linux / macOS / WSL : ./scripts/dump-from-adb.sh <IP_колонки>"
  echo "     Windows PowerShell  : .\\scripts\\dump-from-adb.ps1 <IP_колонки>"
  echo ""
  echo "  2) Или укажите путь к готовому дампу (rootfs.tar.gz / ubi0_0_rootfs.img / system.img):"
  echo "     Linux / WSL (только сборка) : sudo ./scripts/prepare-rootfs.sh --dump <путь_к_дампу>"
  echo "     Linux / WSL (сборка+запуск) : ./scripts/run-qemu.sh --dump <путь_к_дампу>"
  echo "     Windows PowerShell          : .\\scripts\\run-qemu.ps1 -DumpPath <путь_к_дампу>"
  echo "=================================================================="
  exit 1
fi

echo "[prepare] Все найденные образы готовы в ${WORK_DIR}!"
