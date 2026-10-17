# CLAUDE.md – lancast

A small Windows tool for LAN parties. A participant selects a game window, and the tool sends the image of that window plus the audio of the associated process as an NDI source on the LAN. A central machine (OBS + DistroAV, or NDI Studio Monitor) shows the source on the projector.

The full specification is in `docs/SPEC.md`. Read it before making any larger change.

## Constraints
- Target platform: Windows 10/11 x64 only. Build natively on Windows with the MSVC toolchain, not in WSL.
- Language: Rust (stable, edition 2021 or newer). Write code, comments and commit messages in English.
- Hard rule because of anti-cheat systems: no injection, no hooking, no overlays in foreign processes. Use only OS APIs (Windows Graphics Capture, WASAPI).
- Preferred crates. Check the current version and API on docs.rs before use; do not assume them from memory.
  - `windows-capture` for WGC
  - `windows` (windows-rs) for Win32/WASAPI, if no crate cleanly covers process loopback
  - `grafton-ndi` for the NDI 6 SDK
  - `eframe`/`egui` for the minimal UI
- External dependencies: NDI 6 SDK (env `NDI_SDK_DIR`) and LLVM/Clang for bindgen. Defaults for both are in `.cargo/config.toml`.

## Way of working
- Work in milestones as defined in SPEC §7. Each milestone must run and build with `cargo build --release` without warnings.
- Before using any Windows API, check the Microsoft documentation (learn.microsoft.com) for minimum version and limitations. Record assumptions and findings, with sources, in `docs/DECISIONS.md`.
- Keep `unsafe` confined to clearly scoped modules (`capture/audio_wasapi.rs` etc.), each block with a `// SAFETY:` comment.
- Threading: video capture, audio capture and NDI send run decoupled via bounded channels (`crossbeam-channel`). When a channel is full, drop the frame rather than block.
- Logging with `tracing`, to stdout and to a log file next to the exe.

## Commands
- Build: `cargo build --release`
- Lint: `cargo clippy --all-targets -- -D warnings`
- Format: `cargo fmt`
- Test receiver: NDI Studio Monitor (NDI Tools) on the same or a second machine.
