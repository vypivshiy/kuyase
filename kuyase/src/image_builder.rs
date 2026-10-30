use crate::ext_fs::{read_ext2_image, write_ext2_image, EXT2_SUPER_MAGIC};
use crate::session::{resolve_bridge_binary, EMBEDDED_HW_BRIDGE};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use protocol::normalize_board_slug;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use tar::{Archive, Builder, EntryType, Header};

pub const EMBEDDED_INIT_SH: &[u8] = include_bytes!("../../guest-init/equasar-qemu-init.sh");

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RootfsNodeKind {
    Dir,
    File(Vec<u8>),
    Symlink(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootfsNode {
    pub kind: RootfsNodeKind,
    pub mode: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RootfsTree {
    pub entries: BTreeMap<String, RootfsNode>,
}

pub type VfsTree = RootfsTree;
pub type VfsNode = RootfsNode;

pub fn normalize_rel_path(raw: &str) -> Option<String> {
    let replaced = raw.replace('\\', "/");
    let mut parts = Vec::new();
    for seg in replaced.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

impl RootfsTree {
    pub fn insert_dir(&mut self, path: &str, mode: u16) {
        if let Some(norm) = normalize_rel_path(path) {
            self.entries.insert(
                norm,
                RootfsNode {
                    kind: RootfsNodeKind::Dir,
                    mode: if mode == 0 { 0o755 } else { mode & 0o7777 },
                },
            );
        }
    }

    pub fn insert_file(&mut self, path: &str, data: Vec<u8>, mode: u16) {
        if let Some(norm) = normalize_rel_path(path) {
            self.entries.insert(
                norm,
                RootfsNode {
                    kind: RootfsNodeKind::File(data),
                    mode: if mode == 0 { 0o644 } else { mode & 0o7777 },
                },
            );
        }
    }

    pub fn insert_symlink(&mut self, path: &str, target: String) {
        if let Some(norm) = normalize_rel_path(path) {
            self.entries.insert(
                norm,
                RootfsNode {
                    kind: RootfsNodeKind::Symlink(target),
                    mode: 0o777,
                },
            );
        }
    }

    pub fn get(&self, path: &str) -> Option<&RootfsNode> {
        let norm = normalize_rel_path(path)?;
        self.entries.get(&norm)
    }

    pub fn get_file_bytes(&self, path: &str) -> Option<&[u8]> {
        match &self.get(path)?.kind {
            RootfsNodeKind::File(b) => Some(b.as_slice()),
            _ => None,
        }
    }
}

pub fn read_tar_from_reader<R: Read>(reader: R) -> Result<RootfsTree, String> {
    let mut archive = Archive::new(reader);
    let mut tree = RootfsTree::default();
    let entries = archive.entries().map_err(|e| format!("tar read error: {}", e))?;
    for entry_res in entries {
        let mut entry = entry_res.map_err(|e| format!("tar entry error: {}", e))?;
        let raw_path = entry
            .path()
            .map_err(|e| format!("tar path error: {}", e))?
            .to_string_lossy()
            .to_string();
        let Some(norm) = normalize_rel_path(&raw_path) else {
            continue;
        };
        let mode = (entry.header().mode().unwrap_or(0o644) as u16) & 0o7777;
        match entry.header().entry_type() {
            EntryType::Directory => tree.insert_dir(&norm, if mode == 0 { 0o755 } else { mode }),
            EntryType::Symlink => {
                if let Ok(Some(link)) = entry.link_name() {
                    tree.insert_symlink(&norm, link.to_string_lossy().to_string());
                }
            }
            EntryType::Regular | EntryType::Continuous => {
                let mut buf = Vec::new();
                entry
                    .read_to_end(&mut buf)
                    .map_err(|e| format!("tar file read error: {}", e))?;
                tree.insert_file(&norm, buf, if mode == 0 { 0o644 } else { mode });
            }
            _ => {}
        }
    }
    Ok(tree)
}

pub fn read_tar_gz_into_tree(path: &Path) -> Result<RootfsTree, String> {
    let file = File::open(path).map_err(|e| format!("failed to open '{}': {}", path.display(), e))?;
    let gz = GzDecoder::new(file);
    read_tar_from_reader(gz)
}

pub fn write_tree_to_tar_gz(tree: &RootfsTree, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let file =
        File::create(path).map_err(|e| format!("failed to create '{}': {}", path.display(), e))?;
    let gz = GzEncoder::new(file, Compression::fast());
    let mut builder = Builder::new(gz);

    for (rel, node) in &tree.entries {
        let mut header = Header::new_gnu();
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        match &node.kind {
            RootfsNodeKind::Dir => {
                header.set_entry_type(EntryType::Directory);
                header.set_mode(node.mode as u32);
                header.set_size(0);
                header.set_cksum();
                builder
                    .append_data(&mut header, format!("{}/", rel), std::io::empty())
                    .map_err(|e| e.to_string())?;
            }
            RootfsNodeKind::File(bytes) => {
                header.set_entry_type(EntryType::Regular);
                header.set_mode(node.mode as u32);
                header.set_size(bytes.len() as u64);
                header.set_cksum();
                builder
                    .append_data(&mut header, rel, Cursor::new(bytes))
                    .map_err(|e| e.to_string())?;
            }
            RootfsNodeKind::Symlink(target) => {
                header.set_entry_type(EntryType::Symlink);
                header.set_mode(0o777);
                header.set_size(0);
                builder
                    .append_link(&mut header, rel, target)
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    let gz = builder.into_inner().map_err(|e| e.to_string())?;
    gz.finish().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn sanitize_tar_gz_file(path: &Path) -> Result<usize, String> {
    let mut tree = read_tar_gz_into_tree(path)?;
    let before = tree.entries.len();
    sanitize_tree(&mut tree);
    let removed = before.saturating_sub(tree.entries.len());
    write_tree_to_tar_gz(&tree, path)?;
    Ok(removed)
}

pub fn detect_board_from_tree(tree: &RootfsTree) -> String {
    if let Some(cfg_bytes) = tree.get_file_bytes("system/vendor/quasar/quasar.cfg") {
        let text = String::from_utf8_lossy(cfg_bytes);
        if let Some(pos) = text.find("\"deviceType\"") {
            let after = &text[pos + "\"deviceType\"".len()..];
            if let Some(colon) = after.find(':') {
                let rest = after[colon + 1..].trim_start();
                if let Some(stripped) = rest.strip_prefix('"') {
                    if let Some(end_q) = stripped.find('"') {
                        let dev = stripped[..end_q].trim();
                        if !dev.is_empty() {
                            return match dev {
                                "yandexmini_2" | "mini2" => "yandexmini2".to_string(),
                                "station_2" | "station-midi" | "yandexmidi" => {
                                    "station2".to_string()
                                }
                                "mini3" => "bergamot".to_string(),
                                "mini3pro" => "pickle".to_string(),
                                "lite2" => "plum".to_string(),
                                "lite1" | "micro" => "yandexmicro".to_string(),
                                "station3" => "orion".to_string(),
                                "street" => "mango".to_string(),
                                "hub" => "saturn".to_string(),
                                other => normalize_board_slug(other),
                            };
                        }
                    }
                }
            }
        }
    }

    if tree.get("etc/init.d/S61leds_brightness").is_some() {
        "yandexmini2".to_string()
    } else if tree.get("boot/yandex-cucumber-i2ctop-overlay.dtb").is_some() {
        "cucumber".to_string()
    } else if tree.get("boot/yandex-orion-i2c_top-overlay.dtb").is_some() {
        "orion".to_string()
    } else {
        "yandexmini".to_string()
    }
}

pub fn detect_board_from_vfs(tree: &VfsTree) -> String {
    detect_board_from_tree(tree)
}

pub fn sanitize_tree(tree: &mut RootfsTree) {
    let empty_dirs = [
        "data",
        "bkupgrade",
        "tee",
        "nv",
        "env",
        "ota",
        "tmp",
        "run",
        "var/run",
        "var/db/dhcpcd",
        "var/lib/chrony",
    ];
    let data_is_symlink = matches!(
        tree.get("data").map(|n| &n.kind),
        Some(RootfsNodeKind::Symlink(_))
    );

    let secret_exact = [
        "etc/wpa_supplicant.conf",
        "etc/hostapd_temp.conf",
        "etc/wifi/ap_name",
        "etc/shadow",
        "etc/dropbear",
        "etc/dhcpcd.exit-hook",
        "etc/unbound/unbound.conf.head",
        "etc/init.d/S99zdropbear",
        "etc/init.d/S99alinad",
        "etc/init.d/S100privacy",
        "etc/init.d/S89usbgadget",
        "etc/init.d/S97adb",
        "root/.ssh",
        "root/.ash_history",
        "usr/local/bin/dropbear",
        "usr/local/bin/equasar-daemon",
        "usr/bin/adbd_backup",
    ];

    let keys: Vec<String> = tree.entries.keys().cloned().collect();
    for k in keys {
        // 1. Check empty_dirs children
        let mut drop_entry = false;
        for d in &empty_dirs {
            if *d == "data" && data_is_symlink {
                continue;
            }
            if k.starts_with(&format!("{}/", d)) {
                drop_entry = true;
                break;
            }
        }
        // 2. Check secret_exact or children of secret_exact
        if !drop_entry {
            for s in &secret_exact {
                if k == *s || k.starts_with(&format!("{}/", s)) {
                    drop_entry = true;
                    break;
                }
            }
        }
        // 3. Check *.db / *.sqlite* under etc/, var/, root/
        if !drop_entry
            && (k.starts_with("etc/") || k.starts_with("var/") || k.starts_with("root/"))
            && (k.ends_with(".db") || k.contains(".sqlite"))
        {
            drop_entry = true;
        }
        if drop_entry {
            tree.entries.remove(&k);
        }
    }

    // Ensure required directories exist
    for dir in [
        "proc",
        "sys",
        "dev",
        "run",
        "var",
        "var/run",
        "mnt",
        "media",
        "root",
        "ota",
        "tee",
        "bkupgrade",
        "bkupgrade/data",
        "bkupgrade/data/quasar",
        "bkupgrade/data/quasar/daemons_logs",
    ] {
        tree.insert_dir(dir, 0o755);
    }
    tree.insert_dir("tmp", 0o1777);
    if !data_is_symlink {
        tree.insert_dir("data", 0o755);
    }
}

pub fn sanitize_vfs(tree: &mut VfsTree) {
    sanitize_tree(tree);
}

pub fn write_ext_image(tree: &VfsTree, output_path: &Path, size_bytes: u64) -> Result<(), String> {
    write_ext2_image(tree, output_path, size_bytes)
}

pub fn read_ext_image_to_vfs(path: &Path) -> Result<VfsTree, String> {
    read_ext2_image(path)
}

fn load_dump_into_tree(path: &Path) -> Result<RootfsTree, String> {
    if path.is_dir() {
        let candidates = [
            path.join("rootfs.tar.gz"),
            path.join("system.img.gz"),
            path.join("system.img"),
            path.join("ubivol").join("ubi0_0_rootfs.img"),
            path.join("ubi0_0_rootfs.img"),
            path.join("mtd").join("mtd3_system.bin"),
            path.join("mtd3_system.bin"),
        ];
        for c in &candidates {
            if c.is_file() {
                return load_dump_into_tree(c);
            }
        }
        if let Ok(rd) = fs::read_dir(path) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) == Some("tgz") {
                    return load_dump_into_tree(&p);
                }
            }
        }
        // Load extracted directory tree
        let mut tree = RootfsTree::default();
        load_host_dir_recursive(path, path, &mut tree)?;
        return Ok(tree);
    }

    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        return read_tar_gz_into_tree(path);
    }
    if name.ends_with(".tar") {
        let f = File::open(path).map_err(|e| e.to_string())?;
        return read_tar_from_reader(f);
    }
    if name.ends_with(".zip") {
        let f = File::open(path).map_err(|e| e.to_string())?;
        let mut za = zip::ZipArchive::new(f).map_err(|e| format!("zip error: {}", e))?;
        // Check if zip contains a nested rootfs.tar.gz
        for i in 0..za.len() {
            let mut zf = za.by_index(i).map_err(|e| e.to_string())?;
            if zf.name().ends_with("rootfs.tar.gz") || zf.name().ends_with(".tgz") {
                let mut bytes = Vec::new();
                zf.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                return read_tar_from_reader(GzDecoder::new(Cursor::new(bytes)));
            }
        }
        let mut tree = RootfsTree::default();
        for i in 0..za.len() {
            let mut zf = za.by_index(i).map_err(|e| e.to_string())?;
            let Some(norm) = normalize_rel_path(zf.name()) else {
                continue;
            };
            let mode = zf.unix_mode().map(|m| (m as u16) & 0o7777).unwrap_or(0o755);
            if zf.is_dir() {
                tree.insert_dir(&norm, mode);
            } else {
                let mut buf = Vec::new();
                zf.read_to_end(&mut buf).map_err(|e| e.to_string())?;
                tree.insert_file(&norm, buf, mode);
            }
        }
        return Ok(tree);
    }

    let mut raw = fs::read(path).map_err(|e| format!("failed to read '{}': {}", path.display(), e))?;
    if raw.starts_with(&[0x1f, 0x8b]) {
        let mut dec = Vec::new();
        GzDecoder::new(Cursor::new(&raw))
            .read_to_end(&mut dec)
            .map_err(|e| format!("gzip decode failed: {}", e))?;
        raw = dec;
    }

    // Check ext2/ext4 magic at offset 1080
    if raw.len() >= 2048 && u16::from_le_bytes([raw[1080], raw[1081]]) == EXT2_SUPER_MAGIC {
        let tmp = std::env::temp_dir().join(format!("kuyase-ext-in-{}.img", std::process::id()));
        fs::write(&tmp, &raw).map_err(|e| e.to_string())?;
        let res = read_ext2_image(&tmp);
        let _ = fs::remove_file(&tmp);
        return res;
    }

    // Check if UBI image (magic "UBI#") or raw image containing an embedded gzip/tar/ext payload
    if let Some(pos) = raw.windows(2).position(|w| w == [0x1f, 0x8b]) {
        let mut dec = Vec::new();
        if GzDecoder::new(Cursor::new(&raw[pos..])).read_to_end(&mut dec).is_ok() {
            if let Ok(t) = read_tar_from_reader(Cursor::new(&dec)) {
                if !t.entries.is_empty() {
                    return Ok(t);
                }
            }
        }
    }

    if let Ok(t) = read_tar_from_reader(Cursor::new(&raw)) {
        if !t.entries.is_empty() {
            return Ok(t);
        }
    }

    Err(format!(
        "unsupported or unrecognized dump format: {}",
        path.display()
    ))
}

fn load_host_dir_recursive(base: &Path, cur: &Path, tree: &mut RootfsTree) -> Result<(), String> {
    let entries = fs::read_dir(cur).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let p = entry.path();
        let rel = p
            .strip_prefix(base)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if p.is_dir() {
            tree.insert_dir(&rel, 0o755);
            load_host_dir_recursive(base, &p, tree)?;
        } else if p.is_file() {
            let bytes = fs::read(&p).map_err(|e| e.to_string())?;
            tree.insert_file(&rel, bytes, 0o755);
        }
    }
    Ok(())
}

pub fn normalize_crlf_to_lf(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().copied().filter(|&b| b != b'\r').collect()
}

pub fn inject_guest_files(root: &Path, tree: &mut RootfsTree) -> Result<(), String> {
    sanitize_tree(tree);
    tree.insert_dir("usr", 0o755);
    tree.insert_dir("usr/local", 0o755);
    tree.insert_dir("usr/local/bin", 0o755);

    let init_path = root.join("guest-init").join("equasar-qemu-init.sh");
    let raw_init = if init_path.is_file() {
        let b = fs::read(&init_path).map_err(|e| e.to_string())?;
        let norm = normalize_crlf_to_lf(&b);
        if norm != b {
            let _ = fs::write(&init_path, &norm);
        }
        norm
    } else {
        normalize_crlf_to_lf(EMBEDDED_INIT_SH)
    };
    tree.insert_file("quasar-qemu-init.sh", raw_init, 0o755);

    let bridge_bytes = match resolve_bridge_binary(root) {
        Ok(p) => fs::read(&p).map_err(|e| e.to_string())?,
        Err(_) => EMBEDDED_HW_BRIDGE.to_vec(),
    };
    tree.insert_file("usr/local/bin/qemu-hw-bridge", bridge_bytes, 0o755);
    Ok(())
}

pub fn build_guest_image_from_dump(
    root: &Path,
    requested_board: Option<&str>,
    dump_path: &Path,
    out_img_override: Option<&Path>,
) -> Result<(String, PathBuf), String> {
    let mut tree = load_dump_into_tree(dump_path)?;
    let detected = detect_board_from_tree(&tree);
    let board = match requested_board {
        Some(b) if !b.trim().is_empty() && !b.eq_ignore_ascii_case("auto") => {
            normalize_board_slug(b)
        }
        _ => detected,
    };

    inject_guest_files(root, &mut tree)?;

    let work_dir = root.join("work");
    let _ = fs::create_dir_all(&work_dir);
    let out_img = match out_img_override {
        Some(p) => p.to_path_buf(),
        None => work_dir.join(format!("{}-system.ext4", board)),
    };

    let payload_bytes: usize = tree
        .entries
        .values()
        .map(|n| match &n.kind {
            RootfsNodeKind::File(b) => b.len(),
            _ => 0,
        })
        .sum();
    let img_size = if payload_bytes > 180 * 1024 * 1024 {
        1024 * 1024 * 1024
    } else if out_img_override.is_some() && payload_bytes < 8 * 1024 * 1024 {
        32 * 1024 * 1024
    } else {
        256 * 1024 * 1024
    };

    write_ext2_image(&tree, &out_img, img_size)?;
    let _ = fs::write(work_dir.join(".last-board"), format!("{}\n", board));
    println!(
        "=== [KUYASE] Built Guest Image: {} (Board Profile: {}) ===",
        out_img.display(),
        board
    );
    Ok((board, out_img))
}

pub fn prepare_images(
    root: &Path,
    requested_board: Option<&str>,
    dump_path: Option<&Path>,
) -> Result<Vec<(String, PathBuf)>, String> {
    if let Some(dp) = dump_path {
        let res = build_guest_image_from_dump(root, requested_board, dp, None)?;
        return Ok(vec![res]);
    }
    if let Some(b) = requested_board {
        let candidate = Path::new(b);
        if candidate.exists() {
            let res = build_guest_image_from_dump(root, None, candidate, None)?;
            return Ok(vec![res]);
        }
        let slug = normalize_board_slug(b);
        let existing = root.join("work").join(format!("{}-system.ext4", slug));
        if existing.is_file() {
            let res = build_guest_image_from_dump(root, Some(&slug), &existing, None)?;
            return Ok(vec![res]);
        }
    }

    let mut built = Vec::new();
    let backups_dir = root.join("backups");
    if let Ok(entries) = fs::read_dir(&backups_dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.file_name().and_then(|s| s.to_str()) == Some("README.md") {
                continue;
            }
            if let Ok(res) = build_guest_image_from_dump(root, requested_board, &p, None) {
                built.push(res);
            }
        }
    }
    if built.is_empty() {
        return Err(format!(
            "no firmware dumps found in {}/backups (use --dump <path> or run `kuyase dump <ip>`)",
            root.display()
        ));
    }
    Ok(built)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_synthetic_dump_board_detection_scrubbing_crlf_and_ext4_roundtrip() {
        let temp_dir =
            std::env::temp_dir().join(format!("kuyase-img-builder-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(temp_dir.join("guest-init").join("bin")).unwrap();

        // Create guest-init/equasar-qemu-init.sh with CRLF line endings
        fs::write(
            temp_dir.join("guest-init").join("equasar-qemu-init.sh"),
            b"#!/bin/sh\r\necho hello\r\n",
        )
        .unwrap();
        let mut mock_elf = b"\x7fELFmockbridge".to_vec();
        mock_elf.resize(128, 0);
        fs::write(
            temp_dir
                .join("guest-init")
                .join("bin")
                .join("qemu-hw-bridge"),
            &mock_elf,
        )
        .unwrap();

        // Build synthetic firmware dump .tar.gz with secrets and mini2 quasar.cfg
        let mut dump_tree = RootfsTree::default();
        dump_tree.insert_file(
            "system/vendor/quasar/quasar.cfg",
            br#"{"deviceType": "mini2"}"#.to_vec(),
            0o644,
        );
        dump_tree.insert_file("bin/busybox", b"\x7fELFbusybox".to_vec(), 0o755);
        dump_tree.insert_symlink("bin/sh", "busybox".to_string());
        dump_tree.insert_file("etc/wpa_supplicant.conf", b"psk=secret".to_vec(), 0o600);
        dump_tree.insert_file("data/secret.txt", b"token=123".to_vec(), 0o600);
        dump_tree.insert_file("usr/local/bin/dropbear", b"backdoor".to_vec(), 0o755);
        dump_tree.insert_file("var/auth.sqlite3", b"sqlite".to_vec(), 0o600);

        let dump_tar_path = temp_dir.join("rootfs.tar.gz");
        write_tree_to_tar_gz(&dump_tree, &dump_tar_path).unwrap();

        let out_img = temp_dir.join("work").join("yandexmini2-system.ext4");
        let (board, built_path) =
            build_guest_image_from_dump(&temp_dir, None, &dump_tar_path, Some(&out_img)).unwrap();

        assert_eq!(board, "yandexmini2");
        assert_eq!(built_path, out_img);
        assert!(out_img.is_file());

        // Read back the generated .ext4 image and verify invariants
        let readback = read_ext_image_to_vfs(&out_img).unwrap();
        assert_eq!(detect_board_from_vfs(&readback), "yandexmini2");
        assert!(readback.get("etc/wpa_supplicant.conf").is_none());
        assert!(readback.get("data/secret.txt").is_none());
        assert!(readback.get("usr/local/bin/dropbear").is_none());
        assert!(readback.get("var/auth.sqlite3").is_none());

        let init_bytes = readback.get_file_bytes("quasar-qemu-init.sh").unwrap();
        assert!(!init_bytes.contains(&b'\r'));
        assert_eq!(init_bytes, b"#!/bin/sh\necho hello\n");
        assert_eq!(readback.get("quasar-qemu-init.sh").unwrap().mode, 0o755);

        let bridge_node = readback.get("usr/local/bin/qemu-hw-bridge").unwrap();
        assert_eq!(bridge_node.mode, 0o755);
        assert_eq!(
            readback.get_file_bytes("usr/local/bin/qemu-hw-bridge").unwrap(),
            mock_elf.as_slice()
        );
        assert_eq!(readback.get("tmp").unwrap().mode, 0o1777);
        assert_eq!(
            readback.get("bin/sh").unwrap().kind,
            RootfsNodeKind::Symlink("busybox".to_string())
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
