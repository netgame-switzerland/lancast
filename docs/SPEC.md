# lancast – Specification

Findings, sources and the reasoning behind decisions are in `docs/DECISIONS.md` (referenced as D<n>).

## 1. Goal
A participant launches a single exe, picks their game window and clicks "Send". An NDI® source then appears on the LAN, carrying the window image and **only the game's audio** (no Discord, no system sounds). No installation, no configuration, and no OBS on the participant's machine.

The app shows a small live preview and an audio level meter, so the participant can see that picture and sound are actually being captured. It lives in the system tray while sending.

## 2. Non-goals (v1)
- No receiving and no projector control. The central machine handles that with OBS + DistroAV or NDI Studio Monitor.
- No webcam, no microphone, no overlays.
- No own encoding and no recording. The NDI SDK compresses (SpeedHQ, full bandwidth).
- No NDI|HX (H.264/H.265). The standard SDK cannot send it; that needs the commercial Advanced SDK (D6).
- No macOS or Linux.

## 3. User flow and UI

### 3.1 Main window (egui)
```
┌─ lancast ──────────────────────────────────────────────┐
│ Window:  [ Warcraft III (war3.exe)            ▼] [⟳]   │
│ Source name: [ war3            ]  → MY-PC (war3)     │
│                                                        │
│ ┌──────────── preview 320×180 ───────────┐  ┌─┐ ┌─┐    │
│ │                                        │  │█│ │█│ 0  │
│ │          (live miniview)               │  │█│ │█│-20 │
│ │                                        │  │█│ │█│-60 │
│ └────────────────────────────────────────┘  └─┘ └─┘    │
│                                              L   R     │
│ Audio: Game only (process loopback)  ● active          │
│ 1920×1080 · 60.0 fps · 2 receivers                     │
│                                                        │
│ [ Send ]   ☐ Show capture border  ☐ Capture cursor     │
│ ▸ Advanced (discovery server, FPS cap, max resolution) │
└────────────────────────────────────────────────────────┘
```
1. The participant launches `lancast.exe`. The main window opens and the tray icon appears.
2. **Window list**: visible top-level windows with title and process name, plus a "Refresh" button. Implementation: `src/windows_list.rs` (D9).
   1. Enumerate windows with `EnumWindows`.
   2. Keep visible, non-cloaked, unowned, non-tool windows that have a title.
   3. Get the PID with `GetWindowThreadProcessId` and the name with `QueryFullProcessImageNameW`.
   4. Create the capture item from the HWND.

   The WGC system picker (`GraphicsCapturePicker`) is not used, because it returns neither the HWND nor the PID.
3. **Source name**: editable.
   - It is pre-filled with a proposal: the process name without `.exe` (e.g. `war3`). The user may also pick the window title instead.
   - NDI prepends the machine name itself (`MY-PC (war3)`, D4). The UI shows the resulting full name next to the field.
   - The last name used per process is remembered in the config file (§4.6).
4. **Send / Stop**: "Send" starts streaming and "Stop" ends it. If the target window closes, sending stops automatically and a notice appears (also as a tray notification).
5. **Preview (miniview)**: a live preview of what is sent.
   - About 320×180, letterboxed, refreshed at 10–15 fps. It is shown only while the window is visible.
   - When no frames arrive, it says "No picture: window minimized?" or "Please use borderless windowed mode" (§5.1).
6. **Audio meter**: one vertical bar per channel (L/R) in the style of the OBS audio mixer.
   - Scale −60…0 dBFS. Zones: green below −20, yellow −20…−9, red from −9 dB (OBS defaults, D12).
   - Each bar shows the RMS level, the peak level and a peak-hold marker.
   - Next to the meter, the audio mode is shown: "Game only" (process loopback) or "System audio – includes Discord etc." (fallback, §5.2). A dot marks whether non-silent audio is arriving.
   - The meter is only an indicator; there is no volume control in v1.
7. **Status line**: resolution, FPS actually sent, and the number of connected receivers (`connection_count`).
8. **Options**:
   - **Show capture border**: default **off** where the OS allows it. The checkbox is disabled (always on) where the OS does not allow it (§5.1, D8).
   - **Capture cursor**: default off. It is disabled on systems that cannot turn the cursor off.
   - These options can change only while not sending. Changing them restarts the capture session.
9. **Advanced** (collapsed by default): discovery server (§5.4), FPS cap, max resolution. These are stored in the config.
10. Footer or About: "NDI® is a registered trademark of Vizrt NDI AB" with a link to https://ndi.video/ (D5).

### 3.2 Tray icon
- The tray icon is always present while lancast runs. Its state shows: grey = idle, green = sending, red = error (e.g. the window was closed).
- Tooltip: `lancast – sending war3 (1920×1080 60 fps, 2 receivers)` or `lancast – idle`.
- **Left click** shows or hides the main window.
- **Menu**: `Show window`, `Start sending` / `Stop sending` (Start uses the last selected window), separator, `Quit`.
- **Closing the main window (×) hides it to the tray.** Only `Quit` (tray or main window) exits. The first time the window is hidden, a tray notification explains this.
- Implementation constraint (D12): eframe does not call `update()` for a hidden window. Tray and menu events are therefore handled in `set_event_handler` callbacks, which:
  - show or hide the window directly via Win32 `ShowWindow` on the eframe HWND (not via `ViewportCommand::Visible`)
  - forward commands to the controller through a channel
  - call `ctx.request_repaint()`

  Preview and meter rendering are skipped while the window is hidden.

## 4. Architecture
```
            ┌───────── tray icon (event handler) ─────────┐
            ▼                                              │
[UI thread (eframe/egui)] ◄── status, preview, levels ──┐  │
      │ start/stop, options                              │  │
      ▼                                                  │  │
[Controller] ──► [Video: WGC capture thread] ──frames──┐ │  │
             └─► [Audio: WASAPI loopback thread] ──────┤ │  │
                                                       ▼ │  │
                                          [NDI sender thread] (owns the Sender)
```
- **Channels.** Capture → NDI uses bounded `crossbeam-channel`s. When a channel is full, frames are dropped rather than blocking.
- **Preview and levels.** These go to the UI through "latest value" slots (a mutex-protected `Option`, or a channel of size 1 that is overwritten). They never block capture.
- **Single sender thread.** One NDI sender thread owns the `Sender` (`send_video_async` needs `&mut`). It runs with `clock_video = true, clock_audio = false` and submits audio from the same thread (D3).

Modules:
- `src/windows_list.rs`: enumerate and filter windows, map HWND → PID → process name (done in M0)
- `src/os_caps.rs`: detect the OS build and feature support at startup (§5.0)
- `src/capture/video.rs`: WGC via `windows-capture`, produces BGRA frames (CPU buffer) and the preview image
- `src/capture/audio_wasapi.rs`: process loopback via the `wasapi` crate, with fallback to system loopback, and level metering
- `src/ndi_out.rs`: NDI sender, frame conversion, timecode
- `src/ndi_config.rs`: writes the NDI config JSON for the discovery server (§5.4)
- `src/app.rs`: egui UI and controller
- `src/ui/preview.rs`, `src/ui/meter.rs`: preview widget and audio meter
- `src/tray.rs`: tray icon, menu, show/hide
- `src/config.rs`: `lancast.toml` next to the exe (FPS cap, max resolution, discovery server, options, source name per process)

### 4.6 Config file
`lancast.toml` sits next to the exe. If that folder is not writable, it goes to `%APPDATA%\lancast\`. All fields are optional. A missing or invalid file falls back to defaults and logs a warning.

## 5. Technical details

### 5.0 Supported Windows versions (graceful degradation)
Minimum: **Windows 10 version 1903 (build 18362)**, the first build where WGC can capture a window (`CreateForWindow`). Each feature is detected at runtime (`ApiInformation::IsPropertyPresent` / `GraphicsCaptureApi::is_*_supported()`, D8). A missing feature only disables that feature, never the app.

| Feature | Available from | Below that |
|---|---|---|
| Window capture (WGC) | 18362 (Win10 1903) | App shows an error and exits |
| Cursor on/off | 19041 (Win10 2004) | OS default (cursor visible); checkbox disabled |
| Border on/off | 20348 → in practice Win11 | Border always visible; checkbox disabled with hint |
| Process loopback (game audio only) | 20348 per docs; may work on Win10 19041+ | Fallback to system loopback, UI hint "System audio – includes Discord etc." |
| `MinUpdateInterval` (OS-side FPS cap) | 26100 (Win11 24H2) | FPS cap by dropping frames in our own code |

Windows 10 cannot be tested by the developer (D7); these paths are built "blind" against the documentation. Activation failures must therefore lead to the fallback without exceptions, with clear logging.

### 5.1 Video (Windows Graphics Capture)
- Use the `windows-capture` crate and create the item from the HWND.
- **Check every non-default session setting with `is_*_supported()` first.** windows-capture fails at startup otherwise (D8).
- Cursor **off** by default (user option).
- Capture border **off** by default where supported (user option).
  - Per the docs, a border-free capture may require `GraphicsCaptureAccess.RequestAccessAsync(Borderless)`. How this behaves for an unpackaged exe still needs checking.
  - If the border still appears with `false`, call `RequestAccessAsync` before starting the session.
- Pixel format is BGRA8, sent as `BGRX` (alpha is irrelevant; with alpha NDI uses a costlier codec, D6).
- Size changes at runtime are handled by windows-capture (frame pool recreation). A minimized window produces no frames. The sender keeps the last frame, and NDI re-sends it to new receivers.
- WGC delivers frames only when the content changes. The NDI thread sends at most the FPS cap and does not duplicate frames in v1.
- FPS cap: default 60, configurable.
- Max resolution: default 1920×1080. Downscale larger windows on the CPU, keeping the aspect ratio; a box or bilinear filter is enough in v1.
- Preview: produced on the capture thread from the CPU frame, downscaled to ≤ 320×180 and swizzled to RGBA in one pass, at most 15 fps. It is produced only when the UI is visible.
- Known limitation: games in **exclusive fullscreen** may deliver black frames or none at all. The UI shows the hint "Please use borderless windowed mode".

### 5.2 Audio (WASAPI process loopback)
- Use the `wasapi` crate (0.26): `AudioClient::new_application_loopback_client(pid, include_tree = true)` (D7).
  - Call `wasapi::initialize_mta()` on the audio thread first.
  - Pass the PID of the window's process; it is the root of the tree.
- `GetMixFormat` is not supported, so set the format explicitly: 48 kHz, stereo, Float32, `StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 }`.
- Fallback: if activation or Initialize fails, use system loopback (default render endpoint). Show this in the UI ("System audio – includes Discord etc.").
- Silence:
  - Measured on Win11: the client delivers 10 ms packets continuously, even when the target is silent.
  - On `AUDCLNT_BUFFERFLAGS_SILENT`, write zeros.
  - If the stream falls more than 50 ms behind the wall clock, pad with silence. NDI must always receive continuous audio.
- Batch audio into 10 ms frames for NDI (480 samples). WASAPI delivers interleaved data and NDI wants planar FLTP; `AudioLayout::Interleaved` in grafton-ndi converts.
- Level metering on the audio thread, per 10 ms block and channel: sample peak and RMS in dBFS. The UI applies OBS-like ballistics (D12):
  - peak: instant attack, decay 11.76 dB/s
  - RMS: 300 ms integration
  - peak hold: 20 s

### 5.3 NDI output
- Use the `grafton-ndi` crate (2.0, NDI 6 SDK). Check the current API in the crate source when upgrading.
- Sender settings: `clock_video = true`, `clock_audio = false`. One sender thread submits both (D3).
- Video: `send_video_async` with two `VideoFrame`s alternating (the crate hands the previous frame back once the SDK has released it).
- Audio: planar float FLTP, 10 ms frames.
- Timecode: the default `NDIlib_send_timecode_synthesize` (the crate's default).
- Optional: with 0 receivers, reduce the capture rate (e.g. 5 fps for the preview only) to save CPU.
- Groups: none set (default "Public"). Receivers such as DistroAV and Studio Monitor only see their configured receive groups; a dedicated group would make the sources invisible to them (D10).

### 5.4 Discovery and network (how the stream reaches the central machine)
- **There is no target address.** The sender announces itself and **the receiver connects to the sender** (D10). On the central machine, the source is picked from a list:
  - NDI Studio Monitor: the ☰ menu at top left (or right-click) → `MY-PC (war3)`. The "Web Control URL" shown there is only for remote-controlling the monitor.
  - OBS + DistroAV: add an "NDI Source" → "Source name" dropdown.
- **Default: mDNS** (UDP 5353). This works when sender and receiver share a subnet / L2 segment, as on a typical LAN party network. No configuration is needed.
- **Optional: NDI Discovery Server**, for networks with several subnets/VLANs or blocked multicast. "Advanced → Discovery server" takes an IP (or `ip:port`, default port 5959).
  - The standard SDK has no API for this. lancast writes `ndi-config.v1.json` with `{"ndi":{"networks":{"discovery":"<ip>"}}}` to `%APPDATA%\lancast\ndi\` and sets `NDI_CONFIG_DIR` to that folder **before** NDI is initialized.
  - Changing the value therefore takes effect only at the next send start, after the NDI instance has been dropped and recreated (or after restarting the app). The UI says so.
  - Caveat shown in the UI: with a discovery server set, the sender **stops announcing via mDNS**, so receivers must use the same server.
  - The server ships with NDI Tools / the NDI SDK (`NDI Discovery Service.exe`) and runs on the central machine or any server.
- Alternative without a server, configured on the receiver: "Extra IPs" in NDI Access Manager on the central machine. Documented in the README only.
- **Transport:** NDI 6 default is reliable UDP. Receivers connect to the sender on TCP 5960 and UDP/TCP 5960+.
- **Firewall:** the sender needs **inbound** access. On first launch, Windows shows the "Allow access" dialog; the participant must allow it for **Private** networks (and check that the LAN is not classified as "Public").
  - The README documents this, including a `netsh advfirewall firewall add rule … program=… dir=in action=allow` command for admins.
  - Symptom of a blocked firewall: the source is listed but stays black.
- **Bandwidth:** full NDI at 1080p60 is up to ~132 Mbit/s per source (D6), so a 1 GbE uplink at the central machine carries about 6 such sources.
  - Mitigations: a lower max resolution or FPS cap per participant (config), and the receiver's "Lowest" bandwidth mode for multiview tiles. The latter makes the sender produce an additional low-resolution stream and costs sender CPU.

### 5.5 Performance roadmap (GPU)
- **v1:** WGC (GPU) → CPU readback of BGRA → optional CPU downscale → NDI as BGRX. Measured readback cost: 1–5.6 ms per frame at 2560×1392 (D8).
- **v2 (M5):** scaling and BGRA→**UYVY** conversion on the GPU (D3D11 pixel/compute shader into a half-width RGBA8 texture), a ring of 2–3 staging textures, then send UYVY.
  - This halves memory traffic and saves NDI's internal colour conversion. The NDI docs explicitly recommend it (D11).
  - Vendor-neutral; no NVIDIA/AMD SDK needed.
- **Evaluated and rejected (D11):**
  - NVIDIA Capture SDK: NvFBC is deprecated on Windows, NvIFR is removed and would be injection.
  - ShadowPlay / Instant Replay: no public API. Highlights SDK: legacy, and only usable when integrated into the game.
  - AMD AMF DisplayCapture: full display only, AMD-only. ReLive: no API.
  - Hardware encoders (NVENC/AMF/QSV): only useful for NDI|HX via the Advanced SDK (v3 at the earliest, with a commercial licence).

## 6. Distribution
- Ship `lancast.exe`, `Processing.NDI.Lib.x64.dll`, `Processing.NDI.Lib.Licenses.txt`, `README.txt` and `EULA.txt` in the same folder, as a ZIP. `build.rs` copies the DLL next to the exe on build (done in M0).
- NDI SDK license requirements (verified, D5):
  - The DLL may be redistributed in the app folder.
  - The license file must accompany it.
  - Trademark notice "NDI® is a registered trademark of Vizrt NDI AB" (README and About) and a link to https://ndi.video/.
  - An EULA with the clauses from NDI License Agreement §3d.
  - Ship the latest SDK version at release time.
  - "NDI" must not be part of the product name.
- The exe has an icon (embedded via `winresource`) that the tray also uses.
- The exe is unsigned, so a SmartScreen warning is expected. That is acceptable for a LAN party; note it in the README.
- Windows Firewall: see §5.4; note it in the README.
- Single instance: a second launch brings the running instance's window to the front (named mutex + `FindWindowW`).

## 7. Milestones
- **M0 – Spikes (done, see D1–D9):** `list_windows`, `ndi_testpattern`, `wgc_window`, `process_loopback` under `examples/`.
- **M1:** video of a selected window (CLI argument PID/HWND) → NDI, including `os_caps` and the capture options (border, cursor) with support checks.
- **M2:** integrate audio (process loopback + fallback + silence handling); A/V in sync at the receiver. Lip-sync test: YouTube in a browser as the "game", and `ndi_testpattern`'s flash/beep as reference.
- **M3:** egui UI per §3.1 (window list, source name proposal, preview, audio meter, status, options) and tray icon per §3.2.
- **M4:** config file, discovery server (§5.4), CPU scaling/FPS cap, single instance, exe icon, release ZIP with README/EULA/license file.
- **M5 (v2):** GPU scaling + UYVY conversion (§5.5), reduced capture when there are 0 receivers.

## 8. Acceptance criteria
- A 1080p60 game in borderless windowed mode appears in NDI Studio Monitor on a second PC, with subjective latency below roughly 200 ms.
- Only the game audio is audible. Discord audio on the same system is **not** audible (on Windows 11; on Windows 10 the fallback is clearly indicated in the UI).
- The preview shows the captured window, and the audio meter moves when the game makes sound and stays at the bottom when it is silent.
- Minimizing, restoring and resizing the window cause neither a crash nor a permanent freeze.
- Quitting the game leads to a clean stop with a notice.
- Closing the main window keeps lancast running in the tray. The tray menu can start and stop sending and quit.
- The tool's CPU load at 1080p60 is reasonable on a mid-range PC (guideline: < 10 %; checked again after M5).

## 9. Open points
- Resolved in M0: process loopback minimum version (D7), NDI license (D5), NDI|HX and bandwidth (D6).
- Does the capture border disappear for an unpackaged exe without `RequestAccessAsync`? Check visually in M1.
- Process loopback on Windows 10 19041–19045: untested (no test machine). Rely on the fallback.
- CPU cost on the sender when receivers request the "Lowest" bandwidth proxy stream: measure in M2/M3.
