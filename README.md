# LANcast

**Cast your game to the big screen at a LAN party: one window, the picture and only the game's sound, as an NDI® source on the local network.**

![LANcast with a selected game window, live preview and audio meter](docs/images/screenshot.png)

## Idea and motivation
At a LAN party everybody wants to show their game on the projector: a tournament final, a speedrun, a funny moment. The usual setups are clumsy:
- **HDMI cables** across the hall don't scale.
- **OBS on every participant's PC** means installation, configuration and a lot of support at the event.
- **Desktop capture** shows notifications, chat windows and other private things.
- **System audio** leaks everybody's Discord voice chat onto the PA.

LANcast is the participant's side of a simpler setup:
1. **Start one exe, pick the game, click "Send".** No configuration needed.
2. Only **that window** is captured: no desktop, no notifications.
3. Only **that game's audio** is sent: Discord, music and system sounds stay private.
4. The stream appears on the LAN as `PC-NAME (Player - Game)`. The central PC (OBS with the DistroAV plugin, NDI Studio Monitor, …) picks it from a list and puts it on the projector.

## Features
- **Window picker like Alt-Tab:**
  - a carousel with live thumbnails, which scrolls with the mouse wheel and can be dragged with mouse or touch
  - locked with a "LIVE" marker while casting
- **Live preview and audio meter** (OBS-style peak/RMS) before and while sending.
- **Game audio only** via WASAPI process loopback.
  - If Windows can't do that, LANcast warns *before* sending and asks: "Send with system audio" or "Send without audio".
- **Tray icon** with a red REC dot while casting. Close and minimize go to the tray.
- **Stream name `Hostname (User - Game)`:** the user name defaults to the Windows login and is editable.
- **Network help:**
  - automatic discovery (mDNS)
  - an optional NDI Discovery Server
  - a warning if a stale discovery server is configured in NDI Access Manager
- **Remembers** settings, window size and position, and the game name per game.
- **Anti-cheat friendly:** no injection, no hooks, no overlays in the game process. Only Windows APIs:
  - Windows Graphics Capture
  - WASAPI
  - DWM thumbnails

## How it works
```
┌──────────── LANcast.exe (participant PC) ─────────────┐         ┌── central PC ──────────┐
│ Windows Graphics Capture ──► scale/pace ─┐            │  NDI    │ OBS + DistroAV,        │
│  (one window, GPU → CPU)                 ├─► NDI send ─┼────────►│ NDI Studio Monitor,    │
│ WASAPI process loopback ──► 10 ms chunks ┘ (SpeedHQ)  │  LAN    │ … → projector / PA     │
│  (the game's process tree only)                       │         └────────────────────────┘
│ egui window + tray icon + controller thread           │
└───────────────────────────────────────────────────────┘
```
- **Video:**
  - Captured with [Windows Graphics Capture](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture) via the `windows-capture` crate.
  - Paced to the frame-rate cap. A deadline pacer avoids the 48 fps trap on 144 Hz monitors.
  - Downscaled to the maximum resolution, then sent asynchronously as BGRX.
- **Audio:**
  - WASAPI **process loopback** (`AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`, include process tree) via the `wasapi` crate.
  - 48 kHz stereo float in continuous 10 ms chunks; silence is filled in.
  - Falls back to system loopback only with the user's explicit consent.
- **NDI:**
  - The NDI 6 SDK via `grafton-ndi`, with one sender thread: video is clocked, audio is sent unclocked from the same thread.
  - Measured A/V offset: about +28 ms, i.e. audio slightly late, well below what people notice.
- **UI:**
  - [`eframe`/`egui`](https://github.com/emilk/egui) for the window.
  - DWM thumbnails (the Alt-Tab mechanism) for the carousel.
  - The tray is a small `Shell_NotifyIconW` module on its own thread.
  - A controller thread owns the session, so everything keeps working while the window is hidden.

The full specification is in [`docs/SPEC.md`](docs/SPEC.md). Every technical decision, with sources and measurements, is in [`docs/DECISIONS.md`](docs/DECISIONS.md). The icon is described in [`docs/ICON.md`](docs/ICON.md).

## Event links
Organizers can configure every participant's LANcast with one link on the event website:

```html
<a href="lancast://config?event=NetGame%202026&discovery=10.0.0.5&max=1280x720&fps=60&level=unity">
  Configure LANcast for NetGame 2026
</a>
```

| Key | Meaning |
|---|---|
| `event` | Event name, shown in the settings |
| `discovery` | The event's NDI Discovery Server ("gateway"), `host[:port]`; empty = automatic (mDNS) |
| `max`, `fps` | **Limits** that keep the network bandwidth in check; users can only choose lower values |
| `level`, `border`, `cursor` | **Defaults** (receiver audio level `unity`/`ndi`, capture border, cursor `1`/`0`) |

LANcast shows the settings and asks before applying them; a link never starts sending. "Leave event" in the settings removes them again.

The installer registers the `lancast:` scheme and the uninstaller removes it; the ZIP version registers it on first start. *Settings → Links* shows which copy opens the links and can switch it to the current one, e.g. after moving the folder.

Full reference with all keys, encoding rules, a website template and testing tips: [`docs/EVENT_LINKS.md`](docs/EVENT_LINKS.md).

### Event edition ("LAN distro")
For events that want more control, there is an event edition: a LANcast package for one event that **does not send until it has the event's configuration**, signed by the organizer.
- **Two sections.** The configuration has an *enforced* section (signed: discovery server, limits, anything else the event wants fixed) and a *defaults* section (unsigned starting values participants may change).
- **Delivery.** The configuration comes as `LANcast.event` in the event's download, or as a signed `lancast://event?…` link. It is valid from/until given dates.
- **Signing.** Ed25519: the edition contains only the organizer's public key, so a configuration can't be forged or changed, not even with the exe in hand.

```powershell
cargo build --release --bin lancast-event
target\release\lancast-event keygen secret.key        # once per event; keep secret.key private
# describe the event in event.toml (see packaging\event.example.toml), then:
powershell -File tools\package.ps1 -EventConfig event.toml -SigningKey secret.key
```
This builds the normal packages plus `dist\LANcast-<ver>-<event_id>-win64.zip`, `…-setup.exe` and `…-link.txt` with the event link. Details: [`docs/EVENT_LINKS.md`](docs/EVENT_LINKS.md#event-edition-lan-distro).

## Requirements
- **Windows 10 1903 or newer, 64-bit.** Windows 11 is recommended: process loopback (game-only audio) is documented from build 20348.
- Games should run in **borderless windowed** mode. Exclusive fullscreen may not be captured.
- **A receiver on the LAN:** OBS with [DistroAV](https://github.com/DistroAV/DistroAV), [NDI Tools](https://ndi.video/tools/) (Studio Monitor), or other NDI software.

## Using it
1. Install with `LANcast-<version>-setup.exe`, or unzip `LANcast-<version>-win64.zip`.
   - With admin rights, the installer also adds the Windows Firewall rule; receivers connect to the sender.
2. Start LANcast, pick your game, check the preview and the meter, and click **Send**.
3. On the central PC, add the source `PC-NAME (Player - Game)`.

**Audio level tip:**
- OBS/DistroAV plays NDI audio 1:1.
- NDI Studio Monitor follows the NDI reference level and plays it 20 dB quieter. Set *Settings → Audio → Receiver level* to "NDI standard +20 dB" for it.

Participant-facing instructions (firewall, SmartScreen, troubleshooting) are in [`packaging/README.txt`](packaging/README.txt), which ships with the release.

## Building
**Prerequisites:**
- **Rust:** stable (edition 2024) with the MSVC toolchain (`x86_64-pc-windows-msvc`). Build natively on Windows, not in WSL.
- **[NDI 6 SDK](https://ndi.video/for-developers/ndi-sdk/):** the default path is `C:\Program Files\NDI\NDI 6 SDK`; otherwise set `NDI_SDK_DIR`.
- **LLVM/Clang** for bindgen (`winget install LLVM.LLVM`); set `LIBCLANG_PATH` if it's not found.
  - Both paths can be set in a local `.cargo/config.toml` (git-ignored) under `[env]`.
- **For packaging only:** Python 3 (third-party license list; Pillow for regenerating icons) and [Inno Setup 6](https://jrsoftware.org/isinfo.php) (`winget install JRSoftware.InnoSetup`).

**Build and check:**
```powershell
cargo build --release                         # target\release\LANcast.exe (+ NDI DLL copied next to it)
cargo clippy --all-targets -- -D warnings
cargo fmt
cargo test --release                          # unit tests (pacer, scaler, levels, config, links, signatures)
```
Quit a running LANcast before rebuilding: Windows does not let cargo replace a running exe.

**Release package:**
```powershell
powershell -File tools\package.ps1   # dist\LANcast-<ver>-win64.zip and dist\LANcast-<ver>-setup.exe
```
With `-EventConfig event.toml -SigningKey secret.key` it also builds the event edition (see [Event edition](#event-edition-lan-distro)).

## Testing
Unit tests cover the pure logic. Capture, audio and NDI are checked with small tools in `examples/`:

| Example | Purpose |
|---|---|
| `ndi_testpattern` | Sends colour bars plus a beep with a white flash marker, to check a receiver and lip sync. |
| `ndi_probe [name] [secs] [--av]` | A receiver: resolution, fps, audio rate and level. `--av` measures the flash→beep offset. |
| `av_sync_source [secs] [--silent]` | A test "game" window that flashes and beeps, for end-to-end lip-sync and isolation tests. |
| `list_windows` | Lists the capturable windows with HWND, PID and process. |
| `wgc_window <0xHWND\|PID>` | Captures one window and logs fps, size and readback cost. |
| `process_loopback <PID\|--system>` | Records a process's audio to a WAV file. |
| `tray_preview` | Renders all tray icon states into `docs/images/tray-states-4x.png`. |

Run them with `cargo run --release --example <name> -- <args>`.

**Typical end-to-end check:**
1. Run `av_sync_source 60`.
2. Start LANcast, pick that window and click Send.
3. Run `ndi_probe "av_sync_source" 15 --av`.

**Environment switches for tests and demos:**
- `LANCAST_SIMULATE_NO_PROCESS_LOOPBACK=1` makes process loopback fail, to exercise the Windows 10 warning path.
- `LANCAST_WINDOW_FILTER=Game.exe,Other.exe` restricts the window list, e.g. for screenshots without private windows.
- `LANCAST_LOG=debug` sets the log level.

**Command-line mode** (no window, useful for testing): `LANcast <0xHWND|PID> [--name N] [--fps N] [--max-size WxH] [--system-audio|--no-audio] …`. Run `LANcast --help` for the full list.

## Project layout
```
src/
  main.rs            entry point: GUI without arguments, CLI with arguments
  app/               egui UI, carousel, meter, controller thread
  capture/           video (WGC), audio (WASAPI), pacer, scaler
  session.rs         one send session (NDI + video + audio) and the preview capture
  ndi_out.rs         NDI sender thread
  ndi_config.rs      NDI network configuration (discovery server)
  config.rs          LANcast.toml, data folder (%LOCALAPPDATA%\LANcast, portable mode)
  event_link.rs      lancast:// links: enforced and default sections
  distro.rs          event edition: signed event configurations (Ed25519)
  bin/lancast-event.rs  organizer tool: key, sign, verify
  tray.rs            tray icon, notifications, single instance
  windows_list.rs    window enumeration (Alt-Tab-like)
  win_thumbnail.rs   DWM live thumbnails
examples/            spikes and test tools (see above)
packaging/           README/EULA for the release, Inno Setup script, event.example.toml
tools/               package.ps1, make_icons.py, third_party_licenses.py
docs/                SPEC, DECISIONS, ICON, EVENT_LINKS
```

## License
LANcast is licensed under the [Apache License 2.0](LICENSE); see [`NOTICE`](NOTICE).

The release package also contains the NDI® runtime library. It is **not** covered by the Apache License and is subject to the NDI SDK license terms (see `packaging/EULA.txt` and `Processing.NDI.Lib.Licenses.txt`). Third-party crates and their licenses are listed in the generated `THIRD-PARTY-LICENSES.txt`.

NDI® is a registered trademark of Vizrt NDI AB. [ndi.video](https://ndi.video/)
