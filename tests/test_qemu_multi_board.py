import argparse
import json
import shutil
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")

REPO_ROOT = Path(__file__).resolve().parents[1]
KERNEL_IMG = REPO_ROOT / "kernel" / "Image"
if not KERNEL_IMG.exists() and (REPO_ROOT / "Releases" / "Image").exists():
    KERNEL_IMG = REPO_ROOT / "Releases" / "Image"
WORK_DIR = REPO_ROOT / "work"
QEMU_EXE = Path(shutil.which("qemu-system-aarch64") or r"C:\Program Files\qemu\qemu-system-aarch64.exe")


def http_json(url: str, method: str = "GET", payload: dict | None = None, timeout: float = 5.0) -> dict:
    data = json.dumps(payload).encode("utf-8") if payload is not None else None
    headers = {"Content-Type": "application/json"} if data is not None else {}
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read().decode("utf-8"))


def wait_for_bridge(bridge_port: int, timeout_sec: float = 20.0) -> dict:
    deadline = time.time() + timeout_sec
    last_err = None
    while time.time() < deadline:
        try:
            b = http_json(f"http://127.0.0.1:{bridge_port}/api/state", timeout=1.5)
            if b.get("ok"):
                return b
        except Exception as e:
            last_err = e
        time.sleep(0.25)
    raise RuntimeError(f"QEMU hw-bridge did not come up in time: {last_err}")


def adb_shell(adb_exe: Path, adb_port: int, script: str, timeout: float = 8.0) -> str:
    res = subprocess.run(
        [str(adb_exe), "-s", f"127.0.0.1:{adb_port}", "shell", script],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
    )
    return (res.stdout or "").strip()


def resolve_bridge_bin() -> Path | None:
    candidates = [
        REPO_ROOT / "hw-bridge" / "target" / "armv7-unknown-linux-musleabihf" / "release" / "qemu-hw-bridge",
        REPO_ROOT / "guest-init" / "bin" / "qemu-hw-bridge",
        REPO_ROOT / "Releases" / "qemu-hw-bridge",
    ]
    for c in candidates:
        if c.exists():
            return c
    return None


def run_board_verification(board: str, drive_img: Path, bridge_port: int, adb_port: int):
    assert KERNEL_IMG.exists(), f"Missing {KERNEL_IMG}"
    assert drive_img.exists(), f"Missing {drive_img}"

    serial_log = WORK_DIR / f"{board}-serial.log"
    bridge_bin = resolve_bridge_bin()
    cmd = [
        str(QEMU_EXE),
        "-M",
        "virt",
        "-cpu",
        "cortex-a53",
        "-smp",
        "2",
        "-m",
        "512M",
        "-kernel",
        str(KERNEL_IMG),
        "-append",
        "console=ttyAMA0 root=/dev/vda rw ip=dhcp init=/quasar-qemu-init.sh quasar.board=auto quasar.mode=stub panic=-1",
        "-drive",
        f"file={drive_img},if=virtio,format=raw",
    ]
    if bridge_bin is not None:
        cmd.extend(["-drive", f"file={bridge_bin},if=virtio,format=raw,readonly=on"])
    cmd.extend([
        "-netdev",
        f"user,id=net0,hostfwd=tcp:127.0.0.1:{bridge_port}-:8091,hostfwd=tcp:127.0.0.1:{adb_port}-:5555",
        "-device",
        "virtio-net-pci,netdev=net0",
        "-serial",
        f"file:{serial_log}",
        "-display",
        "none",
    ])

    print(f"\n=== [TEST] Starting KUYASE for board='{board}' (Twin :{bridge_port}, ADB :{adb_port}) ===")
    proc = subprocess.Popen(
        cmd,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        # 1. Verify Virtual Speaker Twin (:18091) & auto-detected board profile
        bridge = wait_for_bridge(bridge_port)
        assert bridge["board_slug"] == board, f"Expected board {board}, got {bridge['board_slug']}"
        print(f"[{board}] Auto-detected board: {bridge['board_name']} (ring_leds={bridge['ring_leds']}, clock={bridge['has_clock']})")

        with urllib.request.urlopen(f"http://127.0.0.1:{bridge_port}/", timeout=3.0) as b_ui:
            bridge_html = b_ui.read().decode("utf-8")
        assert "VIRTUAL SPEAKER TWIN" in bridge_html

        # 2. Verify ADB server (:5555 -> adb_port) & clean standalone image
        adb_exe = Path(shutil.which("adb") or "adb")
        subprocess.run([str(adb_exe), "disconnect", f"127.0.0.1:{adb_port}"], check=False, capture_output=True, timeout=5)
        subprocess.run([str(adb_exe), "connect", f"127.0.0.1:{adb_port}"], check=False, capture_output=True, timeout=5)
        adb_out = adb_shell(
            adb_exe,
            adb_port,
            "hostname; uname -m; [ ! -e /usr/local/bin/equasar-daemon ] && echo CLEAN_STANDALONE; cat /tmp/quasar-emu-hw.json",
        )
        assert f"qemu-{board}" in adb_out and "CLEAN_STANDALONE" in adb_out, f"Unexpected adb output: {adb_out}"
        hw_json_str = adb_out[adb_out.find("{") :]
        hw_info = json.loads(hw_json_str)
        sysfs_prefix = hw_info["sysfs_prefix"]
        led_driver = hw_info["led_driver"]
        ring_leds = int(hw_info["ring_leds"])
        has_clock = bool(hw_info["has_clock"])
        print(f"[{board}] ADB & /tmp/quasar-emu-hw.json verified (driver={led_driver}, prefix={sysfs_prefix})")

        # 4. Verify LED Ring & Clock via I2C SN3236 / sysfs / framebuffer (including trigger conflict check)
        if led_driver == "i2c_sn3236":
            # 4a-i2c: Write PWM registers 0x01..0x12 WITHOUT latching 0x25 -> LEDs must remain [0, 0, 0]
            adb_shell(
                adb_exe,
                adb_port,
                "i2cset -y 0 0x3c 0x00 0x01 && "
                "i2cset -y 0 0x3c 0x26 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 i && "
                "i2cset -y 0 0x3c 0x25 0xaa && "
                "i2cset -y 0 0x3c 0x01 0xff 0x0f 0x6b 0xff 0x0f 0x6b 0xff 0x0f 0x6b 0xff 0x0f 0x6b 0xff 0x0f 0x6b 0xff 0x0f 0x6b i",
            )
            st_unlatched = http_json(f"http://127.0.0.1:{bridge_port}/api/state")
            assert st_unlatched["ring_pixels"][0] == [0, 0, 0], f"Expected unlatched SN3236 PWM to stay [0,0,0], got {st_unlatched['ring_pixels'][0]}"

            # Latch via 0x25 -> LEDs must become Alice purple [107, 15, 255] (#6B0FFF)
            adb_shell(adb_exe, adb_port, "i2cset -y 0 0x3c 0x25 0xaa")
            st_latched = http_json(f"http://127.0.0.1:{bridge_port}/api/state")
            assert st_latched["ring_pixels"][0] == [107, 15, 255], f"Expected latched SN3236 [107, 15, 255], got {st_latched['ring_pixels'][0]}"

            # Current divider SL=01 (Imax/2 -> 0x03 in 0x26..0x28) -> LED 0 brightness must halve to [53, 7, 127]
            adb_shell(adb_exe, adb_port, "i2cset -y 0 0x3c 0x26 0x03 0x03 0x03 i && i2cset -y 0 0x3c 0x25 0xaa")
            st_half = http_json(f"http://127.0.0.1:{bridge_port}/api/state")
            assert st_half["ring_pixels"][0] == [53, 7, 127], f"Expected Imax/2 [53, 7, 127], got {st_half['ring_pixels'][0]}"

            # OUT_EN=0 with SL=01 (0x02 in 0x26..0x28) -> LED 0 must turn off [0, 0, 0]
            adb_shell(adb_exe, adb_port, "i2cset -y 0 0x3c 0x26 0x02 0x02 0x02 i && i2cset -y 0 0x3c 0x25 0xaa")
            st_off = http_json(f"http://127.0.0.1:{bridge_port}/api/state")
            assert st_off["ring_pixels"][0] == [0, 0, 0], f"Expected OUT_EN=0 [0, 0, 0], got {st_off['ring_pixels'][0]}"

            # Reset via 0x4a -> all registers & LEDs must clear to [0, 0, 0] and Stealth Locked
            adb_shell(adb_exe, adb_port, "i2cset -y 0 0x3c 0x4a 0x01")
            st_reset = http_json(f"http://127.0.0.1:{bridge_port}/api/state")
            assert st_reset["ring_pixels"][0] == [0, 0, 0] and "Stealth" in st_reset["ring_trigger"], f"Expected reset SN3236, got {st_reset['ring_pixels'][0]} / {st_reset['ring_trigger']}"

            # Re-enable chip & 18 channels for subsequent MUTE / Volume tests
            adb_shell(
                adb_exe,
                adb_port,
                "i2cset -y 0 0x3c 0x00 0x01 && "
                "i2cset -y 0 0x3c 0x26 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 0x01 i && "
                "i2cset -y 0 0x3c 0x25 0xaa",
            )
            print(f"[{board}] SN3236 I2C (0x3c) 0x25 latch, BGR order & 0x4a reset verified!")
        else:
            # Writing brightness while trigger is still [ledfb-ring] must be rejected!
            adb_shell(
                adb_exe,
                adb_port,
                f"echo '[ledfb-ring] none' > /sys/class/leds/{sysfs_prefix}:red:ring0/trigger 2>/dev/null || true; "
                f"echo 200 > /sys/class/leds/{sysfs_prefix}:red:ring0/brightness 2>/dev/null || true",
            )
            st_conflict = http_json(f"http://127.0.0.1:{bridge_port}/api/state")
            assert st_conflict.get("led_trigger_conflict_count", 0) >= 1, "Expected trigger conflict when writing brightness with [ledfb-ring] active"

            adb_shell(
                adb_exe,
                adb_port,
                f"for i in $(seq 0 {ring_leds}); do "
                f"echo none > /sys/class/leds/{sysfs_prefix}:red:ring$i/trigger 2>/dev/null || true; "
                f"echo 34 > /sys/class/leds/{sysfs_prefix}:red:ring$i/brightness 2>/dev/null || true; "
                f"echo 204 > /sys/class/leds/{sysfs_prefix}:green:ring$i/brightness 2>/dev/null || true; "
                f"echo 255 > /sys/class/leds/{sysfs_prefix}:blue:ring$i/brightness 2>/dev/null || true; "
                f"done",
            )
            time.sleep(0.15)
            st_led = http_json(f"http://127.0.0.1:{bridge_port}/api/state")
            assert len(st_led["ring_pixels"]) == ring_leds
            assert st_led["ring_pixels"][0] == [34, 204, 255], f"Unexpected ring pixel: {st_led['ring_pixels'][0]}"
            print(f"[{board}] {ring_leds}-LED ring & [ledfb-ring] trigger guard verified: {st_led['ring_pixels'][0]}")
            # Restore [ledfb-ring] trigger so native animations (mute_mics.led, volume.led, alice_listening.led) render
            adb_shell(
                adb_exe,
                adb_port,
                f"echo '[ledfb-ring] none' > /sys/class/leds/{sysfs_prefix}:red:ring0/trigger 2>/dev/null || true; "
                f"for i in $(seq 0 {ring_leds}); do "
                f"echo 0 > /sys/class/leds/{sysfs_prefix}:red:ring$i/brightness 2>/dev/null || true; "
                f"echo 0 > /sys/class/leds/{sysfs_prefix}:green:ring$i/brightness 2>/dev/null || true; "
                f"echo 0 > /sys/class/leds/{sysfs_prefix}:blue:ring$i/brightness 2>/dev/null || true; "
                f"done",
            )

        # 4b. Verify MUTE MIC -> Solid RED LED ring (mute_mics.led) & Volume control (volume.led)
        http_json(f"http://127.0.0.1:{bridge_port}/api/hw/button", "POST", {"button": "mute", "action": "single"})
        time.sleep(0.2)
        st_mute = http_json(f"http://127.0.0.1:{bridge_port}/api/state")
        assert st_mute["mics_muted"] is True, "Expected mics_muted=True after mute button press"
        assert st_mute["ring_pixels"][0] == [255, 0, 0], f"Expected RED LED ring on MUTE MIC, got {st_mute['ring_pixels'][0]}"
        # Unmute
        http_json(f"http://127.0.0.1:{bridge_port}/api/hw/button", "POST", {"button": "mute", "action": "single"})
        time.sleep(0.15)
        st_unmute = http_json(f"http://127.0.0.1:{bridge_port}/api/state")
        assert st_unmute["mics_muted"] is False, "Expected mics_muted=False after second mute button press"

        # Volume control -> volume_level + volume.led arc
        http_json(f"http://127.0.0.1:{bridge_port}/api/hw/volume", "POST", {"volume_level": 8})
        time.sleep(0.15)
        st_vol = http_json(f"http://127.0.0.1:{bridge_port}/api/state")
        assert st_vol["volume_level"] == 8 and st_vol["master_volume"] == 8000
        assert sum(st_vol["ring_pixels"][0]) > 0, f"Expected lit volume.led arc on volume change, got {st_vol['ring_pixels']}"
        print(f"[{board}] MUTE MIC red ring ([255, 0, 0]) & Volume (8/10 -> {st_vol['ring_pixels'][0]}) verified!")

        if has_clock:
            http_json(
                f"http://127.0.0.1:{bridge_port}/api/hw/clock",
                "POST",
                {"text": "19:30°", "brightness": 255},
            )
            time.sleep(0.15)
            st_clk = http_json(f"http://127.0.0.1:{bridge_port}/api/state")
            assert st_clk["clock_segments"].get("b1", 0) == 255 and st_clk["clock_segments"].get("deg", 0) == 255
            # Verify direct per-segment PWM brightness control
            http_json(
                f"http://127.0.0.1:{bridge_port}/api/hw/clock",
                "POST",
                {"segments": {"a1": 144, "p4": 80, "deg": 0}, "replace": False},
            )
            time.sleep(0.1)
            st_clk2 = http_json(f"http://127.0.0.1:{bridge_port}/api/state")
            assert st_clk2["clock_segments"].get("a1") == 144 and st_clk2["clock_segments"].get("p4") == 80 and st_clk2["clock_segments"].get("deg") == 0
            print(f"[{board}] 7-segment clock display ('19:30°' + custom PWM segments) verified!")

        if board == "orion":
            # Verify Orion-specific peripherals:
            # 1) 21x12 Monochrome LED Matrix (/dev/fb1 'YNDX LED screen')
            # Wait for 1.5s volume overlay to expire or set display text explicitly
            time.sleep(1.55)
            http_json(
                f"http://127.0.0.1:{bridge_port}/api/hw/clock",
                "POST",
                {"text": "19:30°", "brightness": 255},
            )
            time.sleep(0.15)
            st_orion = http_json(f"http://127.0.0.1:{bridge_port}/api/state")
            assert st_orion.get("display_kind") == "matrix_21x12"
            assert st_orion.get("screen_width") == 21 and st_orion.get("screen_height") == 12
            assert len(st_orion.get("screen_pixels", [])) == 252
            assert sum(1 for px in st_orion["screen_pixels"] if px > 0) > 25
            # Verify direct custom pixel buffer write on 21x12 matrix
            custom_px = [0] * 252
            custom_px[0] = 255
            custom_px[10] = 128
            custom_px[251] = 64
            http_json(
                f"http://127.0.0.1:{bridge_port}/api/hw/clock",
                "POST",
                {"pixels": custom_px},
            )
            time.sleep(0.1)
            st_orion_px = http_json(f"http://127.0.0.1:{bridge_port}/api/state")
            assert st_orion_px["screen_pixels"][0] == 255 and st_orion_px["screen_pixels"][10] == 128 and st_orion_px["screen_pixels"][251] == 64
            # 2) Ambilight 12x2 RGBW (/dev/fb0 'ambilight' + sysfs orion:*:ambilight{0..11}{a,b})
            adb_shell(
                adb_exe,
                adb_port,
                "echo 180 > /sys/class/leds/orion:red:ambilight0a/brightness && "
                "echo 90 > /sys/class/leds/orion:green:ambilight0b/brightness && "
                "echo 240 > /sys/class/leds/orion:blue:ambilight0a/brightness && "
                "echo 40 > /sys/class/leds/orion:white:ambilight0b/brightness",
            )
            st_ambi = http_json(f"http://127.0.0.1:{bridge_port}/api/state")
            assert st_ambi.get("aux_light_kind") == "ambilight" and len(st_ambi.get("logo_pixels", [])) == 12
            assert st_ambi["logo_pixels"][0] == [200, 110, 255], f"Unexpected ambilight zone 0: {st_ambi['logo_pixels'][0]}"
            # 3) 4x ACM8625S amplifiers + Ultrasound TDM-A (hw:0,0 / pcm.ultrasound)
            amps = st_ambi.get("amplifiers", [])
            assert len(amps) == 4, f"Expected 4 ACM8625S amplifiers, got {len(amps)}"
            roles = {a["role"]: a for a in amps}
            assert set(roles.keys()) == {"woofer", "lr", "center", "us"}
            assert roles["woofer"]["settings_file"] == "orion-acm8625-woofer.cfg"
            us = st_ambi.get("ultrasound") or {}
            assert us.get("sample_rate_hz") == 96000 and us.get("carrier_freq_hz") == 38400
            fb_check = adb_shell(
                adb_exe,
                adb_port,
                "cat /sys/class/graphics/fb0/name; cat /sys/class/graphics/fb1/name; cat /sys/class/graphics/fb2/name; "
                "aplay -D ultrasound -q -t raw -f S16_LE -c 2 -r 96000 -s 1 /dev/zero && echo ULTRASOUND_PCM_OK",
            )
            assert "ambilight" in fb_check and "YNDX LED screen" in fb_check and "ring" in fb_check and "ULTRASOUND_PCM_OK" in fb_check
            print(f"[{board}] Orion 21x12 matrix, 12x2 RGBW ambilight, 4x ACM8625S & 96kHz/38.4kHz Ultrasound TDM verified!")

        # 5. Verify Sensor Simulation (:18091/api/hw/sensor -> /sys/class/thermal & /sys/bus/iio)
        http_json(
            f"http://127.0.0.1:{bridge_port}/api/hw/sensor",
            "POST",
            {"temp_c": 51.4, "als_lux": 640, "tof_mm": 165, "doa_angle": 90, "mic_rms": 1800},
        )
        sens_out = adb_shell(
            adb_exe,
            adb_port,
            "cat /sys/class/thermal/thermal_zone0/temp; cat /sys/bus/iio/devices/iio:device0/in_illuminance_raw",
        ).splitlines()
        assert "51400" in sens_out[0] and "640" in sens_out[-1], f"Unexpected sensor sysfs: {sens_out}"
        print(f"[{board}] Sensors verified in sysfs: temp=51400 mC, als=640 lux")

        # 6. Verify Voice Injection (/api/voice/inject -> maind.log), Formant TTS & ALSA snd-aloop WAV capture
        http_json(f"http://127.0.0.1:{bridge_port}/api/audio/clear", "POST", {})
        http_json(
            f"http://127.0.0.1:{bridge_port}/api/voice/inject",
            "POST",
            {"phrase": "включи ночной режим", "doa_angle": 90},
        )
        log_out = adb_shell(adb_exe, adb_port, "cat /tmp/daemons_logs/maind.log")
        subprocess.run([str(adb_exe), "disconnect", f"127.0.0.1:{adb_port}"], check=False, capture_output=True, timeout=5)
        assert "recognizedPhrase='включи ночной режим'" in log_out, f"Missing phrase in maind.log: {log_out}"
        print(f"[{board}] Voice Input -> /tmp/daemons_logs/maind.log verified!")

        # Trigger START_VOCALIZE with a multi-word phrase (~6s) and verify it stays SPEAKING beyond 1.2s, then interrupt via STOP_TTS
        http_json(
            f"http://127.0.0.1:{bridge_port}/api/ipc/test",
            "POST",
            {"cmd": "vocalize", "text": "Привет! Сегодня в Москве плюс 21 градус, без осадков. Включить музыку?"},
        )
        time.sleep(1.25)
        st_tts = http_json(f"http://127.0.0.1:{bridge_port}/api/state")
        assert st_tts["alice_state"] == "SPEAKING" and st_tts["aliced_say_count"] >= 1, (
            f"Expected AliceState=SPEAKING at 1.25s during long TTS phrase, got {st_tts['alice_state']}"
        )
        http_json(f"http://127.0.0.1:{bridge_port}/api/ipc/test", "POST", {"cmd": "stop_tts"})
        time.sleep(0.15)
        st_stopped = http_json(f"http://127.0.0.1:{bridge_port}/api/state")
        assert st_stopped["alice_state"] == "IDLE", f"Expected IDLE after STOP_TTS, got {st_stopped['alice_state']}"

        # Rapid-fire SAY (:9875 START_VOCALIZE) burst test -> must not deadlock ALSA dmix or stay stuck in SPEAKING
        for burst_i in range(4):
            http_json(
                f"http://127.0.0.1:{bridge_port}/api/ipc/test",
                "POST",
                {"cmd": "vocalize", "text": f"Тест {burst_i}"},
            )
            time.sleep(0.05)
        # Wait for the final short phrase ("Тест 3") to complete naturally and return to IDLE
        returned_idle = False
        for _ in range(30):
            time.sleep(0.15)
            if http_json(f"http://127.0.0.1:{bridge_port}/api/state").get("alice_state") == "IDLE":
                returned_idle = True
                break
        assert returned_idle, "Expected AliceState to return to IDLE after rapid START_VOCALIZE burst"

        with urllib.request.urlopen(f"http://127.0.0.1:{bridge_port}/api/audio/output.wav", timeout=5.0) as resp:
            captured_wav = resp.read()
        assert captured_wav[:4] == b"RIFF" and len(captured_wav) > 32000, f"Expected captured TTS WAV from snd-aloop, got {len(captured_wav)}B"
        with urllib.request.urlopen(f"http://127.0.0.1:{bridge_port}/api/audio/output.wav?from=4096", timeout=5.0) as resp_delta:
            delta_wav = resp_delta.read()
        expected_pcm_slice = captured_wav[44 + 4096 :]
        assert delta_wav[:4] == b"RIFF" and delta_wav[44 : 44 + len(expected_pcm_slice)] == expected_pcm_slice, (
            f"Expected delta WAV slice of >={44 + len(expected_pcm_slice)}B matching PCM offset 4096, got {len(delta_wav)}B"
        )
        out_wav = WORK_DIR / f"{board}-captured.wav"
        out_wav.write_bytes(captured_wav)
        final_st = http_json(f"http://127.0.0.1:{bridge_port}/api/state")
        assert final_st.get("ipc_error_count", 0) == 0, f"Unexpected IPC errors in logs: {final_st.get('recent_logs')}"
        print(f"[{board}] Expressive Formant TTS, STOP_TTS interrupt, ALSA snd-aloop capture & 0 IPC errors verified ({len(captured_wav)} bytes WAV -> {out_wav.name})!")

    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="KUYASE Standalone Multi-Board QEMU Verification Suite")
    parser.add_argument("--board", help="Test a specific board image in work/<board>-system.ext4")
    args = parser.parse_args()

    if args.board:
        images = [WORK_DIR / f"{args.board}-system.ext4"]
    else:
        images = sorted(WORK_DIR.glob("*-system.ext4"))
        images = [p for p in images if not p.name.endswith("-system-orig.ext4")]

    if not images:
        print("==================================================================")
        print("[TEST] В папке work/ ещё нет собранных образов *-system.ext4.")
        print("Дампы прошивок не входят в комплект репозитория KUYASE.")
        print("Сначала соберите образ из своего дампа:")
        print("  .\\scripts\\run-qemu.ps1 -DumpPath <путь_к_дампу>")
        print("==================================================================")
        sys.exit(1)

    for idx, img in enumerate(images):
        board_name = img.name[: -len("-system.ext4")]
        run_board_verification(
            board=board_name,
            drive_img=img,
            bridge_port=18091 + idx * 2,
            adb_port=15555 + idx,
        )

    tested = ", ".join(p.name[: -len("-system.ext4")] for p in images)
    print(f"\nALL KUYASE STANDALONE EMULATOR TESTS PASSED ({tested})!")
