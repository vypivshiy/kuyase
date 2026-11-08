# Pull Request Checklist for Agents

Use this checklist when preparing, opening, updating, or reviewing a pull request against this repository.

## 1. Code & Protocol Hygiene

- **Single-source protocol and timing constants**: Define shared sample rates, frame intervals, buffer sizes, and wire-format constants once (`pub const` in `hw-bridge/src/proto.rs` or `protocol/src/lib.rs`) and derive dependent values from them (for example, `YIO_AUDIO_FRAME_SAMPLES = (YIO_AUDIO_SAMPLE_RATE as usize * YIO_AUDIO_FRAME_MS as usize) / 1000`). Import those constants in consumers (`hw-bridge/src/ipc.rs`, `kuyase`) instead of repeating raw literals coupled by comments.
- **Deadline-paced streaming with lag cap**: Schedule periodic IPC and audio loops (`yio_audio` `:9850`, `glagold`, ALSA) against a monotonic `Instant` deadline (`next_frame += frame_interval`). Cap accumulated lag by resetting `next_frame = now` whenever `now.saturating_duration_since(next_frame) > frame_interval` so socket backpressure or QEMU TCG CPU stalls resume cleanly at real-time cadence without emitting a zero-sleep burst of stale frames.
- **Trait-based peripheral dispatch**: Route board-specific hardware behavior through `BoardPeripherals` traits (`LedRingDevice`, `AuxLightDevice`, `FrontDisplayDevice`, `AudioSubsystemDevice` in `hw-bridge/src/devices/`) rather than branching on `board_slug` inside `bridge_server.rs` or `leds.rs`.
- **Unit test naming**: Prefix every unit test function with `test_` (`fn test_<behavior>()`).

## 2. Build & Verification Gate

Before opening or approving a PR, verify:

1. **Workspace unit tests**:
   ```bash
   cargo test --workspace
   ```
2. **Static ARMv7 guest bridge build** (required whenever `hw-bridge` or `protocol` changes):
   ```bash
   cargo build --release -p qemu-hw-bridge --target armv7-unknown-linux-musleabihf
   ```
   State in the PR description whether `guest-init/bin/qemu-hw-bridge` was updated in the branch or intentionally left untouched to avoid binary merge conflicts.
3. **End-to-end guest check** (when guest behavior, IPC, or init scripts change):
   Run `cargo run --release -p kuyase -- test --board <board>` or `cargo run --release -p kuyase -- run --headless --board <board> --snapshot --exec "<cmd>"` and confirm `ipc_error_count == 0` and `led_trigger_conflict_count == 0`.

## 3. PR Description Structure & Artifact Scrubbing

Structure the PR body with three sections:

- `## Problem` — observable defect or missing capability, including reproduction metrics (frame counts, durations, IPC packet types, or board slugs).
- `## Change` — root-cause fix and touched modules.
- `## Validation` — exact commands executed and measured outcomes before and after the change.

**Completion criterion**: `git status` and `git diff` contain only source, test, and documentation changes—zero `.ext4` images, `backups/`, `/data`, `/nv`, `/env`, `/tee`, Wi-Fi credentials, or device serials/keys.
