# Builds the release ZIP (SPEC §6): dist\LANcast-<version>-win64.zip with
# LANcast.exe, the NDI runtime DLL, its license file, README.txt and EULA.txt.
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

$zip = "dist\LANcast-$version-win64.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path "$stage\*" -DestinationPath $zip
Write-Host "Created $zip"
Get-ChildItem $stage | Format-Table Name, Length -AutoSize
