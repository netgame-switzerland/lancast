# Shows and cleans up the Windows Firewall rules for LANcast (SPEC 5.4, D10, D40).
#
# A receiver connects to the sender, so LANcast needs an inbound allow rule.
# Symptom of a missing one: the source is listed (announcing is outbound) but
# the picture stays black. Windows asks only when an exe has no rule at all, so
# a leftover block rule (from a cancelled prompt) or a rule for the wrong
# network profile blocks silently.
#
# Without -Apply the script only reports: network profiles, and every firewall
# rule whose program is a LANcast.exe (any folder). With -Apply (as admin) it
# deletes all of those rules and adds one clean allow rule for the installed
# LANcast.exe on all network profiles (D55).
#
# Usage (from an admin PowerShell; plain text, no install needed):
#   powershell -ExecutionPolicy Bypass -File firewall.ps1
#   powershell -ExecutionPolicy Bypass -File firewall.ps1 -Apply [-Exe <path>]
#   powershell -ExecutionPolicy Bypass -File firewall.ps1 -Apply -Dev
# -Exe      the LANcast.exe to allow; default: the installed one (all users,
#           then "for me only").
# -Dev      also removes rules for any exe under a cargo "target" folder whose
#           path contains "lancast" (dev builds, examples); adds no rule.
#
# Keep this file ASCII only: Windows PowerShell 5.1 reads it without a BOM.

param(
    [switch]$Apply,
    [switch]$Dev,
    [string]$Exe
)

$ErrorActionPreference = "Stop"

function Get-LancastRules {
    # One row per LANcast rule; the program path expanded. Filters by path
    # first: looking up the rule of every filter on the system takes minutes.
    Get-NetFirewallApplicationFilter |
        Where-Object { $_.Program -and $_.Program -ne "Any" -and (Test-LancastPath ([Environment]::ExpandEnvironmentVariables($_.Program))) } |
        ForEach-Object {
            $program = [Environment]::ExpandEnvironmentVariables($_.Program)
            $rule = $_ | Get-NetFirewallRule
            [pscustomobject]@{
                Id      = $rule.Name
                Name    = $rule.DisplayName
                Dir     = $rule.Direction
                Action  = $rule.Action
                Profile = $rule.Profile
                Enabled = $rule.Enabled
                Program = $program
            }
        }
}

function Test-LancastPath([string]$program) {
    if ((Split-Path $program -Leaf) -eq "LANcast.exe") { return $true }
    if ($Dev -and $program -match "lancast" -and $program -match "\\target\\") { return $true }
    return $false
}

if (-not $Exe) {
    $candidates = @(
        (Join-Path $env:ProgramFiles "LANcast\LANcast.exe"),
        (Join-Path $env:LOCALAPPDATA "Programs\LANcast\LANcast.exe")
    )
    $Exe = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
}

Write-Host "Network profiles:"
Get-NetConnectionProfile | Format-Table InterfaceAlias, Name, NetworkCategory -AutoSize | Out-Host

$rules = @(Get-LancastRules)
if ($rules.Count -eq 0) {
    Write-Host "No firewall rules for LANcast."
} else {
    Write-Host "Firewall rules for LANcast:"
    $rules | Sort-Object Program, Name | Format-Table Name, Dir, Action, Profile, Enabled, Program -AutoSize | Out-Host
    if ($rules | Where-Object { $_.Action -eq "Block" -and $_.Enabled -eq "True" }) {
        Write-Warning "There are block rules. They win over every allow rule, and Windows will not ask again."
    }
}

if (-not $Apply) {
    Write-Host "Nothing changed. Run again with -Apply (as admin) to clean up."
    exit 0
}

$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) {
    Write-Error "-Apply needs an admin PowerShell (right-click > Run as administrator)."
}

foreach ($rule in $rules) {
    Remove-NetFirewallRule -Name $rule.Id
}
Write-Host "Removed $($rules.Count) rule(s)."

if ($Exe) {
    $exePath = (Resolve-Path $Exe).Path
    # All profiles, like the setup: a LAN party network is usually Public (D55).
    $profiles = "Domain,Private,Public"
    New-NetFirewallRule -DisplayName "LANcast" -Direction Inbound -Action Allow `
        -Program $exePath -Profile $profiles -Enabled True | Out-Null
    Write-Host "Added: LANcast, inbound allow, $profiles, $exePath"
} else {
    Write-Warning "No installed LANcast.exe found; no allow rule added. Use -Exe <path>."
}
