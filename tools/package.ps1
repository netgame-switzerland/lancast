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
#   powershell -File tools\package.ps1 -Release [-EventOnly -EventConfig ... -SigningKey ...]
# -EventOnly skips the normal edition. LANcast must not be running (the build
# replaces LANcast.exe).

param(
    [string]$EventConfig,
    [string]$SigningKey,
    [switch]$Release,
    [switch]$EventOnly
)

$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

if ([bool]$EventConfig -ne [bool]$SigningKey) {
    throw "-EventConfig and -SigningKey must be given together."
}
if ($EventOnly -and -not $EventConfig) {
    throw "-EventOnly needs -EventConfig and -SigningKey."
}
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
    $defines = @("/DAppVersion=$version", "/DFileVersion=$cargoVersion", "/DStageDir=$((Resolve-Path $stage).Path)",
        "/DOutputDir=$((Resolve-Path dist).Path)", "/DOutputBase=$Name-setup")
    if ($Edition) { $defines += @("/DEdition=$Edition", "/DEventFile=1") }
    & $iscc /Q @defines packaging\LANcast.iss
    if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed" }
    Write-Host "Created dist\$Name-setup.exe"
}

# The normal edition: never with an event key, even if one is set in the shell.
Remove-Item Env:LANCAST_DISTRO_KEY, Env:LANCAST_DISTRO_NAME -ErrorAction SilentlyContinue
if (-not $EventOnly) {
    Write-Host "Building LANcast $cargoVersion$(if (-not $Release) { ' (snapshot)' }) ..."
    $exe = Build-LANcast "target"
    $version = Get-Version $exe
    New-Package $exe "LANcast-$version" "" "" ""
}

if (-not $EventConfig) { return }

# Event edition (D28). The private key is only used by lancast-event here; the
# exe gets the public key.
Write-Host "Building the event edition from $EventConfig ..."
cargo build --release --bin lancast-event
if ($LASTEXITCODE -ne 0) { throw "cargo build (lancast-event) failed" }
$tool = "target\release\lancast-event.exe"
$work = "dist\event-work"
New-Item -ItemType Directory -Force $work | Out-Null
$eventFile = Join-Path $work "LANcast.event"
$link = & $tool sign $EventConfig $SigningKey $eventFile
if ($LASTEXITCODE -ne 0) { throw "lancast-event sign failed" }
$publicKey = & $tool pubkey $SigningKey
if ($LASTEXITCODE -ne 0) { throw "lancast-event pubkey failed" }

# Event name and id from the signed payload (base64url JSON).
$payload = (Select-String -Path $eventFile -Pattern '^payload\s*=\s*"(.+)"').Matches[0].Groups[1].Value
$b64 = $payload.Replace('-', '+').Replace('_', '/')
$b64 += "=" * ((4 - $b64.Length % 4) % 4)
$info = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($b64)) | ConvertFrom-Json

$env:LANCAST_DISTRO_KEY = "$publicKey".Trim()
$env:LANCAST_DISTRO_NAME = $info.event
try {
    $distroExe = Build-LANcast "target\distro"
} finally {
    Remove-Item Env:LANCAST_DISTRO_KEY, Env:LANCAST_DISTRO_NAME -ErrorAction SilentlyContinue
}
$version = Get-Version $distroExe
$intro = "LANcast - $($info.event) edition`r`n" +
    "This LANcast sends only with the event configuration of $($info.event)`r`n" +
    "(LANcast.event, valid $($info.valid_from) to $($info.valid_until)).`r`n`r`n"
$name = "LANcast-$version-$($info.event_id)"
New-Package $distroExe $name $eventFile $info.event $intro

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
