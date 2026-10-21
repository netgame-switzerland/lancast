# Landing page for LANcast event links

A brief for building a landing page on an event website: one shareable `https://` page that opens LANcast automatically if it is installed, and always offers both **Open LANcast** and **Download LANcast**. It is written so that it can be handed as-is to a developer (or a coding agent) working on the website. It is framework-agnostic; adapt the reference implementation at the end to the site's stack.

Background: [EVENT_LINKS.md](EVENT_LINKS.md) describes the `lancast://` links themselves.

## Which link and which download
A LAN event normally uses an edition of LANcast that sends only with a configuration signed by the organizer ([EVENT_LINKS.md](EVENT_LINKS.md#event-edition-lan-distro)). There are two kinds; this brief covers both:

| | **Organizer edition** (D35) | **Event edition** (D28) |
|---|---|---|
| Built | Once per release, for all of the organizer's events | Once per event |
| Download | `LANcast-<ver>-<organizer>-setup.exe` (e.g. `…-netgame-setup.exe`), attached to the GitHub release | `LANcast-<ver>-<event_id>-setup.exe`, from the packaging or the *Event package* workflow |
| Configuration in the download | None | `LANcast.event`, applied at the first start |
| Role of the link | **Required:** the only way a participant gets the event's configuration | Refresh: for participants who downloaded before the configuration changed |
| After a configuration change | Sign again, replace the link | Sign again, replace the link, refresh the download |

Both editions replace a normal LANcast install.

- **Link:** the signed `lancast://event?p=…&s=…` link. It is printed by `lancast-event sign` (event edition packaging: the last line of `LANcast-<ver>-<event_id>-link.txt`), or created by the website itself ([below](#signing-on-the-website-server-optional)). Copy it verbatim: `p` and `s` must not change, or the signature becomes invalid.
- **Download:** the edition's setup, not the normal release. GitHub's `releases/latest/download/<file>` only works for file names without a version, so link the asset of a specific release (`https://github.com/netgame-switzerland/lancast/releases/download/v<ver>/LANcast-<ver>-netgame-setup.exe`) or host a copy on the event site.
- **Organizer edition across events:** participants who installed it for an earlier event keep it. They only click this event's link, which replaces the old configuration. The page should say so.
- **Changes during the event:** sign again and replace the link in the site configuration; the page URL stays the same. With an event edition, also refresh the download (re-run the packaging or the *Event package* workflow), so new downloads contain the new `LANcast.event`.

Without either edition, the same page works with an unsigned `lancast://config?…` link and the normal setup. Then LANcast's confirmation dialog is the only safeguard, and participants can change the values afterwards.

## Signing on the website server (optional)
Instead of pasting a link from `lancast-event sign`, the website can create the signed link itself, e.g. from an organizer admin form. The format, the cryptography (Ed25519, no encryption) and a dependency-free TypeScript module for signing **and** verifying are in [EVENT_LINKS.md › Signed links](EVENT_LINKS.md#signed-links-format-signing-and-verifying-yourself). Requirements for the website:
- **Key handling:** the content of `secret.key` lives only in the server's secret store (e.g. environment variable `LANCAST_SIGNING_KEY`). It never goes into client code, the repository, logs or a download. The public key (`LANCAST_PUBLIC_KEY`, from `lancast-event pubkey`) may be public.
- **Organizer edition:** the server holds the organizer's one key, which signs configurations for **all** of the organizer's events. A leaked key affects every event and needs a new organizer edition with a new key. Protect it accordingly: only on the server that needs it, with access limited to the organizers.
- **Who signs:** only authenticated organizers. There must be no endpoint that signs input from the public; that would let anyone forge configurations.
- **What is signed:** event name, `event_id`, validity dates and the enforced settings, validated with `checkSetting` before signing. Defaults are appended unsigned.
- **When:** sign once per configuration change and store the link as site configuration; the landing page only reads the stored link. Don't sign per page view.
- **Self-check:** after signing, and when the site starts, run `verifyEventLink` on the stored link with the public key. Show the organizers the decoded configuration and whether it is valid today (`validOn(localDate())`); refuse to publish a link that doesn't verify.
- **Tests:** the implementation must reproduce the test vector in EVENT_LINKS.md exactly, and reject the vector's link after changing one character of `p` or `s`.
- **Downloads:** with an organizer edition, nothing else changes. With an event edition, a new configuration also needs a refreshed event download (see *Changes during the event* above). The website can't do that; the packaging or the *Event package* workflow does.
- **Older links stay valid:** a link signed earlier remains usable until its `valid_until`. Keep validity windows short; with an organizer edition, a link of a past event is otherwise still accepted.

## Why a landing page
- Custom schemes like `lancast://` do nothing useful when LANcast is not installed: depending on the browser the click is silently ignored or shows an error. Browsers deliberately don't tell a web page whether a scheme handler exists.
- Chat apps (Discord, WhatsApp, …) don't make `lancast://` clickable, but they do make `https://` links clickable.
- So participants get **one `https://` link** (or a QR code on the projector). The page tries LANcast and, in any case, shows the download.

## Inputs (site configuration)
| Name | Example | Notes |
|---|---|---|
| `LANCAST_LINK` | `lancast://event?p=eyJ2IjoxLC4uLn0&s=3q2-7w...&level=unity` | The signed event link, verbatim (see above). Without an edition: `lancast://config?event=NetGame%202026&discovery=10.0.0.5&max=1280x720&fps=60`. |
| `DOWNLOAD_URL` | `https://github.com/netgame-switzerland/lancast/releases/download/v0.2.0/LANcast-0.2.0-netgame-setup.exe` | The edition's setup: organizer edition (as in the example) or event edition (`…-netgame-2026-setup.exe`). Without an edition: the normal setup, or `https://github.com/netgame-switzerland/lancast/releases/latest`. |
| `EDITION_NAME` | `netgame` | The edition's name as LANcast shows it (*LANcast – netgame edition*): the organizer name, or for an event edition the event name. Display only. |
| `EVENT_NAME` | `NetGame 2026` | Display only. |
| `EVENT_FILE_URL` | `https://example.org/lancast/LANcast.event` | Optional, for the fallback: the signed `LANcast.event` written by `lancast-event sign` together with the link. Public; it contains the same signed configuration as the link. |

## Required behavior
1. **Both buttons are always visible**, from the first paint, without JavaScript:
   - **Open LANcast**: a plain `<a href="LANCAST_LINK">`. A real click is a user gesture, which every browser accepts for external schemes.
   - **Download LANcast**: a plain `<a href="DOWNLOAD_URL">`.
   - Short steps: "1. Install the netgame edition of LANcast (Windows). 2. Click *Open LANcast* and confirm the browser prompt. 3. In LANcast, click *Apply* (asked only the first time, or when the event's settings changed)."
   - Below the steps: "Already installed, e.g. for an earlier event? Just click *Open LANcast* to load the NetGame 2026 configuration." With an organizer edition, step 2 is what makes LANcast send; say that it is needed even right after installing.
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
5. **Fallback (optional, recommended):** a collapsible "Can't open the link?" section. Manual entry in LANcast's settings doesn't help with either edition, because they only send with a signed configuration.
   - **With `EVENT_FILE_URL`** (both editions): "Download `LANcast.event`, put it into the LANcast folder next to `LANcast.exe`, and restart LANcast." The installer's default folders are `%LOCALAPPDATA%\Programs\LANcast` (installed for the current user) and `C:\Program Files\LANcast` (for all users; copying there needs admin rights).
   - **Event edition:** alternatively download and reinstall the event edition, which brings the current configuration. Reinstalling an organizer edition doesn't help: it contains no configuration.
   - **Without an edition:** list the values for manual entry in LANcast's settings (event name, discovery server, limits), written statically in the site configuration, never derived from untrusted input.

## Security requirements
- **The link is fixed site configuration.** Never build `LANCAST_LINK` from the page's query string, form input or any other request data. Otherwise `https://<event-site>/lancast?link=lancast://config?discovery=attacker` would turn the official site into a launcher for attacker-chosen settings. LANcast asks before applying, but the prompt would carry the event site's credibility.
- Don't reflect query parameters into the page.
- Emit the link HTML-escaped in attributes (`&` → `&amp;`) and as a JSON string in scripts (`JSON.stringify` / the framework's escaping). Don't concatenate it into inline script source.
- Serve the page over HTTPS. If the site has a Content Security Policy, the reference script needs no `eval` and can be moved to an external file.
- Use the signed `lancast://event?…` link: organizer and event editions verify it against their built-in key and reject unsigned links. A normal LANcast opens a signed link too, but cannot check the signature; its dialog says so.

## Acceptance tests
Test with the current Chrome, Edge and Firefox on Windows, each in these states:

| State | Expected |
|---|---|
| Organizer or event edition installed | Browser prompt "Open LANcast?" appears on load; after confirming, LANcast shows its Apply/Cancel dialog with "Signed by the event organizers, valid … to …"; the page shows "LANcast should open now…". After *Apply*, Send is enabled. |
| Organizer edition freshly installed, link not yet clicked | LANcast shows the banner that it sends only with a signed event configuration, and Send is disabled; clicking *Open LANcast* and *Apply* enables it. |
| Organizer edition with the configuration of an earlier event | The link replaces it: the dialog shows this event, and after *Apply* the banner and limits are this event's. |
| Normal LANcast installed | Same flow, but the dialog says the signature is only checked by an event edition; the page's steps point to the edition download. |
| LANcast installed, prompt declined | Page stays usable; *Open LANcast* button triggers the prompt again. |
| LANcast installed and already running | The running LANcast comes to the front with the dialog (no second instance). |
| Same link again, after *Apply* | LANcast comes up without the dialog; the participant's own changes to the defaults are kept. |
| Link with changed enforced settings (re-signed) | The dialog appears again and shows the new values. |
| LANcast not installed | Page stays as is, no error page; after ~2.5 s the install hint appears and the download is emphasized. |
| Fallback with `EVENT_FILE_URL` | `LANcast.event` downloads; next to `LANcast.exe` and after a restart of LANcast, Send is enabled with this event's configuration. |
| Reload / back button | No second automatic attempt in the same tab. |
| `?noauto` | No automatic attempt; buttons work. |
| macOS/Linux/phone | No automatic attempt; Windows note visible; buttons visible. |
| JavaScript disabled | Both buttons and the steps are visible and work. |

**Known risk to verify:** for an unknown scheme, a top-level `location.href` navigation may make some browser (historically Firefox) replace the page with an error page ("The address wasn't understood") instead of ignoring it. If a test shows that, use a hidden same-origin `<iframe>` with `src = LANCAST_LINK` for the automatic attempt in that browser. Keep the visible button as a plain link. Re-run the table afterwards. Browsers also change their external-protocol handling over time (e.g. requiring a user gesture). The page must stay correct when the automatic attempt is blocked, and the buttons guarantee that.

## Reference implementation
Plain HTML/JS, no dependencies, for an organizer edition named `netgame`. Replace the link, the download URL, the names and the event file URL (or render them from the site configuration with proper escaping). The signed link below is a shortened placeholder; use the real one. For an event edition, swap the download for the event edition's setup and offer its reinstall in the fallback.

```html
<section class="lancast" id="lancast">
  <h2>Stream your game with LANcast: NetGame 2026</h2>
  <ol>
    <li>Install the netgame edition of LANcast (Windows 10/11).</li>
    <li>Click <em>Open LANcast</em> and confirm the browser prompt. LANcast sends only after this step.</li>
    <li>In LANcast, click <em>Apply</em>.</li>
  </ol>
  <p>Already installed, e.g. for an earlier event? Just click <em>Open LANcast</em> to load the NetGame 2026 configuration.</p>
  <p class="lancast-buttons">
    <a class="button primary" id="lancast-open"
       href="lancast://event?p=eyJ2IjoxLC4uLn0&amp;s=3q2-7w...&amp;level=unity">Open LANcast</a>
    <a class="button" id="lancast-download"
       href="https://github.com/netgame-switzerland/lancast/releases/download/v0.2.0/LANcast-0.2.0-netgame-setup.exe">Download LANcast</a>
  </p>
  <p class="lancast-status" id="lancast-status" role="status" aria-live="polite"></p>
  <details>
    <summary>Can't open the link?</summary>
    <p>Download <a href="https://example.org/lancast/LANcast.event" download>LANcast.event</a>,
       put it into the LANcast folder next to <code>LANcast.exe</code>
       (usually <code>%LOCALAPPDATA%\Programs\LANcast</code> or <code>C:\Program Files\LANcast</code>),
       and restart LANcast.</p>
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
- With an organizer edition, the page can stay the same from event to event: only the link, the event name and the optional `LANcast.event` change.
