# LANcast – Specification

The product and the program are called **LANcast** (`LANcast.exe`, window and tray titles). The project, repository and crate keep the name `lancast`.

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
Before sending (window picker):
```
┌─ LANcast ──────────────────────────────────────────────┐
│ Choose the window to cast                              │
│ ┌─┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌─┐         │
│ │<│ │ (live    │ │ (live    │ │ (live    │ │>│         │
│ │ │ │ thumb)   │ │ thumb)   │ │ thumb)   │ │ │         │
│ │ │ │▣ Title…  │ │▣ Title…  │ │▣ Title…  │ │ │         │
│ └─┘ └──────────┘ └──────────┘ └──────────┘ └─┘         │
│ Source name: [ war3 ]  NDI name: MY-PC (war3)        │
│ Audio: game only (checked)                             │
│ [ Send ]                                               │
│ ☑ Close and minimize to tray   ☐ Always on top         │
│ ☑ Show capture border          ☐ Capture cursor        │
│ ▸ Advanced                                             │
│ NDI® is a registered trademark of Vizrt NDI AB. ndi.video │
└────────────────────────────────────────────────────────┘
```
Below the carousel, as soon as a window is selected, the **live preview** and the L/R audio meter appear. They are centered and scale with the window. At the bottom: source name, status, Send/Stop, and the **Settings** panel (Window / Capture / Video / Audio in a framed grid).

- **Preview before sending.** Selecting a window starts a preview capture without NDI: 20 fps, plus game audio for the meter if game-only audio is available. Video is read back only while the preview is visible, and the preview image is made straight from the readback (D21).
- **Preview quality.** The image is produced at the size it is displayed, in physical pixels (up to 1920×1080), with area averaging, and refreshes about 20 times per second.
- **Switching to the session.** "Send" replaces the preview capture with the session; "Stop" returns to the preview. Changing the border or cursor option restarts the preview, so the effect is visible immediately.
- **While sending,** the carousel can still be scrolled but cannot select another window.

1. The participant launches `LANcast.exe`. The main window opens and the tray icon appears.
2. **Window picker: a carousel like Alt-Tab** (requirement 2026-10-17, D19).
   - **Tiles:** three are visible, and the carousel cycles endlessly. Each tile shows a **live thumbnail** of the window, the exe's icon, the title and the process name. A click selects the tile and centers it.
   - **Movement is continuous** (D20):
     - the mouse wheel or touchpad scrolls it softly over the carousel
     - it can be **dragged ("pulled")** with mouse or finger, with momentum after release
     - afterwards it eases onto the nearest tile
     - the arrows move by one tile
   - **Order:** like Alt-Tab, i.e. Z-order, most recently active first; LANcast's own window is excluded.
   - **Refresh:** every 2 s. Known windows keep their place, new windows are inserted at the front, and the centered window stays centered.
   - **Thumbnails:** DWM thumbnails (`DwmRegisterThumbnail`, the Alt-Tab mechanism). They are composited by Windows, cost no CPU and send nothing to the game process (anti-cheat safe).
   - **Icons:** read from the exe file (`SHGetFileInfo`).
   - **Window list:** `src/windows_list.rs` (D9).
     1. Enumerate windows with `EnumWindows`.
     2. Keep visible, non-cloaked, unowned, non-tool windows that have a title.
     3. Get the PID with `GetWindowThreadProcessId` and the path with `QueryFullProcessImageNameW`.
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
   - **System-audio warning (requirement 2026-10-17).** The fallback must be impossible to miss, and the user must see it **before** switching to the game, because nobody reads the window or the log while playing.
     1. **Pre-flight check on selection.** As soon as the user selects a window (not only on "Send"), lancast checks whether process loopback can be activated for that PID. The check activates and initializes the client, then releases it; this takes a few milliseconds (D7, D16).
     2. **If it fails, show a prominent warning banner** (amber/red, icon, above the Send button):
        > ⚠ Game-only audio is not available on this system. lancast would send **all system audio** – everyone will hear your Discord, music and notifications.

        The Send button is replaced by two explicit choices: **"Send with system audio"** and **"Send without audio"**. A plain "Send" that silently uses system audio is not allowed.
     3. **While sending with system audio:**
        - The tray icon shows a distinct warning state (amber, not the normal green).
        - The tray tooltip says "sending SYSTEM audio".
        - A Windows notification appears when sending starts: "lancast is sending ALL system audio (incl. Discord)".
        - The main-window banner stays visible.
     4. **Mid-session fallback:** if process loopback fails after "Send" despite a passing pre-flight check, lancast does **not** silently fall back. It sends without audio, shows the banner and a notification, and offers to switch to system audio.
     5. The choice is not remembered across sessions; the warning appears every time.
   - The meter is only an indicator; there is no volume control in v1.
7. **Status line**: resolution, FPS actually sent, and the number of connected receivers (`connection_count`).
8. **Options**:
   - **Show capture border**: default **on** (requirement 2026-10-17; the player sees what is captured). It can be turned off where the OS allows it; the checkbox is disabled (always on) where the OS does not allow it (§5.1, D8).
   - **Capture cursor**: default off. It is disabled on systems that cannot turn the cursor off.
   - These options can change only while not sending. Changing them restarts the capture session.
   - **Close and minimize to tray**: default **on**. When on, both × and minimize hide LANcast to the tray. When off, × quits and minimize behaves normally. There is no separate Quit button; quit from the tray menu, or with × when this option is off.
   - **Always on top**: default off. Keeps LANcast above the game to check the setup. Works with borderless windowed games only.
9. **Advanced** (collapsed by default): discovery server (§5.4), FPS cap, max resolution. These are stored in the config.
10. Footer or About: "NDI® is a registered trademark of Vizrt NDI AB" with a link to https://ndi.video/ (D5).

### 3.2 Tray icon
- The tray icon is always present while LANcast runs. Its state dot shows: none = idle, green = sending game audio (or no audio), **amber = sending SYSTEM audio** (§3.1 item 6), red = error (e.g. the window was closed).
- Tooltip: `lancast – sending war3 (1920×1080 60 fps, 2 receivers)` or `lancast – idle`.
- **Left click** shows or hides the main window.
- **Menu**: `Show window`, `Start sending` / `Stop sending` (Start uses the last selected window), separator, `Quit`.
- **× and minimize hide the main window to the tray** while "Close and minimize to tray" is on (the default). `Quit` in the tray menu exits. The first time the window is hidden, a tray notification explains this.
- **Icon:** the LANcast icon (`assets/icon-source.png`). The state is shown as a small dot in the lower right corner: none = idle, green = sending, amber = sending SYSTEM audio, red = error.
- Implementation (D18): the tray is lancast's own `Shell_NotifyIconW` icon on a dedicated thread with its own message loop, so it is independent of eframe, which does not paint a hidden window.
  - The send session lives in a controller thread, so stop-on-close, tray state and notifications keep working while the window is hidden.
  - The window is hidden and shown with Win32 `ShowWindow` on the eframe HWND.
  - Preview and meter rendering are skipped while the window is hidden.
  - "Start sending" in the tray uses game audio only. If the pre-flight check failed, it opens the window instead, so system audio is never sent without the explicit choice.

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

### 4.5 Logging
`tracing` to stdout and to `lancast.log` next to the exe. The previous run is kept as `lancast.prev.log`, and the level is set via env `LANCAST_LOG`. **Window titles are logged at debug level only.** Participants may share their log file for troubleshooting, and titles can contain private text (browser tabs, documents). Info level identifies windows by process name and HWND.

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
  - Verified on Win11 26200: `IsBorderRequired = false` works for the unpackaged exe without `RequestAccessAsync` (D8), so no consent call is needed.
- Pixel format is BGRA8, sent as `BGRX` (alpha is irrelevant; with alpha NDI uses a costlier codec, D6).
- Size changes at runtime are handled by windows-capture (frame pool recreation). A minimized window produces no frames. The sender keeps the last frame, and NDI re-sends it to new receivers.
- WGC delivers frames only when the content changes. The NDI thread sends at most the FPS cap and does not duplicate frames in v1.
- FPS cap: default 60, configurable. Enforced by a deadline pacer on the capture thread that drops surplus frames before the readback. `MinUpdateInterval` (where supported) is set to only half the target period. Setting it to the full period quantizes to the refresh grid, e.g. 48 fps on 144 Hz displays (D8).
- Max resolution: default 1920×1080. Downscale larger windows on the CPU, keeping the aspect ratio; a box or bilinear filter is enough in v1.
- Preview: produced on the capture thread from the CPU frame, downscaled to ≤ 320×180 and swizzled to RGBA in one pass, at most 15 fps. It is produced only when the UI is visible.
- Known limitation: games in **exclusive fullscreen** may deliver black frames or none at all. The UI shows the hint "Please use borderless windowed mode".

### 5.2 Audio (WASAPI process loopback)
- Use the `wasapi` crate (0.26): `AudioClient::new_application_loopback_client(pid, include_tree = true)` (D7).
  - Call `wasapi::initialize_mta()` on the audio thread first.
  - Pass the PID of the window's process; it is the root of the tree.
- `GetMixFormat` is not supported, so set the format explicitly: 48 kHz, stereo, Float32, `StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 }`.
- Fallback: if activation or Initialize fails, use system loopback (default render endpoint). In the GUI this is never automatic: it requires the explicit choice after the pre-flight warning (§3.1 item 6). The CLI keeps the automatic fallback with a log warning.
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
- **Audio level convention (D17).** NDI defines float 1.0 = +4 dBu, with full scale at 10.0 (20 dB headroom); NDI Studio Monitor follows this. OBS + DistroAV passes the floats through unchanged (1.0 = full scale).
  - Receivers at the party may be OBS + DistroAV, NDI Studio Monitor or **Resolume** (requirement 2026-10-17).
  - Default: unity gain (correct in OBS + DistroAV).
  - Setting "Receiver audio level: Unity – OBS/DistroAV (default) / NDI standard +20 dB – Studio Monitor and SDK-compliant apps", as a config option and in Advanced (M4). CLI: `--audio-gain-db 20`.
  - Resolume: unverified. Its forum says NDI audio input is not supported at all (D17). Measure on site before the event with a known test tone.
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
- **Stale system discovery-server check (D14).** At startup, lancast reads the system-wide `C:\ProgramData\NDI\ndi-config.v1.json` (written by NDI Access Manager). If `ndi.networks.discovery` is set:
  1. Test each listed server with a TCP connect to `ip:port` (default 5959, timeout ~1 s, in the background so the UI is not blocked).
  2. **Unreachable:** show a warning: "A discovery server (10.0.18.19) is configured in NDI Access Manager but cannot be reached. Your source will be invisible to receivers using mDNS."
     - Offer **"Ignore for lancast"**. It writes lancast's own `ndi-config.v1.json` (§5.4) with the discovery server cleared, so the sender uses mDNS again, and sets `NDI_CONFIG_DIR` to it.
     - The system file is **never** modified.
     - The choice is stored in `lancast.toml`.
  3. **Reachable:** show an info line "Using discovery server <ip>". Receivers must use the same server.
  4. If the user set a discovery server in lancast's own Advanced settings, that one wins and the system file is not consulted.

  Also log the effective NDI network configuration (discovery servers, groups, multicast) at startup.
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
- Ship `LANcast.exe`, `Processing.NDI.Lib.x64.dll`, `Processing.NDI.Lib.Licenses.txt`, `README.txt` and `EULA.txt` in the same folder, as a ZIP. `build.rs` copies the DLL next to the exe on build (done in M0).
- NDI SDK license requirements (verified, D5):
  - The DLL may be redistributed in the app folder.
  - The license file must accompany it.
  - Trademark notice "NDI® is a registered trademark of Vizrt NDI AB" (README and About) and a link to https://ndi.video/.
  - An EULA with the clauses from NDI License Agreement §3d.
  - Ship the latest SDK version at release time.
  - "NDI" must not be part of the product name.
- Ship `LANcast.exe`. Its icon and version info are embedded by `build.rs` via `winresource` from `assets/LANcast.ico`. The window and the tray use the pre-scaled `assets/icon-<N>.png`; `tools/make_icons.py` regenerates all of them from `assets/icon-source.png`. See `docs/ICON.md` for previews and details.
- The exe is unsigned, so a SmartScreen warning is expected. That is acceptable for a LAN party; note it in the README.
- Windows Firewall: see §5.4; note it in the README.
- Single instance: a second launch brings the running instance's window to the front (named mutex + `FindWindowW`).

## 7. Milestones
- **M0 – Spikes (done, see D1–D9):** `list_windows`, `ndi_testpattern`, `wgc_window`, `process_loopback` under `examples/`.
- **M1 (done, D15):** video of a selected window (CLI argument PID/HWND) → NDI, including `os_caps` and the capture options (border, cursor) with support checks. CLI: `lancast <0xHWND | PID> [--name NAME] [--fps N] [--max-size WxH] [--border] [--cursor]`; without arguments it lists the capturable windows. `examples/ndi_probe` is a test receiver.
- **M2 (done, D16):** integrate audio (process loopback + fallback + silence handling); A/V in sync at the receiver. CLI adds `--system-audio` and `--no-audio`.
  - Measurement tools:
    - `examples/av_sync_source` is a window that flashes and beeps once per second; `--silent` only flashes.
    - `ndi_probe --av` measures the flash→beep offset at the receiver.
  - Still to check by hand: YouTube in a browser, and Discord on a real system.
- **M3 (done, D18):** egui UI per §3.1 (window list, source name proposal, preview, audio meter, status, options), including the system-audio pre-flight check and warning (§3.1 item 6), and tray icon per §3.2.
  - `lancast` without arguments opens the window; with arguments it runs as the CLI (`--list`, `--help`).
  - Release builds are GUI-subsystem exes; the CLI attaches to the parent console.
  - The window list refreshes whenever the dropdown is opened.
  - The preview is shown while sending. A preview before sending, to confirm the right window, is a possible later addition.
  - Test switch: env `LANCAST_SIMULATE_NO_PROCESS_LOOPBACK=1` makes process loopback fail, to exercise the Windows 10 warning path.
- **M4:** config file, discovery server (§5.4), stale system discovery-server check with "Ignore for lancast" override (§5.4, D14), receiver audio level setting (§5.3, D17), CPU scaling/FPS cap, single instance, release ZIP with README/EULA/license file.
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
- Process loopback on Windows 10 19041–19045: untested (no test machine). Rely on the fallback.
- CPU cost on the sender when receivers request the "Lowest" bandwidth proxy stream: measure in M2/M3.
- **Idea: tournament mode (delayed stream).** Send the stream with a configurable delay of a few seconds, so the projector gives the opposing team no live information. Feasibility and options are in D20. **Deferred** (user decision 2026-10-17).
