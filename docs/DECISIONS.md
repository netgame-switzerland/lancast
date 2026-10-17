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
- **Decision:** the default source name is the **process name only** (e.g. `game.exe` → `game`). SPEC §3's
  `<Hostname> – <process>` would show the hostname twice.

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
  windows-capture does not call `RequestAccessAsync`. **To check visually:** does the border appear
  during `wgc_window` on Win11? If it does, accept it (SPEC allows that) or call `RequestAccessAsync`
  in M1.
- FPS cap: on ≥ 26100 we use `MinUpdateInterval = 1/60 s`. On older systems we drop frames in our own code.
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

### Open items after M0
- Check visually: test pattern and tone in NDI Studio Monitor (needs NDI Tools on a receiver).
  Also check the yellow border during `wgc_window`.
- Process loopback on a real Windows 10 22H2 machine (D7).
- Discord isolation and lip sync: M2.
