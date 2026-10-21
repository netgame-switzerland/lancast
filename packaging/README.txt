LANcast {VERSION}
==============

Send your game window - picture and ONLY the game's sound - to the big screen
at a LAN party. LANcast turns the window into an NDI(R) source on the local
network; the central PC shows it with OBS (DistroAV plugin), NDI Studio
Monitor or other NDI-capable software.


QUICK START
-----------
1. Run LANcast-setup.exe (recommended: it also sets up the Windows Firewall
   for you), or unzip the ZIP anywhere and start LANcast.exe.
2. Pick your game in "Choose the window to cast". A live preview and the
   audio meter appear below.
3. Click "Send". Your stream appears on the network as
   "<YOUR-PC-NAME> (<source name>)".
4. Ask the person at the central PC to pick your source.

LANcast keeps running in the tray (red dot = casting). Right-click the tray
icon to stop or quit.


REQUIREMENTS
------------
- Windows 10 version 1903 or newer, 64-bit. Windows 11 recommended.
- "Game audio only" needs a recent Windows (Windows 11, possibly also newer
  Windows 10 builds). If it is not available, LANcast says so BEFORE you send
  and lets you choose "Send with system audio" (everyone hears your Discord,
  music and notifications!) or "Send without audio".
- Play in BORDERLESS WINDOWED mode. Games in exclusive fullscreen may not be
  captured (black picture).


FIRST START: WINDOWS WARNINGS
-----------------------------
- SmartScreen: LANcast is not code-signed, so Windows may say "Windows
  protected your PC". Click "More info" -> "Run anyway".
- Firewall: Windows asks whether LANcast may communicate on networks. Allow
  it for PRIVATE networks. Receivers connect TO your PC; if this is blocked,
  your source may appear but stay black.
  Admins can add the rule manually:
    netsh advfirewall firewall add rule name="LANcast" dir=in action=allow ^
          program="C:\path\to\LANcast.exe" profile=private
  Also make sure the LAN connection is a "Private" network in Windows.


SETTINGS
--------
- Close and minimize to tray: on by default.
- Always on top: keep LANcast visible over your (windowed) game.
- Show capture border: Windows draws a yellow frame around the captured
  window (cannot be turned off on Windows 10).
- Max. frame rate / max. resolution: lower them if the network or your PC
  struggles. Full-quality NDI at 1080p60 needs up to ~130 Mbit/s.
- Receiver level: "Unity" fits OBS/DistroAV. Choose "NDI standard +20 dB" if
  the central PC uses NDI Studio Monitor and your sound is very quiet there.
- Discovery server: leave empty. Only needed when the organizers run an NDI
  Discovery Server (several networks/VLANs); enter its address then.

Settings and logs are stored in %LOCALAPPDATA%\LANcast\ (link "Settings &
logs folder" at the bottom of the window). Portable mode: create an empty
file "LANcast.toml" next to LANcast.exe and LANcast keeps everything there.


UPDATES
-------
At startup LANcast checks GitHub for a newer version (Settings > Updates >
"Check at startup"; "Check now" checks right away). Without an internet
connection it simply starts as usual. If an update exists, a blue banner
offers it:
- Installed with the setup: "Update now" downloads the new setup, checks
  its signature and starts it. LANcast closes and starts again afterwards;
  Windows may ask for permission.
- ZIP / portable: "Download..." opens the download page.
Organizers can offer updates on the event LAN: put update_url =
"http://<server>/<folder>/LANcast-update.json" into LANcast.toml, with the
files of the GitHub release in that folder.


EVENT LINKS
-----------
Organizers may publish a "Configure LANcast" link (lancast://config?...).
Click it after installing (or after starting LANcast once from the ZIP):
LANcast shows the event settings (discovery server, resolution limit, ...)
and asks before applying them. Clicking the same link again only opens
LANcast; it asks again when the event's settings change. "Leave event" in
the settings removes them.


TROUBLESHOOTING
---------------
- The central PC does not see my source:
  - Firewall (see above); LAN must be a "Private" network.
  - LANcast warns if NDI Access Manager still points to an old discovery
    server - click "Ignore it for LANcast".
- Black picture: switch the game to borderless windowed mode.
- No sound / meter does not move: many games mute themselves in the
  background; check the in-game "mute when unfocused" option.
- Very quiet sound in NDI Studio Monitor: set "Receiver level" to
  "NDI standard +20 dB".
- Please send your log (logs\LANcast.log in the settings folder) when
  reporting a problem.


NDI
---
NDI(R) is a registered trademark of Vizrt NDI AB. https://ndi.video/
This package includes the NDI(R) runtime library (Processing.NDI.Lib.x64.dll),
Copyright (C) 2023-2026 Vizrt NDI AB. All rights reserved. See
Processing.NDI.Lib.Licenses.txt. Free NDI Tools (e.g. NDI Studio Monitor):
https://ndi.video/tools/

LICENSE
-------
LANcast is Copyright 2026 Kay Huber and licensed under the Apache License,
Version 2.0 (LICENSE.txt, NOTICE.txt). The NDI(R) runtime is not covered by
that license; see EULA.txt. Open-source components compiled into
LANcast.exe: THIRD-PARTY-LICENSES.txt.
