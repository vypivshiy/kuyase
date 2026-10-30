#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_ROOT="/root/kernel-build"
KVER="6.6.60"
KSRC="${BUILD_ROOT}/linux-${KVER}"
KOUT="${BUILD_ROOT}/out-arm64"

mkdir -p "${BUILD_ROOT}" "${KOUT}"
cd "${BUILD_ROOT}"

if [ ! -d "${KSRC}" ]; then
    if [ ! -f "linux-${KVER}.tar.xz" ]; then
        echo "[kernel] Downloading linux-${KVER}.tar.xz..."
        curl -L --fail -o "linux-${KVER}.tar.xz" "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-${KVER}.tar.xz"
    fi
    echo "[kernel] Extracting linux-${KVER}.tar.xz..."
    tar -xf "linux-${KVER}.tar.xz"
fi

echo "[kernel] Patching drivers/i2c/Kconfig & drivers/i2c/i2c-stub.c for built-in I2C_RDWR support..."
sed -i '/config I2C_STUB/,/help/{/depends on m/d}' "${KSRC}/drivers/i2c/Kconfig"

cat > "${KSRC}/drivers/i2c/i2c-stub.c" << 'EOF'
// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * i2c-stub.c - I2C/SMBus chip emulator with full I2C_RDWR (.master_xfer) support
 * for Quasar QEMU Emulator (SN3236 0x3c, Midi Clock 0x21/0x37, VL53L0X 0x29, PCM5122 0x4c)
 */

#define pr_fmt(fmt) "i2c-stub: " fmt

#include <linux/errno.h>
#include <linux/i2c.h>
#include <linux/init.h>
#include <linux/kernel.h>
#include <linux/list.h>
#include <linux/module.h>
#include <linux/slab.h>

#define MAX_CHIPS 10

#define STUB_FUNC_DEFAULT \
	(I2C_FUNC_I2C | I2C_FUNC_SMBUS_QUICK | I2C_FUNC_SMBUS_BYTE | \
	 I2C_FUNC_SMBUS_BYTE_DATA | I2C_FUNC_SMBUS_WORD_DATA | \
	 I2C_FUNC_SMBUS_I2C_BLOCK | I2C_FUNC_SMBUS_BLOCK_DATA)

static unsigned short chip_addr[MAX_CHIPS] = {
	0x3c, 0x21, 0x37, 0x10, 0x1f, 0x29, 0x4c, 0
};
module_param_array(chip_addr, ushort, NULL, S_IRUGO);

static unsigned long functionality = STUB_FUNC_DEFAULT;
module_param(functionality, ulong, S_IRUGO | S_IWUSR);

struct stub_chip {
	u8 pointer;
	u16 words[256];
};

static struct stub_chip *stub_chips;
static int stub_chips_nr;

static struct stub_chip *stub_find_chip(u16 addr)
{
	int i;
	for (i = 0; i < stub_chips_nr; i++) {
		if (chip_addr[i] == addr)
			return &stub_chips[i];
	}
	return stub_chips_nr > 0 ? &stub_chips[0] : NULL;
}

static int stub_master_xfer(struct i2c_adapter *adap, struct i2c_msg *msgs, int num)
{
	int i, j;
	for (i = 0; i < num; i++) {
		struct i2c_msg *m = &msgs[i];
		struct stub_chip *chip = stub_find_chip(m->addr);
		if (!chip)
			return -ENODEV;
		if (m->flags & I2C_M_RD) {
			for (j = 0; j < m->len; j++)
				m->buf[j] = chip->words[(chip->pointer + j) & 0xff] & 0xff;
			chip->pointer = (chip->pointer + m->len) & 0xff;
		} else {
			if (m->len >= 1) {
				chip->pointer = m->buf[0];
				for (j = 1; j < m->len; j++)
					chip->words[(chip->pointer + j - 1) & 0xff] = m->buf[j];
			}
		}
	}
	return num;
}

static s32 stub_xfer(struct i2c_adapter *adap, u16 addr, unsigned short flags,
	char read_write, u8 command, int size, union i2c_smbus_data *data)
{
	int i, len;
	struct stub_chip *chip = stub_find_chip(addr);
	if (!chip)
		return -ENODEV;

	switch (size) {
	case I2C_SMBUS_QUICK:
		return 0;
	case I2C_SMBUS_BYTE:
		if (read_write == I2C_SMBUS_WRITE)
			chip->pointer = command;
		else
			data->byte = chip->words[chip->pointer++] & 0xff;
		return 0;
	case I2C_SMBUS_BYTE_DATA:
		if (read_write == I2C_SMBUS_WRITE)
			chip->words[command] = data->byte;
		else
			data->byte = chip->words[command] & 0xff;
		chip->pointer = command + 1;
		return 0;
	case I2C_SMBUS_WORD_DATA:
		if (read_write == I2C_SMBUS_WRITE)
			chip->words[command] = data->word;
		else
			data->word = chip->words[command];
		return 0;
	case I2C_SMBUS_I2C_BLOCK_DATA:
	case I2C_SMBUS_BLOCK_DATA:
		len = data->block[0];
		if (len > 256 - command)
			len = 256 - command;
		if (read_write == I2C_SMBUS_WRITE) {
			for (i = 0; i < len; i++)
				chip->words[(command + i) & 0xff] = data->block[1 + i];
		} else {
			for (i = 0; i < len; i++)
				data->block[1 + i] = chip->words[(command + i) & 0xff] & 0xff;
		}
		return 0;
	default:
		return -EOPNOTSUPP;
	}
}

static u32 stub_func(struct i2c_adapter *adapter)
{
	return functionality;
}

static const struct i2c_algorithm smbus_algorithm = {
	.functionality	= stub_func,
	.master_xfer	= stub_master_xfer,
	.smbus_xfer	= stub_xfer,
};

static struct i2c_adapter stub_adapter = {
	.owner		= THIS_MODULE,
	.class		= I2C_CLASS_HWMON | I2C_CLASS_SPD,
	.algo		= &smbus_algorithm,
	.name		= "Quasar I2C stub driver",
	.nr		= 0,
};

static int __init i2c_stub_init(void)
{
	int i, ret;
	for (i = 0; i < MAX_CHIPS && chip_addr[i]; i++)
		pr_info("Virtual chip at 0x%02x\n", chip_addr[i]);

	stub_chips_nr = i;
	stub_chips = kcalloc(stub_chips_nr, sizeof(struct stub_chip), GFP_KERNEL);
	if (!stub_chips)
		return -ENOMEM;

	ret = i2c_add_numbered_adapter(&stub_adapter);
	if (ret)
		kfree(stub_chips);
	return ret;
}

static void __exit i2c_stub_exit(void)
{
	i2c_del_adapter(&stub_adapter);
	kfree(stub_chips);
}

MODULE_LICENSE("GPL");
module_init(i2c_stub_init);
module_exit(i2c_stub_exit);
EOF

echo "[kernel] Patching drivers/video/fbdev/vfb.c for 3 built-in virtual framebuffers (/dev/fb0..2)..."
python3 - "${KSRC}/drivers/video/fbdev/vfb.c" << 'PYEOF'
import sys
p = sys.argv[1]
s = open(p).read()
s = s.replace("static bool vfb_enable __initdata = 0;", "static bool vfb_enable __initdata = 1;")
s = s.replace("if (!options)\n\t\treturn 1;", "if (!options) {\n\t\tvfb_enable = 1;\n\t\treturn 1;\n\t}")
old_init = """\t\tvfb_device = platform_device_alloc("vfb", 0);

\t\tif (vfb_device)
\t\t\tret = platform_device_add(vfb_device);
\t\telse
\t\t\tret = -ENOMEM;

\t\tif (ret) {
\t\t\tplatform_device_put(vfb_device);
\t\t\tplatform_driver_unregister(&vfb_driver);
\t\t}"""
new_init = """\t\tfor (int idx = 0; idx < 3; idx++) {
\t\t\tstruct platform_device *pdev = platform_device_alloc("vfb", idx);
\t\t\tif (pdev)
\t\t\t\tret = platform_device_add(pdev);
\t\t\tif ( idx == 0 ) vfb_device = pdev;
\t\t}"""
if old_init in s:
    s = s.replace(old_init, new_init)
open(p, "w").write(s)
PYEOF

echo "[kernel] Generating minimal QEMU virt arm64 config..."
cd "${KSRC}"
make ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu- O="${KOUT}" allnoconfig

cat >> "${KOUT}/.config" << 'EOF'
CONFIG_64BIT=y
CONFIG_ARM64=y
CONFIG_SMP=y
CONFIG_NR_CPUS=8
CONFIG_PREEMPT=y
CONFIG_HZ_250=y
CONFIG_ARCH_VIRT=y
CONFIG_ARM_AMBA=y
CONFIG_ARM_GIC=y
CONFIG_ARM_GIC_V2M=y
CONFIG_ARM_GIC_V3=y
CONFIG_ARM_GIC_V3_ITS=y
CONFIG_ARM_ARCH_TIMER=y
CONFIG_ARM_PSCI_FW=y

# 32-bit armhf userspace compatibility (maind, busybox, aplay, qemu-hw-bridge)
CONFIG_COMPAT=y
CONFIG_COMPAT_32BIT_TIME=y
CONFIG_COMPAT_BRK=y
CONFIG_COMPAT_ALIGNMENT_FIXUPS=y
CONFIG_KUSER_HELPERS=y
CONFIG_ARMV8_DEPRECATED=y
CONFIG_SWP_EMULATION=y
CONFIG_CP15_BARRIER_EMULATION=y
CONFIG_SETEND_EMULATION=y
CONFIG_SYSVIPC=y
CONFIG_SYSVIPC_COMPAT=y
CONFIG_POSIX_MQUEUE=y
CONFIG_FUTEX=y
CONFIG_EPOLL=y
CONFIG_SIGNALFD=y
CONFIG_TIMERFD=y
CONFIG_EVENTFD=y
CONFIG_SHMEM=y
CONFIG_AIO=y
CONFIG_IO_URING=y
CONFIG_INOTIFY_USER=y
CONFIG_FANOTIFY=y
CONFIG_MULTIUSER=y
CONFIG_PRINTK=y
CONFIG_BUG=y
CONFIG_ELF_CORE=y
CONFIG_BINFMT_ELF=y
CONFIG_COMPAT_BINFMT_ELF=y
CONFIG_BINFMT_SCRIPT=y
CONFIG_BINFMT_MISC=y
CONFIG_COREDUMP=y
CONFIG_SYSCTL=y
CONFIG_ANON_INODES=y

# Block, Virtio, PCI, OF/DeviceTree
CONFIG_OF=y
CONFIG_OF_FLATTREE=y
CONFIG_OF_EARLY_FLATTREE=y
CONFIG_OF_ADDRESS=y
CONFIG_OF_IRQ=y
CONFIG_OF_RESERVED_MEM=y
CONFIG_PCI=y
CONFIG_PCI_DOMAINS=y
CONFIG_PCI_DOMAINS_GENERIC=y
CONFIG_PCI_MSI=y
CONFIG_PCI_HOST_COMMON=y
CONFIG_PCI_HOST_GENERIC=y
CONFIG_VIRTIO=y
CONFIG_VIRTIO_MENU=y
CONFIG_VIRTIO_PCI=y
CONFIG_VIRTIO_PCI_LEGACY=y
CONFIG_VIRTIO_MMIO=y
CONFIG_VIRTIO_MMIO_CMDLINE_DEVICES=y
CONFIG_BLOCK=y
CONFIG_BLK_DEV=y
CONFIG_BLK_DEV_LOOP=y
CONFIG_BLK_DEV_RAM=y
CONFIG_VIRTIO_BLK=y
CONFIG_VIRTIO_CONSOLE=y

# Serial / Console (ttyAMA0)
CONFIG_TTY=y
CONFIG_VT=y
CONFIG_VT_CONSOLE=y
CONFIG_UNIX98_PTYS=y
CONFIG_LEGACY_PTYS=y
CONFIG_SERIAL_EARLYCON=y
CONFIG_SERIAL_AMBA_PL011=y
CONFIG_SERIAL_AMBA_PL011_CONSOLE=y

# Filesystems
CONFIG_EXT4_FS=y
CONFIG_EXT4_USE_FOR_EXT2=y
CONFIG_EXT4_FS_POSIX_ACL=y
CONFIG_EXT4_FS_SECURITY=y
CONFIG_FS_MBCACHE=y
CONFIG_JBD2=y
CONFIG_CRC16=y
CONFIG_CRC32=y
CONFIG_PROC_FS=y
CONFIG_PROC_SYSCTL=y
CONFIG_SYSFS=y
CONFIG_TMPFS=y
CONFIG_TMPFS_POSIX_ACL=y
CONFIG_DEVTMPFS=y
CONFIG_DEVTMPFS_MOUNT=y
CONFIG_CONFIGFS_FS=y
CONFIG_OVERLAY_FS=y
CONFIG_BLK_DEV_INITRD=y
CONFIG_RD_GZIP=y

# Networking & Virtio-Net
CONFIG_NET=y
CONFIG_PACKET=y
CONFIG_UNIX=y
CONFIG_INET=y
CONFIG_IP_MULTICAST=y
CONFIG_IP_PNP=y
CONFIG_IP_PNP_DHCP=y
CONFIG_IPV6=y
CONFIG_NETDEVICES=y
CONFIG_NET_CORE=y
CONFIG_VIRTIO_NET=y
CONFIG_LOOPBACK_TARGET=y
CONFIG_NETFILTER=y
CONFIG_NETFILTER_ADVANCED=y
CONFIG_NF_CONNTRACK=y
CONFIG_NETFILTER_XTABLES=y
CONFIG_IP_NF_IPTABLES=y
CONFIG_IP_NF_FILTER=y
CONFIG_IP_NF_NAT=y

# ALSA & snd-aloop (Virtual Microphone Array + Speaker Loopback)
CONFIG_SOUND=y
CONFIG_SND=y
CONFIG_SND_TIMER=y
CONFIG_SND_PCM=y
CONFIG_SND_DRIVERS=y
CONFIG_SND_ALOOP=y
CONFIG_SND_DYNAMIC_MINORS=y
CONFIG_SND_PROC_FS=y

# I2C & Built-in I2C Stub (/dev/i2c-0 at 0x3c, 0x21, 0x37)
CONFIG_I2C=y
CONFIG_I2C_CHARDEV=y
CONFIG_I2C_STUB=y

# Virtual Framebuffers (/dev/fb0, /dev/fb1, /dev/fb2)
CONFIG_FB=y
CONFIG_FB_VIRTUAL=y
CONFIG_FB_SYS_FILLRECT=y
CONFIG_FB_SYS_COPYAREA=y
CONFIG_FB_SYS_IMAGEBLIT=y
CONFIG_FB_SYS_FOPS=y
CONFIG_FB_DEFERRED_IO=y

# Input & uinput (/dev/uinput + /dev/input/event*)
CONFIG_INPUT=y
CONFIG_INPUT_EVDEV=y
CONFIG_INPUT_KEYBOARD=y
CONFIG_INPUT_MISC=y
CONFIG_INPUT_UINPUT=y

# RTC & Power reset
CONFIG_RTC_CLASS=y
CONFIG_RTC_HCTOSYS=y
CONFIG_RTC_DRV_PL031=y
CONFIG_POWER_RESET=y
CONFIG_POWER_RESET_SYSCON=y
CONFIG_POWER_RESET_SYSCON_POWEROFF=y
CONFIG_MFD_SYSCON=y
EOF

make ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu- O="${KOUT}" olddefconfig
echo "[kernel] Key options check:"
grep -E "CONFIG_(COMPAT|KUSER_HELPERS|SYSVIPC|VIRTIO_BLK|VIRTIO_NET|EXT4_FS|SND_ALOOP|I2C_CHARDEV|I2C_STUB|FB_VIRTUAL|INPUT_UINPUT|INPUT_EVDEV)=" "${KOUT}/.config"

echo "[kernel] Building Image on $(nproc) cores..."
make ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu- O="${KOUT}" -j"$(nproc)" Image

cp -f "${KOUT}/arch/arm64/boot/Image" "${SCRIPT_DIR}/Image"
cp -f "${KOUT}/.config" "${SCRIPT_DIR}/equasar_virt.config"
ls -lh "${SCRIPT_DIR}/Image"
echo "[kernel] Successfully built ${SCRIPT_DIR}/Image!"
