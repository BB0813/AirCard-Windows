# Installs the genuine Apple CoreFP.dll and repairs the FairPlay registration.
#
# Runs elevated via UAC. Invoked by the user (or by AirCard's repair flow) after
# corefp_out\CoreFP.dll has been extracted from the official iTunes installer.

#Requires -RunAsAdministrator

$ErrorActionPreference = 'Stop'

# CoreFP.dll is staged next to this script by Extract-CoreFP.ps1. A copy left
# in the temp directory by an earlier run is accepted as a fallback.
$candidates = @(
    (Join-Path $PSScriptRoot 'dist\corefp_out\CoreFP.dll'),
    (Join-Path $PSScriptRoot 'corefp_out\CoreFP.dll'),
    (Join-Path $env:TEMP 'corefp_out\CoreFP.dll')
)
$extracted = $candidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1

if (-not $extracted) {
    Write-Host "ERROR: CoreFP.dll was not found in any of:" -ForegroundColor Red
    $candidates | ForEach-Object { Write-Host "  $_" }
    Write-Host "`nRun tools\Extract-CoreFP.ps1 first to extract it from the official iTunes installer." -ForegroundColor Yellow
    exit 1
}

$amdsDir   = "$env:ProgramFiles\Common Files\Apple\Mobile Device Support"
$pluginDir = "$env:ProgramFiles\Common Files\Apple\Internet Plug-Ins"
$coreFpKey = 'HKLM:\SOFTWARE\Apple Inc.\CoreFP'
$wowKey    = 'HKLM:\SOFTWARE\WOW6432Node\Apple Inc.\CoreFP'

function Write-Step($message) {
    Write-Host "  $message" -ForegroundColor Cyan
}

if (-not (Test-Path -LiteralPath $extracted)) {
    Write-Host "ERROR: CoreFP.dll not found at $extracted" -ForegroundColor Red
    exit 1
}

$info = (Get-Item -LiteralPath $extracted).VersionInfo
Write-Host ""
Write-Host "Installing CoreFP.dll ($($info.FileVersion)) - signed to $($info.CompanyName)" -ForegroundColor Green
Write-Host ""

# Back up whatever the registry currently says, so the change is reversible.
$backup = "$env:TEMP\CoreFP-registry-backup.reg"
if (Test-Path $coreFpKey) {
    Write-Step "Backing up the current CoreFP key to $backup"
    & reg.exe export 'HKLM\SOFTWARE\Apple Inc.\CoreFP' $backup /y | Out-Null
} else {
    Write-Step "No existing CoreFP key to back up"
}

# CoreFP.dll next to MobileDevice.dll is the location AirTrafficHost already
# searches, so no PATH change is needed. Internet Plug-Ins is kept in sync with
# where a full iTunes install would put it.
New-Item -ItemType Directory -Path $amdsDir   -Force | Out-Null
New-Item -ItemType Directory -Path $pluginDir -Force | Out-Null

$original = "$amdsDir\CoreFP.dll"
if (Test-Path -LiteralPath $original) {
    Write-Step "Preserving any pre-existing CoreFP.dll as CoreFP.dll.bak"
    Copy-Item -LiteralPath $original -Destination "$original.bak" -Force
}

Write-Step "Copying CoreFP.dll into $amdsDir"
Copy-Item -LiteralPath $extracted -Destination $original -Force

Write-Step "Copying CoreFP.dll into $pluginDir"
Copy-Item -LiteralPath $extracted -Destination "$pluginDir\CoreFP.dll" -Force

$installed = "$pluginDir\CoreFP.dll"

# Remove the stale third-party values before writing the authoritative one,
# otherwise a bad Libi4CFPath keeps winning.
foreach ($key in @($coreFpKey, $wowKey)) {
    if (-not (Test-Path $key)) { continue }
    foreach ($name in @('LibraryPath', 'Libi4CFPath', 'LibiiiiPath')) {
        if ((Get-Item $key).GetValue($name) -ne $null) {
            Remove-ItemProperty -Path $key -Name $name -Force -ErrorAction SilentlyContinue
            Write-Step "Removed stale value $key :: $name"
        }
    }
}

New-Item -Path $coreFpKey -Force | Out-Null
Set-ItemProperty -Path $coreFpKey -Name 'LibraryPath' -Value $installed -Force
Write-Step "Registered CoreFP :: LibraryPath = $installed"

New-Item -Path $wowKey -Force | Out-Null
Set-ItemProperty -Path $wowKey -Name 'LibraryPath' -Value $installed -Force
Write-Step "Registered WOW6432Node CoreFP :: LibraryPath"

# Verify, so a failed install never looks like a successful one.
Write-Host "`nVerifying" -ForegroundColor Cyan
$ok = $true
foreach ($file in @($original, $installed)) {
    if (Test-Path -LiteralPath $file) {
        $len = (Get-Item -LiteralPath $file).Length
        Write-Host "  OK   $file ($len bytes)" -ForegroundColor Green
    } else {
        Write-Host "  FAIL $file is missing" -ForegroundColor Red
        $ok = $false
    }
}
$verify = (Get-ItemProperty -Path $coreFpKey -Name 'LibraryPath' -ErrorAction SilentlyContinue).LibraryPath
if ($verify -eq $installed -and (Test-Path -LiteralPath $verify)) {
    Write-Host "  OK   registry LibraryPath resolves to an existing file" -ForegroundColor Green
} else {
    Write-Host "  FAIL registry LibraryPath is '$verify'" -ForegroundColor Red
    $ok = $false
}

Write-Host ""
if ($ok) {
    Write-Host "CoreFP.dll installed and registered." -ForegroundColor Green
    Write-Host "Reboot, then retry the transfer in AirCard." -ForegroundColor Green
} else {
    Write-Host "Installation did not verify. Restore the backup from $backup if needed." -ForegroundColor Red
}

exit $(if ($ok) { 0 } else { 1 })
