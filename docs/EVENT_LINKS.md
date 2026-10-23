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

**The same link again** (D36): if the link names the event already in force with the same enforced settings, LANcast just comes up with the current settings, without the dialog. The defaults are not applied again, so changes the participant made are kept. The dialog appears again as soon as an enforced setting differs, the link is for another event, or the participant left the event. In an organizer or event edition, a link re-signed with only new dates is taken silently, so a longer validity takes effect.

A link never starts sending and never changes the system-wide NDI configuration (NDI Access Manager).

## Two sections: enforced and defaults
Every setting in an event configuration is in one of two sections:
- **Enforced:** fixed while the event is active. Participants see them disabled, marked *(set by the event)*. Limits (`max`, `fps`) cap the participant's choice, marked *(event limit …)*; the other keys replace it.
- **Defaults:** starting values, applied once when the event is applied. Participants can change them afterwards.

In a `lancast://config` link (normal edition), `discovery`, `groups`, `max` and `fps` are enforced, and `level`, `border` and `cursor` are defaults. In the [event edition](#event-edition-lan-distro) the organizer chooses for every key.

## Changing enforced settings during an event
Publish a new link; participants click it and confirm again. The new link replaces the event's enforced settings and sets its defaults once more. Which link LANcast accepts depends on the edition:

| | Normal LANcast | Event edition |
|---|---|---|
| Unsigned `lancast://config?…` | Accepted | Rejected |
| Signed `lancast://event?p=…&s=…` | Accepted, but the signature is **not checked** (no key); the dialog says "only an event edition checks the signature" | Accepted only if signed with the event's key; outside its dates Send stays disabled |
| To change enforced settings | New `config` link | Edit `event.toml`, [sign again](#building-an-event-edition), publish the new signed link (no rebuild while the key stays the same) |

So in a normal LANcast the only safeguard is the confirmation dialog; a signature counts only in an event edition. A website or [landing page](LANDING_PAGE.md) can keep its URL and just swap the link.

## Keys
All keys are optional, but a link must set at least one. Unknown keys are ignored. An invalid value rejects the whole link, so a link is never half-applied.

| Key | Values | Kind | Effect |
|---|---|---|---|
| `event` | text, max. 64 characters | info | Event name shown in the settings and the dialog. |
| `discovery` | `host[:port]`, comma-separated; empty = automatic (mDNS) | setting | NDI Discovery Server for LANcast's sender. Port 5959 if omitted. Receivers must use the same server. |
| `groups` | NDI group names, comma-separated: letters, digits, space, `.`, `-`, `_`, max. 64 characters each, 248 in all; empty = NDI's own setting (Access Manager) | setting | NDI send groups. LANcast's default is `Public,lancast`. Receivers list the stream only if they receive in one of these groups, e.g. enforce `NetGame` and set the projector PC's receive group to `NetGame` in NDI Access Manager, so it lists only the event's LANcast streams. Groups sort sources; they don't stop anyone from joining a group (see below). |
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
- **Event edition:** use the signed link instead: the last line of `…-link.txt`, or the link printed by `lancast-event sign`. Offer the event edition's setup as the download:
  ```html
  <a href="lancast://event?p=eyJ2IjoxLC4uLn0&amp;s=3q2-7w...&amp;level=unity">Configure LANcast for NetGame 2026</a>
  ```
  Copy it verbatim (`p` and `s` are base64url and need no further encoding). Don't edit `p` or `s`: any change makes the signature invalid. The keys after them are unsigned defaults.
- **Chat apps:** Discord, WhatsApp etc. usually do not make custom schemes like `lancast://` clickable. Post a link to the web page instead.
- **Fallback:** for people who cannot click the link, list the values on the page; they can enter them in LANcast's settings by hand.
- **Landing page:** one shareable `https://` page that opens LANcast automatically if installed and always offers *Open* and *Download*: see [LANDING_PAGE.md](LANDING_PAGE.md).

## Which LANcast opens the link
*Settings → Links* shows which copy opens `lancast://` links:
- **A different copy** (an older install, a moved ZIP folder): *Use this copy* registers the current one for your Windows user.
- **Not set up:** *Set up for this copy* registers it.

LANcast also re-registers itself automatically at startup if the registered program no longer exists. The registration lives in `HKEY_CURRENT_USER\Software\Classes\lancast`, or in `HKEY_LOCAL_MACHINE` for an all-users install; the uninstaller removes it. A registration for your Windows user wins over the all-users one, so an all-users install removes the installing user's own registration: the link opens the installed LANcast right after setup.

## Event edition ("LAN distro")
An event edition is a LANcast package built for one event. It **only sends with that event's configuration**, signed by the organizer:
- `LANcast.event` next to `LANcast.exe` (part of the event download; applied automatically), or
- a signed link `lancast://event?p=<payload>&s=<signature>&<defaults>` (asks first, like every link that changes the event's enforced settings).

Without a valid configuration (missing, not yet valid, expired, or not signed with the event's key), Send is disabled and a banner explains why. Unsigned `lancast://config` links are rejected. The window title reads *LANcast – <event> edition*.

**How it is secured:** Ed25519 signatures. The organizer keeps the private key (`secret.key`); the event edition contains only the public key. Changing anything in the signed part, e.g. raising the resolution limit, makes the signature invalid. The defaults are deliberately unsigned, because participants may change them anyway.

**What it does not do:** it cannot stop someone from running a normal LANcast or another NDI sender on the event network. Nor do NDI groups: any NDI software can send or receive in any group by its name (D39). Use the event's network setup (discovery server, switch limits) for that, and pick the sources at the receiver. The event edition makes the right configuration the default and stops tampering within it.

### Building an event edition
Keep `secret.key` and the event files **outside the repository**, e.g. in `%USERPROFILE%\lancast-netgame\` (`$dir` below), and back the key up in a password manager. A working copy is the wrong place: `git clean -fdx`, a fresh clone or deleting the checkout removes ignored files.

1. **Key pair**, once per event (or once per organizer, for an [organizer edition](#organizer-edition-one-build-for-all-events)). Keep `secret.key` private: never commit it, never put it into a download.
   ```powershell
   cargo build --release --bin lancast-event
   $dir = "$env:USERPROFILE\lancast-netgame"; mkdir $dir
   target\release\lancast-event keygen $dir\secret.key     # prints the public key
   ```
2. **Describe the event:** copy the template and adjust it.
   ```powershell
   copy packaging\event.example.toml $dir\event.toml
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
   powershell -File tools\package.ps1 -EventConfig $dir\event.toml -SigningKey $dir\secret.key
   ```
   Besides the normal packages, this creates:
   - `dist\LANcast-<ver>-<event_id>-win64.zip`: the event edition plus `LANcast.event`
   - `dist\LANcast-<ver>-<event_id>-setup.exe`: the same as an installer ("LANcast (<event> edition)"; replaces a normal LANcast install)
   - `dist\LANcast-<ver>-<event_id>-link.txt`: the signed event link for the website

To change settings later (e.g. a new discovery server), edit `event.toml` and sign again. The edition doesn't need to be rebuilt as long as the key stays the same:
```powershell
target\release\lancast-event sign $dir\event.toml $dir\secret.key $dir\LANcast.event   # prints the new link
target\release\lancast-event verify $dir\LANcast.event <public key>   # check a file
```
Participants get the new configuration from the new link, or from a new download (a changed `LANcast.event` is picked up at the next start).

### Building in GitHub Actions
A local build is a snapshot (D30). For the event download, build the event edition of a **released** version with the *Event package* workflow:
1. **Create a GitHub environment** for the event: repository *Settings → Environments → New environment*, e.g. `netgame-2026`. Add:
   - the variable `EVENT_TOML`: the whole content of `event.toml`
   - the secret `EVENT_SIGNING_KEY`: the content of `secret.key`

   Optionally restrict the environment to required reviewers, so that nobody else can build with the key.
2. **Run** *Actions → Event package → Run workflow* with the release tag (e.g. `v0.2.0`) and the environment name.
3. **Download** the workflow artifact. It has `LANcast-<ver>-<event_id>-win64.zip`, `…-setup.exe` and `…-link.txt`. With *attach* checked, the ZIP and the setup are also added to the GitHub release; the event edition contains only the public key and the signed configuration, so it is safe to publish.

The key only exists as a GitHub secret and as a temporary file on the runner, deleted after signing.

### Organizer edition (one build for all events)
An organizer who runs several events can use **one key pair for all of them** and build the edition once per release, instead of once per event (D35). The organizer edition is the same exe, named after the organizer (*LANcast – netgame edition*), but without a bundled `LANcast.event`. Until a participant clicks a signed event link of that organizer (or puts a `LANcast.event` next to the exe), Send stays disabled with a banner. For each event, only `event.toml` is signed with the organizer's key; nothing is rebuilt.

Its setup registers `lancast:` links but doesn't offer to start LANcast at the end, since LANcast couldn't send yet. The last page says to open the event website and click the LANcast link there, which starts LANcast with the event's configuration.

**The organizer key** is created once with `lancast-event keygen` (see [Building an event edition](#building-an-event-edition), step 1) and used for years. Back it up in a password manager (it is one line of text) and keep it outside the repository. If it is lost or leaked, every event of the organizer is affected: create a new key, set the new public key and release a new organizer edition, which participants have to install.

Building it needs only the **public key**, no secret:
```powershell
powershell -File tools\package.ps1 -Edition netgame -PublicKey <public key>
```
This creates `dist\LANcast-<ver>-netgame-win64.zip` and `…-setup.exe` (with `-EventOnly`, only those). With `-SigningKey` instead of `-PublicKey`, the public key is taken from `secret.key`. Together with `-EventConfig`, `-Edition` names an event edition after the organizer, still with the event's `LANcast.event` bundled.

**In every GitHub release:** set the repository variables (*Settings → Secrets and variables → Actions → Variables*) `ORGANIZER_EDITION` (e.g. `netgame`) and `ORGANIZER_PUBLIC_KEY` (the output of `lancast-event pubkey`). The *Release* workflow then also builds the organizer edition and attaches it to the release. Both values are public; the private key stays with the organizer, who signs each event's configuration locally or on the event website ([below](#signed-links-format-signing-and-verifying-yourself)).

Trade-off: anyone holding the organizer's key can sign configurations for every event of that organizer, and a lost key affects all of them. Changing the key needs a new organizer edition (set the new public key and release again).

### Personal links (participant name)
An organizer or event edition can take the participant's name from the link instead of the Windows login name (D38). The event website gives every participant their own link: the event's signed link plus a signed **user id**, `&id=<payload>.<signature>`. The stream name then reads `HOST (<name> - <game>)`, with the computer name added by NDI as always.
- **Fixed while the event is in force.** The name field shows the name, marked *(set by the event)*. The participant's own name is kept and comes back when another event replaces this one.
- **Persists:** the id is stored with the event configuration. Every link of the same event (`event_id`) keeps it, including the generic link without `id=` (for example a landing page, or a new link after changing the limits). A link for another event drops it.
- **Asks once:** the first personal link shows the name in the confirmation dialog. The same link again opens without a question (D36). A different name asks again.
- **Signed and bound to the event:** the id is signed with the same key as the event configuration and names its `event_id`. LANcast rejects the whole link if the id is not signed with the edition's key or belongs to another event. A participant cannot make up a name or move an id to another event.
- **Normal edition:** ignores `id=` (it can't check the signature) and keeps the participant's own name.
- **Names:** 1–32 characters, no control characters, no leading or trailing spaces.

For a test, or a few participants without a website:
```powershell
target\release\lancast-event user $dir\LANcast.event "Alice" $dir\secret.key   # prints Alice's personal link
```
A website signs the id itself, once per participant and event ([below](#personal-user-ids)).

**What it does not prove:** the name in the stream is public on the LAN, like every NDI source name. Another NDI sender (OBS, a normal LANcast) can announce the same name. The signature stops participants from choosing a name in the edition; it does not authenticate the stream at the receiver.

### Signed links: format, signing and verifying yourself
`lancast-event sign` is the reference, but any program can create and check signed links, e.g. an event website that lets the organizers change the configuration. This section is the complete format.

**Cryptography: a signature, no encryption.** The configuration is plain, readable JSON; anyone can decode it. The signature only proves that it comes from the holder of the private key and that nothing was changed. Never put secrets into it.
- Algorithm: **Ed25519** (RFC 8032, plain Ed25519: no pre-hashing, no context string). Signatures are deterministic: the same key and payload always give the same signature.
- **Secret key** (`secret.key`): the 32-byte Ed25519 seed, base64url without padding (43 characters), plus a newline.
- **Public key**: the 32-byte Ed25519 public key, base64url without padding. Printed by `lancast-event keygen` and `lancast-event pubkey secret.key`; compiled into the event edition. It is safe to publish.

**Payload:** a UTF-8 JSON object.

| Field | Type | Rule |
|---|---|---|
| `v` | number | Always `1`. |
| `event` | string | Display name, 1–64 characters, no control characters. |
| `event_id` | string | Letters, digits, `-`, `_` (names the download files). |
| `valid_from`, `valid_until` | string | Local dates `YYYY-MM-DD`, inclusive; `valid_from` ≤ `valid_until`. |
| `enforced` | object | Settings from the [key table](#keys), **all values as strings** (`"fps": "60"`, `"border": "1"`). |

**Link:** `lancast://event?p=<payload>&s=<signature>[&<defaults>][&id=<user id>]`
- `p` = base64url (no padding) of the exact payload bytes.
- `s` = base64url (no padding) of the 64-byte Ed25519 signature over **those bytes**. The signature covers bytes, not JSON semantics: key order and whitespace are free when signing, but a verifier must check the decoded `p` bytes as they are and never re-serialize the JSON.
- Defaults follow as unsigned `&key=value` pairs, percent-encoded, with the keys from the key table.
- `id` makes it a [personal link](#personal-links-participant-name); see [Personal user ids](#personal-user-ids).
- At most 4096 characters.

The `LANcast.event` file carries the same data as TOML: `payload = "<p>"`, `signature = "<s>"`, and a `[defaults]` table.

**What LANcast checks:**
- An event edition accepts a signed link only if `s` is a valid signature of `p` with its built-in public key.
- `v` must be 1, and every value must be valid; an invalid value rejects the whole link. Unknown keys are ignored by LANcast but refused by `lancast-event`; don't use them.
- The dates are checked against the participant's local date when sending, not when the link is clicked. Outside the window, Send is disabled.

**Limits of the scheme:**
- There is no revocation and no "newest link wins". Every link signed with the key stays valid until its `valid_until`, so a participant could apply an older link with looser limits again. Keep validity windows short and limits consistent.
- The public key is compiled into the event edition: a new key needs a new edition.

**Protecting the key on a website:**
- The private key exists only on the server (a secret store or environment variable), never in client code, the repository or a download.
- Only organizers can trigger signing (authenticated admin form). An endpoint that signs arbitrary input would let anyone forge configurations.
- Validate before signing (see `checkSetting` below), sign once per change and store the resulting link; don't sign per page view.

#### TypeScript (Node.js 18+)
No dependencies; uses `node:crypto` (backed by OpenSSL). Node takes keys as DER, so the raw 32-byte keys are wrapped in the fixed RFC 8410 headers.

```ts
// lancast-link.ts: create and verify signed LANcast event links.
import { createPrivateKey, createPublicKey, sign, verify, type KeyObject } from "node:crypto";

// DER headers that wrap raw 32-byte Ed25519 keys (RFC 8410).
const PKCS8_ED25519 = Buffer.from("302e020100300506032b657004220420", "hex");
const SPKI_ED25519 = Buffer.from("302a300506032b6570032100", "hex");
const BASE64URL = /^[A-Za-z0-9_-]+$/;
const DATE = /^\d{4}-\d{2}-\d{2}$/;
const BOOLEANS = ["1", "0", "true", "false", "yes", "no", "on", "off"];
const MAX_LINK = 4096;

export interface EventPayload {
  v: 1;
  event: string;
  event_id: string;
  valid_from: string; // YYYY-MM-DD, local date, inclusive
  valid_until: string;
  enforced: Record<string, string>;
}

export interface VerifiedLink {
  payload: EventPayload;
  defaults: Record<string, string>;
  validOn: (date: string) => boolean;
}

function rawKey(base64url: string, what: string): Buffer {
  const text = base64url.trim();
  const bytes = Buffer.from(text, "base64url");
  if (!BASE64URL.test(text) || bytes.length !== 32) throw new Error(`invalid ${what}`);
  return bytes;
}

/** The private key from the content of `secret.key`. */
export function privateKey(secretKeyFile: string): KeyObject {
  const der = Buffer.concat([PKCS8_ED25519, rawKey(secretKeyFile, "secret key")]);
  return createPrivateKey({ key: der, format: "der", type: "pkcs8" });
}

/** The public key as printed by `lancast-event pubkey`. */
export function publicKey(base64url: string): KeyObject {
  const der = Buffer.concat([SPKI_ED25519, rawKey(base64url, "public key")]);
  return createPublicKey({ key: der, format: "der", type: "spki" });
}

/** Throws unless `key=value` is a valid LANcast setting (same rules as LANcast). */
export function checkSetting(key: string, value: unknown): void {
  if (typeof value !== "string") throw new Error(`${key}: values are strings, e.g. "60"`);
  const v = value.trim();
  switch (key.toLowerCase()) {
    case "discovery": // comma-separated host[:port]; empty = automatic (mDNS)
      for (const entry of v.split(",").map((e) => e.trim()).filter(Boolean)) {
        if (!/^[A-Za-z0-9.:_-]{1,253}$/.test(entry)) throw new Error(`invalid discovery server ${entry}`);
      }
      return;
    case "groups": { // comma-separated NDI groups; empty = NDI's own setting
      const names = v.split(",").map((g) => g.trim()).filter(Boolean);
      for (const name of names) {
        if (!/^[A-Za-z0-9 ._-]{1,64}$/.test(name)) throw new Error(`invalid NDI group ${name}`);
      }
      const unique = names.filter((g, i) => names.findIndex((o) => o.toLowerCase() === g.toLowerCase()) === i);
      if (unique.join(",").length > 248) throw new Error("the NDI group list is longer than 248 characters");
      return;
    }
    case "max": {
      const m = /^(\d+)\s*[xX]\s*(\d+)$/.exec(v);
      const [w, h] = m ? [Number(m[1]), Number(m[2])] : [0, 0];
      if (w < 320 || w > 3840 || h < 180 || h > 2160) throw new Error("max must be 320x180 … 3840x2160");
      return;
    }
    case "fps":
      if (!/^\d+$/.test(v) || Number(v) < 10 || Number(v) > 120) throw new Error("fps must be 10 … 120");
      return;
    case "level":
      if (!["unity", "obs", "ndi", "studio-monitor"].includes(v.toLowerCase())) throw new Error("level must be unity or ndi");
      return;
    case "border":
    case "cursor":
      if (!BOOLEANS.includes(v.toLowerCase())) throw new Error(`${key} must be 1 or 0`);
      return;
    default:
      throw new Error(`unknown setting ${key}`);
  }
}

function checkPayload(p: EventPayload): void {
  if (p?.v !== 1) throw new Error("unsupported payload version");
  if (typeof p.event !== "string" || !p.event.trim() || [...p.event].length > 64 || /\p{Cc}/u.test(p.event)) {
    throw new Error("event: 1-64 characters, no control characters");
  }
  if (typeof p.event_id !== "string" || !/^[A-Za-z0-9_-]+$/.test(p.event_id)) throw new Error("invalid event_id");
  if (!DATE.test(p.valid_from) || !DATE.test(p.valid_until) || p.valid_from > p.valid_until) {
    throw new Error("valid_from/valid_until must be YYYY-MM-DD, from <= until");
  }
  if (typeof p.enforced !== "object" || p.enforced === null) throw new Error("enforced must be an object");
  for (const [key, value] of Object.entries(p.enforced)) checkSetting(key, value);
}

/** Signs `payload` and returns the complete link, with `defaults` appended unsigned. */
export function signEventLink(payload: EventPayload, defaults: Record<string, string>, secret: KeyObject): string {
  checkPayload(payload);
  for (const [key, value] of Object.entries(defaults)) checkSetting(key, value);
  const bytes = Buffer.from(JSON.stringify(payload), "utf8");
  const signature = sign(null, bytes, secret); // Ed25519: the algorithm argument must be null
  let link = `lancast://event?p=${bytes.toString("base64url")}&s=${signature.toString("base64url")}`;
  for (const [key, value] of Object.entries(defaults)) link += `&${key}=${encodeURIComponent(value)}`;
  if (link.length > MAX_LINK) throw new Error("the link is too long");
  return link;
}

/** Verifies a signed link with the event's public key; throws if it is invalid. */
export function verifyEventLink(link: string, key: KeyObject): VerifiedLink {
  const text = link.trim();
  if (text.length > MAX_LINK) throw new Error("the link is too long");
  const m = /^lancast:(?:\/\/)?event\/?\?(.*)$/i.exec(text);
  if (!m) throw new Error("not a signed lancast://event link");
  const params = new URLSearchParams(m[1]);
  const p = params.get("p") ?? "";
  const s = params.get("s") ?? "";
  if (!BASE64URL.test(p) || !BASE64URL.test(s)) throw new Error("p= and s= must be base64url");
  const bytes = Buffer.from(p, "base64url");
  const signature = Buffer.from(s, "base64url");
  // Verify the transmitted bytes; never re-serialize the JSON before this.
  if (signature.length !== 64 || !verify(null, bytes, key, signature)) {
    throw new Error("the signature is not valid for this key");
  }
  const payload = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)) as EventPayload;
  checkPayload(payload);
  const defaults: Record<string, string> = {};
  for (const [k, v] of params) {
    if (k === "p" || k === "s") continue;
    checkSetting(k, v);
    defaults[k.toLowerCase()] = v;
  }
  return {
    payload,
    defaults,
    validOn: (date) => payload.valid_from <= date && date <= payload.valid_until,
  };
}

/** Today's local date as YYYY-MM-DD (the server's time zone must be the event's). */
export function localDate(d = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}
```

Usage:
```ts
const secret = privateKey(process.env.LANCAST_SIGNING_KEY!); // content of secret.key
const link = signEventLink(
  {
    v: 1,
    event: "NetGame 2026",
    event_id: "netgame-2026",
    valid_from: "2026-11-01",
    valid_until: "2026-11-05",
    enforced: { discovery: "10.0.0.5", max: "1280x720", fps: "60" },
  },
  { level: "unity" },
  secret,
);

const checked = verifyEventLink(link, publicKey(process.env.LANCAST_PUBLIC_KEY!));
console.log(checked.payload.enforced, checked.defaults, checked.validOn(localDate()));
```
- `verifyEventLink` is stricter than LANcast: it also refuses unknown keys among the defaults, like `lancast-event`.
- Other runtimes: without `node:crypto` (e.g. some edge runtimes), use Web Crypto's `Ed25519` (raw key import for the public key, PKCS#8 for the private key with the header above) or a library such as `@noble/ed25519`. The format stays the same.

#### Personal user ids
The `id` parameter of a [personal link](#personal-links-participant-name) (D38) is `<payload>.<signature>`. Both parts are base64url without padding, and the dot is not part of that alphabet. The signature is Ed25519 over the payload bytes, with the **same key** as the event configuration.

| Field | Type | Rule |
|---|---|---|
| `v` | number | Always `1`. |
| `event_id` | string | The `event_id` of the event payload in the same link. |
| `user` | string | 1–32 characters (Unicode code points), no control characters, no leading or trailing spaces. |

- The two payload types can't be mixed up: an id has no `event`/dates, an event payload no `user`, and LANcast rejects either in the other's place.
- Sign once per participant and event, and store the link. The name is readable by anyone (no encryption), so use the participant's nickname, not private data.

```ts
export interface UserPayload {
  v: 1;
  event_id: string; // the event payload's event_id
  user: string;
}

const MAX_USER = 32;

/** Throws unless `user` is a valid name for the stream name (same rules as LANcast). */
export function checkUser(user: unknown): asserts user is string {
  if (typeof user !== "string" || !user.trim() || user.trim() !== user || [...user].length > MAX_USER || /\p{Cc}/u.test(user)) {
    throw new Error(`user: 1-${MAX_USER} characters, no control characters or surrounding spaces`);
  }
}

/** The `id=` value: `user`, signed for one event. */
export function signUserId(payload: UserPayload, secret: KeyObject): string {
  if (payload?.v !== 1 || typeof payload.event_id !== "string" || !/^[A-Za-z0-9_-]+$/.test(payload.event_id)) {
    throw new Error("invalid user id payload");
  }
  checkUser(payload.user);
  const bytes = Buffer.from(JSON.stringify(payload), "utf8");
  return `${bytes.toString("base64url")}.${sign(null, bytes, secret).toString("base64url")}`;
}

/** A participant's personal link: the event's signed link plus the id. */
export function personalLink(eventLink: string, id: string): string {
  const link = `${eventLink}&id=${id}`;
  if (link.length > MAX_LINK) throw new Error("the link is too long");
  return link;
}

/** Verifies an `id=` value for `eventId` and returns the user name. */
export function verifyUserId(id: string, eventId: string, key: KeyObject): string {
  const [p, s, ...rest] = id.trim().split(".");
  if (rest.length || !BASE64URL.test(p ?? "") || !BASE64URL.test(s ?? "")) throw new Error("id must be <payload>.<signature>");
  const bytes = Buffer.from(p, "base64url");
  const signature = Buffer.from(s, "base64url");
  if (signature.length !== 64 || !verify(null, bytes, key, signature)) throw new Error("the id signature is not valid for this key");
  const payload = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)) as UserPayload;
  if (payload?.v !== 1 || payload.event_id !== eventId) throw new Error("the id belongs to another event");
  checkUser(payload.user);
  return payload.user;
}
```

Usage, with `link` from the example above:
```ts
const id = signUserId({ v: 1, event_id: "netgame-2026", user: "Alice" }, secret);
const alicesLink = personalLink(link, id);
```

#### Test vector
Ed25519 is deterministic, so an implementation must reproduce this exactly. **Test key only, never use it for an event.**
- Secret key: `AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA` (bytes `01 02 … 20`)
- Public key: `ebVWLo_mVPlAeLES6KmLp5AfhTrmlb7X4OORC60ElmQ`
- Payload bytes, the JSON exactly as written, without whitespace (this is what `JSON.stringify` of the usage example above produces):
  `{"v":1,"event":"NetGame 2026","event_id":"netgame-2026","valid_from":"2026-11-01","valid_until":"2026-11-05","enforced":{"discovery":"10.0.0.5","max":"1280x720","fps":"60"}}`
- With the default `level=unity`, the link is:
  ```
  lancast://event?p=eyJ2IjoxLCJldmVudCI6Ik5ldEdhbWUgMjAyNiIsImV2ZW50X2lkIjoibmV0Z2FtZS0yMDI2IiwidmFsaWRfZnJvbSI6IjIwMjYtMTEtMDEiLCJ2YWxpZF91bnRpbCI6IjIwMjYtMTEtMDUiLCJlbmZvcmNlZCI6eyJkaXNjb3ZlcnkiOiIxMC4wLjAuNSIsIm1heCI6IjEyODB4NzIwIiwiZnBzIjoiNjAifX0&s=etDE8XfuKaeE9rJ5o-oZ0rrMWqqHs-RiFLtvxukEJKx15EhFniYFq6_v2-i06EfDpzVVnAdwM7vm6IAGDZrCBg&level=unity
  ```
- User id for `Alice`, payload bytes `{"v":1,"event_id":"netgame-2026","user":"Alice"}`:
  ```
  eyJ2IjoxLCJldmVudF9pZCI6Im5ldGdhbWUtMjAyNiIsInVzZXIiOiJBbGljZSJ9.sMT4LH6taUK0O8TYaUkl_iznszzEAgOXBF3gAEgfma6DPijanfeidETBPdNZ239TFfwgercD8nQ6NwvbR2TcAw
  ```
  Alice's personal link is the link above plus `&id=` and this value. `lancast-event user` with a `LANcast.event` of the link above and the test key prints exactly that.

**Cross-check with the reference tool:** `lancast-event verify` reads a `LANcast.event` file. Write `p` and `s` from a link into one and check it with the public key:
```powershell
@"
payload = "<p>"
signature = "<s>"

[defaults]
level = "unity"
"@ | Set-Content test.event
target\release\lancast-event verify test.event <public key>
```

## Testing a link without a website
```powershell
Start-Process "lancast://config?event=Test&max=1280x720&fps=30"
```
A signed link for an event edition, from the file the packaging wrote:
```powershell
Start-Process (Get-Content dist\LANcast-<ver>-<event_id>-link.txt | Select-Object -Last 1)
```
A normal LANcast also opens a signed link, but cannot check its signature; test signed links with the event edition.
