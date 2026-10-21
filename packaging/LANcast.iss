; Inno Setup script for the LANcast installer (D25).
; Built by tools\package.ps1, which passes:
;   /DAppVersion=<version, e.g. 0.2.0 or 0.2.0-snapshot.g1a2b3c4>
;   /DFileVersion=<numeric Cargo.toml version>  /DStageDir=<folder with the release files>
;   /DOutputDir=<folder for the setup.exe>  /DOutputBase=<setup file name>
; and for an event edition (D28) also /DEdition=<event name> /DEventFile=1
; (installs the signed LANcast.event next to the exe). An organizer edition
; (D35) gets only /DEdition=<organizer>: it has no configuration until an
; event link is clicked, so the last page points to the event website instead
; of offering to start LANcast.
;
; LANcast updates itself by running the new setup silently (D37):
;   /SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /ALLUSERS|/CURRENTUSER /LANCASTSTART=1
; The last one starts LANcast again when the setup is done.
;
; Install modes: "for all users" (default; admin) installs to Program Files
; and adds an inbound firewall rule; "for me only" installs to
; %LOCALAPPDATA%\Programs without admin rights and without a firewall rule.

#ifndef AppVersion
  #error AppVersion must be defined (/DAppVersion=...)
#endif
#ifndef StageDir
  #error StageDir must be defined (/DStageDir=...)
#endif
#ifndef FileVersion
  #define FileVersion AppVersion
#endif
#ifndef OutputDir
  #define OutputDir "..\dist"
#endif
#ifndef OutputBase
  #define OutputBase "LANcast-" + AppVersion + "-setup"
#endif
; An event edition replaces a normal install (same AppId): one LANcast per PC.
#ifdef Edition
  #define DisplayName "LANcast (" + Edition + " edition)"
#else
  #define DisplayName "LANcast"
#endif
#if defined(Edition) && !defined(EventFile)
  #define OrganizerEdition
#endif

[Setup]
; Never change the AppId: it identifies LANcast for updates and uninstall.
AppId={{F98C4151-0F3E-495C-B8D3-B823EE2D71F2}
AppName={#DisplayName}
AppVersion={#AppVersion}
AppVerName={#DisplayName} {#AppVersion}
AppPublisher=Kay Huber
AppCopyright=Copyright 2026 Kay Huber
AppPublisherURL=https://ndi.video/
DefaultDirName={autopf}\LANcast
DefaultGroupName=LANcast
DisableProgramGroupPage=yes
LicenseFile={#StageDir}\EULA.txt
OutputDir={#OutputDir}
OutputBaseFilename={#OutputBase}
SetupIconFile=..\assets\LANcast.ico
UninstallDisplayIcon={app}\LANcast.exe
UninstallDisplayName={#DisplayName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Windows 10 version 1903 (window capture), SPEC §5.0.
MinVersion=10.0.18362
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog
; Close a running LANcast (it holds the exe and the NDI DLL).
CloseApplications=yes
RestartApplications=no
VersionInfoVersion={#FileVersion}
VersionInfoDescription=LANcast setup

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "german"; MessagesFile: "compiler:Languages\German.isl"

[CustomMessages]
english.FirewallTask=Allow LANcast through Windows Firewall (private networks; recommended)
german.FirewallTask=LANcast in der Windows-Firewall freigeben (private Netzwerke; empfohlen)
english.LaunchLANcast=Start LANcast
german.LaunchLANcast=LANcast starten

#ifdef OrganizerEdition
; The lancast: scheme is registered by this setup, so the event link works
; right away; it starts LANcast with the event's configuration.
[Messages]
english.FinishedLabel=Setup has finished installing [name] on your computer.%n%nThis LANcast sends only with the configuration of a {#Edition} event. Open the event website and click the LANcast link there: it starts LANcast and loads the event's configuration.
english.FinishedLabelNoIcons=Setup has finished installing [name] on your computer.%n%nThis LANcast sends only with the configuration of a {#Edition} event. Open the event website and click the LANcast link there: it starts LANcast and loads the event's configuration.
german.FinishedLabel=Das Setup hat die Installation von [name] auf Ihrem Computer abgeschlossen.%n%nDieses LANcast sendet nur mit der Konfiguration eines {#Edition}-Events. Öffnen Sie die Event-Website und klicken Sie dort auf den LANcast-Link: Er startet LANcast und lädt die Konfiguration des Events.
german.FinishedLabelNoIcons=Das Setup hat die Installation von [name] auf Ihrem Computer abgeschlossen.%n%nDieses LANcast sendet nur mit der Konfiguration eines {#Edition}-Events. Öffnen Sie die Event-Website und klicken Sie dort auf den LANcast-Link: Er startet LANcast und lädt die Konfiguration des Events.
#endif

[Tasks]
Name: "firewall"; Description: "{cm:FirewallTask}"; Check: IsAdminInstallMode
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#StageDir}\LANcast.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\Processing.NDI.Lib.x64.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\Processing.NDI.Lib.Licenses.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\README.txt"; DestDir: "{app}"; Flags: ignoreversion isreadme
Source: "{#StageDir}\EULA.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\NOTICE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\THIRD-PARTY-LICENSES.txt"; DestDir: "{app}"; Flags: ignoreversion
#ifdef EventFile
Source: "{#StageDir}\LANcast.event"; DestDir: "{app}"; Flags: ignoreversion
#endif

[Icons]
Name: "{group}\LANcast"; Filename: "{app}\LANcast.exe"
Name: "{group}\README"; Filename: "{app}\README.txt"
Name: "{autodesktop}\LANcast"; Filename: "{app}\LANcast.exe"; Tasks: desktopicon

[Registry]
; lancast:// event links (D27). HKA = HKLM for all-users installs, HKCU otherwise.
Root: HKA; Subkey: "Software\Classes\lancast"; ValueType: string; ValueName: ""; ValueData: "URL:LANcast event link"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\lancast"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\lancast\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\LANcast.exe"",0"
Root: HKA; Subkey: "Software\Classes\lancast\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\LANcast.exe"" ""%1"""

[Run]
; Replace an older rule (reinstall/update), then allow inbound connections:
; NDI receivers connect to the sender (SPEC §5.4).
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""LANcast"""; Flags: runhidden; Tasks: firewall
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""LANcast"" dir=in action=allow program=""{app}\LANcast.exe"" profile=private,domain enable=yes"; Flags: runhidden; Tasks: firewall
; Start LANcast as the normal user, not elevated like the installer. Not for
; an organizer edition: without an event link it can't send (see [Messages]).
#ifndef OrganizerEdition
Filename: "{app}\LANcast.exe"; Description: "{cm:LaunchLANcast}"; Flags: nowait postinstall skipifsilent runasoriginaluser
#endif
; After an update started by LANcast (D37), every edition starts again.
Filename: "{app}\LANcast.exe"; Flags: nowait runasoriginaluser; Check: StartAfterUpdate

[UninstallRun]
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""LANcast"""; Flags: runhidden; RunOnceId: "RemoveFirewallRule"; Check: IsAdminInstallMode

[Code]
// /LANCASTSTART=1 on the command line: LANcast started this setup (D37).
function StartAfterUpdate: Boolean;
begin
  Result := ExpandConstant('{param:LANCASTSTART|0}') = '1';
end;

[UninstallDelete]
; Settings and logs in %LOCALAPPDATA%\LANcast are kept (per user, may be reused).
