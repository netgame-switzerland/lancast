# Event links (`lancast://`)

An event link configures LANcast for a LAN event in one click: the event's NDI discovery server ("gateway"), limits that keep the network bandwidth in check, and a few defaults. Put it on the event website as a button; participants click it after installing LANcast.

```
lancast://config?event=NetGame%202026&discovery=10.0.0.5&max=1280x720&fps=60&level=unity
```

## What happens when someone clicks it
1. **Browser prompt.** The browser asks "Open LANcast?". The `lancast:` scheme is registered by the installer, or by the ZIP version on its first start.
2. **LANcast opens or receives the link.** If LANcast is already running, the link goes to the running window, and the newly started instance exits again.
3. **Confirmation dialog.** LANcast lists every setting the link wants to change and asks **Apply** / **Cancel**.
4. **Apply.** The settings are saved (`LANcast.toml`), and the settings panel shows *Event: NetGame 2026* with a **Leave event** button.

A link never starts sending and never changes the system-wide NDI configuration (NDI Access Manager).

## Two sections: enforced and defaults
Every setting in an event configuration is in one of two sections:
- **Enforced:** fixed while the event is active. Participants see them disabled, marked *(set by the event)*. Limits (`max`, `fps`) cap the participant's choice, marked *(event limit …)*; the other keys replace it.
- **Defaults:** starting values, applied once when the event is applied. Participants can change them afterwards.

In a `lancast://config` link (normal edition), `discovery`, `max` and `fps` are enforced, and `level`, `border` and `cursor` are defaults. In the [event edition](#event-edition-lan-distro) the organizer chooses for every key.

## Keys
All keys are optional, but a link must set at least one. Unknown keys are ignored. An invalid value rejects the whole link, so a link is never half-applied.

| Key | Values | Kind | Effect |
|---|---|---|---|
| `event` | text, max. 64 characters | info | Event name shown in the settings and the dialog. |
| `discovery` | `host[:port]`, comma-separated; empty = automatic (mDNS) | setting | NDI Discovery Server for LANcast's sender. Port 5959 if omitted. Receivers must use the same server. |
| `max` | `<W>x<H>`, 320×180 … 3840×2160 | **limit** | Maximum resolution. Users can only choose lower values; larger windows are scaled down. |
| `fps` | 10 … 120 | **limit** | Maximum frame rate. |
| `level` | `unity` (OBS/DistroAV) or `ndi` (+20 dB, NDI Studio Monitor) | default | Receiver audio level convention. |
| `border` | `1` / `0` (also `true`/`false`, `yes`/`no`, `on`/`off`) | default | Yellow capture border. |
| `cursor` | `1` / `0` | default | Capture the mouse cursor. |

- **Limits stay in force** until the user clicks *Leave event*. Raising them needs a new link.
- **Defaults are set once.** The user can change them afterwards.

**Bandwidth guide:** full-quality NDI needs up to about 130 Mbit/s at 1080p60 (D6). That is roughly 50 Mbit/s at `max=1280x720&fps=30`. Pick limits that fit the central PC's uplink and the number of simultaneous streams.

## Encoding
- Use URL encoding for values: a space is `%20` (or `+`), a comma inside a value is fine.
- Accepted forms: `lancast://config?…`, `lancast:config?…` and `lancast://config/?…`.
- Maximum link length: 4096 characters.

## Website example
```html
<p>
  1. Download and install LANcast:
     <a href="https://example.org/LANcast-0.1.0-setup.exe">LANcast setup</a><br>
  2. Then click:
     <a href="lancast://config?event=NetGame%202026&amp;discovery=10.0.0.5&amp;max=1280x720&amp;fps=60&amp;level=unity">
       Configure LANcast for NetGame 2026
     </a>
</p>
```
- Write `&` as `&amp;` inside HTML attributes; browsers accept both.
- **Chat apps:** Discord, WhatsApp etc. usually do not make custom schemes like `lancast://` clickable. Post a link to the web page instead.
- **Fallback:** for people who cannot click the link, list the values on the page; they can enter them in LANcast's settings by hand.

## Which LANcast opens the link
*Settings → Links* shows which copy opens `lancast://` links:
- **A different copy** (an older install, a moved ZIP folder): *Use this copy* registers the current one for your Windows user.
- **Not set up:** *Set up for this copy* registers it.

LANcast also re-registers itself automatically at startup if the registered program no longer exists. The registration lives in `HKEY_CURRENT_USER\Software\Classes\lancast`, or in `HKEY_LOCAL_MACHINE` for an all-users install; the uninstaller removes it.

## Event edition ("LAN distro")
An event edition is a LANcast package built for one event. It **only sends with that event's configuration**, signed by the organizer:
- `LANcast.event` next to `LANcast.exe` (part of the event download; applied automatically), or
- a signed link `lancast://event?p=<payload>&s=<signature>&<defaults>` (asks first, like every link).

Without a valid configuration (missing, not yet valid, expired, or not signed with the event's key), Send is disabled and a banner explains why. Unsigned `lancast://config` links are rejected. The window title reads *LANcast – <event> edition*.

**How it is secured:** Ed25519 signatures. The organizer keeps the private key (`secret.key`); the event edition contains only the public key. Changing anything in the signed part, e.g. raising the resolution limit, makes the signature invalid. The defaults are deliberately unsigned, because participants may change them anyway.

**What it does not do:** it cannot stop someone from running a normal LANcast or another NDI sender on the event network. Use the event's network setup (discovery server, switch limits) for that. The event edition makes the right configuration the default and stops tampering within it.

### Building an event edition
Keep the event's files in a folder **outside the repository**, e.g. `..\netgame-2026\`.

1. **Key pair**, once per event. Keep `secret.key` private: never commit it, never put it into a download.
   ```powershell
   cargo build --release --bin lancast-event
   mkdir ..\netgame-2026
   target\release\lancast-event keygen ..\netgame-2026\secret.key     # prints the public key
   ```
2. **Describe the event:** copy the template and adjust it.
   ```powershell
   copy packaging\event.example.toml ..\netgame-2026\event.toml
   ```
   For example:
   ```toml
   event = "NetGame 2026"
   event_id = "netgame-2026"        # names the download files
   valid_from = "2026-11-01"        # local dates, inclusive
   valid_until = "2026-11-05"

   [enforced]                       # signed, fixed during the event
   discovery = "10.0.0.5"
   max = "1280x720"
   fps = 60

   [defaults]                       # unsigned, participants may change them
   level = "unity"
   border = true
   ```
   All keys from the table above may go into either section. `discovery = ""` enforces automatic discovery (mDNS).
3. **Build the packages:**
   ```powershell
   powershell -File tools\package.ps1 -EventConfig ..\netgame-2026\event.toml -SigningKey ..\netgame-2026\secret.key
   ```
   Besides the normal packages, this creates:
   - `dist\LANcast-<ver>-<event_id>-win64.zip`: the event edition plus `LANcast.event`
   - `dist\LANcast-<ver>-<event_id>-setup.exe`: the same as an installer ("LANcast (<event> edition)"; replaces a normal LANcast install)
   - `dist\LANcast-<ver>-<event_id>-link.txt`: the signed event link for the website

To change settings later (e.g. a new discovery server), edit `event.toml` and sign again. The edition doesn't need to be rebuilt as long as the key stays the same:
```powershell
target\release\lancast-event sign ..\netgame-2026\event.toml ..\netgame-2026\secret.key ..\netgame-2026\LANcast.event   # prints the new link
target\release\lancast-event verify ..\netgame-2026\LANcast.event <public key>   # check a file
```
Participants get the new configuration from the new link, or from a new download (a changed `LANcast.event` is picked up at the next start).

## Testing a link without a website
```powershell
Start-Process "lancast://config?event=Test&max=1280x720&fps=30"
```
