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

### Open items after M0
- Check visually: test pattern and tone in NDI Studio Monitor (pick the source from the ☰ menu).
  Also check the yellow border during `wgc_window`.
- Process loopback on Windows 10: no test machine, so built against the docs (D13).
- Discord isolation and lip sync: M2.
