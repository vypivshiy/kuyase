use std::fs::{File, OpenOptions};
use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "linux")]
extern "C" {
    fn ioctl(fd: i32, request: u32, ...) -> i32;
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct UinputUserDev {
    name: [u8; 80],
    id_bustype: u16,
    id_vendor: u16,
    id_product: u16,
    id_version: u16,
    ff_effects_max: u32,
    absmax: [i32; 64],
    absmin: [i32; 64],
    absfuzz: [i32; 64],
    absflat: [i32; 64],
}

#[cfg(target_os = "linux")]
pub fn create_uinput_device(
    name: &str,
    ev_bits: &[u32],
    key_bits: &[u32],
    abs_bits: &[(u32, i32)],
) -> Option<File> {
    use std::os::unix::io::AsRawFd;

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/uinput")
        .ok()?;
    let fd = file.as_raw_fd();

    const UI_SET_EVBIT: u32 = 0x4004_5564;
    const UI_SET_KEYBIT: u32 = 0x4004_5565;
    const UI_SET_ABSBIT: u32 = 0x4004_5567;
    const UI_SET_SWBIT: u32 = 0x4004_5569;
    const UI_DEV_CREATE: u32 = 0x5501;

    unsafe {
        for &ev in ev_bits {
            ioctl(fd, UI_SET_EVBIT, ev);
        }
        for &k in key_bits {
            if ev_bits.contains(&5) {
                ioctl(fd, UI_SET_SWBIT, k);
            } else {
                ioctl(fd, UI_SET_KEYBIT, k);
            }
        }
        for &(abs_code, _) in abs_bits {
            ioctl(fd, UI_SET_ABSBIT, abs_code);
        }
    }

    let mut udev = UinputUserDev {
        name: [0u8; 80],
        id_bustype: 0x0010,
        id_vendor: 0x0001,
        id_product: 0x0001,
        id_version: 0x0100,
        ff_effects_max: 0,
        absmax: [0i32; 64],
        absmin: [0i32; 64],
        absfuzz: [0i32; 64],
        absflat: [0i32; 64],
    };
    let name_bytes = name.as_bytes();
    let copy_len = name_bytes.len().min(79);
    udev.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
    for &(abs_code, max_val) in abs_bits {
        if (abs_code as usize) < 64 {
            udev.absmax[abs_code as usize] = max_val;
        }
    }

    let udev_slice = unsafe {
        std::slice::from_raw_parts(
            &udev as *const UinputUserDev as *const u8,
            std::mem::size_of::<UinputUserDev>(),
        )
    };
    file.write_all(udev_slice).ok()?;
    let ret = unsafe { ioctl(fd, UI_DEV_CREATE) };
    if ret < 0 {
        return None;
    }
    Some(file)
}

#[cfg(not(target_os = "linux"))]
pub fn create_uinput_device(
    _name: &str,
    _ev_bits: &[u32],
    _key_bits: &[u32],
    _abs_bits: &[(u32, i32)],
) -> Option<File> {
    None
}

pub fn emit_compat_input_event(file: &mut File, ev_type: u16, code: u16, value: i32) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let sec = now.as_secs() as u32;
    let usec = now.subsec_micros();
    let mut buf = [0u8; 16];
    buf[0..4].copy_from_slice(&sec.to_le_bytes());
    buf[4..8].copy_from_slice(&usec.to_le_bytes());
    buf[8..10].copy_from_slice(&ev_type.to_le_bytes());
    buf[10..12].copy_from_slice(&code.to_le_bytes());
    buf[12..16].copy_from_slice(&value.to_le_bytes());
    let _ = file.write_all(&buf);
    let _ = file.flush();
}

pub fn spawn_serial_console_watcher() {
    thread::spawn(|| {
        thread::sleep(Duration::from_millis(600));
        let _ = Command::new("stty")
            .args(["-F", "/dev/ttyAMA0", "sane", "echo", "icrnl"])
            .status();
        if let Ok(mut tty) = OpenOptions::new().write(true).open("/dev/ttyAMA0") {
            let _ = tty.write_all(
                b"\r\n=== [ttyAMA0] Root shell active (pure serial console, no ADB required) ===\r\n",
            );
            let _ = tty.flush();
        }
        loop {
            if let (Ok(sin), Ok(sout), Ok(serr)) = (
                OpenOptions::new().read(true).open("/dev/ttyAMA0"),
                OpenOptions::new().write(true).open("/dev/ttyAMA0"),
                OpenOptions::new().write(true).open("/dev/ttyAMA0"),
            ) {
                let _ = Command::new("/bin/sh")
                    .arg("-i")
                    .stdin(Stdio::from(sin))
                    .stdout(Stdio::from(sout))
                    .stderr(Stdio::from(serr))
                    .status();
            }
            thread::sleep(Duration::from_millis(500));
        }
    });
}
