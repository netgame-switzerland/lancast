# lancast – Specification

## 1. Goal
A participant launches a single exe, picks their game window and clicks "Send". An NDI source then appears on the LAN, carrying the window image and **only the game's audio** (no Discord, no system sounds). No installation, no configuration, and no OBS on the participant's machine.

## 2. Non-goals (v1)
- No receiving and no projector control. The central machine handles that with OBS + DistroAV or NDI Studio Monitor.
- No webcam, no microphone, no overlays.
- No encoding and no recording. The NDI SDK handles compression.
- No macOS or Linux.

## 3. User flow
1. The participant launches `lancast.exe`. A small window (egui) opens.
2. The window shows a list of visible top-level windows with title and process name, plus a "Refresh" button.
3. The participant selects a window. The source name can be edited and defaults to `<Hostname> – <process name>`. NDI prepends the machine name itself, so check this and avoid duplication.
4. "Send" starts streaming. The UI shows status: resolution, FPS, audio active yes/no, and the number of connected receivers (`get_no_connections`).
5. "Stop" ends streaming. If the target window closes, sending stops automatically with a notice.

**Important:** The WGC system picker (`GraphicsCapturePicker`) does not return an HWND, and therefore no PID. Process-loopback audio needs the PID, so we use **our own window list** instead:
1. Enumerate windows with `EnumWindows`.
2. Keep visible, non-cloaked windows that have a title.
3. Get the PID with `GetWindowThreadProcessId`.
4. Create the capture item from the HWND.

## 4. Architecture
```
[UI thread (egui)]
      │ start/stop, status
      ▼
[Controller] ──► [Video: WGC capture thread] ──frames──┐
             └─► [Audio: WASAPI process loopback] ─────┤ bounded channels
                                                       ▼
                                            [NDI sender thread]
```

Modules:
- `src/windows_list.rs`: enumerate and filter windows, map HWND → PID
- `src/capture/video.rs`: WGC via `windows-capture`, produces BGRA frames (CPU buffer)
- `src/capture/audio_wasapi.rs`: process loopback, with fallback to system loopback
- `src/ndi_out.rs`: NDI sender and frame conversion
- `src/app.rs`: egui UI and controller
- `src/config.rs`: optional `lancast.toml` next to the exe (FPS cap, max resolution, source name)

## 5. Technical details

### 5.1 Video (Windows Graphics Capture)
- Use the `windows-capture` crate and create the item from the HWND.
- Cursor **off** by default.
- Disable the yellow capture border where possible (`DrawBorderSettings`). This only works on Windows 11; on Windows 10 the border stays, which is acceptable.
- Pixel format is BGRA8. Pass it to NDI as `BGRX` (alpha is irrelevant and this saves work) or as `BGRA`.
- Handle size changes at runtime (the window is resized or minimized). A minimized window produces no frames.
- FPS cap: default 60, configurable.
- Max resolution: default 1920×1080. Downscale larger windows on the CPU. In v1 a simple box or bilinear filter is enough; v2 may use the GPU (D3D11 Video Processor).
- Known limitation: games in **exclusive fullscreen** may deliver black frames or none at all. The UI should show the hint "Please use borderless windowed mode".

### 5.2 Audio (WASAPI process loopback)
- Use `ActivateAudioInterfaceAsync` with `VAD\Process_Loopback`, `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK` and `PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE`. The include-tree mode matters because games often render audio in child processes or within a launcher tree.
- `GetMixFormat` is not supported in this mode, so set the format explicitly: 48 kHz, stereo, Float32.
- **Verify the minimum Windows version.** Sources disagree between Windows 10 2004 (19041) and build 20348. The Microsoft documentation for `AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS` is authoritative. Record the result in `DECISIONS.md`.
- Fallback: if activation fails, use regular system loopback (default render endpoint) and show this in the UI ("System audio – includes Discord etc.").
- First check whether the `wasapi` crate (HEnquist/wasapi-rs) now cleanly wraps process loopback. Otherwise use `windows`-rs directly, with Microsoft's "ApplicationLoopback" sample as reference.
- When the game is silent, WASAPI delivers no packets or "silent" packets. Still send silence to NDI so the receiver gets no audio dropouts or sync issues.

### 5.3 NDI output
- Use the `grafton-ndi` crate (NDI 6 SDK). Check the current API on docs.rs; this crate has frequent breaking changes.
- Sender settings: `clock_video = true`, `clock_audio = false`. Video and audio are submitted from separate threads; see the SDK documentation on clocking.
- Video: use async send for zero-copy, with double buffering. Reuse a buffer only after its completion.
- Audio: NDI expects **planar** float (FLTP), while WASAPI delivers interleaved, so convert. Batch packets sensibly (e.g. 10–20 ms).
- Timecode: use `NDIlib_send_timecode_synthesize` or the crate's equivalent.
- Optional: with 0 receivers, throttle or pause capture to save CPU.

## 6. Distribution
- Ship `lancast.exe` and `Processing.NDI.Lib.x64.dll` in the same folder, as a ZIP. A `build.rs` should copy the DLL next to the exe on build.
- **Verify** the NDI SDK license before distributing: runtime redistribution terms and required notices (trademark, link to ndi.video). The alternative is to require the NDI Runtime or NDI Tools on participants' machines.
- The exe is unsigned, so a SmartScreen warning is expected. That is acceptable for a LAN party; note it in the README.
- Windows Firewall: NDI needs mDNS and TCP/UDP ports, so the firewall dialog appears on first launch. Note it in the README.

## 7. Milestones
- **M0 – Spikes (one example each under `examples/`):**
  1. `list_windows`: print windows with PID and process name.
  2. `ndi_testpattern`: send color bars and a sine tone as NDI, visible in NDI Studio Monitor.
  3. `wgc_window`: capture one HWND and log FPS and resolution.
  4. `process_loopback`: record 10 s of a PID's audio to a WAV file.
- **M1:** video of a selected window (CLI argument PID/HWND) → NDI.
- **M2:** integrate audio; A/V in sync at the receiver. Lip-sync test: use YouTube in a browser as the "game".
- **M3:** egui UI per §3, with status display.
- **M4:** config file, scaling/FPS cap, fallback paths, release ZIP with README.

## 8. Acceptance criteria
- A 1080p60 game in borderless windowed mode appears in NDI Studio Monitor on a second PC, with subjective latency below roughly 200 ms.
- Only the game audio is audible. Discord audio on the same system is **not** audible.
- Minimizing, restoring and resizing the window cause neither a crash nor a permanent freeze.
- Quitting the game leads to a clean stop with a notice.
- The tool's CPU load at 1080p60 is reasonable on a mid-range PC (guideline: < 10 %).

## 9. Open points
- Verify the minimum Windows version for process loopback.
- Clarify the NDI license regarding DLL redistribution.
- Bandwidth: full NDI at 1080p60 needs roughly 100–150 Mbit/s per source. With many simultaneous senders, check the switch and the central machine's uplink. Sending NDI|HX is probably not available in the free SDK (verify).
