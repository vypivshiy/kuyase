#!/usr/bin/env bash
# KUYASE — Сборка релизных бинарников и портативных архивов для GitHub Releases (Linux / WSL2)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

export PATH="${HOME}/.cargo/bin:${PATH}"

if [ "${SKIP_BRIDGE:-0}" != "1" ]; then
  echo "=== [1/4] Кросс-компиляция гостевого моста qemu-hw-bridge (armv7-unknown-linux-musleabihf) ==="
  if command -v rustup >/dev/null 2>&1; then
    rustup target list --installed | grep -q '^armv7-unknown-linux-musleabihf$' || rustup target add armv7-unknown-linux-musleabihf
  fi
  cargo build --release -p qemu-hw-bridge --target armv7-unknown-linux-musleabihf
  mkdir -p guest-init/bin Releases
  cp -f target/armv7-unknown-linux-musleabihf/release/qemu-hw-bridge guest-init/bin/qemu-hw-bridge
  cp -f target/armv7-unknown-linux-musleabihf/release/qemu-hw-bridge Releases/qemu-hw-bridge
  chmod 0755 guest-init/bin/qemu-hw-bridge Releases/qemu-hw-bridge
else
  echo "=== [1/4] Использование готового гостевого моста Releases/qemu-hw-bridge ==="
fi

echo "=== [2/4] Сборка нативного бинарника kuyase (Linux x86_64) ==="
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target-linux}" cargo build --release -p kuyase
LINUX_BIN="${CARGO_TARGET_DIR:-target-linux}/release/kuyase"
cp -f "${LINUX_BIN}" Releases/kuyase
chmod 0755 Releases/kuyase

if [ -f kernel/kuyase-kernel-arm64.img ]; then
  cp -f kernel/kuyase-kernel-arm64.img Releases/kuyase-kernel-arm64.img
elif [ -f kernel/Image ]; then
  cp -f kernel/Image Releases/kuyase-kernel-arm64.img
elif [ -f Releases/Image ] && [ ! -f Releases/kuyase-kernel-arm64.img ]; then
  cp -f Releases/Image Releases/kuyase-kernel-arm64.img
fi

if [ ! -f Releases/kuyase-kernel-arm64.img ]; then
  echo "[kuyase] Ошибка: не найден образ ядра Releases/kuyase-kernel-arm64.img (или kernel/Image)" >&2
  exit 1
fi
mkdir -p kernel
cp -f Releases/kuyase-kernel-arm64.img kernel/kuyase-kernel-arm64.img
cp -f Releases/kuyase-kernel-arm64.img kernel/Image

echo "=== [3/4] Упаковка Releases/kuyase-linux64-portable.tar.gz ==="
STAGE_LINUX="$(mktemp -d)"
trap 'rm -rf "${STAGE_LINUX}"' EXIT
mkdir -p "${STAGE_LINUX}/examples" "${STAGE_LINUX}/scripts"
cp -f Releases/kuyase "${STAGE_LINUX}/kuyase"
cp -f Releases/kuyase-kernel-arm64.img "${STAGE_LINUX}/kuyase-kernel-arm64.img"
cp -f Releases/qemu-hw-bridge "${STAGE_LINUX}/qemu-hw-bridge"
chmod 0755 "${STAGE_LINUX}/kuyase" "${STAGE_LINUX}/qemu-hw-bridge"
cp -f README.md DUMP_GUIDE.md "${STAGE_LINUX}/"
cp -f examples/README.md examples/test_yam_lineup.py "${STAGE_LINUX}/examples/"
cp -f scripts/dump-from-adb.sh scripts/dump-from-adb.ps1 scripts/sanitize-rootfs.sh scripts/sanitize-rootfs.ps1 "${STAGE_LINUX}/scripts/"
chmod 0755 "${STAGE_LINUX}/scripts/"*.sh "${STAGE_LINUX}/examples/test_yam_lineup.py" 2>/dev/null || true

tar -czf Releases/kuyase-linux64-portable.tar.gz -C "${STAGE_LINUX}" \
  kuyase kuyase-kernel-arm64.img qemu-hw-bridge README.md DUMP_GUIDE.md examples scripts

echo "=== [4/4] Генерация контрольных сумм Releases/SHA256SUMS.txt ==="
(
  cd Releases
  FILES=()
  [ -f kuyase-win64-portable.zip ] && FILES+=(kuyase-win64-portable.zip)
  [ -f kuyase-linux64-portable.tar.gz ] && FILES+=(kuyase-linux64-portable.tar.gz)
  [ -f kuyase-kernel-arm64.img ] && FILES+=(kuyase-kernel-arm64.img)
  sha256sum "${FILES[@]}" > SHA256SUMS.txt
)
cat Releases/SHA256SUMS.txt
