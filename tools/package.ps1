# Builds the release ZIP and the installer (SPEC §6, D25):
# dist\LANcast-<version>-win64.zip and dist\LANcast-<version>-setup.exe with
# LANcast.exe, the NDI runtime DLL and its license file, README.txt,
# EULA.txt, LICENSE.txt (Apache-2.0), NOTICE.txt and THIRD-PARTY-LICENSES.txt.
# Needs Python 3 (third-party license list) and Inno Setup 6 (installer;
# skipped with a warning if missing).
#
# Usage (from the project root): powershell -File tools\package.ps1
# LANcast must not be running (the build replaces LANcast.exe).

$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

if (Get-Process LANcast -ErrorAction SilentlyContinue) {
    throw "LANcast is running. Quit it (tray icon -> Quit) and try again."
}

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$sdk = if ($env:NDI_SDK_DIR) { $env:NDI_SDK_DIR } else { "C:\Program Files\NDI\NDI 6 SDK" }

Write-Host "Building LANcast $version ..."
$started = Get-Date
cargo build --release --bin LANcast
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
$exe = Get-Item target\release\LANcast.exe
# Guard against an old exe being packaged (e.g. if it could not be replaced).
if ($exe.LastWriteTime -lt $started.AddMinutes(-60) -and (git status --porcelain src build.rs Cargo.toml)) {
    throw "target\release\LANcast.exe looks outdated ($($exe.LastWriteTime)); check the build output."
}

$stage = "dist\LANcast-$version"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null

Copy-Item $exe.FullName $stage
Copy-Item (Join-Path $sdk "Bin\x64\Processing.NDI.Lib.x64.dll") $stage
Copy-Item (Join-Path $sdk "Bin\x64\Processing.NDI.Lib.Licenses.txt") $stage
(Get-Content packaging\README.txt -Raw).Replace("{VERSION}", $version) |
    Set-Content -Encoding utf8 (Join-Path $stage "README.txt")
Copy-Item packaging\EULA.txt $stage
Copy-Item LICENSE (Join-Path $stage "LICENSE.txt")
Copy-Item NOTICE (Join-Path $stage "NOTICE.txt")
# Python writes the file itself (UTF-8); piping through PowerShell 5.1 would
# re-encode it with the console code page.
python tools\third_party_licenses.py (Join-Path $stage "THIRD-PARTY-LICENSES.txt")
if ($LASTEXITCODE -ne 0) { throw "third_party_licenses.py failed" }

$zip = "dist\LANcast-$version-win64.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path "$stage\*" -DestinationPath $zip
Write-Host "Created $zip"
Get-ChildItem $stage | Format-Table Name, Length -AutoSize

# Installer (Inno Setup, D25). Optional: skipped with a note if ISCC is missing.
$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) {
    Write-Warning "Inno Setup not found; no setup.exe built (winget install JRSoftware.InnoSetup)."
    return
}
$stageFull = (Resolve-Path $stage).Path
$distFull = (Resolve-Path dist).Path
& $iscc /Q "/DAppVersion=$version" "/DStageDir=$stageFull" "/DOutputDir=$distFull" packaging\LANcast.iss
if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed" }
Write-Host "Created dist\LANcast-$version-setup.exe"
