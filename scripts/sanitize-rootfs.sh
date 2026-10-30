#!/usr/bin/env bash
# Детектор и очиститель чувствительных данных (ID/серийный номер устройства,
# Wi-Fi/сетевые настройки, авторизация/токены Яндекса, ключи /tee и SSH)
# в дампах прошивок и образах .ext4. Не модифицирует вендорские скрипты прошивки.
#
# Использование:
#   sudo bash ./scripts/sanitize-rootfs.sh --check [путь_к_дампу_или_ext4_или_папке]
#   sudo bash ./scripts/sanitize-rootfs.sh --clean [путь_к_дампу_или_ext4_или_папке]
#
# Если путь не указан, проверяются/очищаются все образы в work/*.ext4.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
WORK_DIR="${REPO_ROOT}/work"

MODE="clean"
TARGETS=()
ORIG_ARGS=("$@")

usage() {
  cat << 'EOF'
Использование:
  sudo ./scripts/sanitize-rootfs.sh [--check | --clean] [цель ...]

Режимы:
  --check, -c   Только детекция (аудит) без изменения файлов. Возвращает код 1 при обнаружении.
  --clean, -f   Детекция + очистка чувствительных данных (по умолчанию). Для .ext4 при наличии
                следов в неразмеченных блоках пересобирает ФС с нуля (mke2fs -d).

Цели (если не указаны — проверяются все work/*.ext4):
  - Папка с распакованным rootfs (например, точка монтирования /tmp/mnt-prep)
  - Папка дампа (содержащая rootfs.tar.gz)
  - Архив rootfs (*.tar.gz, *.tgz)
  - Образ файловой системы (*.ext4, *.img)
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --check|-c) MODE="check"; shift ;;
    --clean|--fix|-f) MODE="clean"; shift ;;
    --help|-h) usage; exit 0 ;;
    -*)
      echo "[sanitize] Неизвестный флаг: $1" >&2
      usage
      exit 1
      ;;
    *)
      TARGETS+=("$1")
      shift
      ;;
  esac
done

if [ "$(id -u)" -ne 0 ]; then
  if command -v sudo >/dev/null 2>&1; then
    exec sudo bash "$0" "${ORIG_ARGS[@]}"
  else
    echo "[sanitize] Требуются права root (запустите через sudo или wsl -u root)." >&2
    exit 1
  fi
fi

if [ "${#TARGETS[@]}" -eq 0 ]; then
  shopt -s nullglob
  for img in "${WORK_DIR}"/*.ext4; do
    TARGETS+=("${img}")
  done
  shopt -u nullglob
fi

if [ "${#TARGETS[@]}" -eq 0 ]; then
  echo "[sanitize] Цели для проверки не найдены."
  exit 0
fi

TOTAL_ISSUES=0

report_issue() {
  local target_label="$1"
  local category="$2"
  local detail="$3"
  echo "  [!] [${target_label}] (${category}) ${detail}"
  TOTAL_ISSUES=$((TOTAL_ISSUES + 1))
}

# ---------------------------------------------------------------------------
# 1. Аудит и очистка дерева корневой ФС (распакованная директория или mountpoint)
# ---------------------------------------------------------------------------
audit_and_clean_rootfs_tree() {
  local root="$1"
  local label="$2"
  local do_clean="$3"

  # [A] Пользовательские разделы, токены, ключи шифрования и runtime-данные
  # (/data, /bkupgrade, /tee, /nv, /env, /ota, /var/db/dhcpcd, /var/lib/chrony)
  local part_dirs=("data" "bkupgrade" "tee" "nv" "env" "ota" "tmp" "run" "var/run" "var/db/dhcpcd" "var/lib/chrony")
  for pd in "${part_dirs[@]}"; do
    local full_pd="${root}/${pd}"
    if [ -d "${full_pd}" ] && [ ! -L "${full_pd}" ]; then
      local files
      files="$(find "${full_pd}" -type f ! -name '*NOT_MOUNTED*' 2>/dev/null || true)"
      if [ -n "${files}" ]; then
        while IFS= read -r f; do
          [ -n "${f}" ] || continue
          report_issue "${label}" "AUTH_AND_STORAGE" "Файл в пользовательском/ключевом разделе: /${f#${root}/}"
        done <<< "${files}"
      fi
      if [ "${do_clean}" = "1" ]; then
        rm -rf "${full_pd:?}/"* 2>/dev/null || true
      fi
    fi
  done

  # [B] Файлы учетных данных Wi-Fi, паролей, ключей SSH/Dropbear и сторонних демонов в /etc/init.d
  local secret_paths=(
    "etc/wpa_supplicant.conf"
    "etc/hostapd_temp.conf"
    "etc/wifi/ap_name"
    "etc/shadow"
    "etc/dropbear"
    "etc/dhcpcd.exit-hook"
    "etc/unbound/unbound.conf.head"
    "etc/init.d/S99zdropbear"
    "etc/init.d/S99alinad"
    "etc/init.d/S100privacy"
    "etc/init.d/S89usbgadget"
    "etc/init.d/S97adb"
    "root/.ssh"
    "root/.ash_history"
    "usr/local/bin/dropbear"
    "usr/local/bin/equasar-daemon"
    "usr/bin/adbd_backup"
  )
  for sp in "${secret_paths[@]}"; do
    if [ -e "${root}/${sp}" ] || [ -L "${root}/${sp}" ]; then
      report_issue "${label}" "SENSITIVE_FILE" "Обнаружен файл с чувствительными данными: /${sp}"
      if [ "${do_clean}" = "1" ]; then
        rm -rf "${root:?}/${sp}" 2>/dev/null || true
      fi
    fi
  done

  # Поиск любых БД SQLite (*.db, *.sqlite*) в /etc, /var, /root
  local db_files
  db_files="$(find "${root}/etc" "${root}/var" "${root}/root" \( -name "*.db" -o -name "*.sqlite*" \) -type f 2>/dev/null || true)"
  if [ -n "${db_files}" ]; then
    while IFS= read -r dbf; do
      [ -n "${dbf}" ] || continue
      report_issue "${label}" "AUTH_DB" "База данных в системном разделе: /${dbf#${root}/}"
      if [ "${do_clean}" = "1" ]; then
        rm -f "${dbf}" 2>/dev/null || true
      fi
    done <<< "${db_files}"
  fi

  # [C] Нештатные записи / локальные IP-адреса (MITM/DNS) в /etc/hosts и /etc/init.d/*
  if [ -f "${root}/etc/hosts" ]; then
    local extra_hosts
    extra_hosts="$(grep -vE '^(127\.0\.0\.1[[:space:]]+localhost|127\.0\.1\.1[[:space:]]+[A-Za-z0-9_.-]+|[[:space:]]*)$' "${root}/etc/hosts" 2>/dev/null || true)"
    if [ -n "${extra_hosts}" ]; then
      report_issue "${label}" "NETWORK_HOSTS" "Нештатные записи/IP в /etc/hosts ($(echo "${extra_hosts}" | wc -l) стр.)"
      if [ "${do_clean}" = "1" ]; then
        local hn="buildroot"
        hn="$(awk '$1 == "127.0.1.1" {print $2; exit}' "${root}/etc/hosts" 2>/dev/null || true)"
        [ -z "${hn}" ] && hn="buildroot"
        printf "127.0.0.1\tlocalhost\n127.0.1.1\t%s\n" "${hn}" > "${root}/etc/hosts"
      fi
    fi
  fi

  if [ -d "${root}/etc/init.d" ]; then
    local init_ips
    init_ips="$(grep -rnI -E '(10\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}|192\.168\.[0-9]{1,3}\.[0-9]{1,3})' "${root}/etc/init.d" 2>/dev/null | grep -vE '(10\.10\.10\.254|10\.0\.0\.0|192\.168\.5\.1)' || true)"
    if [ -n "${init_ips}" ]; then
      while IFS= read -r line; do
        [ -n "${line}" ] || continue
        report_issue "${label}" "NETWORK_IP" "Локальный IP-адрес в init-скрипте: ${line#${root}/}"
      done <<< "${init_ips}"
      if [ "${do_clean}" = "1" ]; then
        sed -i -E 's/RouterDNS="10\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}"/RouterDNS="8.8.8.8"/g' "${root}/etc/init.d/"* 2>/dev/null || true
      fi
    fi
  fi

  if [ "${do_clean}" = "1" ]; then
    mkdir -p "${root}/proc" "${root}/sys" "${root}/dev" "${root}/tmp" "${root}/run" "${root}/var/run" "${root}/mnt" "${root}/media" "${root}/root" "${root}/ota" "${root}/tee" "${root}/bkupgrade"
    [ -L "${root}/data" ] || mkdir -p "${root}/data"
    chmod 1777 "${root}/tmp"
  fi
}

# ---------------------------------------------------------------------------
# 2. Аудит и очистка архива rootfs (*.tar.gz / *.tgz)
# ---------------------------------------------------------------------------
process_tar_archive() {
  local archive="$1"
  local do_clean="$2"
  local label
  label="$(basename "${archive}")"

  echo "[sanitize] Проверка архива: ${archive}"
  local tmp_dir="/tmp/sanitize-tar-$$"
  rm -rf "${tmp_dir}"
  mkdir -p "${tmp_dir}"
  tar -xzf "${archive}" -C "${tmp_dir}" 2>/dev/null || true

  local before="${TOTAL_ISSUES}"
  audit_and_clean_rootfs_tree "${tmp_dir}" "${label}" "${do_clean}"
  local after="${TOTAL_ISSUES}"

  if [ "${do_clean}" = "1" ] && [ "${after}" -gt "${before}" ]; then
    echo "  [clean] Перепаковка очищенного архива ${archive}..."
    (cd "${tmp_dir}" && tar -czf "${archive}" .)
  fi
  rm -rf "${tmp_dir}"
}

# ---------------------------------------------------------------------------
# 3. Аудит и очистка образа .ext4 (включая raw-блоки неразмеченной области)
# ---------------------------------------------------------------------------
process_ext4_image() {
  local img="$1"
  local do_clean="$2"
  local label
  label="$(basename "${img}")"

  echo "[sanitize] Проверка образа ext4: ${img}"
  local mnt="/tmp/sanitize-mnt-$$"
  mkdir -p "${mnt}"
  umount "${mnt}" 2>/dev/null || true
  e2fsck -pf "${img}" >/dev/null 2>&1 || true

  local before="${TOTAL_ISSUES}"
  if [ "${do_clean}" = "1" ]; then
    mount -o loop "${img}" "${mnt}"
    audit_and_clean_rootfs_tree "${mnt}" "${label}" "1"
    umount "${mnt}"
  else
    mount -o loop,ro "${img}" "${mnt}"
    audit_and_clean_rootfs_tree "${mnt}" "${label}" "0"
    umount "${mnt}"
  fi
  rmdir "${mnt}" 2>/dev/null || true

  # Проверка неразмеченных блоков (raw binary grep) на следы удалённых ключей, серийников и локальных IP
  local raw_hits
  raw_hits="$(grep -a -o -E '(.{0,20}(androidboot\.serialno=[0-9A-Za-z]{6,}|quasar-[0-9a-fA-F]{6,}|10\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}|192\.168\.[0-9]{1,3}\.[0-9]{1,3}|ssh-(rsa|ed25519)|AAAAC3Nza).{0,20})' "${img}" 2>/dev/null | grep -vE '(androidboot\.serialno=1234567890|10\.10\.10\.254|10\.0\.0\.0|192\.168\.5\.1)' | head -n 10 || true)"
  if [ -n "${raw_hits}" ]; then
    while IFS= read -r hit; do
      [ -n "${hit}" ] || continue
      local clean_hit
      clean_hit="$(echo "${hit}" | tr -cd '[:print:]')"
      report_issue "${label}" "RAW_UNALLOCATED_BLOCKS" "Следы в блоках .ext4: ${clean_hit}"
    done <<< "${raw_hits}"
  fi

  local after="${TOTAL_ISSUES}"
  if [ "${do_clean}" = "1" ] && [ "${after}" -gt "${before}" ]; then
    echo "  [clean] Пересборка ${label} с нуля (mke2fs -d) для 100% обнуления свободных блоков..."
    local size_bytes
    size_bytes="$(stat -c '%s' "${img}")"
    local src_mnt="/tmp/sanitize-src-$$"
    local clean_tree="/tmp/sanitize-tree-$$"
    local new_img="/tmp/sanitize-new-$$.ext4"
    mkdir -p "${src_mnt}" "${clean_tree}"
    mount -o loop,ro "${img}" "${src_mnt}"
    cp -a "${src_mnt}/." "${clean_tree}/"
    umount "${src_mnt}"
    rm -rf "${clean_tree}/lost+found"
    truncate -s "${size_bytes}" "${new_img}"
    mke2fs -t ext4 -F -L system -d "${clean_tree}" "${new_img}" >/dev/null
    e2fsck -pf "${new_img}" >/dev/null 2>&1 || true
    mv -f "${new_img}" "${img}"
    rm -rf "${src_mnt}" "${clean_tree}"
    echo "  [clean] Образ ${label} полностью очищен и пересобран."
  fi
}

# ---------------------------------------------------------------------------
# Обход всех переданных целей
# ---------------------------------------------------------------------------
for target in "${TARGETS[@]}"; do
  if [ ! -e "${target}" ]; then
    echo "[sanitize] Путь не найден: ${target}" >&2
    exit 1
  fi

  if [ -d "${target}" ]; then
    if [ -f "${target}/rootfs.tar.gz" ]; then
      echo "[sanitize] Проверка папки дампа: ${target}"
      process_tar_archive "${target}/rootfs.tar.gz" "$([ "${MODE}" = "clean" ] && echo 1 || echo 0)"
    elif [ -d "${target}/system" ] || [ -d "${target}/etc" ]; then
      echo "[sanitize] Проверка дерева rootfs: ${target}"
      audit_and_clean_rootfs_tree "${target}" "$(basename "${target}")" "$([ "${MODE}" = "clean" ] && echo 1 || echo 0)"
    else
      echo "[sanitize] Пропуск папки (не содержит rootfs или rootfs.tar.gz): ${target}"
    fi
  elif [ -f "${target}" ]; then
    case "${target}" in
      *.tar.gz|*.tgz)
        process_tar_archive "${target}" "$([ "${MODE}" = "clean" ] && echo 1 || echo 0)"
        ;;
      *.ext4|*.img)
        process_ext4_image "${target}" "$([ "${MODE}" = "clean" ] && echo 1 || echo 0)"
        ;;
      *)
        echo "[sanitize] Неподдерживаемый тип файла: ${target}" >&2
        exit 1
        ;;
    esac
  fi
done

echo "=================================================================="
if [ "${TOTAL_ISSUES}" -eq 0 ]; then
  echo "[sanitize] РЕЗУЛЬТАТ: ЧИСТО (обнаружено чувствительных элементов: 0)"
  echo "=================================================================="
  exit 0
else
  if [ "${MODE}" = "clean" ]; then
    echo "[sanitize] РЕЗУЛЬТАТ: Обнаружено и вычищено чувствительных элементов: ${TOTAL_ISSUES}"
    echo "=================================================================="
    exit 0
  else
    echo "[sanitize] РЕЗУЛЬТАТ: Обнаружено чувствительных элементов: ${TOTAL_ISSUES} (запустите с --clean для очистки)"
    echo "=================================================================="
    exit 1
  fi
fi
