# Landing page for LANcast event links

A brief for building a landing page on an event website: one shareable `https://` page that opens LANcast automatically if it is installed, and always offers both **Open LANcast** and **Download LANcast**. It is written so that it can be handed as-is to a developer (or a coding agent) working on the website. It is framework-agnostic; adapt the reference implementation at the end to the site's stack.

Background: [EVENT_LINKS.md](EVENT_LINKS.md) describes the `lancast://` links themselves.

## Which link and which download
A LAN event normally uses an **event edition** of LANcast ([EVENT_LINKS.md](EVENT_LINKS.md#event-edition-lan-distro)). This brief assumes one:
- **Link:** the signed `lancast://event?p=…&s=…` link, the last line of `LANcast-<ver>-<event_id>-link.txt` from the packaging (or printed by `lancast-event sign`). Copy it verbatim: `p` and `s` must not change, or the signature becomes invalid.
- **Download:** the event edition's setup `LANcast-<ver>-<event_id>-setup.exe`, not the normal release. It replaces a normal LANcast install.
- **What the link is for:** the event-edition download already contains the configuration (`LANcast.event`) and applies it at the first start. The link matters for participants who downloaded earlier and need a configuration that changed after their download (re-signed). It also helps a participant whose LANcast doesn't have the configuration yet.
- **Changes during the event:** sign again and replace the link in the site configuration; the page URL stays the same. Also refresh the download (re-run the packaging or the *Event package* workflow), so new downloads contain the new `LANcast.event`.

Without an event edition, the same page works with an unsigned `lancast://config?…` link and the normal setup. Then LANcast's confirmation dialog is the only safeguard.

## Signing on the website server (optional)
Instead of pasting a link from `lancast-event sign`, the website can create the signed link itself, e.g. from an organizer admin form. The format, the cryptography (Ed25519, no encryption) and a dependency-free TypeScript module for signing **and** verifying are in [EVENT_LINKS.md › Signed links](EVENT_LINKS.md#signed-links-format-signing-and-verifying-yourself). Requirements for the website:
- **Key handling:** the content of the event's `secret.key` lives only in the server's secret store (e.g. environment variable `LANCAST_SIGNING_KEY`). It never goes into client code, the repository, logs or a download. The public key (`LANCAST_PUBLIC_KEY`, from `lancast-event pubkey`) may be public.
- **Who signs:** only authenticated organizers. There must be no endpoint that signs input from the public; that would let anyone forge configurations.
- **What is signed:** event name, `event_id`, validity dates and the enforced settings, validated with `checkSetting` before signing. Defaults are appended unsigned.
- **When:** sign once per configuration change and store the link as site configuration; the landing page only reads the stored link. Don't sign per page view.
- **Self-check:** after signing, and when the site starts, run `verifyEventLink` on the stored link with the public key. Show the organizers the decoded configuration and whether it is valid today (`validOn(localDate())`); refuse to publish a link that doesn't verify.
- **Tests:** the implementation must reproduce the test vector in EVENT_LINKS.md exactly, and reject the vector's link after changing one character of `p` or `s`.
- **Downloads:** a new configuration also needs a refreshed event download (see *Changes during the event* above). The website can't do that; the packaging or the *Event package* workflow does.
- **Older links stay valid:** a link signed earlier remains usable until its `valid_until`. Keep validity windows short.

## Why a landing page
- Custom schemes like `lancast://` do nothing useful when LANcast is not installed: depending on the browser the click is silently ignored or shows an error. Browsers deliberately don't tell a web page whether a scheme handler exists.
- Chat apps (Discord, WhatsApp, …) don't make `lancast://` clickable, but they do make `https://` links clickable.
- So participants get **one `https://` link** (or a QR code on the projector). The page tries LANcast and, in any case, shows the download.

## Inputs (site configuration)
| Name | Example | Notes |
|---|---|---|
| `LANCAST_LINK` | `lancast://event?p=eyJ2IjoxLC4uLn0&s=3q2-7w...&level=unity` | The signed event link, verbatim (see above). Without an event edition: `lancast://config?event=NetGame%202026&discovery=10.0.0.5&max=1280x720&fps=60`. |
| `DOWNLOAD_URL` | `https://example.org/downloads/LANcast-0.2.0-netgame-2026-setup.exe` | The event edition's setup. Without an event edition: the normal setup, or `https://github.com/netgame-switzerland/lancast/releases/latest`. |
| `EVENT_NAME` | `NetGame 2026` | Display only. |

## Required behavior
1. **Both buttons are always visible**, from the first paint, without JavaScript:
   - **Open LANcast**: a plain `<a href="LANCAST_LINK">`. A real click is a user gesture, which every browser accepts for external schemes.
   - **Download LANcast**: a plain `<a href="DOWNLOAD_URL">`.
   - Short steps: "1. Install the NetGame 2026 edition of LANcast (Windows). 2. Click *Open LANcast* and confirm the browser prompt. 3. In LANcast, click *Apply*." Mention that participants who installed it earlier should click *Open LANcast* once to get the current configuration.
2. **Automatic attempt on load**, once:
   - On Windows desktop browsers only (see *Platform* below), shortly after load, navigate to `LANCAST_LINK` once (`window.location.href = LANCAST_LINK`).
   - At most once per browser tab session, so reloads and the back button don't re-trigger it (`sessionStorage`, wrapped in try/catch; without storage simply attempt once per load).
   - Skippable with `?noauto` in the page URL (useful for organizers and testing).
3. **Soft feedback, never an automatic redirect to the download:**
   - If the page loses focus or becomes hidden (`blur`, `visibilitychange`, `pagehide`) after the attempt, show: "LANcast should open now. Confirm the dialog in LANcast."
   - If nothing like that happens within about 2.5 s, show: "Nothing happened? Install LANcast first, then click *Open LANcast*." Also emphasize the download button visually. Do not say "not installed": the detection is a heuristic. The browser prompt may still be open, or the user may have declined it.
   - If focus loss happens later (the user accepted the prompt after the timeout), switch to the first message.
4. **Platform:**
   - LANcast is Windows-only for now. On other systems (macOS, Linux, Android, iOS) skip the automatic attempt and show a note: "LANcast runs on Windows 10/11." Keep both buttons.
   - Detect via `navigator.userAgentData?.platform` with a fallback to `navigator.userAgent` containing `Windows`. When unsure, behave as on Windows.
5. **Fallback (optional, recommended):** a collapsible "Can't open the link?" section. For an event edition: download and reinstall the event edition, which brings the current configuration (manual entry doesn't help, because the event edition only sends with a signed configuration). Without an event edition: list the values for manual entry in LANcast's settings (event name, discovery server, limits), written statically in the site configuration, never derived from untrusted input.

## Security requirements
- **The link is fixed site configuration.** Never build `LANCAST_LINK` from the page's query string, form input or any other request data. Otherwise `https://<event-site>/lancast?link=lancast://config?discovery=attacker` would turn the official site into a launcher for attacker-chosen settings. LANcast asks before applying, but the prompt would carry the event site's credibility.
- Don't reflect query parameters into the page.
- Emit the link HTML-escaped in attributes (`&` → `&amp;`) and as a JSON string in scripts (`JSON.stringify` / the framework's escaping). Don't concatenate it into inline script source.
- Serve the page over HTTPS. If the site has a Content Security Policy, the reference script needs no `eval` and can be moved to an external file.
- Use the signed `lancast://event?…` link: the event edition verifies it against its built-in key and rejects unsigned links. A normal LANcast opens a signed link too, but cannot check the signature; its dialog says so.

## Acceptance tests
Test with the current Chrome, Edge and Firefox on Windows, each in both states:

| State | Expected |
|---|---|
| Event edition installed | Browser prompt "Open LANcast?" appears on load; after confirming, LANcast shows its Apply/Cancel dialog with "Signed by the event organizers, valid … to …"; the page shows "LANcast should open now…". |
| Normal LANcast installed | Same flow, but the dialog says the signature is only checked by an event edition; the page's steps point to the event edition download. |
| LANcast installed, prompt declined | Page stays usable; *Open LANcast* button triggers the prompt again. |
| LANcast installed and already running | The running LANcast comes to the front with the dialog (no second instance). |
| LANcast not installed | Page stays as is, no error page; after ~2.5 s the install hint appears and the download is emphasized. |
| Reload / back button | No second automatic attempt in the same tab. |
| `?noauto` | No automatic attempt; buttons work. |
| macOS/Linux/phone | No automatic attempt; Windows note visible; buttons visible. |
| JavaScript disabled | Both buttons and the steps are visible and work. |

**Known risk to verify:** for an unknown scheme, a top-level `location.href` navigation may make some browser (historically Firefox) replace the page with an error page ("The address wasn't understood") instead of ignoring it. If a test shows that, use a hidden same-origin `<iframe>` with `src = LANCAST_LINK` for the automatic attempt in that browser. Keep the visible button as a plain link. Re-run the table afterwards. Browsers also change their external-protocol handling over time (e.g. requiring a user gesture). The page must stay correct when the automatic attempt is blocked, and the buttons guarantee that.

## Reference implementation
Plain HTML/JS, no dependencies, for an event edition. Replace the link, the download URL and the event name (or render them from the site configuration with proper escaping). The signed link below is a shortened placeholder; use the real one from `…-link.txt`.

```html
<section class="lancast" id="lancast">
  <h2>Stream your game with LANcast: NetGame 2026</h2>
  <ol>
    <li>Install the NetGame 2026 edition of LANcast (Windows 10/11).</li>
    <li>Click <em>Open LANcast</em> and confirm the browser prompt.</li>
    <li>In LANcast, click <em>Apply</em>.</li>
  </ol>
  <p>Installed it earlier? Click <em>Open LANcast</em> once to get the current configuration.</p>
  <p class="lancast-buttons">
    <a class="button primary" id="lancast-open"
       href="lancast://event?p=eyJ2IjoxLC4uLn0&amp;s=3q2-7w...&amp;level=unity">Open LANcast</a>
    <a class="button" id="lancast-download"
       href="https://example.org/downloads/LANcast-0.2.0-netgame-2026-setup.exe">Download LANcast</a>
  </p>
  <p class="lancast-status" id="lancast-status" role="status" aria-live="polite"></p>
  <details>
    <summary>Can't open the link?</summary>
    <p>Download and install the NetGame 2026 edition again; it brings the current configuration.</p>
  </details>
</section>

<script>
(() => {
  // The link comes from the href above (site configuration), never from the URL.
  const open = document.getElementById("lancast-open");
  const download = document.getElementById("lancast-download");
  const status = document.getElementById("lancast-status");
  const link = open.getAttribute("href");
  const say = (text) => { status.textContent = text; };

  const platform = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.userAgent || "";
  const known = /Windows|Mac|Linux|Android|iPhone|iPad|CrOS/i.test(platform);
  const windows = /Windows/i.test(platform) || !known;
  if (!windows) {
    say("LANcast runs on Windows 10/11.");
    return;
  }

  const params = new URLSearchParams(location.search);
  if (params.has("noauto")) return;

  const key = "lancast-auto:" + link;
  try {
    if (sessionStorage.getItem(key)) return;
    sessionStorage.setItem(key, "1");
  } catch (_) { /* no storage: attempt once per load */ }

  let left = false;
  const onLeave = () => {
    if (left) return;
    left = true;
    download.classList.remove("emphasized");
    say("LANcast should open now. Confirm the dialog in LANcast.");
  };
  window.addEventListener("blur", onLeave);
  window.addEventListener("pagehide", onLeave);
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") onLeave();
  });

  setTimeout(() => {
    window.location.href = link;
    setTimeout(() => {
      if (left) return;
      download.classList.add("emphasized");
      say("Nothing happened? Install LANcast first, then click “Open LANcast”.");
    }, 2500);
  }, 300);
})();
</script>
```

Style `.emphasized` (e.g. primary color, subtle pulse) to match the site.

## Sharing
- Share the landing page's `https://` URL, not the `lancast://` link.
- A QR code of that URL on the projector or the seat cards works well at the event.
- If the event's configuration changes (new discovery server, new signed link), update the site configuration only; the shared URL stays the same.
