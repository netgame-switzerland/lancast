# Decisions and findings

Assumptions, verified facts and decisions, each with its source. Newest milestone first.
Test machine for the M0 measurements: Windows 11 build 26200 (`ver` → 10.0.26200.9711),
NDI SDK 6.3.2.0, rustc 1.99.1 (MSVC).

---

## M0 – 2026-10-17

### D1. Crate versions (checked on crates.io / docs.rs and in the crate sources)

| Crate | Version | Notes |
|---|---|---|
| `grafton-ndi` | 2.0.0 | Default feature `image-encoding` disabled (not needed). Links `Processing.NDI.Lib.x64.lib` from `NDI_SDK_DIR`, generates bindings via bindgen (needs `LIBCLANG_PATH`). |
| `windows-capture` | 2.0.1 | Uses `windows` 0.62.2. |
| `wasapi` | 0.26.0 | Uses `windows` 0.62.2. **Wraps process loopback cleanly** (see D6), so no hand-written `ActivateAudioInterfaceAsync` code is needed. |
| `windows` | 0.62.2 | Same version as the two crates above, so there is only one copy in the build. Used directly for the window list. |

Edition 2024 (the `cargo new` default; CLAUDE.md requires ≥ 2021).

`.cargo/config.toml` was in a nested `lancast/.cargo/`, where cargo does not find it. It now sits in
the project root, and the `.gitignore` entry was changed to `/.cargo/config.toml`.

### D2. grafton-ndi 2.0 API (read from the crate source, `src/sender.rs`, `src/frames.rs`)

- `NDI::new()` → `Sender::new(&ndi, &SenderOptions::builder(name).clock_video(b).clock_audio(b).build())`.
  **Both clocks default to `true`** in the builder, so `clock_audio(false)` must be set explicitly.
- Video: `VideoFrame::builder().resolution(w,h).pixel_format(PixelFormat::BGRX).frame_rate(n,d).build()`;
  pixels are reached through `data_mut()`. The timecode defaults to `NDIlib_send_timecode_synthesize`.
- Async video: `send_video_async(frame) -> Option<VideoFrame>` takes ownership. It returns the
  *previous* frame once the SDK has released it, so double buffering is built in.
  `finish_video_async()` flushes and returns the last frame. It needs `&mut Sender`.
- Audio: `AudioFrame::builder().sample_rate().channels().samples().data(Vec<f32>)` is planar FLTP by
  default (`channel_stride = samples*4`). `.layout(AudioLayout::Interleaved)` converts interleaved data
  to planar at build time, so the WASAPI → FLTP conversion can be left to the crate.
- `send_audio(&self)` is synchronous. `connection_count(timeout)` wraps `NDIlib_send_get_no_connections`.
  `source()` returns the full network name.

### D3. NDI clocking and threading
- SDK documentation (`Processing.NDI.Send.h` l.43–48; *NDI SDK Documentation.pdf* p.18–19): "if you are
  submitting video and audio off a single thread, you should only clock one of them (video is probably
  the better choice…). If you are submitting audio and video of separate threads, then having both
  clocked can be useful."
- **Decision:** SPEC §4 plans a *single* NDI sender thread that takes frames from both channels. That
  fits `clock_video = true, clock_audio = false` (as in SPEC §5.3). `send_video_async` needs `&mut Sender`,
  which is another reason for a single owning thread.
- Async send buffer rules (PDF §13.1, p.21–22): the buffer "will continue to be used until a synchronizing
  API call is made" (the next async send, an async send with NULL, a sync send, or destroy). grafton-ndi
  enforces this through ownership (D2).
- Audio buffers: the PDF recommends audio frames "approximately half the length of the video frames"
  (p.69–70). At 60 fps that is about 8 ms, so batching to 10 ms WASAPI packets (D7) fits well.
- **Measured:** `ndi_testpattern` with async video plus audio from the same thread holds 60.0–60.2 fps
  at 1080p BGRX.

### D4. NDI source name
- PDF p.18: name "My Video" on machine "MyMachine" → "MyMachine (My Video)". Duplicate names on the same
  machine get a counter.
- **Measured:** `ndi_testpattern "lancast test"` → `MY-PC (lancast test)`.
- **Decision (confirmed 2026-10-17):** the source name is editable in the GUI. The proposed default is the
  **process name without `.exe`** (e.g. `game.exe` → `game`). The old SPEC proposal `<Hostname> – <process>`
  would show the hostname twice. The UI shows the resulting full name.

### D5. NDI license and distribution (SPEC §9, §6)
Sources: `NDI SDK License Agreement.pdf`, `Documentation/NDI SDK Documentation.pdf` §3, §4.2, §4.3, §6.1,
`Bin/x64/Processing.NDI.Lib.Licenses.txt` (all in the local SDK install).
- **Shipping `Processing.NDI.Lib.x64.dll` next to the exe is allowed and recommended.** §4.2: "You may
  distribute these files [Bin\*.*] within your application if your EULA terms cover the specific
  requirements of the NDI SDK EULA…". §3: "keep them in your application folders… Please do not install
  your NDI DLLs into the system path." Free products are explicitly allowed with no license fee.
- Required:
  - `Processing.NDI.Lib.Licenses.txt` must ship with the DLL.
  - A link to https://ndi.video/ near where NDI is used in the UI, on the website and in the docs.
  - "NDI® is a registered trademark of Vizrt NDI AB" at the first use and **in the About box**.
  - An end-user EULA with the clauses from License Agreement §3d: no reverse engineering, keep the
    notices, warranty/liability disclaimer, export compliance, NDI copyright notice.
  - Ship the latest SDK version available at distribution time (§2b).
  - Do not put "NDI" in the product name; "lancast – for NDI®" is fine (§3f, brand guidelines).
  - The NDI Tools must not be redistributed; a link to https://ndi.video/tools/ is fine.
- Alternative: the user installs the runtime from http://ndi.link/NDIRedistV6, and the runtime folder
  comes from env `NDI_RUNTIME_DIR_V6` (`Processing.NDI.Lib.h` l.51–53).
- **Decision:** ship the DLL plus `Processing.NDI.Lib.Licenses.txt` in the ZIP (SPEC §6). `build.rs`
  already copies the DLL to `target/<profile>/` and `target/<profile>/examples/`. M4 adds the license
  file, a short EULA, the notices in the README and the About text.
- Open: because `grafton-ndi` links the import library, Windows searches for the DLL only in the exe
  folder and on `PATH`. A fallback to `NDI_RUNTIME_DIR_V6` would need delay-loading or
  `SetDllDirectory` before the first NDI call. Not needed while we ship the DLL ourselves.

### D6. NDI|HX and bandwidth (SPEC §9)
- **HX sending is not possible with the standard SDK.** "For HX codecs (H.264 and H.265), the SDK provides
  decoding only" (https://docs.ndi.video/all/developing-with-ndi/developer-guides/decoding-with-ndi).
  Compressed sending needs the NDI Advanced SDK (`NDIlib_compressed_packet_t`), which requires a vendor ID
  from licensing@ndi.video (https://docs.ndi.video/docs/advanced-sdk/overview). Not pursued for v1.
- Full bandwidth (SpeedHQ2, 8-bit 4:2:2) at 1080p60: **132.14 Mbit/s** maximum
  (https://docs.ndi.video/all/getting-started/white-paper/bandwidth/ndi-high-bandwidth-based-on-speedhq2-8bit-4-2-2);
  the white paper says "up to 150 Mbps per stream". With alpha (SpeedHQ7) it is 165 Mbit/s, so we send
  **BGRX, not BGRA**.
- Consequence: a 1 GbE uplink on the central machine carries roughly 6 sources at 1080p60. Using
  720p/30 or lowering the max resolution (SPEC §5.1, config) helps proportionally. Plan the LAN
  accordingly; that sits outside this tool.

### D7. WASAPI process loopback (SPEC §5.2, §9)
- **Minimum version: Windows 10 build 20348.** Every reference page lists "Minimum supported client:
  Windows 10 Build 20348":
  - https://learn.microsoft.com/en-us/windows/win32/api/audioclientactivationparams/ns-audioclientactivationparams-audioclient_process_loopback_params
  - the same on the pages for `AUDIOCLIENT_ACTIVATION_PARAMS`, `AUDIOCLIENT_ACTIVATION_TYPE` and `PROCESS_LOOPBACK_MODE`.

  The ApplicationLoopback sample README says "requires Windows 10 build 20348 or later"
  (https://github.com/microsoft/Windows-classic-samples/blob/main/Samples/ApplicationLoopback/README.md).
  The `ActivateAudioInterfaceAsync` page says "20438", which is probably a typo. The 19041 claim comes
  only from community sources (a GStreamer commit), not from Microsoft. Build 20348 is Windows Server 2022.
  No consumer Windows 10 release (19041–19045) reaches it, so **in practice that means Windows 11 (≥ 22000)**.
- **Decision:** don't gate on the build number. Always try process loopback. If activation fails, fall
  back to system loopback with the UI hint (SPEC §5.2). On Windows 10 22H2 it may well work; that has to
  be tested on a Win10 machine.
- `GetMixFormat` / `IsFormatSupported` return E_NOTIMPL on this client (Microsoft Q&A,
  https://learn.microsoft.com/en-us/answers/a/1183153). `GetDevicePeriod` is not implemented either
  (wasapi docs). Request 48 kHz / 2 ch / f32 explicitly with `AUTOCONVERTPCM` (wasapi:
  `StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 }`). The official sample uses
  `LOOPBACK | EVENTCALLBACK | AUTOCONVERTPCM` with a buffer duration of 0.
- Threading: the completion handler is called on an MTA worker thread and must be agile
  (https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-activateaudiointerfaceasync).
  `wasapi::AudioClient::new_application_loopback_client(pid, include_tree)` blocks until activation
  finishes. The crate does **not** call `CoInitializeEx`, so the audio thread calls `wasapi::initialize_mta()`.
- `include_tree = true` (`PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE`). Pass the root of the tree
  (the browser or launcher main process). For a window, `GetWindowThreadProcessId` gives that PID.
- **Measured (`process_loopback`):**
  - Activation plus Initialize takes 2–3 ms.
  - The client delivers **10 ms packets continuously, even when the target renders nothing**: an idle
    explorer.exe gave 300 packets in 3 s, all zeros, without `AUDCLNT_BUFFERFLAGS_SILENT`. The docs do not
    specify this, so the code still handles both cases: on the SILENT flag it writes zeros (the buffer
    content is undefined then), and it pads with silence when it falls more than 50 ms behind the wall clock.
  - A PowerShell `SoundPlayer` as the target was recorded correctly (peak -9 dBFS).
  - Isolation from Discord has not been tested yet; that is the M2 acceptance test.

### D8. Windows Graphics Capture (SPEC §5.1)

| API | Minimum | Source |
|---|---|---|
| `IGraphicsCaptureItemInterop::CreateForWindow` | Win10 1903 (18362) | https://learn.microsoft.com/en-us/windows/win32/api/windows.graphics.capture.interop/nf-windows-graphics-capture-interop-igraphicscaptureiteminterop-createforwindow |
| `GraphicsCaptureSession.IsCursorCaptureEnabled` | Win10 2004 (19041) | https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.iscursorcaptureenabled |
| `GraphicsCaptureSession.IsBorderRequired` | build 20348 | https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.isborderrequired |
| `GraphicsCaptureSession.MinUpdateInterval` | Win11 24H2 (26100, per the docs' version moniker) | https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.minupdateinterval |

- **Pitfall in windows-capture 2.0.1:** a non-`Default` setting for an unsupported property makes
  capture start fail (`BorderConfigUnsupported`, `MinimumUpdateIntervalUnsupported`, …). Check
  `GraphicsCaptureApi::is_*_supported()` first and use `Default` otherwise, as `wgc_window` does.
- Border: per the docs, `IsBorderRequired = false` only takes effect after
  `GraphicsCaptureAccess.RequestAccessAsync(Borderless)` and the `graphicsCaptureWithoutBorder` package
  capability; otherwise it is "ignored". How this behaves for an unpackaged exe is not documented.
  windows-capture does not call `RequestAccessAsync`.
  - **Observed 2026-10-17 (Win11 build 26200, unpackaged exe):** `wgc_window` on Rocket League with
    `WithoutBorder` drew **no** yellow border. Without `RequestAccessAsync` and without a package
    manifest, `IsBorderRequired = false` is honoured for an unpackaged desktop app. The consent
    requirement in the docs evidently applies to packaged apps.
  - `wgc_window --border` sets `WithBorder` for the comparison test.
  - **Decision:** no `RequestAccessAsync` call. If a future Windows build starts ignoring the setting,
    the border just reappears, which is harmless.
- FPS cap, corrected 2026-10-17 after a measurement:
  - `MinUpdateInterval = 1/60 s` gave **exactly 48.0 fps** with Rocket League on a 144 Hz display.
    WGC delivers on the refresh grid (6.94 ms). Two ticks (13.9 ms) are shorter than 16.67 ms, so every
    frame waits for the third tick (20.8 ms), and 144/3 = 48.
  - **Decision:** `MinUpdateInterval` is only a coarse prefilter at **half the target period** (≥ 26100).
    The exact cap comes from a **deadline pacer** in our code:
    - The deadline advances by exactly one period per accepted frame.
    - Frames up to 1 ms early are accepted.
    - After falling behind by more than one period, the schedule restarts.
  - The pacer drops frames *before* `frame.buffer()`, so dropped frames cost no readback.
  - Measured: 60.0 fps, 12 of 72 delivered frames dropped per second, readback 1.1–1.4 ms at 2560×1440.
  - On older builds without `MinUpdateInterval`, the pacer alone does the job.
- WGC delivers frames only when the content changes, and none at all while the window is minimized
  (community knowledge: OBS, robmikh/Win32CaptureSample; Microsoft does not document it). Consequence
  for M1: the NDI thread repeats the last frame or sends nothing; NDI re-sends the last frame to new
  receivers anyway (PDF p.19).
- windows-capture recreates the frame pool itself when the window size changes
  (`graphics_capture_api.rs` l.244–258), so resizing works.
- **Measured (`wgc_window`, RustRover window at 2560×1392):**
  - 19–26 fps with changing IDE content.
  - GPU → CPU readback (`frame.buffer()` plus copying without padding) takes **0.9–5.6 ms per frame**.
    At 60 fps that could become the main CPU cost; M1 measures it with a real game. A GPU path
    (staging texture or D3D11 Video Processor for scaling) stays planned for v2.
- Exclusive fullscreen: Microsoft makes no official statement; keep the UI hint from the SPEC.

### D9. Window list (SPEC §3)
- windows-capture's `Window::enumerate()` does not filter cloaked windows, and `process_name()` opens the
  process with `PROCESS_VM_READ`, which fails for elevated or protected processes (often games with
  anti-cheat).
- **Decision:** our own `src/windows_list.rs`:
  - `EnumWindows`, `IsWindowVisible`, `DwmGetWindowAttribute(DWMWA_CLOAKED)` and a non-empty title.
  - Not owned (`GW_OWNER`), no `WS_EX_TOOLWINDOW`.
  - `GetWindowThreadProcessId`, then `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` and `QueryFullProcessImageNameW`.
  - Only the HWND goes to windows-capture (`Window::from_raw_hwnd`).
- **Measured:** `list_windows` shows exactly the Alt-Tab windows, including minimized ones (marked);
  hidden UWP frames do not appear.

### D10. Discovery, routing and firewall (SPEC §5.4)
Sources: SDK headers (`Processing.NDI.Send.h` l.36–62, `Processing.NDI.Find.h` l.41–50), *NDI SDK
Documentation.pdf* (ports table p.11, configuration p.12–14, Discovery Service p.61),
https://docs.ndi.video/tools/tools-for-windows/studio-monitor, DistroAV `src/ndi-source.cpp` and `src/ndi-finder.cpp`.

- **The sender has no target address.** `NDIlib_send_create_t` has only name, groups and the two clock
  flags. The sender advertises itself via mDNS (UDP 5353), and receivers connect to it.
  - In Studio Monitor you pick the source from the ☰ menu (top left) or with a right-click. Its "Web
    Control URL" is only for remote-controlling the monitor.
  - In DistroAV the "Source name" dropdown (editable) is filled by a finder with default groups.
- mDNS only reaches the local segment. For several subnets there are two options:
  - an **NDI Discovery Server** (`NDI Discovery Service.exe`, ships with the SDK and NDI Tools, TCP 5959)
  - "extra IPs" on the *receiver's* finder (Access Manager)
- **Configuring a discovery server on the sender**: the standard SDK has no per-instance API.
  `send_create_v2` and JSON config are Advanced SDK only ("when using the Advanced NDI SDK … all
  settings are per instance"). The standard way is `ndi-config.v1.json`:
  - in `C:\ProgramData\NDI\` (system-wide, written by Access Manager), or
  - in the folder named by env `NDI_CONFIG_DIR`, read when `NDIlib_initialize` runs.

  Schema: `{"ndi":{"networks":{"discovery":"192.168.1.10"}}}`. "Senders however will avoid using mDNS
  when a discovery server is configured."
- **Decision:** lancast writes its own config to `%APPDATA%\lancast\ndi\` and sets `NDI_CONFIG_DIR`
  before creating `NDI`. The system-wide ProgramData file stays untouched. grafton-ndi 2.0 only calls
  `NDIlib_send_create`, so this env-var route is the only one available.
- Transport: NDI 6 defaults to reliable UDP ("5960 and up UDP", one port per process). Source queries
  use TCP 5960. Multicast is off by default. Receivers connect *to* the sender, so the sender needs
  inbound firewall access. NDI only adjusts firewall rules itself "where the application has
  permissions".
- Groups: `NULL` gives "Public". DistroAV and Studio Monitor search with the default receive groups.
  **Decision:** set no group of our own (a "lancast" group would be invisible to them).
- Bandwidth "Lowest" is chosen by the receiver (`NDIlib_recv_bandwidth_lowest`, DistroAV "Bandwidth"
  property). It needs no sender code. That it costs extra sender CPU is an assumption, not documented;
  measure it.

### D11. GPU vendor capture SDKs and GPU path (SPEC §5.5)
- NVIDIA Capture SDK (https://developer.nvidia.com/capture-sdk, v9.0.0 is Linux-only): "NVFBC has been
  deprecated on Windows 10 and above… Last supported Windows 10 version is 1803". "NvIFR headers and
  samples are removed from the SDK" (https://developer.nvidia.com/capture-sdk-archive).
  - NvFBC captured the whole desktop.
  - NvIFR captured inside the game's D3D device, which would violate the no-injection rule anyway.
- ShadowPlay / Instant Replay (NVIDIA) and ReLive (AMD) are consumer features with no public API.
  - The NVIDIA Highlights SDK only lets a *game itself* mark clips, and NVIDIA calls it "a legacy SDK…
    no longer supported" (https://developer.nvidia.com/highlights).
- AMD AMF `DisplayCapture` (github.com/GPUOpen-LibrariesAndSDKs/AMF) captures a whole monitor (DXGI
  output index, Desktop Duplication backend) and only on AMD GPUs.
- NDI performance recommendations (docs.ndi.video, "Performance"):
  - "Use UYVY or UYVA color, if possible, as this avoids internal color conversions."
  - "If the data that you are sending to NDI is on the GPU, and you can have the GPU perform the color
    conversion before downloading it to system memory, you are likely to find that this has the best
    performance."
  - "Sending BGRA or BGRX video will incur a performance penalty."
  - SpeedHQ *encoding* runs on the CPU; GPU acceleration exists only for decoding.
- **Decision:** no vendor SDKs. v1 sends BGRX from the CPU readback. v2 (M5) converts BGRA→UYVY plus
  scaling in a D3D11 shader, with a staging ring, and halves readback traffic. Hardware encoders
  (NVENC/AMF/QSV) would only matter for NDI|HX via the commercial Advanced SDK.

### D12. UI stack: eframe, tray, preview, meter (SPEC §3)
Versions checked with `cargo info` on 2026-10-17.
- **eframe/egui 0.37.0** (MSRV 1.95, winit 0.30). Planned features: `default-features = false,
  features = ["glow", "default_fonts", "accesskit"]`, because glow is lighter than wgpu for a small
  tool window. If glow causes problems (egui #7401 reports high CPU use with pure glow), switch to wgpu.
- **tray-icon 0.26.1** (tauri-apps, menus via the re-exported `muda` 0.21.2). On Windows the tray needs
  a running event loop on the creating thread, so create it in the eframe creation closure on the main
  thread (official example `examples/egui.rs` in the tray-icon repo).
- **Hidden-window pitfall:** a hidden eframe window does not call `update()`, and
  `ViewportCommand::Visible` cannot reliably re-show it (egui #5096, #3655, #8459; #7776 reports CPU use
  while hidden).
  - **Decision:** handle tray and menu events in `set_event_handler` callbacks. Show or hide the window
    with Win32 `ShowWindow`/`SetForegroundWindow` on the HWND (from `CreationContext` via
    `raw-window-handle`).
  - Intercept the close button with `close_requested()` + `ViewportCommand::CancelClose`.
- Preview: egui has no BGRA image constructor. Downscale and swizzle to `Color32` in one pass on the
  capture thread, then `TextureHandle::set` at ≤ 15 fps (320×180 ≈ 230 KB, well under 1 ms).
- Audio meter: no maintained egui crate exists, so paint it with `egui::Painter`. Constants from OBS
  `frontend/components/VolumeMeter.cpp`:
  - range −60 dB; warning −20 dB; error −9 dB
  - peak decay 11.76 dB/s ("20 dB / 1.7 sec"); magnitude integration 0.3 s; peak hold 20 s
- Exe icon: `winresource` 0.1.34 (maintained fork of `winres`). Tray icon via
  `tray_icon::Icon::from_rgba` from an embedded PNG.
- Single instance: named mutex via `windows` (`CreateMutexW` + `ERROR_ALREADY_EXISTS`), no extra crate.

### D13. Minimum Windows version and graceful degradation (SPEC §5.0)
- Requirement (2026-10-17): Windows 10 is the minimum, and a minimal version should still work on older
  machines.
- **Decision:** the minimum is build 18362 (Win10 1903), because `CreateForWindow` exists from there
  (D8). Everything newer is detected at runtime and degrades (cursor setting 19041, border control
  20348, process loopback 20348 with system-loopback fallback, `MinUpdateInterval` 26100).
- Windows 10 paths cannot be tested on our hardware. They are implemented against the documentation,
  with defensive fallbacks and logging.
- Border: a user option, default off where supported (requirement 2026-10-17: the border may disturb
  players).

### D14. Stale system-wide discovery server (observed 2026-10-17)
- Symptom: `ndi_testpattern` ran and logged "NDI source created", but Studio Monitor and the NDI
  Discovery tool on the same PC showed "No Sources on your network". Firewall rules (inbound allow,
  Private and Public), the network profile (Private) and the listening ports (TCP 5960/5961) were all fine.
- Cause: `C:\ProgramData\NDI\ndi-config.v1.json` (last modified 2025-12-06, probably from an earlier
  event) contained `"networks": {"discovery": "10.0.18.19"}`. The server was unreachable (ping and TCP
  5959 failed; the LAN is 192.168.1.0/24).
  - With a discovery server configured, senders skip mDNS (D10), so the source was registered nowhere.
  - Studio Monitor reads the same file and also looked only at that server.
- Fix on the test machine: the user cleared the field in NDI Access Manager → Advanced. After that the
  source was visible.
- **Decision:** expect the same at LAN parties. Participants bring PCs with leftover NDI settings, and
  the failure is silent. M4 adds a startup check that warns about unreachable configured servers. It
  offers a lancast-only override via `NDI_CONFIG_DIR` and never modifies the system file (SPEC §5.4).

---

## M1 – 2026-10-17

### D15. Video pipeline and results
- Threads:
  - The WGC capture thread (windows-capture) runs the pacer (D8). It reads back and scales on the CPU,
    then `try_send`s into a bounded channel (2) to the NDI thread.
  - The NDI thread owns the `Sender` and calls `send_video_async`.
  - The frame the SDK releases goes back to the capture thread through a recycle channel (4).
  - Frames are `grafton_ndi::VideoFrame`s throughout. Capture writes straight into `data_mut()`, so
    there is no per-frame allocation and no copy beyond readback/scaling.
  - If the NDI thread lags, the capture thread checks `is_full()` *before* the readback and skips the
    frame (CLAUDE.md: drop, don't block).
- `VideoFrame` sets the picture aspect ratio to width/height. The builder default is 16:9, which would
  make receivers stretch non-16:9 windows.
- Downscaling: bilinear with fixed-point weights, rows in parallel via `rayon`. The output width is
  rounded to an even number. Unit tests cover `fit_size`, padding removal and colour preservation.
- `frame.buffer()` in windows-capture 2.0.1 creates a new staging texture each frame and maps it
  immediately, which stalls until the GPU copy is done. That is acceptable for v1; M5 replaces it with
  a staging ring.
- OS build via `RtlGetVersion`, which is manifest-independent, unlike `GetVersionEx`. Feature flags via
  `GraphicsCaptureApi::is_*_supported()` (`src/os_caps.rs`).
- **Measured** (Rocket League 2560×1440 → 1920×1080, Win11 26200, 16 logical cores, release build,
  received by `examples/ndi_probe` on the same PC):
  - capture 59.3–59.7 fps, send 59.1–59.9 fps, received 57.8–60.9 fps (per-second windows)
  - backlogged 0
  - readback + scaling 3.1–3.2 ms per frame
  - **CPU ≈ 171 % of one core ≈ 10.7 % of the machine**, including NDI SpeedHQ encoding of BGRX.
    That is just above the < 10 % guideline (SPEC §8); M5 (GPU scaling + UYVY) targets this.
- Robustness test with a Notepad window:
  - Minimize: logged, last frame stays on air.
  - Restore: works.
  - Resize to 890×495: the new size is picked up (`capture size changed`).
  - Close: `on_closed` fires, lancast logs "target window closed" and exits cleanly by itself.
- Ctrl-C: confirmed by the user (2026-10-17). The session stops cleanly ("stopping: interrupted").
- Privacy finding from that test: a window title containing private text ended up in `lancast.log`.
  **Decision:** titles are logged at debug level only (SPEC §4.5).

---

## M2 – 2026-10-17

### D16. Audio pipeline and results
- `src/capture/audio_wasapi.rs`: its own thread with `initialize_mta()`, then
  `new_application_loopback_client(pid, include_tree = true)`. If that fails, it falls back to loopback
  of the default render device. `--system-audio` forces system loopback, `--no-audio` turns audio off.
  - Format: 48 kHz / 2 ch / f32, `EventsShared { autoconvert: true, buffer_duration_hns: 0 }` (D7).
  - Output: continuous 10 ms chunks (480 frames, interleaved), sent to the NDI thread through a bounded
    channel of 50 (500 ms), dropped when full.
  - Wall-clock correction: if capture is more than **50 ms** short, it pads with silence; if more than
    **100 ms** ahead, it trims the oldest samples. Synthesized NDI audio timecodes follow the sample
    count, so this bounds drift between the device clock and the wall clock.
  - Peak and RMS per channel and chunk go into atomics (`AudioLevels`) for the M3 meter.
  - No `unsafe` needed; the `wasapi` crate wraps everything.
- NDI thread: `crossbeam select!` over video and audio. After a (clocked, blocking) video send it drains
  the queued audio. Audio goes out via `AudioFrame` with `AudioLayout::Interleaved`, which the crate
  converts to FLTP.
- Observation: **system loopback delivers no packets at all while nothing plays** (5.2 s padded in 6 s).
  Process loopback, by contrast, delivers continuous zero packets (D7). The padding covers both.
- **Measured: A/V sync** (`ndi_probe --av`, positive = audio late):
  - Calibration with `ndi_testpattern` (pure NDI): **0.0 ms** by timecode. By arrival, audio comes
    about 27 ms *before* video; that is NDI's own video pipeline.
  - End-to-end `av_sync_source` → lancast (process loopback) → probe: **+23 to +40 ms** by timecode
    (mean +28.4 ms over 16 flashes), +17 to +25 ms by arrival. The source schedules its beep by
    *playback* time, while loopback captures at mix time.
  - ITU-R BT.1359 puts the detectability limit for late audio at about 125 ms, so this is imperceptible.
  - **Decision:** keep the synthesized timecodes.
  - Possible later improvement if needed: explicit timecodes from QPC on both paths (WGC
    `SystemRelativeTime`, WASAPI `BufferInfo` QPC timestamp), shifted to UTC.
- **Measured: isolation** (`av_sync_source --silent` captured by lancast while another process plays
  `Alarm01.wav`):
  - `--system-audio`: peak -10 to -16 dBFS, the foreign sound is audible.
  - Process loopback: **-200 dBFS (digital zero)**, the foreign sound is not in the stream.
  - This is the technical stand-in for "Discord not audible" (SPEC §8); the Discord test itself remains
    manual.
- **Measured: Rocket League:**
  - Video stays at 59.9–60 fps, and lancast CPU is 10.4 % of the machine, i.e. audio costs practically
    nothing.
  - The audio stream was continuous (no padding) but digital silence. Probably the game mutes itself
    in the background, since it was out of focus during the test. Check by hand with the game focused.

### D17. NDI audio level convention (observed 2026-10-17)
- Symptom: the user recorded lancast in NDI Studio Monitor and the game audio was audible but very quiet.
- NDI SDK documentation §21.2 (p.69): "a sinewave that is 2.0 floating-point units peak-to-peak (i.e.,
  -1.0 to +1.0) is assumed to represent an audio level of +4 dBU". The SMPTE recipe maps NDI **10.0** to
  digital full scale, giving "20 dB of headroom above the SMPTE reference level". The SDK's 16-bit
  conversion uses `3276.8 * sample`.
- lancast sent WASAPI samples 1:1 (full scale = 1.0), so a convention-following receiver such as Studio
  Monitor plays them **20 dB too quiet**.
- DistroAV (OBS), `ndi_source_thread_process_audio3` in `src/ndi-source.cpp`, hands `p_data` unchanged
  to OBS as `AUDIO_FORMAT_FLOAT_PLANAR`. In OBS, 1.0 = 0 dBFS, so 1:1 is correct there, and ×10 would be
  20 dB too loud and clip.
- **Decision:** unity gain by default, because OBS + DistroAV is the planned receiver. The gain is
  configurable: CLI `--audio-gain-db`, applied in the NDI thread just before sending; the meter shows the
  unscaled source level. M4 adds a setting "Receiver level: OBS / NDI Studio Monitor (+20 dB)".
- Verified: an `av_sync_source` beep at -20 dBFS arrives at `ndi_probe` as 0.0 dB (float 1.0) with
  `--audio-gain-db 20`.
- **Resolume** may also be the receiver (requirement 2026-10-17). It has no official documentation on NDI
  audio levels.
  - Resolume forum posts (about 2016–2024) say NDI audio *input* is not supported: staff "not supported
    currently", feature ticket; the only audio input is the external FFT input
    (https://resolume.com/forum/viewtopic.php?t=20627, https://resolume.com/forum/viewtopic.php?t=18627).
  - The streaming support page (https://resolume.com/support/en/streaming) does not mention audio.
  - The convention conflict is known elsewhere too (vMix forum,
    https://forums.vmix.com/posts/t21599-NDI-output---audio-levels-low).
  - **Open:** check with the current Resolume version whether NDI audio arrives at all, and at which
    level. If it doesn't, game audio must reach the PA another way (event setup, outside lancast).

---

## M3 – 2026-10-17

### D18. GUI architecture: controller thread, own tray, eframe 0.37
- **eframe 0.37 API change:** `App::ui(&mut self, ui, frame)` replaces `update`. A new `App::logic`
  also runs while hidden when `request_repaint` is called. Panels use `Panel::top/bottom(..).show(ui, ..)`
  (`show_inside` is deprecated). The renderer is glow (`default-features = false`).
- **Threads:**
  - The **controller** owns the `Session` (shared with the CLI, `src/session.rs`), runs the pre-flight
    check, polls the session every 250 ms and publishes `AppState` (`Arc<Mutex>`).
  - The **UI** only renders `AppState` and sends `Command`s.
  - The **tray** has its own thread.
  - So closing the window, notifications and tray state work while the window is hidden; this sidesteps
    the hidden-window issues listed in D12.
- **Tray: own `Shell_NotifyIconW` implementation instead of the `tray-icon` crate.** `tray-icon` has no
  API for balloon/toast notifications, which SPEC §3.1 item 6 requires. A small Win32 module
  (`src/tray.rs`, all `unsafe` there) is enough:
  - a hidden window (not message-only, because the context menu needs a foreground-capable window)
  - `NOTIFYICON_VERSION_4`: left click arrives as `NIN_SELECT` and right click as `WM_CONTEXTMENU`
  - re-adding the icon on `TaskbarCreated`, i.e. after an Explorer restart
  - icons drawn in code (circle per state), so no asset files are needed
  - `NIN_KEYSELECT` is missing from windows-rs 0.62 and is defined locally as `NIN_SELECT | 1`
    (shellapi.h).
- **Meter:** the UI samples at about 30 fps while audio arrives at 100 chunks/s, so `AudioLevels` keeps
  the maximum peak since the last read via a lock-free `fetch_max` on the f32 bits (valid for
  non-negative floats). Ballistics are as in D12.
- **Fonts:** egui's default fonts lack ⟳ → ▶ ■ ⚠ ✔ (rendered as boxes; eframe logs "No font has …"), so
  labels are plain text ("Refresh", "Send", "WARNING:").
- **Quit:** eframe does not return while its window is hidden, so the controller exits the process
  after stopping the session cleanly and removing the tray icon.
- **Testing without a human at the screen:**
  - eframe/AccessKit exposes widgets to Windows UI Automation (buttons with InvokePattern, the window
    with WindowPattern).
  - Combo boxes cannot be expanded through UIA, so the test simulated mouse clicks on them.
  - Tray icons are visible as `SystemTray.NormalButton`. Their context menu items are not visible to
    UIA, so the test used the keyboard.
  - Screenshots via `PrintWindow(PW_RENDERFULLCONTENT)` of the lancast window only.
- **Verified by those tests (Win11 26200):**
  - The GUI starts and the window list is filled.
  - **The window list refreshes when opened** (a window started after lancast appeared). This was a real
    usability bug found by the test.
  - Pre-flight "game only (checked)"; Send; the probe receives video plus -20 dBFS audio.
  - Preview shows the window; the meter shows -20 dB with peak hold and the activity dot.
  - Closing hides to the tray while the stream continues. The tray tooltip shows the source, resolution,
    fps and receivers, and a left click on the tray icon restores the window.
  - Target window closed → session stops, tray goes red ("stopped (see window)").
  - Tray menu Quit and the Quit button exit cleanly.
  - With `LANCAST_SIMULATE_NO_PROCESS_LOOPBACK=1`:
    - amber warning banner after selection, with "Send with system audio" / "Send without audio" instead
      of "Send"
    - after choosing system audio: a permanent amber banner, and the tray tooltip says "– SYSTEM audio"
  - The CLI still works in the GUI-subsystem exe (`--list`, `--help`, piped output).
- **Not verified automatically:** how the Windows toast notifications look (Focus Assist may suppress
  them), the amber tray icon colour, and the tray menu's Start/Stop items. Left for a manual check.

### D19. M3 refinements (2026-10-17): name, carousel, tray behaviour, icon
- **Name:** the program is called **LANcast**. A `[[bin]] name = "LANcast"` target produces
  `LANcast.exe` (`#![allow(non_snake_case)]` for the crate name). The window title, tray texts,
  notifications and `LANcast.log` use the same name. Package, crate and repository stay `lancast`.
  - Pitfall: NTFS is case-insensitive, so deleting `lancast.exe` also deletes `LANcast.exe`.
- **Carousel instead of a dropdown** (SPEC §3.1 item 2):
  - Live thumbnails via DWM (`DwmRegisterThumbnail` / `DwmUpdateThumbnailProperties`), the Alt-Tab
    mechanism. They are composited by Windows: no CPU cost, and **nothing is sent to the game process**.
    The alternatives (`PrintWindow` sends WM_PRINT to the target; one WGC frame per window flashes the
    capture border) were rejected.
  - Thumbnails are overlays on our window. LANcast computes their destination rectangle in physical
    pixels (egui points × `pixels_per_point`), clips them to the carousel area and crops the source
    rectangle accordingly. `DWM_THUMBNAIL_PROPERTIES` is `packed(1)` in windows-rs, as in dwmapi.h.
  - Icons come from the exe file via `SHGetFileInfoW` → `GetIconInfo` → `GetDIBits`, again without
    touching the process. Old icons without alpha are treated as opaque.
  - Testing pitfall: a background process cannot bring a window to the foreground, so screenshots
    via `CopyFromScreen` showed other windows on top at first. The window is now set topmost for the
    screenshot. Thumbnails do not appear in `PrintWindow` captures.
- **Close / minimize to tray (default on), Always on top (default off), no Quit button:**
  - eframe does not run `App::ui` for a minimized window, so the minimize check in the UI never fired
    (found by test). The **tray thread now checks `IsIconic` every 250 ms** and hides the window.
  - With the option off, × quits.
  - Always on top uses `ViewportCommand::WindowLevel`.
  - Verified: minimize → hidden; × with the option off → process exits.
- **Capture border on by default** (user requirement). The CLI flag changes from `--border` to
  `--no-border`.
- **Icon:** the user's design (`assets/icon-source.png`). `tools/make_icons.py` (Pillow) crops the
  transparent margin and produces PNGs at 16–64 px plus a multi-size `LANcast.ico` (16–256).
  - `build.rs` embeds the .ico and version info with `winresource` 0.1.34. Verified with
    `ExtractAssociatedIcon` and the `VersionInfo` ProductName.
  - Window and tray decode the embedded PNGs with `png` 0.18, which is already in the tree through
    eframe.
  - The tray state is a coloured dot in the lower right corner instead of the previous coloured circles.
- Known cosmetic gap: emoji in window titles (e.g. Discord channel names) render as boxes with egui's
  default fonts.

### D20. M3 refinements, round 2 (2026-10-17)
- **Carousel physics:**
  - The position is continuous (fractional tile index).
  - The mouse wheel or touchpad (`smooth_scroll_delta`) moves it by `delta × 1.5 / tile step`.
  - Dragging with `Sense::click_and_drag` on the whole strip moves it by `drag_delta`. On release the
    momentum comes from the pointer velocity and decays exponentially (friction 4/s).
  - Below 0.15 tiles/s, and 150 ms after the last wheel input, it eases onto the nearest tile (or the
    arrow/click target).
  - Clicks are mapped to the tile under the pointer. egui-winit turns single-finger touch into pointer
    events, so dragging also works on touchscreens; touchpads send smooth scroll deltas.
  - Verified with simulated input: 3 wheel notches and a 300 px drag each moved the carousel to another
    window.
- **Preview before sending:**
  - `session::Preview` runs `VideoCapture` without an NDI channel (`frames: None`) at 15 fps and reads
    back **only while the preview is visible**. `AudioCapture` without a channel only measures levels.
  - The controller starts it on selection, restarts it when video options change, replaces it on
    "Send", resumes it on "Stop", and drops it with the selection when the window closes.
  - Verified: selection → `preview started`; the screenshot shows the preview and meter.
- **Layout:**
  - Bottom panels: footer, then controls (source name, status, Send/Stop, Settings).
  - The central panel holds the carousel, and the preview fills the rest at the image's aspect ratio,
    centered, next to the meter, so it scales with the window.
  - Default window size 933×1000 (adjusted 2026-10-17: 1400 wide was ⅓ too wide, 800 high ¼ too low), minimum 600×680.
  - Settings is a framed `Frame::group` with a 3-column grid (Window / Capture / Video / Audio). The
    "Advanced" fold-out is gone.
- **Icon v2:** the user's updated design (`Gaming-Streaming-App-Symbol.png`) has **no alpha channel**
  (RGB with a black background). `tools/make_icons.py` now makes the outer black transparent by flood
  fill from the four corners (tolerance 12, then a 0.8 px blur for soft edges). Black inside the shape
  is not affected. Verified on light and dark backgrounds and via the exe icon. Explorer may show the old
  icon until its icon cache refreshes.
- **Tournament mode (idea, user question):** send the stream several seconds late, so the projector
  gives the opposing team no live information.
  - Feasible in LANcast: a ring buffer between capture and NDI send, with audio delayed by the same
    amount in samples. NDI timecodes stay synthesized at send time, so A/V sync is unaffected.
  - The limit is **RAM**, since uncompressed frames are large:
    - 1080p60 BGRX is 8.3 MB/frame, about 5 GB for 10 s.
    - 1080p30 UYVY (M5 path) is 4.1 MB/frame, about 1.2 GB for 10 s.
    - 720p30 BGRX is about 1.1 GB for 10 s.
  - Practical options:
    - (a) Delay mode with a lower rate/resolution, e.g. a fixed 720p30, and a RAM-based maximum delay
      shown in the UI.
    - (b) Compress in the buffer (hardware H.264 via Media Foundation, then decode before sending).
      This allows minutes of delay but is noticeably more work.
    - (c) Delay centrally on the receiving machine. One place for all players, but it depends on the
      receiver software's features.
  - Not scheduled.

### D21. Preview quality and cost (2026-10-17)
- Symptom: the big preview looked blocky. A fixed 320×180 image, made by nearest-neighbour downscaling,
  was stretched to about 1040×585 in a large window.
- **Fix:**
  - The UI reports the displayed size in physical pixels (`PreviewSlot::set_display_size`, capped at
    1920×1080).
  - The capture produces the preview at exactly that size, with an **area-average filter**: each
    output pixel is the mean of the source pixels it covers. Rows run in parallel via rayon, and the
    filter reads padded readback rows directly.
  - Refresh interval 50 ms (about 20 fps). egui displays the image 1:1, so nothing is upscaled.
- **Cost (Rocket League 2560×1440, preview about 1030×580 physical, before sending):**
  - First version: 30 fps preview capture, with each frame first scaled to the NDI size (1920×1080) and
    then reduced → **83 % of one core**.
  - Preview-only mode now builds the image **straight from the GPU readback**, without the NDI-sized
    frame, and captures at 20 fps (the preview rate) → **44 % of one core (≈ 2.8 % of 16 threads)**, at
    the same visual quality.
  - Nothing is read back while the window is hidden or in the tray.
  - While sending, the preview is still made from the NDI frame, which exists anyway.

### D22. Carousel with four tiles, locked while casting; build fixes (2026-10-17)
- **Four tiles:** with an even count there is no center tile, so `position` is now the continuous index
  of the **leftmost** visible tile. Like Alt-Tab, the most recent window is on the left. A click
  selects without moving the carousel.
- **Locked while casting:**
  - The strip senses hover only, and the wheel and drag are ignored. The arrows are greyed out via
    `add_enabled_ui`.
  - On locking, the cast window is scrolled into view once (shortest way).
  - The cast tile gets a red frame and a LIVE badge.
  - The other tiles get a dark fill and grey text, and their DWM thumbnails `opacity = 70/255` (the
    `opacity` field of `DWM_THUMBNAIL_PROPERTIES`).
  - The lock note sits in the heading, because DWM thumbnails are composited **above** egui content and
    would hide any overlay text on the tiles. The padlock is drawn with shapes, since egui's default
    fonts have no glyph for it.
  - Verified: a click on another tile while casting does not change the selection.
- **Default window width** 1400 (was 700).
- **Build pitfalls found:**
  - A running `LANcast.exe` blocks the replacement of the exe, so cargo fails with "failed to remove
    file". A shortened output (`grep … | head`) once hid that error and an old exe was reported as
    rebuilt. Since then the exe timestamp is checked after every build.
  - `build.rs` now skips copying the NDI DLL if the copy already exists with the same size and is not
    older. This avoids "failed to copy … (os error 32)" while a running LANcast has the DLL loaded.

---

## M4 – 2026-10-17

### D23. Settings, data folder, NDI discovery, single instance, release package
- **Data folder (user question: "Windows best practice?"):**
  - Microsoft's guidance: no writing into the program folder (`Program Files` is read-only anyway).
    Per-user, machine-specific data goes in `%LOCALAPPDATA%`; roaming data in `%APPDATA%`.
  - LANcast's settings, NDI config and logs are machine/network-specific, so everything lives in
    `%LOCALAPPDATA%\LANcast\`.
  - Opt-in **portable mode:** a `LANcast.toml` next to the exe redirects everything there.
  - Earlier drafts wrote next to the exe, or to `%APPDATA%` as a fallback; that is replaced.
- **Settings** `LANcast.toml` (serde + toml 1.1):
  - All fields are optional (`#[serde(default)]`) and clamped to the UI ranges.
  - An invalid file is renamed to `.bak`.
  - Writes go through a temp file, then a rename. Saves are debounced (0.7 s); the source name per exe
    is saved on "Send".
  - Verified: file written with the expected content, source name remembered.
- **NDI discovery:**
  - `NDI_CONFIG_DIR` is set exactly once, at the start of `main`, before any thread exists:
    `set_var` is `unsafe` in edition 2024 because it races with other threads.
  - Before each session, a copy of `C:\ProgramData\NDI\ndi-config.v1.json` with an adjusted
    `ndi.networks.discovery` is written: own server, `""` for "Ignore it for LANcast", or unchanged.
    grafton-ndi destroys the runtime when the last handle drops, so each session re-reads the file.
  - **Verified end to end:** with `discovery_server = "10.255.255.1"` (unreachable) an mDNS receiver
    (`ndi_probe`) does **not** find the source; with an empty value it does. So the SDK honours
    LANcast's file, and a configured server disables mDNS announcing (as documented, D10).
- **Stale system discovery server (D14):**
  - At startup the system value is read and checked in the background with a TCP connect to port 5959
    (1.5 s timeout).
  - If it is unreachable and not overridden, an amber banner offers "Ignore it for LANcast", which is
    persisted in the settings.
  - The network row of the settings shows the system value and its status, plus LANcast's own
    discovery server field with a reachability check when editing ends.
  - Verified with a fake `ProgramData` (10.0.18.19) for the test process only: banner shown, button
    removes it, the setting is saved. The real system file was not touched.
- **Single instance:**
  - A named mutex `Local\LANcast.SingleInstance` (per user session).
  - A second start finds the tray window (class `lancast_tray`), calls `AllowSetForegroundWindow`
    for that process and posts `WM_TRAY_ACTIVATE`. The first instance shows its window, even from
    the tray.
  - Verified: the second instance exits; the first becomes visible and foreground.
  - CLI mode is not limited.
- **Release package:**
  - `tools\package.ps1` builds, checks that the exe is fresh, and stages exe, NDI DLL,
    `Processing.NDI.Lib.Licenses.txt`, `README.txt` (from `packaging\`, with the version) and
    `EULA.txt` into `dist\LANcast-<version>-win64.zip`.
  - It refuses to run while LANcast is running. It did refuse once, correctly, when a LANcast was
    started during packaging.
  - The **EULA is a draft.** It contains the clauses required by NDI SDK License Agreement §3d:
    no modification, no reverse engineering (including protocols), no circumvention, notices kept,
    warranty and liability disclaimers on behalf of NDI, US export compliance, copyright notice.
    The copyright notice follows `Processing.NDI.Lib.Licenses.txt` ("Copyright (C) 2023-2026 Vizrt NDI
    AB"), while §3d itself still says "NDI, Inc.". Have it reviewed before public distribution.
- **Default window size** 933×1000 (user: ⅓ narrower, ¼ higher than 1400×800).
- The footer shows the version and links to the data folder.

### D24. License, window geometry, release check (2026-10-17)
- **License:** LANcast is **Apache-2.0**, copyright **Kay Huber** (user decision).
  - `LICENSE` is the official text with the appendix, taken verbatim from a crate's `LICENSE-APACHE`
    (201 lines).
  - `NOTICE` names the holder and points out that the NDI runtime is *not* covered.
  - `Cargo.toml` has `license`; the exe version info has `LegalCopyright`.
- **EULA restructured:** part 1 is LANcast under Apache-2.0, which permits modification. Part 2 holds
  the NDI §3d clauses, which apply only to the NDI components. Parts 3 and 4 are the disclaimers.
  Still a draft for review.
- **Third-party licenses:** `tools/third_party_licenses.py` walks `cargo metadata` (normal dependencies
  only; build-time tools like bindgen are not shipped).
  - Result: **150 crates**, with their license files and repeated texts deduplicated.
  - Licenses: mostly MIT/Apache-2.0 dual, plus Zlib, BSD-3, Unlicense, and OFL-1.1 / Ubuntu Font
    Licence for egui's bundled fonts (from the `fonts/` folder).
  - 12 crates ship no license file; they are all used under Apache-2.0, whose text is included.
  - The script writes UTF-8 itself, because a pipe through PowerShell 5.1 would re-encode it.
- **Package contents** (`dist\LANcast-0.1.0-win64.zip`, 7.5 MB): `LANcast.exe`,
  `Processing.NDI.Lib.x64.dll`, `Processing.NDI.Lib.Licenses.txt`, `README.txt`, `EULA.txt`,
  `LICENSE.txt`, `NOTICE.txt`, `THIRD-PARTY-LICENSES.txt`. Verified: unpacked into an empty folder,
  LANcast starts.
- **Window geometry:**
  - Inner size and outer position (points) plus the maximized flag are saved in `LANcast.toml`; while
    maximized, the normal geometry is kept.
  - A saved position is only applied if its title-bar area lies on a connected monitor
    (`MonitorFromRect`, `MONITOR_DEFAULTTONULL`).
  - Default inner size **1050×980**.
  - Verified: moved to (200,120) and resized to 1134×861, quit, restarted → identical.

### D25. Installer (Inno Setup) (2026-10-17)
- **Choice:** Inno Setup 6 (`setup.exe`) instead of WiX/MSI. It is simple and common for tools; MSI only
  pays off for company rollouts (GPO/Intune). The ZIP stays as the portable option.
- **Script:** `packaging/LANcast.iss`, built by `tools/package.ps1` when ISCC is present. Installed on
  the dev machine with `winget install JRSoftware.InnoSetup --scope user` (6.7.3).
  - Fixed AppId `{F98C4151-0F3E-495C-B8D3-B823EE2D71F2}`; it must never change (updates/uninstall).
  - Default "for all users" (`PrivilegesRequired=admin`): installs to `Program Files` and offers a
    **firewall task**, checked by default. It deletes, then adds, an inbound rule "LANcast" for
    `LANcast.exe` (`profile=private,domain`).
  - `PrivilegesRequiredOverridesAllowed=dialog` allows "for me only" without admin, into
    `%LOCALAPPDATA%\Programs\LANcast`. That mode has no firewall task (`Check: IsAdminInstallMode`).
  - The uninstaller removes the firewall rule (admin mode). Settings and logs in
    `%LOCALAPPDATA%\LANcast` are kept.
  - Other features: EULA page, start menu entry, optional desktop shortcut, `MinVersion=10.0.18362`,
    x64 only, `CloseApplications=yes` (a running LANcast is closed for updates), English and German
    setup UI.
  - The final "Start LANcast" runs with `runasoriginaluser`, so LANcast does not run elevated like the
    setup.
- **Verified:** silent per-user install (`/CURRENTUSER /VERYSILENT`) installs all 8 files, the start menu
  entry and the uninstall entry ("LANcast 0.1.0 by Kay Huber") and offers no firewall task. The silent
  uninstall removes them again.
  - Not verified automatically: the admin install with the firewall rule, which needs UAC → manual test.
- **Unsigned:** SmartScreen warns about the setup as well. Only a code-signing certificate helps (e.g.
  Azure Trusted Signing, or a classic certificate). That is not part of this change.

### D26. User name in the stream name (2026-10-18)
- **Requirement:** the organizers must see who is casting. The format is **`Hostname (Username - Gamename)`**.
- NDI prepends the hostname itself (D4), so LANcast sets the source name to `Username - Gamename`
  (`config::source_name`; with an empty part, only the other one is used).
- The user name defaults to the Windows login name (`%USERNAME%`), is editable ("Your name"), and is
  saved globally in `LANcast.toml` (`user_name`).
- The game name is proposed from the window's process, is editable ("Game"), and is remembered per exe
  as before.
- Verified: the receiver sees `MY-PC (<login name> - av_sync_source)`.

### D27. Event links (`lancast://`) (2026-10-18)
- **Requirement:** an event must hand out the same settings to every participant without anyone
  typing them in. The settings are the NDI gateway (discovery server), a maximum resolution allowed
  (bandwidth), and other defaults.
- **Design:** a link on the event website, e.g.
  `lancast://config?event=NetGame%202026&discovery=10.0.0.5&max=1280x720&fps=60&level=unity`.
  - Keys: `event` (display name) and `discovery`.
  - **Limits** (enforced; the user can only go lower): `max`, `fps`.
  - **Defaults** (applied once): `level`, `border`, `cursor`.
  - Unknown keys are ignored; invalid values reject the whole link (no half-applied link). The length
    is capped at 2 KB, and discovery entries are restricted to plain `host[:port]` characters.
- **Always confirmed:** a modal dialog lists what the link sets; "Apply" or "Cancel". A link never
  starts sending, and the system NDI configuration is never touched. So the worst a malicious link can
  do is propose settings the user sees first.
- **Registration:**
  - The installer writes `HKA\Software\Classes\lancast` (HKLM for all users, HKCU per user) and
    removes it on uninstall.
  - The ZIP/portable version registers itself in HKCU at startup if no registration exists or the
    registered exe is gone (`src/url_scheme.rs`; no admin needed).
- **Delivery to a running LANcast:**
  - The browser starts `LANcast.exe "lancast://…"`.
  - That second instance finds the first instance's tray window and sends the link with `WM_COPYDATA`
    (marker `dwData`, max 8 KB, UTF-16), then exits.
  - The first instance shows its window and the dialog.
- **Stored:** `event_name` and `event_max_width/height/fps` in `LANcast.toml`. The settings panel shows
  "Event: …" with **Leave event**, which removes the name, the limits and the discovery server.
  - The resolution list offers only presets within the limit (plus the limit itself), and the frame
    rate is capped.
- **Verified end to end:**
  - Starting LANcast registered `lancast:` for the user.
  - `Start-Process "lancast://config?event=Test%20LAN%202026&discovery=&max=1280x720&fps=30&level=unity&border=1"`
    started a second instance that handed the link over and exited.
  - The dialog showed all values; Apply saved the event name and the limits (fps 30, 1280×720).
  - Leave event removed them.
- Notes for the event website:
  - Browsers ask "Open LANcast?" before starting it.
  - Chat apps (e.g. Discord) usually do not make custom schemes clickable, so link to a web page with
    the button instead.
  - Participants must have run LANcast once (ZIP), or installed it.

### Open items after M0
- Check visually: test pattern and tone in NDI Studio Monitor (pick the source from the ☰ menu).
  Also check the yellow border during `wgc_window`.
- Process loopback on Windows 10: no test machine, so built against the docs (D13).
- Discord isolation and lip sync: M2.
- Resolume as receiver: does it accept NDI audio, and at which level convention (D17)?
