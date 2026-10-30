use kuyase::adb::AdbClient;
use kuyase::bridge_client::BridgeClient;
use kuyase::session::{resolve_repo_root, EmulatorSession, SessionConfig};
use serde_json::Value;
use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::process;
use std::time::Duration;

fn print_usage() {
    println!(
        "KUYASE — Native Smart Speaker Emulator & Verification Runner\n\
         \n\
         Usage:\n\
           kuyase run --headless [--board <slug>] [--mode <stub|full>] [--snapshot]\n\
         \n\
         Options:\n\
           -b, --board <slug>        Board profile slug (yandexmini, yandexmini2, yandexmicro, cucumber, orion, ...)\n\
           -m, --mode <stub|full>    Daemon mode: stub (default) or full\n\
               --headless            Boot VM in headless mode, verify duplex bridge & ADB, print telemetry, and exit\n\
               --snapshot            Enable ephemeral Clean Boot overlay (-snapshot)\n\
               --bridge-port <port>  Host TCP port forwarded to guest :8091 (default: 18091)\n\
               --adb-port <port>     Host TCP port forwarded to guest :5555 (default: 15555)\n\
           -h, --help                Show this help message"
    );
}

#[derive(Default, Debug)]
struct CliOptions {
    subcommand: String,
    board: Option<String>,
    mode: String,
    headless: bool,
    snapshot: Option<bool>,
    bridge_port: u16,
    adb_port: u16,
}

fn parse_args() -> Result<CliOptions, String> {
    let mut opts = CliOptions {
        subcommand: String::new(),
        board: None,
        mode: "stub".to_string(),
        headless: false,
        snapshot: None,
        bridge_port: 18091,
        adb_port: 15555,
    };

    let args: Vec<String> = env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" | "help" => {
                print_usage();
                process::exit(0);
            }
            "run" | "test" | "dump" | "prepare" if opts.subcommand.is_empty() => {
                opts.subcommand = args[i].clone();
                i += 1;
            }
            "--headless" => {
                opts.headless = true;
                i += 1;
            }
            "--snapshot" => {
                opts.snapshot = Some(true);
                i += 1;
            }
            "--no-snapshot" | "--persist" => {
                opts.snapshot = Some(false);
                i += 1;
            }
            "-b" | "--board" => {
                let Some(val) = args.get(i + 1) else {
                    return Err("missing value for --board".to_string());
                };
                opts.board = Some(val.clone());
                i += 2;
            }
            "-m" | "--mode" => {
                let Some(val) = args.get(i + 1) else {
                    return Err("missing value for --mode".to_string());
                };
                opts.mode = val.clone();
                i += 2;
            }
            "--bridge-port" => {
                let Some(val) = args.get(i + 1) else {
                    return Err("missing value for --bridge-port".to_string());
                };
                opts.bridge_port = val
                    .parse::<u16>()
                    .map_err(|_| format!("invalid --bridge-port: {}", val))?;
                i += 2;
            }
            "--adb-port" => {
                let Some(val) = args.get(i + 1) else {
                    return Err("missing value for --adb-port".to_string());
                };
                opts.adb_port = val
                    .parse::<u16>()
                    .map_err(|_| format!("invalid --adb-port: {}", val))?;
                i += 2;
            }
            other if !other.starts_with('-') && opts.board.is_none() => {
                opts.board = Some(other.to_string());
                i += 1;
            }
            other => {
                return Err(format!("unknown argument: {}", other));
            }
        }
    }

    if opts.subcommand.is_empty() {
        opts.subcommand = "run".to_string();
    }

    Ok(opts)
}

fn run_headless_session(opts: &CliOptions) -> Result<(), String> {
    let root = resolve_repo_root();
    let mut cfg = SessionConfig::for_board(&root, opts.board.as_deref())?;
    cfg.mode = opts.mode.clone();
    cfg.snapshot = opts.snapshot.unwrap_or(true);
    cfg.bridge_port = opts.bridge_port;
    cfg.adb_port = opts.adb_port;

    println!(
        "=== [KUYASE] Starting headless session for board='{}' (image={}, bridge=:{}, adb=:{}) ===",
        cfg.board,
        cfg.drive_img.display(),
        cfg.bridge_port,
        cfg.adb_port
    );

    let mut session = EmulatorSession::start(cfg.clone())?;

    let result = (|| -> Result<(), String> {
        let bridge_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), cfg.bridge_port);
        let bridge = BridgeClient::connect_with_timeout(bridge_addr, Duration::from_secs(25))?;

        let initial_state = bridge.latest_state();
        let refreshed_state = bridge.request_state()?;

        if refreshed_state.board_slug != cfg.board {
            return Err(format!(
                "board_slug mismatch: expected '{}', got '{}'",
                cfg.board, refreshed_state.board_slug
            ));
        }

        let adb = AdbClient::from_host_port("127.0.0.1", cfg.adb_port)?;
        let adb_out = adb.shell(
            "hostname; uname -m; [ ! -e /usr/local/bin/equasar-daemon ] && echo CLEAN_STANDALONE; cat /tmp/quasar-emu-hw.json",
            Duration::from_secs(12),
        )?;

        let expected_host = format!("qemu-{}", cfg.board);
        if !adb_out.contains(&expected_host) {
            return Err(format!(
                "ADB output missing expected hostname '{}': {}",
                expected_host, adb_out
            ));
        }
        if !adb_out.contains("CLEAN_STANDALONE") {
            return Err(format!(
                "ADB output missing CLEAN_STANDALONE marker: {}",
                adb_out
            ));
        }

        let json_start = adb_out
            .find('{')
            .ok_or_else(|| format!("missing JSON in /tmp/quasar-emu-hw.json output: {}", adb_out))?;
        let hw_info: Value = serde_json::from_str(&adb_out[json_start..])
            .map_err(|e| format!("invalid /tmp/quasar-emu-hw.json: {}", e))?;

        let hw_slug = hw_info
            .get("board_slug")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if hw_slug != refreshed_state.board_slug {
            return Err(format!(
                "quasar-emu-hw.json board_slug '{}' != bridge board_slug '{}'",
                hw_slug, refreshed_state.board_slug
            ));
        }

        println!("--- Duplex Bridge StateSnapshot ---");
        println!("  board_slug   : {}", refreshed_state.board_slug);
        println!("  board_name   : {}", refreshed_state.board_name);
        println!(
            "  ring_leds    : {} (trigger: {})",
            refreshed_state.ring_leds, refreshed_state.ring_trigger
        );
        println!(
            "  display      : {} (has_clock: {})",
            refreshed_state.display_kind, refreshed_state.has_clock
        );
        println!(
            "  aux_light    : {} (logo: {}, ambilight: {})",
            refreshed_state.aux_light_kind,
            refreshed_state.logo_leds,
            refreshed_state.ambilight_leds
        );
        println!(
            "  alice_state  : {} (volume: {}/10, mics_muted: {})",
            refreshed_state.alice_state,
            refreshed_state.volume_level,
            refreshed_state.mics_muted
        );
        println!(
            "  ipc_errors   : {} (initial snapshot ok: {})",
            refreshed_state.ipc_error_count, initial_state.ok
        );
        println!(
            "--- Built-in ADB (:15555 -> :5555) ---\n  hostname     : {}\n  led_driver   : {}\n  sysfs_prefix : {}",
            expected_host,
            hw_info.get("led_driver").and_then(|v| v.as_str()).unwrap_or("unknown"),
            hw_info.get("sysfs_prefix").and_then(|v| v.as_str()).unwrap_or("unknown"),
        );

        bridge.close();
        Ok(())
    })();

    session.stop();
    result
}

fn main() {
    let opts = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[kuyase] Error: {}", e);
            print_usage();
            process::exit(2);
        }
    };

    let res = match opts.subcommand.as_str() {
        "run" => run_headless_session(&opts),
        other => Err(format!("subcommand '{}' is not implemented yet", other)),
    };

    if let Err(err) = res {
        eprintln!("[kuyase] FAILED: {}", err);
        process::exit(1);
    }
}
