# Headless CLI Builds via Optional `gui` Feature and Static Linux `musl` Target

Status: accepted

We gate `eframe` and `kuyase::gui` behind a default-enabled Cargo feature (`gui`) in the `kuyase` crate and distribute standalone `kuyase-headless` (`x86_64-unknown-linux-musl` via `rust-lld`) and `kuyase-headless.exe` (`x86_64-pc-windows-msvc`) binaries in dedicated minimal release archives (`kuyase-headless-linux64.tar.gz`, `kuyase-headless-win64.zip`). Full desktop builds (`kuyase` / `kuyase.exe`) preserve launching the GUI when invoked with no arguments, while both builds require an explicit `--headless` flag for headless `run` invocations so accidental double-clicks on `kuyase-headless` fail fast with usage instructions rather than silently starting a background VM.

## Considered Options

- **Feature flag (`default = ["gui"]`) in `kuyase` vs. separate workspace crate**: Every non-GUI module in `kuyase` is already pure Rust with zero references to `eframe` or `crate::gui`; an optional `gui` feature eliminates `eframe` from the dependency graph under `--no-default-features` without duplicating CLI argument parsing or splitting the crate.
- **Static `x86_64-unknown-linux-musl` vs. dynamic `x86_64-unknown-linux-gnu` for Linux headless builds**: Because `kuyase` without `eframe` has only pure-Rust dependencies (`miniz_oxide`, `tar`, `zip`, `serde`), targeting `x86_64-unknown-linux-musl` with `rust-lld` produces a zero-dependency static ELF that runs in any minimal CI container (without `glibc`, X11, Wayland, or OpenGL libraries) and cross-compiles directly from a Windows host without WSL2.
