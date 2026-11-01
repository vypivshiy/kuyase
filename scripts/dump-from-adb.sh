#!/usr/bin/env bash
# КУЯСЭ (KUYASE) — Автономное снятие системного дампа колонки по сетевому ADB (Linux / macOS / WSL)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ "$(basename "${SCRIPT_DIR}")" = "scripts" ] && [ -d "${SCRIPT_DIR}/../guest-init" ]; then
  DEFAULT_BASE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)/backups"
else
  DEFAULT_BASE_DIR="$(pwd)"
fi

if [ $# -lt 1 ] || [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  cat << 'EOF'
Использование:
  ./scripts/dump-from-adb.sh <IP_или_IP:5555> [папка_назначения]

Примеры:
  ./scripts/dump-from-adb.sh 10.1.1.56
  ./scripts/dump-from-adb.sh 10.1.1.56:5555 ./backups/my-speaker-dump
EOF
  exit 0
fi

TARGET="$1"
OUT_DIR="${2:-}"

case "${TARGET}" in
  *:*) ;;
  *)   TARGET="${TARGET}:5555" ;;
esac

ADB_BIN=""
if command -v adb >/dev/null 2>&1; then
  ADB_BIN="adb"
elif command -v adb.exe >/dev/null 2>&1; then
  ADB_BIN="adb.exe"
else
  for cand in \
    /mnt/c/Users/*/AppData/Local/Microsoft/WinGet/Links/adb.exe \
    /mnt/c/Users/*/AppData/Local/Microsoft/WinGet/Packages/Google.PlatformTools*/platform-tools/adb.exe \
    /mnt/c/Users/*/AppData/Local/Android/Sdk/platform-tools/adb.exe; do
    if [ -x "${cand}" ]; then
      ADB_BIN="${cand}"
      break
    fi
  done
fi

if [ -z "${ADB_BIN}" ]; then
  echo "[dump] Ошибка: утилита 'adb' не найдена в PATH (установите: sudo apt install android-tools-adb)."
  exit 1
fi

adb_cmd() {
  "${ADB_BIN}" -s "${TARGET}" "$@"
}

adb_pull() {
  local remote_path="$1"
  local local_path="$2"
  case "${ADB_BIN}" in
    *.exe)
      if command -v wslpath >/dev/null 2>&1; then
        local_path="$(wslpath -w "${local_path}")"
      fi
      ;;
  esac
  "${ADB_BIN}" -s "${TARGET}" pull "${remote_path}" "${local_path}"
}

echo "[1/3] Подключение к колонке по сети (${TARGET})..."
"${ADB_BIN}" connect "${TARGET}"

MODEL_RAW="$(adb_cmd shell "cat /proc/device-tree/compatible /proc/device-tree/model 2>/dev/null" | tr '\0\r\n' '   ')"
DEV_TYPE_CFG="$(adb_cmd shell 'sed -n "s/.*\"deviceType\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" /system/vendor/quasar/quasar.cfg 2>/dev/null | head -n 1' | tr -d '\r\n ')"

SLUG="unknown"
case "${MODEL_RAW}" in
  *prune*) SLUG="plumnc" ;;
esac

if [ "${SLUG}" = "unknown" ]; then
  case "${DEV_TYPE_CFG}" in
    cucumber)                                    SLUG="cucumber" ;;
    bergamot|mini3|yandexmini3|yandexmini_3)     SLUG="bergamot" ;;
    pickle|mini3pro)                             SLUG="pickle" ;;
    orion|station3)                              SLUG="orion" ;;
    plumnc|prune|lite2nc)                        SLUG="plumnc" ;;
    plum|lite2)                                  SLUG="plum" ;;
    mango|street)                                SLUG="mango" ;;
    saturn|hub)                                  SLUG="saturn" ;;
    station_2|station-midi|yandexmidi|station2)  SLUG="station2" ;;
    yandexmini_2_no_clock|yandexmini2nc|mini2nc) SLUG="yandexmini2nc" ;;
    yandexmini_2|mini2|yandexmini2)              SLUG="yandexmini2" ;;
    yandexmicro|lite1|micro)                     SLUG="yandexmicro" ;;
    yandexmini|mini1)                            SLUG="yandexmini" ;;
    ?*)                                          SLUG="${DEV_TYPE_CFG}" ;;
  esac
fi

if [ "${SLUG}" = "unknown" ]; then
  case "${MODEL_RAW}" in
    *cucumber*)               SLUG="cucumber" ;;
    *bergamot*)               SLUG="bergamot" ;;
    *pickle*)                 SLUG="pickle" ;;
    *orion*)                  SLUG="orion" ;;
    *prune*)                  SLUG="plumnc" ;;
    *plum*)                   SLUG="plum" ;;
    *mango*)                  SLUG="mango" ;;
    *saturn*)                 SLUG="saturn" ;;
    *station-midi*)           SLUG="station2" ;;
    *yandexmini2nc*|*mini2nc*) SLUG="yandexmini2nc" ;;
    *yandexmini_2*|*mini2*)   SLUG="yandexmini2" ;;
    *yandexmicro*)            SLUG="yandexmicro" ;;
    *axg*)                    SLUG="yandexmini" ;;
  esac
fi

if [ -z "${OUT_DIR}" ]; then
  TIMESTAMP="$(date +%Y%m%d_%H%M%S)"
  OUT_DIR="${DEFAULT_BASE_DIR}/dump-${SLUG}-${TIMESTAMP}"
fi
mkdir -p "${OUT_DIR}"

echo " -> Определена плата : ${SLUG}"
echo " -> Папка сохранения : ${OUT_DIR}"

echo "[2/3] Сборка системного архива rootfs.tar.gz (без /data, /bkupgrade, /nv, /tee)..."
SCRATCH_OUT="$(adb_cmd shell '
TMP_FREE=$(df -k /tmp 2>/dev/null | awk "NR==2 {print \$4}")
BK_FREE=$( [ -d /bkupgrade ] && df -k /bkupgrade 2>/dev/null | awk "NR==2 {print \$4}" || echo 0 )
DATA_FREE=$( [ -d /data ] && df -k /data 2>/dev/null | awk "NR==2 {print \$4}" || echo 0 )
ROOT_USED=$(df -k / 2>/dev/null | awk "NR==2 {print \$3}")
NEED_KB=$(( ${ROOT_USED:-80000} * 80 / 100 ))
[ "$NEED_KB" -lt 70000 ] && NEED_KB=70000

SCRATCH="/tmp/quasar_emu_rootfs.tar.gz"
if [ "${TMP_FREE:-0}" -ge "$NEED_KB" ]; then
  SCRATCH="/tmp/quasar_emu_rootfs.tar.gz"
elif [ "${BK_FREE:-0}" -ge "$NEED_KB" ] && [ "${BK_FREE:-0}" -ge "${DATA_FREE:-0}" ]; then
  SCRATCH="/bkupgrade/quasar_emu_rootfs.tar.gz"
elif [ "${DATA_FREE:-0}" -ge "$NEED_KB" ]; then
  SCRATCH="/data/quasar_emu_rootfs.tar.gz"
elif [ "${BK_FREE:-0}" -gt "${TMP_FREE:-0}" ]; then
  SCRATCH="/bkupgrade/quasar_emu_rootfs.tar.gz"
elif [ "${DATA_FREE:-0}" -gt "${TMP_FREE:-0}" ]; then
  SCRATCH="/data/quasar_emu_rootfs.tar.gz"
fi
rm -f "$SCRATCH"
cd /
DIRS=""
for d in bin sbin lib lib32 lib64 usr etc system boot linuxrc; do
  [ -e "$d" ] && DIRS="$DIRS $d"
done
tar -cf - $DIRS 2>/dev/null | gzip -1 > "$SCRATCH"
echo "SCRATCH_PATH=$SCRATCH"
ls -lh "$SCRATCH"
')"
echo "${SCRATCH_OUT}"

SCRATCH_PATH="$(echo "${SCRATCH_OUT}" | grep -o 'SCRATCH_PATH=[^[:space:]]*' | head -n 1 | cut -d= -f2 | tr -d '\r')"
[ -z "${SCRATCH_PATH}" ] && SCRATCH_PATH="/tmp/quasar_emu_rootfs.tar.gz"

echo "[3/3] Скачивание rootfs.tar.gz по сети и очистка временного файла на колонке..."
adb_pull "${SCRATCH_PATH}" "${OUT_DIR}/rootfs.tar.gz"
adb_cmd shell "rm -f ${SCRATCH_PATH}"

echo "=================================================================="
echo " Дамп успешно сохранён в: ${OUT_DIR}"
ls -lh "${OUT_DIR}"
echo "=================================================================="
