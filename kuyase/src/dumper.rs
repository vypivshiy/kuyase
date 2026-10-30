use crate::adb::AdbClient;
use crate::image_builder::sanitize_tar_gz_file;
use protocol::normalize_board_slug;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpInfo {
    pub board_slug: String,
    pub target: String,
    pub timestamp: String,
    pub scratch_path: String,
    pub archive_file: String,
    pub archive_bytes: u64,
    pub device_type_cfg: String,
    pub dt_model: String,
    pub partitions: String,
}

#[derive(Clone, Debug)]
pub struct DumpResult {
    pub board_slug: String,
    pub out_dir: PathBuf,
    pub archive_path: PathBuf,
    pub info_path: PathBuf,
    pub archive_bytes: u64,
    pub info: DumpInfo,
}

pub fn detect_board_slug_from_probe(dev_type_cfg: &str, model_raw: &str) -> String {
    let dev = dev_type_cfg.trim();
    if !dev.is_empty() && !dev.eq_ignore_ascii_case("unknown") {
        return match dev {
            "cucumber" => "cucumber".to_string(),
            "bergamot" | "mini3" => "bergamot".to_string(),
            "pickle" | "mini3pro" => "pickle".to_string(),
            "orion" | "station3" => "orion".to_string(),
            "plum" | "lite2" => "plum".to_string(),
            "mango" | "street" => "mango".to_string(),
            "saturn" | "hub" => "saturn".to_string(),
            "station_2" | "station-midi" | "yandexmidi" | "station2" => "station2".to_string(),
            "yandexmini_2" | "mini2" | "yandexmini2" => "yandexmini2".to_string(),
            "yandexmicro" | "lite1" | "micro" => "yandexmicro".to_string(),
            "yandexmini" | "mini1" => "yandexmini".to_string(),
            other => normalize_board_slug(other),
        };
    }

    let model_lc = model_raw.to_ascii_lowercase();
    if model_lc.contains("cucumber") {
        "cucumber".to_string()
    } else if model_lc.contains("bergamot") {
        "bergamot".to_string()
    } else if model_lc.contains("pickle") {
        "pickle".to_string()
    } else if model_lc.contains("orion") {
        "orion".to_string()
    } else if model_lc.contains("plum") {
        "plum".to_string()
    } else if model_lc.contains("mango") {
        "mango".to_string()
    } else if model_lc.contains("saturn") {
        "saturn".to_string()
    } else if model_lc.contains("station-midi") {
        "station2".to_string()
    } else if model_lc.contains("yandexmini_2") || model_lc.contains("mini2") {
        "yandexmini2".to_string()
    } else if model_lc.contains("yandexmicro") {
        "yandexmicro".to_string()
    } else if model_lc.contains("axg") {
        "yandexmini".to_string()
    } else {
        "unknown".to_string()
    }
}

pub fn format_utc_timestamp(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;

    // Howard Hinnant civil-from-days algorithm
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}{:02}{:02}_{:02}{:02}{:02}",
        year, m, d, hour, min, sec
    )
}

const PROBE_SCRIPT: &str = r#"
DEV_TYPE=$(sed -n 's/.*"deviceType"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' /system/vendor/quasar/quasar.cfg 2>/dev/null | head -n 1)
MODEL=$(tr '\0\r\n' '   ' < /proc/device-tree/compatible 2>/dev/null; printf " "; tr '\0\r\n' '   ' < /proc/device-tree/model 2>/dev/null)
echo "KUYASE_DEV_TYPE=$DEV_TYPE"
echo "KUYASE_MODEL=$MODEL"
echo "KUYASE_PARTITIONS_START"
df -k 2>/dev/null || true
"#;

const BUILD_ARCHIVE_SCRIPT: &str = r#"
TMP_FREE=$(df -k /tmp 2>/dev/null | awk 'NR==2 {print $4}')
BK_FREE=$( [ -d /bkupgrade ] && df -k /bkupgrade 2>/dev/null | awk 'NR==2 {print $4}' || echo 0 )
DATA_FREE=$( [ -d /data ] && df -k /data 2>/dev/null | awk 'NR==2 {print $4}' || echo 0 )
ROOT_USED=$(df -k / 2>/dev/null | awk 'NR==2 {print $3}')
NEED_KB=$(( ${ROOT_USED:-80000} * 80 / 100 ))
[ "$NEED_KB" -lt 70000 ] && NEED_KB=70000

SCRATCH="/tmp/quasar_emu_rootfs.tar.gz"
if [ "${TMP_FREE:-0}" -ge "$NEED_KB" ]; then
  SCRATCH="/tmp/quasar_emu_rootfs.tar.gz"
elif [ "${BK_FREE:-0}" -ge "$NEED_KB" ] && [ "${BK_FREE:-0}" -ge "${DATA_FREE:-0}" ]; then
  SCRATCH="/bkupgrade/quasar_emu_rootfs.tar.gz"
elif [ "${DATA_FREE:-0}" -ge "$NEED_KB" ]; then
  SCRATCH="/data/quasar_emu_rootfs.tar.gz"
elif [ "${BK_FREE:-0}" -gt "${TMP_FREE:-0}" ]; then
  SCRATCH="/bkupgrade/quasar_emu_rootfs.tar.gz"
elif [ "${DATA_FREE:-0}" -gt "${TMP_FREE:-0}" ]; then
  SCRATCH="/data/quasar_emu_rootfs.tar.gz"
fi
rm -f "$SCRATCH"
cd /
DIRS=""
for d in bin sbin lib lib32 lib64 usr etc system boot linuxrc; do
  [ -e "$d" ] && DIRS="$DIRS $d"
done
tar -cf - $DIRS 2>/dev/null | gzip -1 > "$SCRATCH"
echo "SCRATCH_PATH=$SCRATCH"
ls -lh "$SCRATCH" 2>/dev/null || true
"#;

pub fn dump_from_adb(
    root: &Path,
    target: &str,
    out_dir_override: Option<&Path>,
) -> Result<DumpResult, String> {
    let client = AdbClient::parse_target(target)?;
    let normalized_target = client.addr().to_string();

    println!(
        "[1/3] Connecting to speaker over network ADB ({})...",
        normalized_target
    );
    let probe_out = client.shell(PROBE_SCRIPT.trim(), Duration::from_secs(20))?;

    let mut dev_type_cfg = String::new();
    let mut dt_model = String::new();
    let mut partitions = String::new();
    let mut in_partitions = false;

    for line in probe_out.lines() {
        let trimmed = line.trim();
        if in_partitions {
            if !partitions.is_empty() {
                partitions.push('\n');
            }
            partitions.push_str(trimmed);
        } else if let Some(rest) = trimmed.strip_prefix("KUYASE_DEV_TYPE=") {
            dev_type_cfg = rest.trim().to_string();
        } else if let Some(rest) = trimmed.strip_prefix("KUYASE_MODEL=") {
            dt_model = rest.trim().to_string();
        } else if trimmed == "KUYASE_PARTITIONS_START" {
            in_partitions = true;
        }
    }

    let board_slug = detect_board_slug_from_probe(&dev_type_cfg, &dt_model);
    let timestamp = format_utc_timestamp(SystemTime::now());
    let out_dir = match out_dir_override {
        Some(dir) => dir.to_path_buf(),
        None => root
            .join("backups")
            .join(format!("dump-{}-{}", board_slug, timestamp)),
    };

    fs::create_dir_all(&out_dir).map_err(|e| {
        format!(
            "failed to create dump directory '{}': {}",
            out_dir.display(),
            e
        )
    })?;

    println!(" -> Detected Board Profile : {}", board_slug);
    println!(" -> Output Directory       : {}", out_dir.display());

    println!(
        "[2/3] Building system archive rootfs.tar.gz (excluding /data, /bkupgrade, /nv, /env, /tee)..."
    );
    let build_out = client.shell(BUILD_ARCHIVE_SCRIPT.trim(), Duration::from_secs(300))?;
    let mut scratch_path = "/tmp/quasar_emu_rootfs.tar.gz".to_string();
    for line in build_out.lines() {
        if let Some(rest) = line.trim().strip_prefix("SCRATCH_PATH=") {
            let candidate = rest.trim();
            if !candidate.is_empty() {
                scratch_path = candidate.to_string();
            }
        }
    }

    let archive_path = out_dir.join("rootfs.tar.gz");
    println!(
        "[3/3] Pulling {} over ADB sync and cleaning temporary file...",
        scratch_path
    );
    let pull_res = client.pull_file(&scratch_path, &archive_path, Duration::from_secs(300));
    let _ = client.shell(
        &format!("rm -f {}", scratch_path),
        Duration::from_secs(20),
    );
    let _pulled_bytes = pull_res?;

    // Scrub any sensitive files (wpa_supplicant.conf, shadow, dropbear, etc.) from the pulled tar.gz
    let _ = sanitize_tar_gz_file(&archive_path)?;
    let archive_bytes = fs::metadata(&archive_path)
        .map(|m| m.len())
        .unwrap_or(_pulled_bytes);

    let info = DumpInfo {
        board_slug: board_slug.clone(),
        target: normalized_target,
        timestamp,
        scratch_path,
        archive_file: "rootfs.tar.gz".to_string(),
        archive_bytes,
        device_type_cfg: dev_type_cfg,
        dt_model,
        partitions,
    };

    let info_path = out_dir.join("dump-info.json");
    let info_json = serde_json::to_string_pretty(&info)
        .map_err(|e| format!("failed to serialize dump-info.json: {}", e))?;
    fs::write(&info_path, info_json)
        .map_err(|e| format!("failed to write '{}': {}", info_path.display(), e))?;

    println!(
        "=== [KUYASE] Firmware Dump saved to {} ({} bytes) ===",
        out_dir.display(),
        archive_bytes
    );

    Ok(DumpResult {
        board_slug,
        out_dir,
        archive_path,
        info_path,
        archive_bytes,
        info,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adb::{
        read_adb_packet, write_adb_packet, A_CLSE, A_CNXN, A_OKAY, A_OPEN, A_VERSION, A_WRTE,
        MAX_ADB_DATA,
    };
    use crate::image_builder::{read_tar_gz_into_tree, RootfsTree};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn test_detect_board_slug_from_probe() {
        assert_eq!(detect_board_slug_from_probe("mini2", ""), "yandexmini2");
        assert_eq!(
            detect_board_slug_from_probe("station3", "yandex,orion"),
            "orion"
        );
        assert_eq!(
            detect_board_slug_from_probe("", "yandex,cucumber-dvt2 amlogic,a5"),
            "cucumber"
        );
        assert_eq!(
            detect_board_slug_from_probe("", "amlogic, axg Amlogic"),
            "yandexmini"
        );
        assert_eq!(
            detect_board_slug_from_probe("lite1", ""),
            "yandexmicro"
        );
    }

    #[test]
    fn test_dump_from_adb_against_mock_speaker() {
        // Build a synthetic tar.gz representing the speaker's raw tar output (including a secret wpa_supplicant.conf that must be scrubbed)
        let mut raw_tree = RootfsTree::default();
        raw_tree.insert_file(
            "system/vendor/quasar/quasar.cfg",
            br#"{"deviceType": "station3"}"#.to_vec(),
            0o644,
        );
        raw_tree.insert_file("bin/busybox", b"\x7fELFbusybox".to_vec(), 0o755);
        raw_tree.insert_symlink("bin/sh", "busybox".to_string());
        raw_tree.insert_file(
            "etc/wpa_supplicant.conf",
            b"network={ssid=\"MyHomeWiFi\" psk=\"supersecret\"}".to_vec(),
            0o600,
        );
        raw_tree.insert_file("data/secret_token.txt", b"oauth_token=12345".to_vec(), 0o600);

        let temp_dir = std::env::temp_dir().join(format!(
            "kuyase-mock-dump-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        let mock_tar_path = temp_dir.join("device_scratch.tar.gz");
        crate::image_builder::write_tree_to_tar_gz(&raw_tree, &mock_tar_path).unwrap();
        let mock_tar_bytes = fs::read(&mock_tar_path).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            // Expect 3 sequential connections:
            // 1) shell probe
            // 2) shell tar creation
            // 3) sync RECV pull
            // 4) shell rm -f cleanup
            for step in 0..4 {
                let (mut stream, _) = listener.accept().unwrap();
                let cnxn = read_adb_packet(&mut stream).unwrap();
                assert_eq!(cnxn.command, A_CNXN);
                write_adb_packet(
                    &mut stream,
                    A_CNXN,
                    A_VERSION,
                    MAX_ADB_DATA,
                    b"device::\0",
                )
                .unwrap();

                let open = read_adb_packet(&mut stream).unwrap();
                assert_eq!(open.command, A_OPEN);
                let client_id = open.arg0;
                let device_id = 10 + step;
                let open_str = String::from_utf8_lossy(&open.payload);

                write_adb_packet(&mut stream, A_OKAY, device_id, client_id, b"").unwrap();

                if open_str.starts_with("shell:") {
                    let reply = match step {
                        0 => "KUYASE_DEV_TYPE=station3\nKUYASE_MODEL=yandex,orion-pvt\nKUYASE_PARTITIONS_START\nFilesystem 1K-blocks Used Available Use% Mounted on\ntmpfs 200000 100 199900 1% /tmp\n",
                        1 => "SCRATCH_PATH=/tmp/quasar_emu_rootfs.tar.gz\n-rw-r--r-- 1 root root 1024 /tmp/quasar_emu_rootfs.tar.gz\n",
                        _ => "",
                    };
                    if !reply.is_empty() {
                        write_adb_packet(
                            &mut stream,
                            A_WRTE,
                            device_id,
                            client_id,
                            reply.as_bytes(),
                        )
                        .unwrap();
                        let _ = read_adb_packet(&mut stream);
                    }
                    write_adb_packet(&mut stream, A_CLSE, device_id, client_id, b"").unwrap();
                    let _ = read_adb_packet(&mut stream);
                } else if open_str.starts_with("sync:") {
                    let recv = read_adb_packet(&mut stream).unwrap();
                    assert_eq!(recv.command, A_WRTE);
                    assert_eq!(&recv.payload[0..4], b"RECV");
                    write_adb_packet(&mut stream, A_OKAY, device_id, client_id, b"").unwrap();

                    let mut sync_payload = Vec::new();
                    sync_payload.extend_from_slice(b"DATA");
                    sync_payload.extend_from_slice(&(mock_tar_bytes.len() as u32).to_le_bytes());
                    sync_payload.extend_from_slice(&mock_tar_bytes);
                    sync_payload.extend_from_slice(b"DONE");
                    sync_payload.extend_from_slice(&0u32.to_le_bytes());

                    write_adb_packet(&mut stream, A_WRTE, device_id, client_id, &sync_payload)
                        .unwrap();
                    let _ = read_adb_packet(&mut stream);
                    let _ = read_adb_packet(&mut stream);
                }
            }
        });

        let out_dir = temp_dir.join("out-dump");
        let res = dump_from_adb(&temp_dir, &addr.to_string(), Some(&out_dir)).unwrap();
        server.join().unwrap();

        assert_eq!(res.board_slug, "orion");
        assert!(res.archive_path.is_file());
        assert!(res.info_path.is_file());

        // Verify the resulting rootfs.tar.gz has wpa_supplicant.conf and /data/secret_token.txt scrubbed!
        let dumped_tree = read_tar_gz_into_tree(&res.archive_path).unwrap();
        assert!(dumped_tree
            .get("system/vendor/quasar/quasar.cfg")
            .is_some());
        assert!(dumped_tree.get("bin/busybox").is_some());
        assert!(dumped_tree.get("etc/wpa_supplicant.conf").is_none());
        assert!(dumped_tree.get("data/secret_token.txt").is_none());

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
