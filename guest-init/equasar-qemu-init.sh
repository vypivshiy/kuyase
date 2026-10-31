#!/bin/sh
# Quasar QEMU Emulator Guest Hardware Initializer (`/quasar-qemu-init.sh`)

export PATH=/usr/local/bin:/bkupgrade/data/opt/bin:/system/workdir/bin:/system/vendor/quasar:/usr/sbin:/usr/bin:/sbin:/bin
export LD_LIBRARY_PATH=/bkupgrade/data/opt/lib:/system/workdir/lib:/system/vendor/quasar:/system/vendor/quasar/libs:/usr/lib:/lib

mount -t proc proc /proc 2>/dev/null || true
mount -t devtmpfs devtmpfs /dev 2>/dev/null || true
mkdir -p /dev/pts /dev/shm /tmp /run /sys /var/run /root /bkupgrade
[ -L /data ] || mkdir -p /data
mount -t devpts devpts /dev/pts 2>/dev/null || true
mount -t tmpfs tmpfs /dev/shm 2>/dev/null || true
mount -t tmpfs tmpfs /tmp 2>/dev/null || true
mount -t tmpfs tmpfs /run 2>/dev/null || true
mount -t tmpfs tmpfs /var/run 2>/dev/null || true
mount -t tmpfs tmpfs /bkupgrade 2>/dev/null || true
[ -L /data ] || mount -t tmpfs tmpfs /data 2>/dev/null || true
mount -t tmpfs tmpfs /sys 2>/dev/null || true

chmod 666 /dev/ptmx /dev/null /dev/zero /dev/urandom /dev/tty 2>/dev/null || true
mkdir -p /tmp/daemons_logs /var/run /bkupgrade/data/quasar/daemons_logs /bkupgrade/data/opt/bin /bkupgrade/data/opt/lib /data/quasar/daemons_logs
touch /tmp/daemons_logs/maind.log

# 1. Detect board profile & flags from kernel cmdline
BOARD="auto"
MODE="stub"
for arg in $(cat /proc/cmdline); do
  case "$arg" in
    quasar.board=*) BOARD="${arg#quasar.board=}" ;;
    quasar.mode=*)  MODE="${arg#quasar.mode=}" ;;
  esac
done

if [ "$BOARD" = "auto" ] || [ -z "$BOARD" ]; then
  if [ -s /etc/quasar-board ]; then
    BOARD=$(head -n 1 /etc/quasar-board 2>/dev/null | tr -d '\r\n ')
  fi
fi

if [ "$BOARD" = "auto" ] || [ -z "$BOARD" ]; then
  DEV_TYPE=""
  if [ -f /system/vendor/quasar/quasar.cfg ]; then
    DEV_TYPE=$(sed -n 's/.*"deviceType"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' /system/vendor/quasar/quasar.cfg 2>/dev/null | head -n 1)
    if [ -z "$DEV_TYPE" ]; then
      DEV_TYPE=$(grep '"deviceType"' /system/vendor/quasar/quasar.cfg 2>/dev/null | head -n 1 | sed 's/.*"deviceType"[^"]*"\([^"]*\)".*/\1/')
    fi
  fi
  case "$DEV_TYPE" in
    yandexmini_2|mini2)
      if grep -q '4794659041' /system/vendor/quasar/quasar.cfg 2>/dev/null; then
        BOARD="yandexmini2nc"
      else
        BOARD="yandexmini2"
      fi
      ;;
    plum)
      if grep -q '4794654278' /system/vendor/quasar/quasar.cfg 2>/dev/null; then
        BOARD="plumnc"
      else
        BOARD="plum"
      fi
      ;;
    station_2|station-midi|yandexmidi) BOARD="station2" ;;
    "")
      if [ -e /etc/init.d/S61leds_brightness ]; then
        BOARD="yandexmini2"
      elif [ -e /boot/yandex-cucumber-i2ctop-overlay.dtb ]; then
        BOARD="cucumber"
      elif [ -e /boot/yandex-orion-i2c_top-overlay.dtb ]; then
        BOARD="orion"
      else
        BOARD="yandexmini"
      fi
      ;;
    *) BOARD="$DEV_TYPE" ;;
  esac
fi

case "$BOARD" in
  mini2|yandexmini_2)        BOARD="yandexmini2" ;;
  mini2nc|yandexmini_2_nc)   BOARD="yandexmini2nc" ;;
  mini3)                     BOARD="bergamot" ;;
  mini3pro)                  BOARD="pickle" ;;
  lite2)                     BOARD="plum" ;;
  prune|lite2nc|plumnc)      BOARD="plumnc" ;;
  lite1|micro)               BOARD="yandexmicro" ;;
  station3)                  BOARD="orion" ;;
  station_2|station-midi|yandexmidi) BOARD="station2" ;;
  street)                    BOARD="mango" ;;
  hub)                       BOARD="saturn" ;;
esac

export QUASAR_BOARD="$BOARD"
hostname "qemu-${BOARD}" 2>/dev/null || true

# 2. Ensure loopback is up (eth0 is configured via kernel `ip=dhcp`)
ip link set lo up 2>/dev/null || ifconfig lo 127.0.0.1 up 2>/dev/null || true

# 3. Ensure passwordless root login for ADB shell
cat > /etc/passwd << 'EOF'
root::0:0:root:/root:/bin/sh
nobody:x:65534:65534:nobody:/nonexistent:/bin/false
EOF
cat > /etc/group << 'EOF'
root:x:0:
nogroup:x:65534:
EOF
echo "/bin/sh" > /etc/shells
rm -f /etc/shadow 2>/dev/null || true

# 4. Start ADB daemon (TCP :5555)
rm -rf /dev/usb-ffs 2>/dev/null || true
if [ -x /usr/bin/adbd ]; then
  /usr/bin/adbd </dev/null >/tmp/adbd.log 2>&1 &
fi

# 5. Spoof /proc/device-tree (`model` & `compatible`) and /sys/devices/virtual/dmi/id via bind/tmpfs
mkdir -p /tmp/fake-dt /sys/devices/virtual/dmi/id /sys/firmware/devicetree

case "$BOARD" in
  cucumber)
    printf "Yandex Cucumber DVT2\0" > /tmp/fake-dt/model
    printf "yandex,cucumber-dvt2\0amlogic,a5\0" > /tmp/fake-dt/compatible
    BOARD_NAME="Яндекс Станция Миди (YNDX-00054 / cucumber)"
    SYSFS_PREFIX="cucumber"; RING_LEDS=24; LOGO_LEDS=6; AMBILIGHT_LEDS=0; HAS_CLOCK="true"; LED_DRIVER="framebuffer"
    ;;
  orion)
    printf "Yandex Orion PVT\0" > /tmp/fake-dt/model
    printf "yandex,orion-pvt\0yandex,orion\0amlogic,a5\0" > /tmp/fake-dt/compatible
    printf "a5_orion_pvt\0" > /tmp/fake-dt/amlogic-dt-id
    BOARD_NAME="Яндекс Станция 3 (YNDX-00055 / orion)"
    SYSFS_PREFIX="orion"; RING_LEDS=36; LOGO_LEDS=0; AMBILIGHT_LEDS=12; HAS_CLOCK="false"; LED_DRIVER="framebuffer"
    ;;
  yandexmini2)
    printf "Yandex Station Mini 2\0" > /tmp/fake-dt/model
    printf "yandex,station-mini2\0amlogic,axg\0" > /tmp/fake-dt/compatible
    printf "axg_mini2_es\0" > /tmp/fake-dt/amlogic-dt-id
    BOARD_NAME="Яндекс Станция Мини 2 с часами (YNDX-00021 / yandexmini2)"
    SYSFS_PREFIX="mini2"; RING_START=1; RING_LEDS=6; LOGO_LEDS=0; AMBILIGHT_LEDS=0; HAS_CLOCK="true"; LED_DRIVER="sysfs"
    ;;
  yandexmini2nc)
    printf "Yandex Station Mini 2\0" > /tmp/fake-dt/model
    printf "yandex,station-mini2\0amlogic,axg\0" > /tmp/fake-dt/compatible
    printf "axg_mini2_es\0" > /tmp/fake-dt/amlogic-dt-id
    BOARD_NAME="Яндекс Станция Мини 2 без часов (YNDX-00020 / yandexmini2nc)"
    SYSFS_PREFIX="mini2"; RING_START=1; RING_LEDS=6; LOGO_LEDS=0; AMBILIGHT_LEDS=0; HAS_CLOCK="false"; LED_DRIVER="sysfs"
    ;;
  bergamot)
    printf "Yandex Bergamot\0" > /tmp/fake-dt/model
    printf "yandex,bergamot\0amlogic,axg\0" > /tmp/fake-dt/compatible
    BOARD_NAME="Яндекс Станция Мини 3 (bergamot)"
    SYSFS_PREFIX="bergamot"; RING_LEDS=6; LOGO_LEDS=0; AMBILIGHT_LEDS=0; HAS_CLOCK="true"; LED_DRIVER="framebuffer"
    ;;
  pickle)
    printf "Yandex Pickle\0" > /tmp/fake-dt/model
    printf "yandex,pickle\0amlogic,a4\0" > /tmp/fake-dt/compatible
    BOARD_NAME="Яндекс Станция Мини 3 Про (pickle)"
    SYSFS_PREFIX="pickle"; RING_LEDS=16; LOGO_LEDS=6; AMBILIGHT_LEDS=0; HAS_CLOCK="true"; LED_DRIVER="sysfs"
    ;;
  plum)
    printf "Yandex Plum EVT2\0" > /tmp/fake-dt/model
    printf "yandex,plum-evt2\0yandex,plum\0amlogic,axg\0" > /tmp/fake-dt/compatible
    BOARD_NAME="Яндекс Станция Лайт 2 Plum (3 LED + часы)"
    SYSFS_PREFIX="plum"; RING_START=1; RING_LEDS=3; LOGO_LEDS=0; AMBILIGHT_LEDS=0; HAS_CLOCK="true"; LED_DRIVER="sysfs"
    ;;
  plumnc)
    printf "Yandex Prune EVT2\0" > /tmp/fake-dt/model
    printf "yandex,prune-evt2\0yandex,prune\0amlogic,axg\0" > /tmp/fake-dt/compatible
    BOARD_NAME="Яндекс Станция Лайт 2 Prune (3 LED, без часов)"
    SYSFS_PREFIX="plum"; RING_START=1; RING_LEDS=3; LOGO_LEDS=0; AMBILIGHT_LEDS=0; HAS_CLOCK="false"; LED_DRIVER="sysfs"
    ;;
  yandexmicro)
    printf "Yandex Station Lite\0" > /tmp/fake-dt/model
    printf "yandex,yandexmicro\0amlogic,axg\0" > /tmp/fake-dt/compatible
    BOARD_NAME="Яндекс Станция Лайт 1 (YNDX-00025 / yandexmicro)"
    SYSFS_PREFIX="micro"; RING_START=1; RING_LEDS=3; LOGO_LEDS=0; AMBILIGHT_LEDS=0; HAS_CLOCK="false"; LED_DRIVER="sysfs"
    ;;
  station2)
    printf "Yandex Station Midi\0" > /tmp/fake-dt/model
    printf "yandex,station-midi\0amlogic,axg\0" > /tmp/fake-dt/compatible
    BOARD_NAME="Яндекс Станция 2 (YNDX-00051 / station-midi)"
    SYSFS_PREFIX="station2"; RING_LEDS=88; LOGO_LEDS=0; AMBILIGHT_LEDS=0; HAS_CLOCK="false"; LED_DRIVER="framebuffer"
    ;;
  mango)
    printf "Yandex Station Mango DVT\0" > /tmp/fake-dt/model
    printf "yandex,mango\0amlogic,a1\0" > /tmp/fake-dt/compatible
    BOARD_NAME="Яндекс Станция Стрит (mango)"
    SYSFS_PREFIX="mango"; RING_LEDS=4; LOGO_LEDS=3; AMBILIGHT_LEDS=0; HAS_CLOCK="false"; LED_DRIVER="sysfs"
    ;;
  saturn)
    printf "Yandex Saturn\0" > /tmp/fake-dt/model
    printf "yandex,saturn\0amlogic,axg\0" > /tmp/fake-dt/compatible
    BOARD_NAME="Умный хаб Яндекса (saturn)"
    SYSFS_PREFIX="saturn"; RING_LEDS=1; LOGO_LEDS=0; AMBILIGHT_LEDS=0; HAS_CLOCK="false"; LED_DRIVER="sysfs"
    ;;
  yandexmini)
    printf "Amlogic\0" > /tmp/fake-dt/model
    printf "amlogic, axg\0" > /tmp/fake-dt/compatible
    BOARD_NAME="Яндекс Станция Мини 1 (YNDX-0004 / yandexmini)"
    SYSFS_PREFIX="yandexmini"; RING_LEDS=6; LOGO_LEDS=0; AMBILIGHT_LEDS=0; HAS_CLOCK="false"; LED_DRIVER="i2c_sn3236"
    ;;
  *)
    printf "Yandex Quasar (%s)\0" "$BOARD" > /tmp/fake-dt/model
    printf "yandex,%s\0amlogic,a5\0" "$BOARD" > /tmp/fake-dt/compatible
    BOARD_NAME="Quasar Speaker ($BOARD)"
    SYSFS_PREFIX="$BOARD"; RING_LEDS=6; LOGO_LEDS=0; AMBILIGHT_LEDS=0; HAS_CLOCK="false"; LED_DRIVER="sysfs"
    # Auto-discover LED count & clock from /system/vendor/quasar/ledpatterns and quasar.cfg
    for pat in /system/vendor/quasar/ledpatterns/ring/alice_listening.led /system/vendor/quasar/ledpatterns/oknix/alice_listening.led /system/vendor/quasar/ledpatterns/alice_listening.led; do
      if [ -f "$pat" ]; then
        cols=$(grep -E '^[0-9a-fA-F]{6} ' "$pat" | head -n 1 | wc -w)
        if [ "$cols" -gt 1 ]; then
          RING_LEDS=$((cols - 1))
          break
        fi
      fi
    done
    if [ -d /system/vendor/quasar/ledpatterns/clock ] || grep -q '"clock"' /system/vendor/quasar/quasar.cfg 2>/dev/null; then
      HAS_CLOCK="true"
    fi
    ;;
esac

echo "$BOARD_NAME" > /sys/devices/virtual/dmi/id/product_name
ln -sfn /tmp/fake-dt /sys/firmware/devicetree/base 2>/dev/null || true
mount --bind /tmp/fake-dt /proc/device-tree 2>/dev/null || true

# Write hardware summary JSON for qemu-hw-bridge and external SDK tools
cat > /tmp/quasar-emu-hw.json << EOF
{
  "board_slug": "$BOARD",
  "sysfs_prefix": "$SYSFS_PREFIX",
  "board_name": "$BOARD_NAME",
  "ring_leds": $RING_LEDS,
  "logo_leds": $LOGO_LEDS,
  "ambilight_leds": $AMBILIGHT_LEDS,
  "has_clock": $HAS_CLOCK,
  "led_driver": "$LED_DRIVER"
}
EOF

# 6. Populate /sys/class/leds & /sys/class/graphics/fb*
mkdir -p /sys/class/leds /sys/class/graphics/fb0 /sys/class/graphics/fb1 /sys/class/graphics/fb2 /sys/class/backlight/fb0 /sys/class/backlight/fb1 /sys/class/backlight/fb2
if [ "$BOARD" = "orion" ]; then
  echo "ambilight" > /sys/class/graphics/fb0/name
  echo "YNDX LED screen" > /sys/class/graphics/fb1/name
  echo "ring" > /sys/class/graphics/fb2/name
  echo 255 > /sys/class/backlight/fb0/brightness
  echo 255 > /sys/class/backlight/fb1/brightness
  echo 255 > /sys/class/backlight/fb2/brightness
  [ -e /dev/fb0 ] || dd if=/dev/zero of=/dev/fb0 bs=72 count=1 2>/dev/null || true
  [ -e /dev/fb1 ] || dd if=/dev/zero of=/dev/fb1 bs=756 count=1 2>/dev/null || true
  [ -e /dev/fb2 ] || dd if=/dev/zero of=/dev/fb2 bs=108 count=1 2>/dev/null || true
else
  echo "ring" > /sys/class/graphics/fb0/name
  echo "YNDX LED screen" > /sys/class/graphics/fb1/name
  echo 255 > /sys/class/backlight/fb0/brightness
  echo 255 > /sys/class/backlight/fb1/brightness
  [ -e /dev/fb0 ] || dd if=/dev/zero of=/dev/fb0 bs=264 count=1 2>/dev/null || true
  [ -e /dev/fb1 ] || dd if=/dev/zero of=/dev/fb1 bs=264 count=1 2>/dev/null || true
fi

if [ "$LED_DRIVER" != "i2c_sn3236" ]; then
  RING_START="${RING_START:-0}"
  i=$RING_START
  while [ $i -lt $((RING_START + RING_LEDS)) ]; do
    for c in red green blue; do
      d="/sys/class/leds/${SYSFS_PREFIX}:${c}:ring${i}"
      mkdir -p "$d"
      echo 0 > "$d/brightness"
      echo 255 > "$d/max_brightness"
      echo "[ledfb-ring] none" > "$d/trigger"
    done
    i=$((i + 1))
  done

  i=0
  while [ $i -lt "$LOGO_LEDS" ]; do
    for c in red green blue; do
      d="/sys/class/leds/${SYSFS_PREFIX}:${c}:logo${i}"
      mkdir -p "$d"
      echo 0 > "$d/brightness"
      echo 255 > "$d/max_brightness"
      echo "[ledfb-logo] none" > "$d/trigger"
    done
    i=$((i + 1))
  done

  i=0
  while [ $i -lt "$AMBILIGHT_LEDS" ]; do
    for sub in a b; do
      for c in red green blue white; do
        d="/sys/class/leds/${SYSFS_PREFIX}:${c}:ambilight${i}${sub}"
        mkdir -p "$d"
        echo 0 > "$d/brightness"
        echo 255 > "$d/max_brightness"
        echo "[ledfb-ambilight] none" > "$d/trigger"
      done
    done
    i=$((i + 1))
  done
  if [ "$BOARD" = "orion" ]; then
    for grp in A B C; do
      for bus in ring ambilight; do
        d="/sys/class/leds/orion:${bus}${grp}:sync"
        mkdir -p "$d"
        echo 0 > "$d/brightness"
        echo 255 > "$d/max_brightness"
        echo "[ledfb-${bus}-sync] none" > "$d/trigger"
      done
    done
  fi

  if [ "$HAS_CLOCK" = "true" ]; then
    for pos in 1 2 3 4; do
      for part in a b c d e f g; do
        d="/sys/class/leds/${SYSFS_PREFIX}:white:clock-${part}${pos}"
        mkdir -p "$d"
        echo 0 > "$d/brightness"
        echo 255 > "$d/max_brightness"
        echo "[none]" > "$d/trigger"
        up=$(echo "$part" | tr 'a-g' 'A-G')
        ln -sfn "${SYSFS_PREFIX}:white:clock-${part}${pos}" "/sys/class/leds/${SYSFS_PREFIX}:white:clock${up}${pos}"
      done
    done
    for sym in p3 q3 p4 q4 p5 q5 p6 q6 p7 q7 h1 h2 m1 m2 l1 l2 deg; do
      d="/sys/class/leds/${SYSFS_PREFIX}:white:clock-${sym}"
      mkdir -p "$d"
      echo 0 > "$d/brightness"
      echo 255 > "$d/max_brightness"
      echo "[none]" > "$d/trigger"
    done
    ln -sfn "${SYSFS_PREFIX}:white:clock-deg" "/sys/class/leds/${SYSFS_PREFIX}:white:clockDEGREE"
    ln -sfn "${SYSFS_PREFIX}:white:clock-p3" "/sys/class/leds/${SYSFS_PREFIX}:white:clockPLUS1"
    ln -sfn "${SYSFS_PREFIX}:white:clock-q3" "/sys/class/leds/${SYSFS_PREFIX}:white:clockPLUS2"
    ln -sfn "${SYSFS_PREFIX}:white:clock-m1" "/sys/class/leds/${SYSFS_PREFIX}:white:clockMINUS"
    ln -sfn "${SYSFS_PREFIX}:white:clock-p3" "/sys/class/leds/${SYSFS_PREFIX}:white:clockDH"
    ln -sfn "${SYSFS_PREFIX}:white:clock-q3" "/sys/class/leds/${SYSFS_PREFIX}:white:clockDL"
    ln -sfn "${SYSFS_PREFIX}:white:clock-m1" "/sys/class/leds/${SYSFS_PREFIX}:white:clockDM"
    for part in a b c d e f g; do
      up=$(echo "$part" | tr 'a-g' 'A-G')
      ln -sfn "${SYSFS_PREFIX}:white:clock-${part}1" "/sys/class/leds/${SYSFS_PREFIX}:white:clock${up}0"
    done
  fi
fi

# 7. Populate /sys/class/thermal, /sys/bus/iio, cpufreq, and ToF sysfs
mkdir -p /sys/class/thermal/thermal_zone0
echo "soc_thermal" > /sys/class/thermal/thermal_zone0/type
echo 40000 > /sys/class/thermal/thermal_zone0/temp

mkdir -p /sys/bus/iio/devices/iio:device0 /sys/bus/iio/devices/iio:device1
if [ "$BOARD" = "yandexmini" ]; then
  echo "meson-axg-saradc" > /sys/bus/iio/devices/iio:device0/name
elif [ "$BOARD" = "orion" ]; then
  echo "light-sensor-top" > /sys/bus/iio/devices/iio:device0/name
  echo "meson-saradc" > /sys/bus/iio/devices/iio:device1/name
else
  echo "ltr308als01" > /sys/bus/iio/devices/iio:device0/name
fi
echo 240 > /sys/bus/iio/devices/iio:device0/in_illuminance_raw
echo 0 > /sys/bus/iio/devices/iio:device0/in_voltage0_mean_raw
echo 621 > /sys/bus/iio/devices/iio:device0/in_voltage1_mean_raw
echo 362 > /sys/bus/iio/devices/iio:device0/in_voltage2_mean_raw
echo 302 > /sys/bus/iio/devices/iio:device0/in_voltage3_mean_raw
echo 2391 > /sys/bus/iio/devices/iio:device0/in_voltage4_mean_raw
echo 2400 > /sys/bus/iio/devices/iio:device0/in_voltage5_mean_raw
echo 2187 > /sys/bus/iio/devices/iio:device0/in_voltage6_mean_raw
echo 2187 > /sys/bus/iio/devices/iio:device0/in_voltage6_raw
echo 741 > /sys/bus/iio/devices/iio:device0/in_voltage7_mean_raw

mkdir -p /sys/devices/system/cpu/cpu0/cpufreq
echo 100000 > /sys/devices/system/cpu/cpu0/cpufreq/scaling_min_freq
echo 1200000 > /sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq

mkdir -p /sys/class/input/event0/device
echo 1 > /sys/class/input/event0/device/enable_ps_sensor
echo 30 > /sys/class/input/event0/device/set_delay_ms

mkdir -p /sys/bus/i2c/devices/3-0021 /sys/bus/i2c/devices/3-0037
if [ "$BOARD" = "orion" ]; then
  mkdir -p /sys/bus/i2c/devices/2-002c /sys/bus/i2c/devices/3-002c /sys/bus/i2c/devices/3-002d /sys/bus/i2c/devices/3-002e
  mkdir -p /sys/bus/i2c/devices/0-0012 /sys/bus/i2c/devices/0-0034 /sys/bus/i2c/devices/0-0035 /sys/bus/i2c/devices/0-0036
  mkdir -p /sys/bus/i2c/devices/2-0008 /sys/bus/i2c/devices/2-0034 /sys/bus/i2c/devices/2-0035 /sys/bus/i2c/devices/2-0036
  echo "orion-acm8625-us.cfg" > /sys/bus/i2c/devices/2-002c/settings_file
  echo "orion-acm8625-center.cfg" > /sys/bus/i2c/devices/3-002c/settings_file
  echo "orion-acm8625-woofer.cfg" > /sys/bus/i2c/devices/3-002d/settings_file
  echo "orion-acm8625-lr.cfg" > /sys/bus/i2c/devices/3-002e/settings_file
  mkdir -p /sys/class/power_supply/ya_pd/device /sys/devices/platform/pcba_ids/pcba_ids_drv
  echo "husb238_pd" > /sys/class/power_supply/ya_pd/device/name
  echo "Good" > /sys/class/power_supply/ya_pd/health
  echo 20000 > /sys/class/power_supply/ya_pd/voltage_now
  echo 3250 > /sys/class/power_supply/ya_pd/current_now
  echo '[{"name":"main","assembly":"1"},{"name":"top","assembly":"1"}]' > /sys/devices/platform/pcba_ids/pcba_ids_drv/pcba_ids
  touch /dev/ree2aocpu
fi

# 8. Configure ALSA (/etc/asound.conf & /tmp/alsa/alsa.conf) for snd-aloop (card 0: Loopback)
if [ -d /system/workdir/lib/alsa ]; then
  mkdir -p /tmp/alsa
  cp -rf /system/workdir/lib/alsa/* /tmp/alsa/ 2>/dev/null || true
  rm -f /tmp/alsa/asoundrc 2>/dev/null || true
elif [ -d /system/workdir/share/alsa ]; then
  mkdir -p /tmp/alsa
  cp -rf /system/workdir/share/alsa/* /tmp/alsa/ 2>/dev/null || true
  rm -f /tmp/alsa/asoundrc 2>/dev/null || true
fi

cat > /etc/asound.conf << 'EOF'
pcm.dmixer {
  type dmix
  ipc_key 1024
  ipc_perm 0666
  slave {
    pcm "hw:0,0,0"
    rate 48000
    channels 2
    format S16_LE
    period_size 1024
    buffer_size 8192
  }
}

pcm.speakers {
  type softvol
  min_dB -60.0
  max_dB 0.0
  resolution 33
  slave.pcm {
    type plug
    slave.pcm "dmixer"
  }
  control {
    name "Master"
    card 0
  }
}

pcm.ultrasound {
  type softvol
  slave.pcm {
    type plug
    slave {
      pcm "hw:0,0,2"
      rate 96000
      channels 2
      format S16_LE
    }
  }
  control {
    name "Ultrasound Playback Volume"
    card 0
  }
}

pcm.bluetooth {
  type softvol
  slave.pcm "speakers"
  control {
    name "Bluetooth Master Playback Volume"
    card 0
  }
}

pcm.mics {
  type plug
  slave {
    pcm "hw:0,0,1"
    channels 4
    rate 48000
    format S16_LE
  }
}

pcm.dmicarray {
  type plug
  slave.pcm "mics"
}

pcm.!default {
  type asym
  playback.pcm "speakers"
  capture.pcm "mics"
}
EOF

if [ -x /system/workdir/bin/aplay ]; then
  /system/workdir/bin/aplay -D default -q -t raw -f S16_LE -r 48000 -c 2 -d 1 /dev/zero 2>/dev/null &
elif [ -x /usr/bin/aplay ]; then
  /usr/bin/aplay -D default -q -t raw -f S16_LE -r 48000 -c 2 -s 1 /dev/zero 2>/dev/null || true
fi

echo "=== [Quasar QEMU Emulator] Booting board=$BOARD maind_mode=$MODE ==="

# 9. Start Virtual Speaker Twin & Hardware Bridge (:8091)
# If host passed an updated qemu-hw-bridge binary via second virtio drive (/dev/vdb), hot-update it before launch
if [ -b /dev/vdb ] && [ "$(dd if=/dev/vdb bs=4 count=1 2>/dev/null)" = "$(printf '\177ELF')" ]; then
  cat /dev/vdb > /usr/local/bin/qemu-hw-bridge 2>/dev/null || true
  chmod 0755 /usr/local/bin/qemu-hw-bridge 2>/dev/null || true
fi
/usr/local/bin/qemu-hw-bridge >/tmp/hw-bridge.log 2>&1 &
sleep 0.4

# 10. Start native Quasar maind stack only when maind_mode=full (otherwise qemu-hw-bridge runs the lightweight IPC/log stub)
if [ "$MODE" = "full" ] && [ -x /system/vendor/quasar/onboot.sh ]; then
  echo "=== Starting native Quasar maind stack (/system/vendor/quasar/onboot.sh start) ==="
  /system/vendor/quasar/onboot.sh start >/tmp/quasar-onboot.log 2>&1 &
fi

# 11. Run any user-installed /etc/init.d/S99* startup scripts (skipping firmware S99onboot.sh / S99zdropbear)
for s in /etc/init.d/S99*; do
  [ -e "$s" ] || continue
  bname=$(basename "$s")
  case "$bname" in
    S99onboot.sh|S99quasar*|S99zdropbear|S99dropbear*|S99adbd*)
      continue
      ;;
  esac
  if [ -x "$s" ]; then
    "$s" start >/tmp/init-user.log 2>&1 &
  fi
done

echo "=== [Quasar QEMU Emulator] READY: ADB (:5555), Twin (:8091) ==="

while true; do
  sleep 3600
done
