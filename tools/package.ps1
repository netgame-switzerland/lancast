# Builds the release ZIP and the installer (SPEC §6, D25):
# dist\LANcast-<version>-win64.zip and dist\LANcast-<version>-setup.exe with
# LANcast.exe, the NDI runtime DLL and its license file, README.txt,
# EULA.txt, LICENSE.txt (Apache-2.0), NOTICE.txt and THIRD-PARTY-LICENSES.txt.
#
# With -EventConfig and -SigningKey it also builds the event edition (LAN
# distro, D28): LANcast.exe with the organizer's public key compiled in, plus
# the signed LANcast.event, as dist\LANcast-<version>-<event_id>-win64.zip and
# ...-setup.exe, and the matching event link in ...-link.txt.
#
# With -Edition <organizer> and -PublicKey (or -SigningKey) it builds the
# organizer edition (D35) instead: the same exe, named after the organizer,
# without LANcast.event, as dist\LANcast-<version>-<organizer>-win64.zip and
# ...-setup.exe. It sends only after a signed event link of that organizer, so
# one build serves all of the organizer's events. It needs no private key.
# -Edition together with -EventConfig names an event edition after the
# organizer instead of the event.
#
# Update check (D37): the normal and the organizer edition look for updates of
# their own edition in the release's LANcast-update.json (LANCAST_UPDATE_EDITION
# = the organizer's name as in the file names); an event edition does not.
#
# Versions (D30): without -Release, the packages are snapshots named after
# the exe's version, e.g. LANcast-0.2.0-snapshot.g1a2b3c4-win64.zip. With
# -Release (used by the GitHub release workflow), the build uses the plain
# Cargo.toml version; HEAD must be the clean, tagged commit v<version>, and
# the installer is required.
#
# Needs Python 3 (third-party license list) and Inno Setup 6 (installer;
# skipped with a warning if missing, except with -Release).
#
# Usage (from the project root):
#   powershell -File tools\package.ps1
#   powershell -File tools\package.ps1 -EventConfig event.toml -SigningKey secret.key
#   powershell -File tools\package.ps1 -Edition netgame -PublicKey <base64url key>
#   powershell -File tools\package.ps1 -Release [-EventOnly -EventConfig ... -SigningKey ...]
# -EventOnly skips the normal edition. LANcast must not be running (the build
# replaces LANcast.exe).

param(
    [string]$EventConfig,
    [string]$SigningKey,
    [string]$Edition,
    [string]$PublicKey,
    [switch]$Release,
    [switch]$EventOnly
)

$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

if ($EventConfig -and -not $SigningKey) {
    throw "-EventConfig needs -SigningKey."
}
if ($PublicKey -and $SigningKey) {
    throw "Give either -PublicKey or -SigningKey, not both."
}
if (($SigningKey -or $PublicKey) -and -not ($EventConfig -or $Edition)) {
    throw "-SigningKey and -PublicKey need -EventConfig or -Edition."
}
if ($Edition -and -not ($PublicKey -or $SigningKey)) {
    throw "-Edition needs -PublicKey (or -SigningKey)."
}
if ($Edition -and $Edition -notmatch '^[A-Za-z0-9][A-Za-z0-9 ._-]{0,39}$') {
    throw "-Edition '$Edition': use letters, digits, space, '.', '_' or '-' (at most 40)."
}
if ($EventOnly -and -not ($EventConfig -or $Edition)) {
    throw "-EventOnly needs -EventConfig or -Edition."
}

# Base64url without padding, as used by the event files and links.
function ConvertFrom-Base64Url([string]$Text) {
    $b64 = $Text.Replace('-', '+').Replace('_', '/')
    $b64 += "=" * ((4 - $b64.Length % 4) % 4)
    [Convert]::FromBase64String($b64)
}

# Checked before anything is built.
function Test-PublicKey([string]$Key) {
    $bytes = try { ConvertFrom-Base64Url $Key } catch { @() }
    if ($Key -notmatch '^[A-Za-z0-9_-]+$' -or $bytes.Length -ne 32) {
        throw "The public key must be 32 bytes in base64url (as printed by lancast-event pubkey)."
    }
}
if ($PublicKey) { $PublicKey = $PublicKey.Trim(); Test-PublicKey $PublicKey }

if (Get-Process LANcast -ErrorAction SilentlyContinue) {
    throw "LANcast is running. Quit it (tray icon -> Quit) and try again."
}

# The numeric version from Cargo.toml (installer file version); the full
# version for names and the README comes from the built exe.
$cargoVersion = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if ($Release) {
    if (git status --porcelain --untracked-files=no) { throw "-Release needs a clean working tree." }
    if (-not (git tag --points-at HEAD | Where-Object { $_ -eq "v$cargoVersion" })) {
        throw "-Release needs HEAD to be tagged v$cargoVersion (Cargo.toml version)."
    }
    $env:LANCAST_RELEASE = "1"
} else {
    Remove-Item Env:LANCAST_RELEASE -ErrorAction SilentlyContinue
}
$sdk = if ($env:NDI_SDK_DIR) { $env:NDI_SDK_DIR } else { "C:\Program Files\NDI\NDI 6 SDK" }
$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if ($Release -and -not $iscc) { throw "Inno Setup not found; -Release needs the installer." }
New-Item -ItemType Directory -Force dist | Out-Null

# Builds LANcast.exe into $TargetDir and checks that it was really replaced.
function Build-LANcast([string]$TargetDir) {
    $started = Get-Date
    cargo build --release --bin LANcast --target-dir $TargetDir
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    $exe = Get-Item "$TargetDir\release\LANcast.exe"
    # Guard against an old exe being packaged (e.g. if it could not be replaced).
    if ($exe.LastWriteTime -lt $started.AddMinutes(-60) -and (git status --porcelain src build.rs Cargo.toml)) {
        throw "$($exe.FullName) looks outdated ($($exe.LastWriteTime)); check the build output."
    }
    $exe
}

# The version for file names: the exe's ProductVersion (set by build.rs), with
# "+" (SemVer build metadata) replaced, since it is awkward in download URLs.
function Get-Version($Exe) {
    $full = $Exe.VersionInfo.ProductVersion
    if (-not $full) { throw "$($Exe.FullName) has no ProductVersion." }
    if ($Release -and $full -ne $cargoVersion) { throw "Release exe reports $full, expected $cargoVersion." }
    $full.Replace("+", "-")
}

# Stages the files, then writes the ZIP and (if Inno Setup exists) the setup.
function New-Package($Exe, [string]$Name, [string]$EventFile, [string]$Edition, [string]$Intro) {
    $stage = "dist\$Name"
    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    New-Item -ItemType Directory -Force $stage | Out-Null

    Copy-Item $Exe.FullName $stage
    Copy-Item (Join-Path $sdk "Bin\x64\Processing.NDI.Lib.x64.dll") $stage
    Copy-Item (Join-Path $sdk "Bin\x64\Processing.NDI.Lib.Licenses.txt") $stage
    $readme = (Get-Content packaging\README.txt -Raw).Replace("{VERSION}", $version)
    Set-Content -Encoding utf8 (Join-Path $stage "README.txt") ($Intro + $readme)
    Copy-Item packaging\EULA.txt $stage
    Copy-Item LICENSE (Join-Path $stage "LICENSE.txt")
    Copy-Item NOTICE (Join-Path $stage "NOTICE.txt")
    # Python writes the file itself (UTF-8); piping through PowerShell 5.1
    # would re-encode it with the console code page.
    python tools\third_party_licenses.py (Join-Path $stage "THIRD-PARTY-LICENSES.txt")
    if ($LASTEXITCODE -ne 0) { throw "third_party_licenses.py failed" }
    if ($EventFile) { Copy-Item $EventFile (Join-Path $stage "LANcast.event") }

    $zip = "dist\$Name-win64.zip"
    if (Test-Path $zip) { Remove-Item -Force $zip }
    Compress-Archive -Path "$stage\*" -DestinationPath $zip
    Write-Host "Created $zip"
    Get-ChildItem $stage | Format-Table Name, Length -AutoSize

    # Installer (Inno Setup, D25). Optional: skipped with a note if ISCC is missing.
    if (-not $iscc) {
        Write-Warning "Inno Setup not found; no setup.exe built (winget install JRSoftware.InnoSetup)."
        return
    }
    # VersionInfoVersion is numeric only: 1.0.0-rc.1 -> 1.0.0.
    $fileVersion = $cargoVersion -replace '-.*$', ''
    $defines = @("/DAppVersion=$version", "/DFileVersion=$fileVersion", "/DStageDir=$((Resolve-Path $stage).Path)",
        "/DOutputDir=$((Resolve-Path dist).Path)", "/DOutputBase=$Name-setup")
    if ($Edition) { $defines += "/DEdition=$Edition" }
    if ($EventFile) { $defines += "/DEventFile=1" }
    & $iscc /Q @defines packaging\LANcast.iss
    if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed" }
    Write-Host "Created dist\$Name-setup.exe"
}

# The normal edition: never with an event key, even if one is set in the shell.
Remove-Item Env:LANCAST_DISTRO_KEY, Env:LANCAST_DISTRO_NAME, Env:LANCAST_UPDATE_EDITION -ErrorAction SilentlyContinue
if (-not $EventOnly) {
    Write-Host "Building LANcast $cargoVersion$(if (-not $Release) { ' (snapshot)' }) ..."
    $exe = Build-LANcast "target"
    $version = Get-Version $exe
    New-Package $exe "LANcast-$version" "" "" ""
}

if (-not ($EventConfig -or $Edition)) { return }

# Event or organizer edition (D28, D35). The private key is only used by
# lancast-event here; the exe gets the public key.
if ($SigningKey) {
    cargo build --release --bin lancast-event
    if ($LASTEXITCODE -ne 0) { throw "cargo build (lancast-event) failed" }
    $tool = "target\release\lancast-event.exe"
    $PublicKey = "$(& $tool pubkey $SigningKey)".Trim()
    if ($LASTEXITCODE -ne 0) { throw "lancast-event pubkey failed" }
}

$eventFile = ""
$info = $null
$work = "dist\event-work"
if ($EventConfig) {
    Write-Host "Signing $EventConfig ..."
    New-Item -ItemType Directory -Force $work | Out-Null
    $eventFile = Join-Path $work "LANcast.event"
    $link = & $tool sign $EventConfig $SigningKey $eventFile
    if ($LASTEXITCODE -ne 0) { throw "lancast-event sign failed" }
    # Event name and id from the signed payload (base64url JSON).
    $payload = (Select-String -Path $eventFile -Pattern '^payload\s*=\s*"(.+)"').Matches[0].Groups[1].Value
    $info = [Text.Encoding]::UTF8.GetString((ConvertFrom-Base64Url $payload)) | ConvertFrom-Json
}
$editionName = if ($Edition) { $Edition } else { $info.event }
$editionSlug = ($editionName.ToLower() -replace '[^a-z0-9]+', '-').Trim('-')

Write-Host "Building the $editionName edition ..."
$env:LANCAST_DISTRO_KEY = $PublicKey
$env:LANCAST_DISTRO_NAME = $editionName
# The organizer edition is released and updated like the normal one. An
# event edition (bundled LANcast.event) is not: without this variable, an exe
# with an event key does no update check.
if (-not $info) { $env:LANCAST_UPDATE_EDITION = $editionSlug }
try {
    $distroExe = Build-LANcast "target\distro"
} finally {
    Remove-Item Env:LANCAST_DISTRO_KEY, Env:LANCAST_DISTRO_NAME, Env:LANCAST_UPDATE_EDITION -ErrorAction SilentlyContinue
}
$version = Get-Version $distroExe
if ($info) {
    $intro = "LANcast - $editionName edition`r`n" +
        "This LANcast sends only with the event configuration of $($info.event)`r`n" +
        "(LANcast.event, valid $($info.valid_from) to $($info.valid_until)).`r`n`r`n"
    $name = "LANcast-$version-$($info.event_id)"
} else {
    $intro = "LANcast - $editionName edition`r`n" +
        "This LANcast sends only with a signed event configuration of $editionName.`r`n" +
        "Click the event link on the event website to load it.`r`n`r`n"
    $name = "LANcast-$version-$editionSlug"
}
New-Package $distroExe $name $eventFile $editionName $intro

if (-not $info) { return }
$linkFile = "dist\$name-link.txt"
@(
    "Event link for $($info.event) (valid $($info.valid_from) to $($info.valid_until)).",
    "Put it on the event website, e.g. <a href=`"...`">Configure LANcast</a>",
    "(write & as &amp; inside the HTML attribute).",
    "",
    "$link"
) | Set-Content -Encoding utf8 $linkFile
Remove-Item -Recurse -Force $work
Write-Host "Created $linkFile"
