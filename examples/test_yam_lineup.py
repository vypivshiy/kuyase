#!/usr/bin/env python3
"""
Пример E2E-теста установщика пакетного менеджера YAM (https://github.com/nyakokitsu/yam):
скачивает оригинальный install.sh из репозитория YAM, вырезает из него без модификаций
функции device_identity() и detect_platform() и сверяет результат автоопределения
с заводским типом устройства из /system/vendor/quasar/quasar.cfg на всех образах в work/.

Запуск:
  python examples/test_yam_lineup.py              # Все собранные платы в work/
  python examples/test_yam_lineup.py --board orion # Только выбранная плата
  python examples/test_yam_lineup.py --ref main    # Проверить против ветки main
"""

# На коммите caf835a404 воспроизводятся сразу 2 бага detect_platform() в install.sh:
# 1) yandexmini (Станция Мини 1): compatible="amlogic, axg" (с пробелом) не матчится шаблоном *amlogic,axg* -> ошибка детекта
# 2) plumnc (Станция Лайт 2 без часов, ревизия prune): нет ветки *prune*, проваливается в *amlogic,axg* -> ошибочно определяется как yandexmini вместо plum

import argparse
import pathlib
import platform
import re
import subprocess
import sys
import urllib.request

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
YAM_COMMIT = "caf835a4046d24810cc40d75fb823e4d4dd23de8"


def fetch_yam_detect_script(ref: str = YAM_COMMIT) -> str:
    url = f"https://raw.githubusercontent.com/nyakokitsu/yam/{ref}/install.sh"
    print(f"[*] Скачивание install.sh ({ref[:10]}): {url}")
    print(
        "[*] Легенда проверки:\n"
        "    - compatible  : строка из /proc/device-tree/compatible (вход для device_identity в install.sh)\n"
        "    - quasar.cfg  : эталонный deviceType прошивки из /system/vendor/quasar/quasar.cfg\n"
        "    - yam_profile : профиль, который вернула функция detect_platform() из install.sh\n",
        flush=True,
    )
    with urllib.request.urlopen(url, timeout=15) as resp:
        install_sh = resp.read().decode("utf-8")

    id_match = re.search(r"(device_identity\(\)\s*\{.*?\n\})", install_sh, re.DOTALL)
    det_match = re.search(r"(detect_platform\(\)\s*\{.*?\n\})", install_sh, re.DOTALL)
    if not id_match or not det_match:
        sys.exit("[ERROR] В install.sh не найдены функции device_identity() / detect_platform()")

    # Обе функции вставляются из install.sh без единого изменения;
    # локальный стаб adb() просто выполняет переданную команду shell внутри самой колонки.
    return (
        'adb() { shift 3; sh -c "$1"; }\n'
        "DEVICE=local\n"
        f"{id_match.group(1)}\n"
        f"{det_match.group(1)}\n"
        "DEVICE_IDENTITY=$(device_identity)\n"
        'QUASAR_CFG=$(sed -n \'s/.*"deviceType"[[:space:]]*:[[:space:]]*"\\([^"]*\\)".*/\\1/p\' /system/vendor/quasar/quasar.cfg 2>/dev/null | head -n 1)\n'
        'DT_LINE=$(printf "%s" "$DEVICE_IDENTITY" | tr "\\n" " ")\n'
        'printf "  compatible  : %s\\n" "$DT_LINE"\n'
        'printf "  quasar.cfg  : %s\\n" "${QUASAR_CFG:-unknown}"\n'
        "if ! plat=$(detect_platform); then\n"
        '  printf "  yam_profile : НЕ РАСПОЗНАНА (detect_platform вернул ошибку)\\n"\n'
        "  exit 1\n"
        "fi\n"
        'printf "  yam_profile : %s\\n" "$plat"\n'
        'if [ -n "$QUASAR_CFG" ] && [ "$plat" != "$QUASAR_CFG" ]; then\n'
        '  printf "  ОШИБКА      : по quasar.cfg ожидался профиль \'%s\', но detect_platform() вернул \'%s\'\\n" "$QUASAR_CFG" "$plat"\n'
        "  exit 1\n"
        "fi"
    )


def find_kuyase() -> str:
    exe = "kuyase.exe" if platform.system() == "Windows" else "kuyase"
    for p in (REPO_ROOT / "target" / "release" / exe, REPO_ROOT / "Releases" / exe, REPO_ROOT / exe):
        if p.is_file():
            return str(p)
    return exe


def main() -> None:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")

    parser = argparse.ArgumentParser(description="Проверка detect_platform() из YAM в KUYASE")
    parser.add_argument("--board", default="all", help="Код платы (например, orion) или all")
    parser.add_argument("--ref", default=YAM_COMMIT, help="Коммит или ветка YAM (например, main)")
    args = parser.parse_args()

    guest_cmd = fetch_yam_detect_script(args.ref)
    cmd = [find_kuyase(), "run", "--headless", "--board", args.board, "--snapshot", "--exec", guest_cmd]
    sys.exit(subprocess.call(cmd, cwd=REPO_ROOT))


if __name__ == "__main__":
    main()
