//! # Декомпозиция эмулируемой периферии через доменные трейты (`devices.rs`)
//!
//! Вместо монолитных условных переходов `if board == "..."` каждое физическое
//! устройство колонки представлено чистым `Send + Sync` трейтом по своей роли:
//!
//! 1. [`LedRingDevice`] — адресное световое кольцо (`SN3236` I2C `0x3c`, `sysfs`
//!    или ядерный фреймбуфер `quasar_led_ring.ko` с защитой триггера `[ledfb-ring]`).
//! 2. [`AuxLightDevice`] — дополнительная подсветка корпуса (`12×2 RGBW` эмбиент
//!    Станции 3 `orion` на `/dev/fb0` + `sysfs`, либо `logo0..N` на `cucumber`/`pickle`/`mango`).
//! 3. [`FrontDisplayDevice`] — фронтальный индикатор времени, громкости и эмоций
//!    (7-сегментный циферблат `*:white:clock-*` либо монохромный матричный LED-экран
//!    `21×12` `gowin_led_device.ko` на `/dev/fb1` в `orion`).
//! 4. [`AudioSubsystemDevice`] — аппаратные усилители (`4× ACM8625S` на I2C в `orion`
//!    с контролем профилей `settings_file`, кривой `hardvold` и `mute`) и отдельный
//!    ультразвуковой TDM-тракт (`pcm.ultrasound` `96 кГц` / `38.4 кГц`).

use crate::leds::parse_led_pattern_str;
use crate::state::BoardProfile;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

fn clock_segs_for_char(c: char) -> &'static [&'static str] {
    match c.to_ascii_uppercase() {
        '0' | 'O' => &["a", "b", "c", "d", "e", "f"],
        '1' | 'I' => &["b", "c"],
        '2' => &["a", "b", "d", "e", "g"],
        '3' => &["a", "b", "c", "d", "g"],
        '4' => &["b", "c", "f", "g"],
        '5' | 'S' => &["a", "c", "d", "f", "g"],
        '6' => &["a", "c", "d", "e", "f", "g"],
        '7' => &["a", "b", "c"],
        '8' => &["a", "b", "c", "d", "e", "f", "g"],
        '9' => &["a", "b", "c", "d", "f", "g"],
        '-' => &["g"],
        _ => &[],
    }
}

// ============================================================================
// Вспомогательные функции sysfs и поиска framebuffer по имени
// ============================================================================

pub fn read_sysfs_u8(path: &str) -> u8 {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<u8>().ok())
        .unwrap_or(0)
}

pub fn read_active_trigger(path: &str) -> String {
    let Ok(s) = fs::read_to_string(path) else {
        return "none".to_string();
    };
    if let (Some(l), Some(r)) = (s.find('['), s.find(']')) {
        if r > l + 1 {
            return s[l + 1..r].to_string();
        }
    }
    s.trim().to_string()
}

/// Находит устройство `/dev/fb*` по `/sys/class/graphics/fb*/name` (аналог `findFrameBufferByName` в `maind`).
pub fn find_framebuffer_by_name(expected_name: &str, fallback_dev: &str) -> String {
    for idx in 0..4 {
        let name_path = format!("/sys/class/graphics/fb{}/name", idx);
        if let Ok(raw) = fs::read_to_string(&name_path) {
            if raw.trim() == expected_name {
                return format!("/dev/fb{}", idx);
            }
        }
    }
    fallback_dev.to_string()
}

// ============================================================================
// 1. Трейт адресного светового кольца (`LedRingDevice`)
// ============================================================================

pub struct RingReadout {
    pub pixels: Vec<[u8; 3]>,
    pub trigger: String,
}

pub trait LedRingDevice: Send + Sync {
    fn driver_name(&self) -> &'static str;
    fn init(&self);
    fn read_ring(&self, native_frame: &[[u8; 3]], conflict_counter: &AtomicU32) -> RingReadout;
    fn write_native_frame(&self, px: &[[u8; 3]]);
}

#[cfg(target_os = "linux")]
mod i2c_sn3236 {
    use std::fs::OpenOptions;
    use std::os::unix::io::AsRawFd;
    use std::sync::{Mutex, OnceLock};

    extern "C" {
        fn ioctl(fd: i32, request: u32, ...) -> i32;
    }

    const I2C_RDWR: u32 = 0x0707;
    const I2C_M_RD: u16 = 0x0001;
    const ADDR: u16 = 0x003c;
    const IDLE_SENTINEL: u8 = 0x5a;

    struct Sn3236Latched {
        pwm: [u8; 18],
        ctrl: [u8; 18],
    }

    fn latched_state() -> &'static Mutex<Sn3236Latched> {
        static STATE: OnceLock<Mutex<Sn3236Latched>> = OnceLock::new();
        STATE.get_or_init(|| {
            Mutex::new(Sn3236Latched {
                pwm: [0u8; 18],
                ctrl: [0x01u8; 18],
            })
        })
    }

    #[repr(C)]
    pub struct I2cMsg {
        pub addr: u16,
        pub flags: u16,
        pub len: u16,
        pub buf: *mut u8,
    }

    #[repr(C)]
    struct I2cRdwrIoctlData {
        msgs: *mut I2cMsg,
        nmsgs: u32,
    }

    fn run_i2c_rdwr(msgs: &mut [I2cMsg]) -> bool {
        let Ok(file) = OpenOptions::new().read(true).write(true).open("/dev/i2c-0") else {
            return false;
        };
        let mut rdwr = I2cRdwrIoctlData {
            msgs: msgs.as_mut_ptr(),
            nmsgs: msgs.len() as u32,
        };
        let ret = unsafe { ioctl(file.as_raw_fd(), I2C_RDWR, &mut rdwr as *mut I2cRdwrIoctlData) };
        ret >= 0
    }

    fn scale_channel(pwm: u8, ctrl: u8, chip_enabled: bool) -> (u8, bool) {
        let out_en = chip_enabled && ((ctrl & 0x01) != 0);
        if !out_en {
            return (0, false);
        }
        let div: u16 = match (ctrl >> 1) & 0x03 {
            0 => 1,
            1 => 2,
            2 => 3,
            _ => 4,
        };
        (((pwm as u16) / div) as u8, true)
    }

    pub fn read_ring() -> Option<([[u8; 3]; 6], bool)> {
        let mut reg_addr = [0x00u8];
        let mut reg_data = [0u8; 76];
        let mut msgs = [
            I2cMsg {
                addr: ADDR,
                flags: 0,
                len: 1,
                buf: reg_addr.as_mut_ptr(),
            },
            I2cMsg {
                addr: ADDR,
                flags: I2C_M_RD,
                len: reg_data.len() as u16,
                buf: reg_data.as_mut_ptr(),
            },
        ];
        if !run_i2c_rdwr(&mut msgs) {
            return None;
        }

        let mut latched = latched_state().lock().unwrap();

        if reg_data[0x4a] != IDLE_SENTINEL {
            latched.pwm = [0u8; 18];
            latched.ctrl = [0u8; 18];
            reg_data.fill(0);
            let mut clear_buf = [0u8; 77];
            clear_buf[0] = 0x00;
            clear_buf[1 + 0x25] = IDLE_SENTINEL;
            clear_buf[1 + 0x4a] = IDLE_SENTINEL;
            let mut clear_msg = [I2cMsg {
                addr: ADDR,
                flags: 0,
                len: clear_buf.len() as u16,
                buf: clear_buf.as_mut_ptr(),
            }];
            let _ = run_i2c_rdwr(&mut clear_msg);
        } else if reg_data[0x25] != IDLE_SENTINEL {
            latched.pwm.copy_from_slice(&reg_data[0x01..=0x12]);
            latched.ctrl.copy_from_slice(&reg_data[0x26..=0x37]);
            let mut rearm = [0x25u8, IDLE_SENTINEL];
            let mut rearm_msg = [I2cMsg {
                addr: ADDR,
                flags: 0,
                len: 2,
                buf: rearm.as_mut_ptr(),
            }];
            let _ = run_i2c_rdwr(&mut rearm_msg);
        }

        let chip_enabled = (reg_data[0x00] & 0x01) != 0;
        let mut ring = [[0u8; 3]; 6];
        let mut any_ch_disabled = false;
        for i in 0..6 {
            let ch = i * 3;
            let (b, b_en) = scale_channel(latched.pwm[ch], latched.ctrl[ch], chip_enabled);
            let (g, g_en) = scale_channel(latched.pwm[ch + 1], latched.ctrl[ch + 1], chip_enabled);
            let (r, r_en) = scale_channel(latched.pwm[ch + 2], latched.ctrl[ch + 2], chip_enabled);
            if !b_en || !g_en || !r_en {
                any_ch_disabled = true;
            }
            ring[i] = [r, g, b];
        }
        let stealth = !chip_enabled || any_ch_disabled;
        Some((ring, stealth))
    }

    pub fn init_default() {
        {
            let mut latched = latched_state().lock().unwrap();
            latched.pwm = [0u8; 18];
            latched.ctrl = [0x01u8; 18];
        }
        let mut en_buf = [0x00u8, 0x01u8];
        let mut ctrl_buf = [0u8; 19];
        ctrl_buf[0] = 0x26;
        for b in &mut ctrl_buf[1..] {
            *b = 0x01;
        }
        let mut sentinel_25 = [0x25u8, IDLE_SENTINEL];
        let mut sentinel_4a = [0x4au8, IDLE_SENTINEL];
        let mut msgs = [
            I2cMsg {
                addr: ADDR,
                flags: 0,
                len: 2,
                buf: en_buf.as_mut_ptr(),
            },
            I2cMsg {
                addr: ADDR,
                flags: 0,
                len: ctrl_buf.len() as u16,
                buf: ctrl_buf.as_mut_ptr(),
            },
            I2cMsg {
                addr: ADDR,
                flags: 0,
                len: 2,
                buf: sentinel_25.as_mut_ptr(),
            },
            I2cMsg {
                addr: ADDR,
                flags: 0,
                len: 2,
                buf: sentinel_4a.as_mut_ptr(),
            },
        ];
        let _ = run_i2c_rdwr(&mut msgs);

        std::thread::spawn(|| loop {
            let _ = read_ring();
            std::thread::sleep(std::time::Duration::from_millis(1));
        });
    }

    pub fn write_pwm(pixels: &[[u8; 3]]) {
        let mut pwm_buf = [0u8; 19];
        pwm_buf[0] = 0x01;
        for i in 0..6.min(pixels.len()) {
            let [r, g, b] = pixels[i];
            pwm_buf[1 + i * 3] = b;
            pwm_buf[1 + i * 3 + 1] = g;
            pwm_buf[1 + i * 3 + 2] = r;
        }
        let mut upd_buf = [0x25u8, 0xaa];
        let mut msgs = [
            I2cMsg {
                addr: ADDR,
                flags: 0,
                len: pwm_buf.len() as u16,
                buf: pwm_buf.as_mut_ptr(),
            },
            I2cMsg {
                addr: ADDR,
                flags: 0,
                len: 2,
                buf: upd_buf.as_mut_ptr(),
            },
        ];
        let _ = run_i2c_rdwr(&mut msgs);
        let _ = read_ring();
    }
}

/// Драйвер кольца `SN3236` (`yandexmini`, шина `/dev/i2c-0`, адрес `0x3c`).
pub struct Sn3236I2cRing;

impl LedRingDevice for Sn3236I2cRing {
    fn driver_name(&self) -> &'static str {
        "i2c_sn3236"
    }

    fn init(&self) {
        #[cfg(target_os = "linux")]
        i2c_sn3236::init_default();
    }

    fn read_ring(&self, _native_frame: &[[u8; 3]], _conflict_counter: &AtomicU32) -> RingReadout {
        #[cfg(target_os = "linux")]
        if let Some((ring6, stealth)) = i2c_sn3236::read_ring() {
            return RingReadout {
                pixels: ring6.to_vec(),
                trigger: if stealth {
                    "i2c-0 SN3236 (Stealth Locked)".to_string()
                } else {
                    "i2c-0 SN3236 (0x3c Active)".to_string()
                },
            };
        }
        RingReadout {
            pixels: vec![[0u8; 3]; 6],
            trigger: "i2c-0 SN3236".to_string(),
        }
    }

    fn write_native_frame(&self, px: &[[u8; 3]]) {
        #[cfg(target_os = "linux")]
        i2c_sn3236::write_pwm(px);
        #[cfg(not(target_os = "linux"))]
        let _ = px;
    }
}

/// Драйвер кольца `sysfs` + `framebuffer` (`orion`, `cucumber`, `yandexmini2`, `yandexmicro` и др.).
pub struct SysfsFbLedRing {
    pub sysfs_prefix: String,
    pub leds: usize,
    pub fallback_fb_dev: String,
    pub driver_label: &'static str,
}

impl SysfsFbLedRing {
    fn ring_fb_path(&self) -> String {
        find_framebuffer_by_name("ring", &self.fallback_fb_dev)
    }
}

impl LedRingDevice for SysfsFbLedRing {
    fn driver_name(&self) -> &'static str {
        self.driver_label
    }

    fn init(&self) {
        let fb_path = self.ring_fb_path();
        if !Path::new(&fb_path).exists() {
            let _ = fs::write(&fb_path, vec![0u8; self.leds * 3]);
        }
    }

    fn read_ring(&self, native_frame: &[[u8; 3]], conflict_counter: &AtomicU32) -> RingReadout {
        let prefix = &self.sysfs_prefix;
        let trig = read_active_trigger(&format!("/sys/class/leds/{}:red:ring0/trigger", prefix));
        let trigger = if trig == "none" {
            "none (Custom Override)".to_string()
        } else {
            format!("{} (Quasar Native)", trig)
        };

        let mut any_sysfs = false;
        let mut sysfs_pixels = vec![[0u8; 3]; self.leds];
        for (i, slot) in sysfs_pixels.iter_mut().enumerate().take(self.leds) {
            let mut r = read_sysfs_u8(&format!(
                "/sys/class/leds/{}:red:ring{}/brightness",
                prefix, i
            ));
            let mut g = read_sysfs_u8(&format!(
                "/sys/class/leds/{}:green:ring{}/brightness",
                prefix, i
            ));
            let mut b = read_sysfs_u8(&format!(
                "/sys/class/leds/{}:blue:ring{}/brightness",
                prefix, i
            ));
            if r == 0 && g == 0 && b == 0 {
                r = read_sysfs_u8(&format!(
                    "/sys/class/leds/{}:red:ring{}/brightness",
                    prefix,
                    i + 1
                ));
                g = read_sysfs_u8(&format!(
                    "/sys/class/leds/{}:green:ring{}/brightness",
                    prefix,
                    i + 1
                ));
                b = read_sysfs_u8(&format!(
                    "/sys/class/leds/{}:blue:ring{}/brightness",
                    prefix,
                    i + 1
                ));
            }
            if r > 0 || g > 0 || b > 0 {
                any_sysfs = true;
            }
            *slot = [r, g, b];
        }

        if trig == "none" {
            return RingReadout {
                pixels: sysfs_pixels,
                trigger,
            };
        }

        if any_sysfs {
            for i in 0..=self.leds {
                for c in ["red", "green", "blue"] {
                    let _ = fs::write(
                        format!("/sys/class/leds/{}:{}:ring{}/brightness", prefix, c, i),
                        "0",
                    );
                }
            }
            conflict_counter.fetch_add(1, Ordering::SeqCst);
        }

        let any_native = native_frame
            .iter()
            .any(|px| px[0] > 0 || px[1] > 0 || px[2] > 0);
        if any_native && native_frame.len() == self.leds {
            return RingReadout {
                pixels: native_frame.to_vec(),
                trigger,
            };
        }

        let mut ring_pixels = vec![[0u8; 3]; self.leds];
        let fb_path = self.ring_fb_path();
        if let Ok(mut f) = File::open(&fb_path) {
            let mut raw = vec![0u8; self.leds * 3];
            if f.read_exact(&mut raw).is_ok() {
                for i in 0..self.leds {
                    ring_pixels[i] = [raw[i * 3], raw[i * 3 + 1], raw[i * 3 + 2]];
                }
            }
        }

        RingReadout {
            pixels: ring_pixels,
            trigger,
        }
    }

    fn write_native_frame(&self, px: &[[u8; 3]]) {
        if self.driver_label == "framebuffer" {
            let fb_path = self.ring_fb_path();
            let mut raw = Vec::with_capacity(self.leds * 3);
            for i in 0..self.leds {
                let c = px.get(i).copied().unwrap_or([0, 0, 0]);
                raw.extend_from_slice(&c);
            }
            let _ = fs::write(fb_path, raw);
        }
    }
}

// ============================================================================
// 2. Трейт дополнительной подсветки (`AuxLightDevice`: Ambilight 12×2 RGBW / Logo)
// ============================================================================

pub trait AuxLightDevice: Send + Sync {
    fn aux_kind(&self) -> &'static str;
    fn zone_count(&self) -> usize;
    fn init(&self);
    fn read_zones(&self, native_frame: &[[u8; 3]]) -> Vec<[u8; 3]>;
    fn write_native_frame(&self, px: &[[u8; 3]]);
}

pub struct NullAuxLight;

impl AuxLightDevice for NullAuxLight {
    fn aux_kind(&self) -> &'static str {
        "none"
    }
    fn zone_count(&self) -> usize {
        0
    }
    fn init(&self) {}
    fn read_zones(&self, _native_frame: &[[u8; 3]]) -> Vec<[u8; 3]> {
        Vec::new()
    }
    fn write_native_frame(&self, _px: &[[u8; 3]]) {}
}

/// Подсветка логотипа Алисы (`cucumber`, `pickle`, `mango`: `logo0..N` в `sysfs` и `/dev/fb1`).
pub struct LogoSysfsFbDevice {
    pub sysfs_prefix: String,
    pub count: usize,
}

impl AuxLightDevice for LogoSysfsFbDevice {
    fn aux_kind(&self) -> &'static str {
        "logo"
    }

    fn zone_count(&self) -> usize {
        self.count
    }

    fn init(&self) {}

    fn read_zones(&self, native_frame: &[[u8; 3]]) -> Vec<[u8; 3]> {
        let mut out = vec![[0u8; 3]; self.count];
        let mut any_sysfs = false;
        for (i, slot) in out.iter_mut().enumerate().take(self.count) {
            let r = read_sysfs_u8(&format!(
                "/sys/class/leds/{}:red:logo{}/brightness",
                self.sysfs_prefix, i
            ));
            let g = read_sysfs_u8(&format!(
                "/sys/class/leds/{}:green:logo{}/brightness",
                self.sysfs_prefix, i
            ));
            let b = read_sysfs_u8(&format!(
                "/sys/class/leds/{}:blue:logo{}/brightness",
                self.sysfs_prefix, i
            ));
            if r > 0 || g > 0 || b > 0 {
                any_sysfs = true;
            }
            *slot = [r, g, b];
        }
        if !any_sysfs && native_frame.len() >= self.count {
            let any_native = native_frame
                .iter()
                .take(self.count)
                .any(|p| p[0] > 0 || p[1] > 0 || p[2] > 0);
            if any_native {
                for i in 0..self.count {
                    out[i] = native_frame[i];
                }
            }
        }
        out
    }

    fn write_native_frame(&self, _px: &[[u8; 3]]) {}
}

/// Эмбиент-подсветка Яндекс Станции 3 (`orion`):
/// - 12 зон × 2 канала (`a`/`b`), RGBW: `orion:{red,green,blue,white}:ambilight{0..11}{a,b}` (96 узлов)
/// - Фреймбуфер `/dev/fb0` (`name = "ambilight"`, `12×1`, `48 bpp`, stride `72` байта).
pub struct OrionAmbilightDevice {
    pub zones: usize,
}

impl OrionAmbilightDevice {
    fn ambilight_fb_path(&self) -> String {
        find_framebuffer_by_name("ambilight", "/dev/fb0")
    }

    fn read_sysfs_zone(i: usize) -> Option<[u8; 3]> {
        // Каждая из 12 зон имеет полузоны `a` и `b` и 4 канала `red`, `green`, `blue`, `white`
        let ra = read_sysfs_u8(&format!("/sys/class/leds/orion:red:ambilight{}a/brightness", i));
        let ga = read_sysfs_u8(&format!(
            "/sys/class/leds/orion:green:ambilight{}a/brightness",
            i
        ));
        let ba = read_sysfs_u8(&format!(
            "/sys/class/leds/orion:blue:ambilight{}a/brightness",
            i
        ));
        let wa = read_sysfs_u8(&format!(
            "/sys/class/leds/orion:white:ambilight{}a/brightness",
            i
        ));

        let rb = read_sysfs_u8(&format!("/sys/class/leds/orion:red:ambilight{}b/brightness", i));
        let gb = read_sysfs_u8(&format!(
            "/sys/class/leds/orion:green:ambilight{}b/brightness",
            i
        ));
        let bb = read_sysfs_u8(&format!(
            "/sys/class/leds/orion:blue:ambilight{}b/brightness",
            i
        ));
        let wb = read_sysfs_u8(&format!(
            "/sys/class/leds/orion:white:ambilight{}b/brightness",
            i
        ));

        let r = ra.max(rb).saturating_add(wa.max(wb) / 2);
        let g = ga.max(gb).saturating_add(wa.max(wb) / 2);
        let b = ba.max(bb).saturating_add(wa.max(wb) / 2);
        if r > 0 || g > 0 || b > 0 {
            Some([r, g, b])
        } else {
            None
        }
    }
}

impl AuxLightDevice for OrionAmbilightDevice {
    fn aux_kind(&self) -> &'static str {
        "ambilight"
    }

    fn zone_count(&self) -> usize {
        self.zones
    }

    fn init(&self) {
        let fb_path = self.ambilight_fb_path();
        if !Path::new(&fb_path).exists() {
            // 12×1 @ 48 bpp = 72 bytes
            let _ = fs::write(&fb_path, vec![0u8; self.zones * 6]);
        }
    }

    fn read_zones(&self, _native_frame: &[[u8; 3]]) -> Vec<[u8; 3]> {
        let mut zones_px = vec![[0u8; 3]; self.zones];
        let mut any_sysfs = false;
        for (i, slot) in zones_px.iter_mut().enumerate().take(self.zones) {
            if let Some(rgb) = Self::read_sysfs_zone(i) {
                *slot = rgb;
                any_sysfs = true;
            }
        }
        if any_sysfs {
            return zones_px;
        }

        let fb_path = self.ambilight_fb_path();
        if let Ok(raw) = fs::read(&fb_path) {
            if raw.len() >= self.zones * 6 && raw.iter().any(|&b| b > 0) {
                for i in 0..self.zones {
                    let base = i * 6;
                    let r = raw[base].max(raw[base + 3]);
                    let g = raw[base + 1].max(raw[base + 4]);
                    let b = raw[base + 2].max(raw[base + 5]);
                    zones_px[i] = [r, g, b];
                }
            }
        }
        zones_px
    }

    fn write_native_frame(&self, px: &[[u8; 3]]) {
        let fb_path = self.ambilight_fb_path();
        let mut raw = Vec::with_capacity(self.zones * 6);
        for i in 0..self.zones {
            let [r, g, b] = px.get(i % px.len().max(1)).copied().unwrap_or([0, 0, 0]);
            // Записываем полузону A и полузону B (6 байт на зону = 72 байта для 12×1 @ 48 bpp)
            raw.extend_from_slice(&[r, g, b, r, g, b]);
        }
        let _ = fs::write(fb_path, raw);
    }
}

// ============================================================================
// 3. Трейт фронтального дисплея (`FrontDisplayDevice`: 7-seg Clock / Matrix 21×12)
// ============================================================================

pub const CLOCK_EXTRA_SYMS: [&str; 17] = [
    "p3", "q3", "p4", "q4", "p5", "q5", "p6", "q6", "p7", "q7", "h1", "h2", "m1", "m2", "l1",
    "l2", "deg",
];

/// Возвращает канонический список всех 45 сегментов часов в фиксированном порядке:
/// `a1..g1`, `a2..g2`, `a3..g3`, `a4..g4` (28) + `p3..deg` (17).
pub fn canonical_clock_segment_names() -> Vec<String> {
    let mut names = Vec::with_capacity(45);
    for pos in 1..=4 {
        for part in ["a", "b", "c", "d", "e", "f", "g"] {
            names.push(format!("{}{}", part, pos));
        }
    }
    for sym in CLOCK_EXTRA_SYMS {
        names.push(sym.to_string());
    }
    names
}

pub fn empty_clock_segments_map() -> BTreeMap<String, u8> {
    let mut map = BTreeMap::new();
    for name in canonical_clock_segment_names() {
        map.insert(name, 0);
    }
    map
}

pub fn is_valid_clock_segment(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    if CLOCK_EXTRA_SYMS.contains(&lower.as_str()) {
        return true;
    }
    if lower.len() == 2 {
        let bytes = lower.as_bytes();
        return (b'a'..=b'g').contains(&bytes[0]) && (b'1'..=b'4').contains(&bytes[1]);
    }
    false
}

/// Разбирает строку перечисления сегментов вида `"a1,b1,deg"` или `"a1=255, g2=128, deg:64"`.
pub fn parse_clock_segment_spec(spec: &str, default_brightness: u8) -> Option<BTreeMap<String, u8>> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.eq_ignore_ascii_case("all") {
        let mut map = empty_clock_segments_map();
        for val in map.values_mut() {
            *val = default_brightness;
        }
        return Some(map);
    }
    let tokens: Vec<&str> = trimmed
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect();
    if tokens.is_empty() {
        return None;
    }

    let mut map = empty_clock_segments_map();
    for tok in tokens {
        let (seg_raw, val_opt) = if let Some((k, v)) = tok.split_once('=') {
            (k.trim(), Some(v.trim()))
        } else if let Some((k, v)) = tok.split_once(':') {
            (k.trim(), Some(v.trim()))
        } else {
            (tok.trim(), None)
        };
        if !is_valid_clock_segment(seg_raw) {
            return None;
        }
        let br = if let Some(v_str) = val_opt {
            v_str.parse::<u16>().ok()?.min(255) as u8
        } else {
            default_brightness
        };
        map.insert(seg_raw.to_ascii_lowercase(), br);
    }
    Some(map)
}

pub enum ParsedLedDisplayFrame {
    Matrix21x12(Vec<u8>),
    Segments45(BTreeMap<String, u8>),
}

/// Разбирает строку или файл формата Quasar `.led` для матрицы `21×12` (252 токена)
/// или 7-сегментного циферблата (28..45 токенов).
pub fn parse_display_led_frame(raw: &str, scale_brightness: u8) -> Option<ParsedLedDisplayFrame> {
    for line in raw.lines() {
        let clean = line.trim();
        if clean.is_empty() || clean.eq_ignore_ascii_case("loop") || clean.starts_with('#') {
            continue;
        }
        let mut parts: Vec<&str> = clean.split_whitespace().collect();
        if parts.len() < 28 {
            continue;
        }
        // Если последний токен — длительность кадра в мс (десятичное число), отбрасываем его
        if parts.len() > 28 {
            if let Some(&last) = parts.last() {
                if last.len() != 6 && last.chars().all(|c| c.is_ascii_digit()) {
                    parts.pop();
                }
            }
        }
        let mut values = Vec::with_capacity(parts.len());
        let mut all_hex = true;
        for &tok in &parts {
            let hex = tok.trim_start_matches("0x").trim_start_matches('#');
            if hex.len() == 6 {
                if let Ok(rgb) = u32::from_str_radix(hex, 16) {
                    let r = ((rgb >> 16) & 0xff) as u8;
                    let g = ((rgb >> 8) & 0xff) as u8;
                    let b = (rgb & 0xff) as u8;
                    let v = r.max(g).max(b);
                    let scaled = ((v as u16 * scale_brightness as u16) / 255) as u8;
                    values.push(scaled);
                } else {
                    all_hex = false;
                    break;
                }
            } else if hex.len() <= 2 {
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    let scaled = ((v as u16 * scale_brightness as u16) / 255) as u8;
                    values.push(scaled);
                } else {
                    all_hex = false;
                    break;
                }
            } else {
                all_hex = false;
                break;
            }
        }
        if !all_hex || values.len() < 28 {
            continue;
        }
        if values.len() >= 60 {
            let mut px = vec![0u8; OrionMatrix21x12Display::PIXELS];
            for (i, v) in values
                .into_iter()
                .enumerate()
                .take(OrionMatrix21x12Display::PIXELS)
            {
                px[i] = v;
            }
            return Some(ParsedLedDisplayFrame::Matrix21x12(px));
        } else {
            let names = canonical_clock_segment_names();
            let mut map = empty_clock_segments_map();
            for (i, v) in values.into_iter().enumerate().take(names.len()) {
                map.insert(names[i].clone(), v);
            }
            return Some(ParsedLedDisplayFrame::Segments45(map));
        }
    }
    None
}

pub struct FrontDisplayReadout {
    pub display_kind: &'static str,
    pub clock_segments: BTreeMap<String, u8>,
    pub screen_width: usize,
    pub screen_height: usize,
    pub screen_pixels: Vec<u8>,
}

pub trait FrontDisplayDevice: Send + Sync {
    fn display_kind(&self) -> &'static str;
    fn init(&self);
    fn read_display(&self) -> FrontDisplayReadout;
    fn write_display_text(&self, text: &str, brightness: u8);
    fn write_display_segments(&self, segments: &BTreeMap<String, u8>, replace_all: bool);
    fn write_display_pixels(&self, pixels: &[u8]);
    fn write_display_led_frame(&self, raw_led: &str, brightness: u8) -> bool;
    fn scale_active_brightness(&self, brightness: u8);
    fn render_volume_overlay(&self, level: u32, active: bool);
}

pub struct NullFrontDisplay {
    pub saved_segments: Mutex<BTreeMap<String, u8>>,
    pub saved_pixels: Mutex<Vec<u8>>,
}

impl NullFrontDisplay {
    pub fn new() -> Self {
        Self {
            saved_segments: Mutex::new(empty_clock_segments_map()),
            saved_pixels: Mutex::new(vec![0u8; OrionMatrix21x12Display::PIXELS]),
        }
    }
}

impl FrontDisplayDevice for NullFrontDisplay {
    fn display_kind(&self) -> &'static str {
        "none"
    }
    fn init(&self) {}
    fn read_display(&self) -> FrontDisplayReadout {
        FrontDisplayReadout {
            display_kind: "none",
            clock_segments: self.saved_segments.lock().unwrap().clone(),
            screen_width: OrionMatrix21x12Display::WIDTH,
            screen_height: OrionMatrix21x12Display::HEIGHT,
            screen_pixels: self.saved_pixels.lock().unwrap().clone(),
        }
    }
    fn write_display_text(&self, text: &str, brightness: u8) {
        if self.write_display_led_frame(text, brightness) {
            return;
        }
        if let Some(seg_map) = parse_clock_segment_spec(text, brightness) {
            self.write_display_segments(&seg_map, true);
            return;
        }
        *self.saved_segments.lock().unwrap() =
            SegmentClockSysfsDisplay::build_segments_map_from_text(text, brightness);
        *self.saved_pixels.lock().unwrap() =
            OrionMatrix21x12Display::render_text_to_matrix(text, brightness);
    }
    fn write_display_segments(&self, segments: &BTreeMap<String, u8>, replace_all: bool) {
        let mut cur = self.saved_segments.lock().unwrap();
        if replace_all {
            *cur = empty_clock_segments_map();
        }
        for (k, &v) in segments {
            if is_valid_clock_segment(k) {
                cur.insert(k.to_ascii_lowercase(), v);
            }
        }
    }
    fn write_display_pixels(&self, pixels: &[u8]) {
        let mut cur = self.saved_pixels.lock().unwrap();
        cur.fill(0);
        for (i, &v) in pixels
            .iter()
            .enumerate()
            .take(OrionMatrix21x12Display::PIXELS)
        {
            cur[i] = v;
        }
    }
    fn write_display_led_frame(&self, raw_led: &str, brightness: u8) -> bool {
        match parse_display_led_frame(raw_led, brightness) {
            Some(ParsedLedDisplayFrame::Matrix21x12(px)) => {
                self.write_display_pixels(&px);
                true
            }
            Some(ParsedLedDisplayFrame::Segments45(segs)) => {
                self.write_display_segments(&segs, true);
                true
            }
            None => false,
        }
    }
    fn scale_active_brightness(&self, brightness: u8) {
        for v in self.saved_segments.lock().unwrap().values_mut() {
            if *v > 0 {
                *v = brightness;
            }
        }
        for v in self.saved_pixels.lock().unwrap().iter_mut() {
            if *v > 0 {
                *v = brightness;
            }
        }
    }
    fn render_volume_overlay(&self, _level: u32, _active: bool) {}
}

/// 7-сегментный светодиодный индикатор часов (`yandexmini2`, `cucumber`, `bergamot`, `pickle`, `plum`).
pub struct SegmentClockSysfsDisplay {
    pub sysfs_prefix: String,
    pub saved_segments: Mutex<BTreeMap<String, u8>>,
    pub saved_pixels: Mutex<Vec<u8>>,
}

impl SegmentClockSysfsDisplay {
    pub fn new(sysfs_prefix: String) -> Self {
        Self {
            sysfs_prefix,
            saved_segments: Mutex::new(empty_clock_segments_map()),
            saved_pixels: Mutex::new(vec![0u8; OrionMatrix21x12Display::PIXELS]),
        }
    }

    pub fn build_segments_map_from_text(text: &str, brightness: u8) -> BTreeMap<String, u8> {
        let mut map = empty_clock_segments_map();
        if text.trim().is_empty() || brightness == 0 {
            return map;
        }
        let mut chars = Vec::new();
        let mut colon = false;
        let mut degree = false;
        let mut plus = false;
        for ch in text.chars() {
            match ch {
                ':' => colon = true,
                '°' | '*' => degree = true,
                '+' => plus = true,
                _ => {
                    if chars.len() < 4 {
                        chars.push(ch);
                    }
                }
            }
        }
        for (idx, &ch) in chars.iter().take(4).enumerate() {
            let pos = idx + 1;
            for &part in clock_segs_for_char(ch) {
                map.insert(format!("{}{}", part, pos), brightness);
            }
        }
        if colon {
            map.insert("p3".to_string(), brightness);
            map.insert("q3".to_string(), brightness);
        }
        if degree {
            map.insert("deg".to_string(), brightness);
        }
        if plus {
            map.insert("h1".to_string(), brightness);
            map.insert("m1".to_string(), brightness);
        }
        map
    }
}

impl FrontDisplayDevice for SegmentClockSysfsDisplay {
    fn display_kind(&self) -> &'static str {
        "segment_clock"
    }

    fn init(&self) {}

    fn read_display(&self) -> FrontDisplayReadout {
        let mut clock_map = BTreeMap::new();
        let mut any_sysfs = false;
        for seg in canonical_clock_segment_names() {
            let path = format!(
                "/sys/class/leds/{}:white:clock-{}/brightness",
                self.sysfs_prefix, seg
            );
            if Path::new(&path).exists() {
                any_sysfs = true;
            }
            let v = read_sysfs_u8(&path);
            clock_map.insert(seg, v);
        }
        if !any_sysfs {
            clock_map = self.saved_segments.lock().unwrap().clone();
        }
        FrontDisplayReadout {
            display_kind: "segment_clock",
            clock_segments: clock_map,
            screen_width: OrionMatrix21x12Display::WIDTH,
            screen_height: OrionMatrix21x12Display::HEIGHT,
            screen_pixels: self.saved_pixels.lock().unwrap().clone(),
        }
    }

    fn write_display_text(&self, text: &str, brightness: u8) {
        if self.write_display_led_frame(text, brightness) {
            return;
        }
        if let Some(seg_map) = parse_clock_segment_spec(text, brightness) {
            self.write_display_segments(&seg_map, true);
            return;
        }
        let segs = Self::build_segments_map_from_text(text, brightness);
        self.write_display_segments(&segs, true);
        *self.saved_pixels.lock().unwrap() =
            OrionMatrix21x12Display::render_text_to_matrix(text, brightness);
    }

    fn write_display_segments(&self, segments: &BTreeMap<String, u8>, replace_all: bool) {
        let mut cur = self.saved_segments.lock().unwrap();
        if replace_all {
            *cur = empty_clock_segments_map();
            for (k, &v) in segments {
                if is_valid_clock_segment(k) {
                    cur.insert(k.to_ascii_lowercase(), v);
                }
            }
            for (loc, &val) in cur.iter() {
                let p = format!(
                    "/sys/class/leds/{}:white:clock-{}/brightness",
                    self.sysfs_prefix, loc
                );
                let _ = fs::write(p, val.to_string());
            }
        } else {
            for (k, &val) in segments {
                if is_valid_clock_segment(k) {
                    let loc = k.to_ascii_lowercase();
                    cur.insert(loc.clone(), val);
                    let p = format!(
                        "/sys/class/leds/{}:white:clock-{}/brightness",
                        self.sysfs_prefix, loc
                    );
                    let _ = fs::write(p, val.to_string());
                }
            }
        }
    }

    fn write_display_pixels(&self, pixels: &[u8]) {
        let mut cur = self.saved_pixels.lock().unwrap();
        cur.fill(0);
        for (i, &v) in pixels
            .iter()
            .enumerate()
            .take(OrionMatrix21x12Display::PIXELS)
        {
            cur[i] = v;
        }
    }

    fn write_display_led_frame(&self, raw_led: &str, brightness: u8) -> bool {
        match parse_display_led_frame(raw_led, brightness) {
            Some(ParsedLedDisplayFrame::Segments45(segs)) => {
                self.write_display_segments(&segs, true);
                true
            }
            Some(ParsedLedDisplayFrame::Matrix21x12(px)) => {
                self.write_display_pixels(&px);
                true
            }
            None => false,
        }
    }

    fn scale_active_brightness(&self, brightness: u8) {
        let current = self.read_display().clock_segments;
        let mut updated = current;
        for val in updated.values_mut() {
            if *val > 0 {
                *val = brightness;
            }
        }
        self.write_display_segments(&updated, false);
        for v in self.saved_pixels.lock().unwrap().iter_mut() {
            if *v > 0 {
                *v = brightness;
            }
        }
    }

    fn render_volume_overlay(&self, _level: u32, _active: bool) {}
}

/// Монохромный матричный LED-дисплей `21×12` Яндекс Станции 3 (`orion`):
/// - Модуль `gowin_led_device.ko` (FPGA Gowin `yandex_led_screen_fpga.bin`)
/// - Фреймбуфер `/dev/fb1` (`name = "YNDX LED screen"`, `21×12` = 252 пикселя, `24 bpp`, stride `63`, `756` байт)
/// - В `maind` за формирование кадра отвечает `OrionFrontPanelLedFrameBuilder` (`orion_front_panel_frame_builder.cpp`),
///   а готовые анимации лежат в `/system/vendor/quasar/ledpatterns/screen/*.led`.
pub struct OrionMatrix21x12Display {
    pub saved_pixels: Mutex<Vec<u8>>,
    pub virtual_clock_segments: Mutex<BTreeMap<String, u8>>,
    pub volume_overlay_active: AtomicBool,
    pub last_volume_level: AtomicU32,
}

impl OrionMatrix21x12Display {
    pub const WIDTH: usize = 21;
    pub const HEIGHT: usize = 12;
    pub const PIXELS: usize = Self::WIDTH * Self::HEIGHT; // 252

    pub fn new() -> Self {
        let initial_px = Self::render_text_to_matrix("12:00", 255);
        Self {
            saved_pixels: Mutex::new(initial_px),
            virtual_clock_segments: Mutex::new(
                SegmentClockSysfsDisplay::build_segments_map_from_text("12:00", 255),
            ),
            volume_overlay_active: AtomicBool::new(false),
            last_volume_level: AtomicU32::new(u32::MAX),
        }
    }

    fn screen_fb_path(&self) -> String {
        find_framebuffer_by_name("YNDX LED screen", "/dev/fb1")
    }

    /// 4×7 растровый шрифт цифр и символов (`OrionFrontPanelLedFrameBuilder`) для матрицы 21×12.
    fn glyph_4x7(ch: char) -> [u8; 7] {
        match ch.to_ascii_uppercase() {
            '0' | 'O' => [
                0b0110, 0b1001, 0b1001, 0b1001, 0b1001, 0b1001, 0b0110,
            ],
            '1' | 'I' => [
                0b0010, 0b0110, 0b0010, 0b0010, 0b0010, 0b0010, 0b0111,
            ],
            '2' => [
                0b0110, 0b1001, 0b0001, 0b0010, 0b0100, 0b1000, 0b1111,
            ],
            '3' => [
                0b1110, 0b0001, 0b0001, 0b0110, 0b0001, 0b0001, 0b1110,
            ],
            '4' => [
                0b1001, 0b1001, 0b1001, 0b1111, 0b0001, 0b0001, 0b0001,
            ],
            '5' | 'S' => [
                0b1111, 0b1000, 0b1000, 0b1110, 0b0001, 0b0001, 0b1110,
            ],
            '6' => [
                0b0111, 0b1000, 0b1000, 0b1110, 0b1001, 0b1001, 0b0110,
            ],
            '7' => [
                0b1111, 0b0001, 0b0010, 0b0010, 0b0100, 0b0100, 0b0100,
            ],
            '8' => [
                0b0110, 0b1001, 0b1001, 0b0110, 0b1001, 0b1001, 0b0110,
            ],
            '9' => [
                0b0110, 0b1001, 0b1001, 0b0111, 0b0001, 0b0001, 0b1110,
            ],
            '-' => [
                0b0000, 0b0000, 0b0000, 0b1111, 0b0000, 0b0000, 0b0000,
            ],
            '+' => [
                0b0000, 0b0100, 0b0100, 0b1110, 0b0100, 0b0100, 0b0000,
            ],
            _ => [0; 7],
        }
    }

    /// Рендерит текстовую строку (например, `"19:30"`, `"+21°"`, `"8"`) в буфер `21×12` (252 байта).
    pub fn render_text_to_matrix(text: &str, brightness: u8) -> Vec<u8> {
        let mut grid = vec![0u8; Self::PIXELS];
        let trimmed = text.trim();
        if trimmed.is_empty() || brightness == 0 {
            return grid;
        }

        // Если запрошен встроенный паттерн экрана (`pattern:joy`, `pattern:volume`, `pattern:mute` и т.д.)
        if let Some(pat_name) = trimmed.strip_prefix("pattern:") {
            if let Some(frame) = Self::load_screen_pattern_frame(pat_name, 0) {
                for (i, v) in frame.into_iter().enumerate().take(Self::PIXELS) {
                    grid[i] = ((v as u16 * brightness as u16) / 255) as u8;
                }
                return grid;
            }
        }

        let mut chars = Vec::new();
        let mut has_colon = false;
        let mut has_degree = false;
        for ch in trimmed.chars() {
            match ch {
                ':' => has_colon = true,
                '°' | '*' => has_degree = true,
                _ => {
                    if chars.len() < 4 {
                        chars.push(ch);
                    }
                }
            }
        }

        let y0 = 2usize; // Вертикальное центрирование шрифта высотой 7 пикселей в матрице 12 строк
        let put_px = |grid: &mut [u8], x: usize, y: usize, val: u8| {
            if x < Self::WIDTH && y < Self::HEIGHT {
                grid[y * Self::WIDTH + x] = val;
            }
        };

        // Расстановка по горизонтали (21 столбец):
        // Для 4 цифр с двоеточием (`19:30`): x = [1, 6, (10 = ':'), 12, 17] -> ровно 21 столбец!
        let x_positions: &[usize] = match (chars.len(), has_colon) {
            (4, true) => &[1, 6, 12, 17],
            (4, false) => &[1, 6, 11, 16],
            (3, _) => &[2, 7, 12],
            (2, _) => &[5, 11],
            (1, _) => &[8],
            _ => &[],
        };

        for (idx, &ch) in chars.iter().enumerate() {
            let Some(&x0) = x_positions.get(idx) else {
                break;
            };
            let rows = Self::glyph_4x7(ch);
            for (ry, &bits) in rows.iter().enumerate() {
                for rx in 0..4 {
                    if (bits & (1 << (3 - rx))) != 0 {
                        put_px(&mut grid, x0 + rx, y0 + ry, brightness);
                    }
                }
            }
        }

        if has_colon {
            put_px(&mut grid, 10, y0 + 2, brightness);
            put_px(&mut grid, 10, y0 + 4, brightness);
        }

        if has_degree {
            let deg_x = x_positions
                .last()
                .map(|&x| (x + 5).min(Self::WIDTH - 2))
                .unwrap_or(18);
            put_px(&mut grid, deg_x, y0, brightness);
            put_px(&mut grid, deg_x + 1, y0, brightness);
            put_px(&mut grid, deg_x, y0 + 1, brightness);
            put_px(&mut grid, deg_x + 1, y0 + 1, brightness);
        }

        grid
    }

    /// Загружает кадр матричной анимации `21×12` из `/system/vendor/quasar/ledpatterns/screen/`.
    pub fn load_screen_pattern_frame(name: &str, frame_index: usize) -> Option<Vec<u8>> {
        let candidates = [
            format!("/system/vendor/quasar/ledpatterns/screen/{}.led", name),
            format!(
                "/system/vendor/quasar/ledpatterns/screen/emotion/{}.led",
                name
            ),
            format!(
                "/system/vendor/quasar/ledpatterns/screen/media/{}.led",
                name
            ),
            format!("/usr/share/animation/screen/{}.led", name),
        ];
        for path in &candidates {
            if let Ok(raw) = fs::read_to_string(path) {
                let frames = parse_led_pattern_str(&raw, Self::PIXELS);
                if !frames.is_empty() {
                    let idx = frame_index.min(frames.len() - 1);
                    let mono: Vec<u8> = frames[idx]
                        .0
                        .iter()
                        .map(|px| px[0].max(px[1]).max(px[2]))
                        .collect();
                    return Some(mono);
                }
            }
        }
        None
    }

    fn write_matrix_to_fb(&self, mono_252: &[u8]) {
        let fb_path = self.screen_fb_path();
        let mut raw_rgb24 = Vec::with_capacity(Self::PIXELS * 3);
        for i in 0..Self::PIXELS {
            let v = mono_252.get(i).copied().unwrap_or(0);
            raw_rgb24.extend_from_slice(&[v, v, v]);
        }
        let _ = fs::write(fb_path, raw_rgb24);
    }
}

impl FrontDisplayDevice for OrionMatrix21x12Display {
    fn display_kind(&self) -> &'static str {
        "matrix_21x12"
    }

    fn init(&self) {
        let grid = self.saved_pixels.lock().unwrap().clone();
        self.write_matrix_to_fb(&grid);
    }

    fn read_display(&self) -> FrontDisplayReadout {
        let fb_path = self.screen_fb_path();
        let mut pixels = vec![0u8; Self::PIXELS];
        let mut read_ok = false;
        if let Ok(raw) = fs::read(&fb_path) {
            if raw.len() >= Self::PIXELS * 3 {
                for i in 0..Self::PIXELS {
                    let r = raw[i * 3];
                    let g = raw[i * 3 + 1];
                    let b = raw[i * 3 + 2];
                    pixels[i] = r.max(g).max(b);
                }
                read_ok = true;
            }
        }
        if !read_ok {
            pixels = self.saved_pixels.lock().unwrap().clone();
        }

        FrontDisplayReadout {
            display_kind: "matrix_21x12",
            clock_segments: self.virtual_clock_segments.lock().unwrap().clone(),
            screen_width: Self::WIDTH,
            screen_height: Self::HEIGHT,
            screen_pixels: pixels,
        }
    }

    fn write_display_text(&self, text: &str, brightness: u8) {
        if self.write_display_led_frame(text, brightness) {
            return;
        }
        if let Some(seg_map) = parse_clock_segment_spec(text, brightness) {
            self.write_display_segments(&seg_map, true);
            return;
        }
        *self.virtual_clock_segments.lock().unwrap() =
            SegmentClockSysfsDisplay::build_segments_map_from_text(text, brightness);
        let grid = Self::render_text_to_matrix(text, brightness);
        *self.saved_pixels.lock().unwrap() = grid.clone();
        self.write_matrix_to_fb(&grid);
    }

    fn write_display_segments(&self, segments: &BTreeMap<String, u8>, replace_all: bool) {
        let mut cur = self.virtual_clock_segments.lock().unwrap();
        if replace_all {
            *cur = empty_clock_segments_map();
        }
        for (k, &v) in segments {
            if is_valid_clock_segment(k) {
                cur.insert(k.to_ascii_lowercase(), v);
            }
        }
    }

    fn write_display_pixels(&self, pixels: &[u8]) {
        let mut grid = vec![0u8; Self::PIXELS];
        for (i, &v) in pixels.iter().enumerate().take(Self::PIXELS) {
            grid[i] = v;
        }
        *self.saved_pixels.lock().unwrap() = grid.clone();
        self.write_matrix_to_fb(&grid);
    }

    fn write_display_led_frame(&self, raw_led: &str, brightness: u8) -> bool {
        match parse_display_led_frame(raw_led, brightness) {
            Some(ParsedLedDisplayFrame::Matrix21x12(px)) => {
                self.write_display_pixels(&px);
                true
            }
            Some(ParsedLedDisplayFrame::Segments45(segs)) => {
                self.write_display_segments(&segs, true);
                true
            }
            None => false,
        }
    }

    fn scale_active_brightness(&self, brightness: u8) {
        let mut px = self.read_display().screen_pixels;
        for v in px.iter_mut() {
            if *v > 0 {
                *v = brightness;
            }
        }
        self.write_display_pixels(&px);
        for v in self.virtual_clock_segments.lock().unwrap().values_mut() {
            if *v > 0 {
                *v = brightness;
            }
        }
    }

    fn render_volume_overlay(&self, level: u32, active: bool) {
        let was_active = self.volume_overlay_active.swap(active, Ordering::SeqCst);
        if active {
            let clamped = level.min(10);
            let prev_lvl = self.last_volume_level.swap(clamped, Ordering::SeqCst);
            if !was_active || prev_lvl != clamped {
                let grid = Self::render_text_to_matrix(&clamped.to_string(), 255);
                self.write_matrix_to_fb(&grid);
            }
        } else if was_active {
            self.last_volume_level.store(u32::MAX, Ordering::SeqCst);
            let grid = self.saved_pixels.lock().unwrap().clone();
            self.write_matrix_to_fb(&grid);
        }
    }
}

// ============================================================================
// 4. Трейт аудиоподсистемы (`AudioSubsystemDevice`: 4× ACM8625S + Ultrasound TDM)
// ============================================================================

#[derive(Clone, Debug, Serialize)]
pub struct AmplifierSlotState {
    pub i2c_device: String,
    pub role: String,
    pub alsa_prefix: String,
    pub settings_file: String,
    pub gain_db: f32,
    pub ramp_db: f32,
    pub muted: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct UltrasoundState {
    pub enabled: bool,
    pub tdm_device: String,
    pub sample_rate_hz: u32,
    pub carrier_freq_hz: u32,
    pub bandwidth_hz: u32,
    pub channels: u32,
    pub active: bool,
    pub rms: u32,
}

pub struct AudioHwReadout {
    pub amplifiers: Vec<AmplifierSlotState>,
    pub ultrasound: Option<UltrasoundState>,
}

pub trait AudioSubsystemDevice: Send + Sync {
    fn init(&self);
    fn on_volume_or_mute_changed(&self, volume_level: u32, muted: bool);
    fn read_audio_hw(&self, tof_mm: u32) -> AudioHwReadout;
}

pub struct GenericAlsaAudio;

impl AudioSubsystemDevice for GenericAlsaAudio {
    fn init(&self) {}
    fn on_volume_or_mute_changed(&self, _volume_level: u32, _muted: bool) {}
    fn read_audio_hw(&self, _tof_mm: u32) -> AudioHwReadout {
        AudioHwReadout {
            amplifiers: Vec::new(),
            ultrasound: None,
        }
    }
}

/// Аудиоподсистема Яндекс Станции 3 (`orion`):
/// - Четыре цифровых усилителя Class-D `ACM8625S` (`snd-soc-acm8625.ko`):
///   1. `2-002c` (`us` — ультразвуковой излучатель, `orion-acm8625-us.cfg`)
///   2. `3-002c` (`center` / `fr2` — центральный канал, `orion-acm8625-center.cfg`)
///   3. `3-002d` (`woofer` / `wf` — сабвуфер, `orion-acm8625-woofer.cfg`)
///   4. `3-002e` (`lr` / `fr1` — левый/правый СЧ/ВЧ, `orion-acm8625-lr.cfg`)
/// - Отдельный ультразвуковой TDM-тракт `hw:0,0` (`TDM-A-acm8625-ultrasonic` / `pcm.ultrasound`),
///   работающий на `96000 Гц` с несущей `38400 Гц` (ширина полосы `600 Гц`, 4 канала).
pub struct OrionQuadAcm8625Audio {
    pub volume_level: AtomicU32,
    pub spk_muted: AtomicBool,
}

impl OrionQuadAcm8625Audio {
    pub fn new() -> Self {
        Self {
            volume_level: AtomicU32::new(7),
            spk_muted: AtomicBool::new(false),
        }
    }

    const AMP_SLOTS: [(&'static str, &'static str, &'static str, &'static str); 4] = [
        (
            "3-002d",
            "woofer",
            "wf Acm8625",
            "orion-acm8625-woofer.cfg",
        ),
        (
            "3-002e",
            "lr",
            "fr1 Acm8625",
            "orion-acm8625-lr.cfg",
        ),
        (
            "3-002c",
            "center",
            "fr2 Acm8625",
            "orion-acm8625-center.cfg",
        ),
        (
            "2-002c",
            "us",
            "us Acm8625",
            "orion-acm8625-us.cfg",
        ),
    ];

    /// Кривая громкости `hardvold` (`/etc/hardvold.config`): `0 -> -66.6 dB`, `255 -> 0.0 dB`.
    pub fn hardvold_gain_db(volume_level: u32) -> f32 {
        let clamped = volume_level.min(10) as f32;
        ((-66.6 + (clamped / 10.0) * 66.6) * 10.0).round() / 10.0
    }
}

impl AudioSubsystemDevice for OrionQuadAcm8625Audio {
    fn init(&self) {
        for (i2c_id, _role, _alsa, default_cfg) in Self::AMP_SLOTS {
            let dir = format!("/sys/bus/i2c/devices/{}", i2c_id);
            let _ = fs::create_dir_all(&dir);
            let _ = fs::write(format!("{}/name", dir), "acm8625s\n");
            let settings_path = format!("{}/settings_file", dir);
            if !Path::new(&settings_path).exists() {
                let _ = fs::write(&settings_path, format!("{}\n", default_cfg));
            }
        }
    }

    fn on_volume_or_mute_changed(&self, volume_level: u32, muted: bool) {
        self.volume_level
            .store(volume_level.min(10), Ordering::SeqCst);
        self.spk_muted.store(muted || volume_level == 0, Ordering::SeqCst);
    }

    fn read_audio_hw(&self, tof_mm: u32) -> AudioHwReadout {
        let vol = self.volume_level.load(Ordering::Relaxed);
        let gain_db = Self::hardvold_gain_db(vol);
        let spk_muted = self.spk_muted.load(Ordering::Relaxed) || vol == 0;

        let mut amplifiers = Vec::with_capacity(4);
        for (i2c_id, role, alsa_prefix, default_cfg) in Self::AMP_SLOTS {
            let sysfs_path = format!("/sys/bus/i2c/devices/{}/settings_file", i2c_id);
            let cfg_name = fs::read_to_string(&sysfs_path)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| default_cfg.to_string());

            let (slot_gain, slot_muted) = if role == "us" {
                (-6.0, false)
            } else {
                (gain_db, spk_muted)
            };

            amplifiers.push(AmplifierSlotState {
                i2c_device: i2c_id.to_string(),
                role: role.to_string(),
                alsa_prefix: alsa_prefix.to_string(),
                settings_file: cfg_name,
                gain_db: slot_gain,
                ramp_db: -6.0,
                muted: slot_muted,
            });
        }

        // При приближении объекта (ToF < 1500 мм) амплитуда отражённого ультразвукового эха (38.4 кГц) растёт
        let echo_boost = if tof_mm < 1500 {
            ((1500 - tof_mm) / 2).min(1200)
        } else {
            0
        };

        AudioHwReadout {
            amplifiers,
            ultrasound: Some(UltrasoundState {
                enabled: true,
                tdm_device: "hw:0,0 (TDM-A-acm8625-ultrasonic / pcm.ultrasound)".to_string(),
                sample_rate_hz: 96000,
                carrier_freq_hz: 38400,
                bandwidth_hz: 600,
                channels: 4,
                active: true,
                rms: 420 + echo_boost,
            }),
        }
    }
}

// ============================================================================
// Составной реестр периферии платы (`BoardPeripherals`)
// ============================================================================

pub struct BoardPeripherals {
    pub ring: Box<dyn LedRingDevice>,
    pub aux_light: Box<dyn AuxLightDevice>,
    pub front_display: Box<dyn FrontDisplayDevice>,
    pub audio_hw: Box<dyn AudioSubsystemDevice>,
}

impl BoardPeripherals {
    pub fn from_profile(profile: &BoardProfile) -> Self {
        let slug = profile.board_slug.as_str();

        // 1. Световое кольцо (`LedRingDevice`)
        let ring: Box<dyn LedRingDevice> = if slug == "yandexmini" {
            Box::new(Sn3236I2cRing)
        } else {
            let (fallback_fb, drv_label) = match slug {
                "orion" => ("/dev/fb2".to_string(), "framebuffer"),
                "cucumber" | "station2" | "bergamot" => ("/dev/fb0".to_string(), "framebuffer"),
                _ => ("/dev/fb0".to_string(), "sysfs"),
            };
            Box::new(SysfsFbLedRing {
                sysfs_prefix: profile.sysfs_prefix.clone(),
                leds: profile.ring_leds,
                fallback_fb_dev: fallback_fb,
                driver_label: drv_label,
            })
        };

        // 2. Дополнительная подсветка (`AuxLightDevice`: Ambilight 12×2 RGBW или Logo)
        let aux_light: Box<dyn AuxLightDevice> = if slug == "orion" || profile.ambilight_leds > 0 {
            Box::new(OrionAmbilightDevice {
                zones: profile.ambilight_leds.max(12),
            })
        } else if profile.logo_leds > 0 {
            Box::new(LogoSysfsFbDevice {
                sysfs_prefix: profile.sysfs_prefix.clone(),
                count: profile.logo_leds,
            })
        } else {
            Box::new(NullAuxLight)
        };

        // 3. Фронтальный дисплей (`FrontDisplayDevice`: монохромная матрица 21×12 или 7-seg часы)
        let front_display: Box<dyn FrontDisplayDevice> = if slug == "orion" {
            Box::new(OrionMatrix21x12Display::new())
        } else if profile.has_clock {
            Box::new(SegmentClockSysfsDisplay::new(profile.sysfs_prefix.clone()))
        } else {
            Box::new(NullFrontDisplay::new())
        };

        // 4. Аудиоподсистема (`AudioSubsystemDevice`: 4× ACM8625S + Ultrasound TDM-A на `orion`)
        let audio_hw: Box<dyn AudioSubsystemDevice> = if slug == "orion" {
            Box::new(OrionQuadAcm8625Audio::new())
        } else {
            Box::new(GenericAlsaAudio)
        };

        let peripherals = Self {
            ring,
            aux_light,
            front_display,
            audio_hw,
        };
        peripherals.ring.init();
        peripherals.aux_light.init();
        peripherals.front_display.init();
        peripherals.audio_hw.init();
        peripherals
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_orion_matrix_21x12_rendering() {
        let grid = OrionMatrix21x12Display::render_text_to_matrix("19:30°", 255);
        assert_eq!(grid.len(), 252);
        let lit_pixels = grid.iter().filter(|&&px| px > 0).count();
        assert!(lit_pixels > 25, "Expected lit pixels for '19:30°' on 21x12 matrix");
        // Двоеточие в столбце x=10, строках y=4 и y=6
        assert_eq!(grid[4 * 21 + 10], 255);
        assert_eq!(grid[6 * 21 + 10], 255);
    }

    #[test]
    fn test_hardvold_gain_curve() {
        assert_eq!(OrionQuadAcm8625Audio::hardvold_gain_db(0), -66.6);
        assert_eq!(OrionQuadAcm8625Audio::hardvold_gain_db(10), 0.0);
        let mid = OrionQuadAcm8625Audio::hardvold_gain_db(5);
        assert!(mid > -34.0 && mid < -32.0);
    }

    #[test]
    fn test_clock_segment_spec_and_led_frame_parsing() {
        let names = canonical_clock_segment_names();
        assert_eq!(names.len(), 45);

        let spec = parse_clock_segment_spec("a1, b2=128, deg:64", 200).unwrap();
        assert_eq!(spec.get("a1"), Some(&200));
        assert_eq!(spec.get("b2"), Some(&128));
        assert_eq!(spec.get("deg"), Some(&64));
        assert_eq!(spec.get("c1"), Some(&0));

        // 45-token 7-segment .led line
        let mut tokens45 = vec!["000000"; 45];
        tokens45[0] = "ffffff"; // a1
        tokens45[44] = "808080"; // deg
        let raw_seg_led = format!("loop\n{} 40\n", tokens45.join(" "));
        match parse_display_led_frame(&raw_seg_led, 255) {
            Some(ParsedLedDisplayFrame::Segments45(map)) => {
                assert_eq!(map.get("a1"), Some(&255));
                assert_eq!(map.get("deg"), Some(&128));
                assert_eq!(map.get("b1"), Some(&0));
            }
            _ => panic!("Expected Segments45 frame"),
        }

        // 252-token 21x12 matrix .led line
        let mut tokens252 = vec!["000000"; 252];
        tokens252[0] = "ffffff";
        tokens252[251] = "404040";
        let raw_mat_led = format!("loop\n{} 35\n", tokens252.join(" "));
        match parse_display_led_frame(&raw_mat_led, 255) {
            Some(ParsedLedDisplayFrame::Matrix21x12(px)) => {
                assert_eq!(px.len(), 252);
                assert_eq!(px[0], 255);
                assert_eq!(px[251], 64);
            }
            _ => panic!("Expected Matrix21x12 frame"),
        }
    }
}
