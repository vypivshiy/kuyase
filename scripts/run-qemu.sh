#!/usr/bin/env bash
# К.У.Я.С.Э. (KUYASE) — Запуск эмулятора и автосборка образа .ext4 (Linux / WSL / macOS)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KERNEL_IMG="${REPO_ROOT}/kernel/Image"
WORK_DIR="${REPO_ROOT}/work"
LAST_BOARD_FILE="${WORK_DIR}/.last-board"
PREP_SCRIPT="${REPO_ROOT}/scripts/prepare-rootfs.sh"

BOARD=""
MODE="stub"
DUMP_PATH=""
REBUILD=0
BRIDGE_PORT=18091
SDK_PORT=18090
ADB_PORT=15555

usage() {
  cat << 'EOF'
Использование:
  ./scripts/run-qemu.sh [опции] [board]

Опции:
  -b, --board <slug>       Код модели (yandexmini, yandexmini2, cucumber, orion, auto и др.)
  -d, --dump <путь>        Путь к папке дампа или файлу (rootfs.tar.gz, ubi0_0_rootfs.img, system.img[.gz])
  -m, --mode <stub|full>   Режим демонов: stub (по умолчанию, быстрый старт) или full (настоящий maind)
  -r, --rebuild            Принудительно пересобрать образ .ext4 перед запуском
      --bridge-port <порт> Порт 2D-визуализатора Virtual Speaker Twin (по умолчанию: 18091)
      --sdk-port <порт>    Проброс гостевого порта :8090 на хост (по умолчанию: 18090)
      --adb-port <порт>    Проброс гостевого ADB :5555 на хост (по умолчанию: 15555)
  -h, --help               Показать эту справку

Примеры:
  # Собрать образ из папки дампа и сразу запустить (модель определится автоматически):
  ./scripts/run-qemu.sh --dump ./backups/dump-yandexmini-20261029_120000

  # Собрать только образ без запуска QEMU:
  sudo ./scripts/prepare-rootfs.sh --dump ./backups/dump-yandexmini-20261029_120000

  # Запустить ранее собранный образ:
  ./scripts/run-qemu.sh
  ./scripts/run-qemu.sh yandexmini2
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    -b|--board)       BOARD="$2"; shift 2 ;;
    -d|--dump)        DUMP_PATH="$2"; shift 2 ;;
    -m|--mode)        MODE="$2"; shift 2 ;;
    -r|--rebuild)     REBUILD=1; shift ;;
    --bridge-port)    BRIDGE_PORT="$2"; shift 2 ;;
    --sdk-port)       SDK_PORT="$2"; shift 2 ;;
    --adb-port)       ADB_PORT="$2"; shift 2 ;;
    -h|--help)        usage; exit 0 ;;
    -*)               echo "[kuyase] Неизвестная опция: $1"; usage; exit 1 ;;
    *)
      if [ -z "${BOARD}" ]; then
        BOARD="$1"
      fi
      shift
      ;;
  esac
done

if [ ! -f "${KERNEL_IMG}" ]; then
  if [ -f "${REPO_ROOT}/Releases/Image" ]; then
    KERNEL_IMG="${REPO_ROOT}/Releases/Image"
  fi
fi

if [ ! -f "${KERNEL_IMG}" ]; then
  echo "[kuyase] Ядро ${KERNEL_IMG} не найдено."
  echo "Соберите ядро командой: ./kernel/build-kernel.sh"
  exit 1
fi

QEMU_BIN=""
if command -v qemu-system-aarch64 >/dev/null 2>&1; then
  QEMU_BIN="qemu-system-aarch64"
elif [ -x "/mnt/c/Program Files/qemu/qemu-system-aarch64.exe" ]; then
  QEMU_BIN="/mnt/c/Program Files/qemu/qemu-system-aarch64.exe"
else
  echo "[kuyase] Не найден qemu-system-aarch64. Установите пакет qemu-system-arm / qemu-system-aarch64."
  exit 1
fi

run_prep() {
  if [ "$(id -u)" -eq 0 ]; then
    bash "${PREP_SCRIPT}" "$@"
  elif command -v sudo >/dev/null 2>&1; then
    sudo bash "${PREP_SCRIPT}" "$@"
  else
    bash "${PREP_SCRIPT}" "$@"
  fi
}

mkdir -p "${WORK_DIR}"

if [ -n "${DUMP_PATH}" ]; then
  if [ ! -e "${DUMP_PATH}" ]; then
    echo "[kuyase] Указанный путь к дампу не найден: ${DUMP_PATH}"
    exit 1
  fi
  REQ_BOARD="${BOARD:-auto}"
  echo "[kuyase] Сборка образа .ext4 из дампа: ${DUMP_PATH} ..."
  run_prep "${REQ_BOARD}" "${DUMP_PATH}"
  if [ -z "${BOARD}" ] && [ -f "${LAST_BOARD_FILE}" ]; then
    BOARD="$(tr -d '\r\n ' < "${LAST_BOARD_FILE}")"
  fi
fi

if [ -z "${BOARD}" ]; then
  if [ -f "${LAST_BOARD_FILE}" ]; then
    CANDIDATE="$(tr -d '\r\n ' < "${LAST_BOARD_FILE}")"
    if [ -n "${CANDIDATE}" ] && [ -f "${WORK_DIR}/${CANDIDATE}-system.ext4" ]; then
      BOARD="${CANDIDATE}"
    fi
  fi
  if [ -z "${BOARD}" ]; then
    LATEST_IMG="$(ls -1t "${WORK_DIR}"/*-system.ext4 2>/dev/null | grep -v '\-system-orig\.ext4$' | head -n 1 || true)"
    if [ -n "${LATEST_IMG}" ]; then
      BOARD="$(basename "${LATEST_IMG}" -system.ext4)"
    fi
  fi
fi

if [ -z "${BOARD}" ]; then
  echo "[kuyase] В папке work/ ещё нет собранных образов. Проверяем папку backups/..."
  run_prep
  if [ -f "${LAST_BOARD_FILE}" ]; then
    BOARD="$(tr -d '\r\n ' < "${LAST_BOARD_FILE}")"
  fi
fi

case "${BOARD}" in
  station-lite|lite|lite1|micro) BOARD="yandexmicro" ;;
  mini|mini1)                    BOARD="yandexmini" ;;
  mini2|yandexmini_2)            BOARD="yandexmini2" ;;
  mini3)                         BOARD="bergamot" ;;
  mini3pro)                      BOARD="pickle" ;;
  lite2)                         BOARD="plum" ;;
  midi)                          BOARD="cucumber" ;;
  station3)                      BOARD="orion" ;;
  station_2|station-midi|yandexmidi) BOARD="station2" ;;
  street)                        BOARD="mango" ;;
  hub)                           BOARD="saturn" ;;
esac

DRIVE_IMG="${WORK_DIR}/${BOARD}-system.ext4"
if [ ! -f "${DRIVE_IMG}" ] && [ -f "${WORK_DIR}/${BOARD}-system.tar.gz" ]; then
  echo "[kuyase] Распаковка готового образа из ${WORK_DIR}/${BOARD}-system.tar.gz..."
  tar -xzf "${WORK_DIR}/${BOARD}-system.tar.gz" -C "${WORK_DIR}"
fi

if [ "${REBUILD}" -eq 1 ] && [ -z "${DUMP_PATH}" ]; then
  echo "[kuyase] Пересборка образа ${BOARD}-system.ext4..."
  run_prep "${BOARD}"
fi

if [ ! -f "${DRIVE_IMG}" ]; then
  echo "=================================================================="
  echo " [kuyase] Образ '${DRIVE_IMG}' не найден!"
  echo " Дампы прошивок не поставляются в комплекте с репозиторием."
  echo ""
  echo " 1) Снимите дамп со своей колонки по сетевому ADB (см. DUMP_GUIDE.md):"
  echo "    ./scripts/dump-from-adb.sh <IP_колонки>"
  echo ""
  echo " 2) Или укажите путь к самостоятельно снятому / скачанному на профильных"
  echo "    ресурсах дампу (папка, rootfs.tar.gz, ubi0_0_rootfs.img, system.img):"
  echo "    ./scripts/run-qemu.sh --dump ./backups/my-speaker-dump"
  echo "=================================================================="
  exit 1
fi

HOST_FWD="hostfwd=tcp:127.0.0.1:${BRIDGE_PORT}-:8091,hostfwd=tcp:127.0.0.1:${ADB_PORT}-:5555,hostfwd=tcp:127.0.0.1:${SDK_PORT}-:8090"

BRIDGE_BIN=""
CARGO_BRIDGE="${REPO_ROOT}/hw-bridge/target/armv7-unknown-linux-musleabihf/release/qemu-hw-bridge"
INIT_BRIDGE="${REPO_ROOT}/guest-init/bin/qemu-hw-bridge"
REL_BRIDGE="${REPO_ROOT}/Releases/qemu-hw-bridge"
if [ -f "${CARGO_BRIDGE}" ]; then
  mkdir -p "${REPO_ROOT}/guest-init/bin"
  cp -f "${CARGO_BRIDGE}" "${INIT_BRIDGE}" 2>/dev/null || true
  BRIDGE_BIN="${CARGO_BRIDGE}"
elif [ -f "${INIT_BRIDGE}" ]; then
  BRIDGE_BIN="${INIT_BRIDGE}"
elif [ -f "${REL_BRIDGE}" ]; then
  BRIDGE_BIN="${REL_BRIDGE}"
fi

EXTRA_DRIVE_ARGS=()
if [ -n "${BRIDGE_BIN}" ] && [ -f "${BRIDGE_BIN}" ]; then
  EXTRA_DRIVE_ARGS=(-drive "file=${BRIDGE_BIN},if=virtio,format=raw,readonly=on")
fi

echo "=================================================================="
echo " К.У.Я.С.Э. (KUYASE) — Кустарный Универсальный Яндекс Станции Эмулятор"
echo " Board: ${BOARD} | maind_mode: ${MODE} | Image: work/${BOARD}-system.ext4"
echo " -> Virtual Speaker Twin (2D UI) : http://127.0.0.1:${BRIDGE_PORT}"
echo " -> ADB Access                   : adb connect 127.0.0.1:${ADB_PORT}"
echo " -> Forwarded Guest SDK Port     : 127.0.0.1:${SDK_PORT} -> guest :8090"
echo " -> Stop Emulator                : Press Ctrl+C or Ctrl+D (or Enter for root shell)"
echo "=================================================================="

exec "${QEMU_BIN}" \
  -M virt -cpu cortex-a53 -smp 2 -m 512M \
  -kernel "${KERNEL_IMG}" \
  -append "console=ttyAMA0 root=/dev/vda rw ip=dhcp init=/quasar-qemu-init.sh quasar.board=${BOARD} quasar.mode=${MODE} panic=-1" \
  -drive "file=${DRIVE_IMG},if=virtio,format=raw" \
  "${EXTRA_DRIVE_ARGS[@]}" \
  -netdev "user,id=net0,${HOST_FWD}" \
  -device virtio-net-pci,netdev=net0 \
  -nographic
