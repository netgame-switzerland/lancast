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
- Maximum link length: 2048 characters.

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

## Testing a link without a website
```powershell
Start-Process "lancast://config?event=Test&max=1280x720&fps=30"
```
