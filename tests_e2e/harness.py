#!/usr/bin/env python3
"""
Minimal E2E runner helper for KUYASE.
Rule: Never write custom TCP/ADB/HTTP clients here. All guest interaction must
execute exclusively through `kuyase run --headless --board <board> --snapshot --exec`.
"""

from __future__ import annotations

import hashlib
import http.server
import json
import os
import pathlib
import platform
import subprocess
import tempfile
import threading
import urllib.request
from typing import Any, Dict, List, Optional

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
WORK_DIR = REPO_ROOT / "work"
CACHE_DIR = WORK_DIR / "e2e-cache"

# Canonical mapping from KUYASE board slug -> YAM v0.2+ platform profile ID
EXPECTED_YAM_PLATFORM: Dict[str, str] = {
    "station2": "yandexstation_2",
    "cucumber": "cucumber",
    "yandexmini": "yandexmini",
    "yandexmini2": "yandexmini_2",
    "yandexmini2nc": "yandexmini_2",
    "bergamot": "bergamot",
    "pickle": "pickle",
    "plum": "plum",
    "plumnc": "plum",
    "yandexmicro": "yandexmicro",
    "mango": "mango",
    "orion": "orion",
    "saturn": "saturn",
}

# Canonical station hostname (without `qemu-` prefix or `nc` suffix) for Luna auto-detection
CANONICAL_STATION_HOSTNAME: Dict[str, str] = EXPECTED_YAM_PLATFORM

# Guest `/bin/sh` shim allowing unmodified host `install.sh` scripts (which invoke
# `adb devices`, `adb -s "$DEVICE" shell ...`, and `adb -s "$DEVICE" push ...`)
# to execute directly inside `kuyase run --headless --snapshot --exec`.
ADB_GUEST_SHIM = r"""
export HOME=/root
adb() {
  if [ "${1:-}" = "devices" ]; then
    printf "List of devices attached\nlocal\tdevice\n"
    return 0
  fi
  if [ "${1:-}" = "-s" ]; then
    shift 2
  fi
  case "${1:-}" in
    get-state)
      echo device
      ;;
    connect)
      return 0
      ;;
    shell)
      shift
      if [ "$#" -gt 0 ]; then
        ( eval "$*" )
      else
        sh -s
      fi
      ;;
    push)
      mkdir -p "$(dirname "$3")"
      cp -f "$2" "$3"
      ;;
    *)
      return 0
      ;;
  esac
}
mount() {
  if [ "${1:-}" = "-o" ] && [ "${2:-}" = "remount,ro" ]; then
    command mount "$@" 2>/dev/null || true
    return 0
  fi
  command mount "$@"
}
wget() {
  _wargs=""
  for _wa in "$@"; do
    case "$_wa" in
      https://*) _wa="http://10.0.2.2:__KUYASE_HTTPS_PROXY_PORT__/https/${_wa#https://}" ;;
    esac
    _wargs="$_wargs \"$_wa\""
  done
  eval "command wget $_wargs"
}
""".strip()


class _HttpsCacheGateway:
    """Host-side HTTP->HTTPS caching gateway for BusyBox `wget` (which lacks TLS on several images)."""

    _instance: Optional["_HttpsCacheGateway"] = None
    _lock = threading.Lock()

    def __init__(self) -> None:
        http_cache = CACHE_DIR / "http"
        http_cache.mkdir(parents=True, exist_ok=True)

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:
                if self.path == "/https/local.kuyase.e2e/airplay.yampkg":
                    local_pkg = os.environ.get("KUYASE_AIRPLAY_PKG", "").strip()
                    p = pathlib.Path(local_pkg)
                    if not local_pkg or not p.is_file():
                        self.send_response(404)
                        self.end_headers()
                        return
                    data = p.read_bytes()
                    self.send_response(200)
                    self.send_header("Content-Type", "application/octet-stream")
                    self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    self.wfile.write(data)
                    return
                if not self.path.startswith("/https/"):
                    self.send_response(404)
                    self.end_headers()
                    return
                target_url = "https://" + self.path[len("/https/") :]
                cache_key = hashlib.sha256(target_url.encode("utf-8")).hexdigest()
                cache_file = http_cache / cache_key
                try:
                    if cache_file.is_file() and cache_file.stat().st_size > 0:
                        data = cache_file.read_bytes()
                    else:
                        req = urllib.request.Request(
                            target_url,
                            headers={"User-Agent": "kuyase-e2e/1.0"},
                        )
                        with urllib.request.urlopen(req, timeout=120.0) as resp:
                            data = resp.read()
                        cache_file.write_bytes(data)
                    self.send_response(200)
                    self.send_header("Content-Type", "application/octet-stream")
                    self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    self.wfile.write(data)
                except Exception as exc:
                    msg = str(exc).encode("utf-8", errors="replace")
                    self.send_response(502)
                    self.send_header("Content-Length", str(len(msg)))
                    self.end_headers()
                    self.wfile.write(msg)

            def log_message(self, format: str, *args: Any) -> None:
                pass

        self.httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.port = self.httpd.server_address[1]
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()

    @classmethod
    def port_number(cls) -> int:
        with cls._lock:
            if cls._instance is None:
                cls._instance = _HttpsCacheGateway()
            return cls._instance.port


def find_kuyase_binary() -> pathlib.Path:
    env_bin = os.environ.get("KUYASE_BIN", "").strip()
    if env_bin:
        p = pathlib.Path(env_bin)
        if p.is_file():
            return p
        raise FileNotFoundError(f"KUYASE_BIN points to non-existent file: {p}")

    is_win = platform.system() == "Windows"
    exe = "kuyase.exe" if is_win else "kuyase"
    headless_exe = "kuyase-headless.exe" if is_win else "kuyase-headless"
    search_dirs = [
        REPO_ROOT / "target" / "release",
        REPO_ROOT / "target-headless" / "release",
        REPO_ROOT / "target" / "x86_64-unknown-linux-musl" / "release",
        REPO_ROOT / "target" / "debug",
        REPO_ROOT / "Releases",
        REPO_ROOT,
    ]
    for directory in search_dirs:
        for name in (exe, headless_exe):
            candidate = directory / name
            if candidate.is_file():
                return candidate
    raise FileNotFoundError(
        "kuyase binary not found; build it first with `cargo build --release -p kuyase` "
        "(or `cargo build --release -p kuyase --no-default-features`)"
    )


def discover_test_boards() -> List[str]:
    env_board = os.environ.get("KUYASE_E2E_BOARD", "").strip()
    if env_board and env_board.lower() != "all":
        return [b.strip() for b in env_board.split(",") if b.strip()]

    boards: List[str] = []
    if WORK_DIR.is_dir():
        for p in sorted(WORK_DIR.glob("*-system.ext4")):
            name = p.name[: -len("-system.ext4")]
            if not name.endswith("-orig"):
                boards.append(name)
    return boards


def run_headless_exec(
    board: str,
    script: str,
    port_offset: int = 200,
    mode: str = "stub",
) -> str:
    """Run a guest shell script via `kuyase run --headless --board <board> --mode <mode> --snapshot --exec`."""
    kuyase_bin = find_kuyase_binary()
    proxy_port = _HttpsCacheGateway.port_number()
    rendered = script.replace("\r\n", "\n").replace(
        "__KUYASE_HTTPS_PROXY_PORT__", str(proxy_port)
    )
    WORK_DIR.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        newline="\n",
        suffix=".sh",
        prefix=f"e2e-{board}-",
        dir=WORK_DIR,
        delete=False,
    ) as tf:
        tf.write(rendered)
        script_path = pathlib.Path(tf.name)

    try:
        cmd = [
            str(kuyase_bin),
            "run",
            "--headless",
            "--board",
            board,
            "--mode",
            mode,
            "--snapshot",
            "--port-offset",
            str(port_offset),
            "--exec",
            f"@{script_path}",
        ]
        proc = subprocess.run(
            cmd,
            cwd=REPO_ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        if proc.returncode != 0:
            raise RuntimeError(
                f"kuyase --exec failed on board '{board}' (exit {proc.returncode}):\n{proc.stdout}"
            )
        return proc.stdout
    finally:
        script_path.unlink(missing_ok=True)


def http_get_text(url: str, timeout: float = 30.0) -> str:
    req = urllib.request.Request(
        url,
        headers={"User-Agent": "kuyase-e2e/1.0"},
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return resp.read().decode("utf-8").replace("\r\n", "\n")


def resolve_yam_repo_dir() -> Optional[pathlib.Path]:
    env_dir = os.environ.get("YAM_REPO_DIR", "").strip()
    if env_dir:
        p = pathlib.Path(env_dir)
        if (p / "install.sh").is_file():
            return p
    sibling = REPO_ROOT.parent / "yam"
    if (sibling / "install.sh").is_file():
        return sibling
    return None


def load_yam_install_sh() -> str:
    yam_dir = resolve_yam_repo_dir()
    if yam_dir is not None:
        return (yam_dir / "install.sh").read_text(encoding="utf-8").replace("\r\n", "\n")
    return http_get_text("https://raw.githubusercontent.com/nyakokitsu/yam/main/install.sh")


def load_yam_install_ps1() -> str:
    yam_dir = resolve_yam_repo_dir()
    if yam_dir is not None and (yam_dir / "install.ps1").is_file():
        return (yam_dir / "install.ps1").read_text(encoding="utf-8").replace("\r\n", "\n")
    return http_get_text("https://raw.githubusercontent.com/nyakokitsu/yam/main/install.ps1")


def resolve_luna_repo_dir() -> Optional[pathlib.Path]:
    env_dir = os.environ.get("LUNA_REPO_DIR", "").strip()
    if env_dir:
        p = pathlib.Path(env_dir)
        if (p / "install.sh").is_file():
            return p
    sibling = REPO_ROOT.parent / "luna"
    if (sibling / "install.sh").is_file():
        return sibling
    return None


def load_luna_install_sh() -> str:
    luna_dir = resolve_luna_repo_dir()
    if luna_dir is not None:
        return (luna_dir / "install.sh").read_text(encoding="utf-8").replace("\r\n", "\n")
    return http_get_text("https://raw.githubusercontent.com/nyakokitsu/luna/main/install.sh")


def load_luna_install_ps1() -> str:
    luna_dir = resolve_luna_repo_dir()
    if luna_dir is not None and (luna_dir / "install.ps1").is_file():
        return (luna_dir / "install.ps1").read_text(encoding="utf-8").replace("\r\n", "\n")
    return http_get_text("https://raw.githubusercontent.com/nyakokitsu/luna/main/install.ps1")


def latest_airplay_release_tag() -> str:
    raw = http_get_text("https://api.github.com/repos/ryysel/Station-airplay/releases/latest")
    data = json.loads(raw)
    return str(data["tag_name"])
