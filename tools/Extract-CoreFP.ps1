# Extracts the genuine Apple CoreFP.dll from the official iTunes installer.
#
# CoreFP.dll is not part of Apple Mobile Device Support, which is why it is
# usually missing on machines where only AMDS was installed. It ships inside
# iTunes64.msi, which the official installer bundles as an embedded cabinet.
#
# No elevation needed: this only downloads and extracts to a local folder.

param(
    [string]$OutDir = (Join-Path $PSScriptRoot 'dist\corefp_out')
)

$ErrorActionPreference = 'Stop'

$setupUrl  = 'https://www.apple.com/itunes/download/win64/'
$workDir   = Join-Path $env:TEMP 'aircard-corefp-extract'
$setupPath = Join-Path $workDir 'iTunes64Setup.exe'

function Write-Step($message) {
    Write-Host "  $message" -ForegroundColor Cyan
}

New-Item -ItemType Directory -Path $OutDir  -Force | Out-Null
New-Item -ItemType Directory -Path $workDir -Force | Out-Null

$sevenZip = @(
    "$env:ProgramFiles\7-Zip\7z.exe",
    "${env:ProgramFiles(x86)}\7-Zip\7z.exe"
) | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1

if (-not $sevenZip) {
    Write-Host "ERROR: 7-Zip is required to unpack the embedded cabinet." -ForegroundColor Red
    Write-Host "Install it from https://www.7-zip.org/ and run this again." -ForegroundColor Yellow
    exit 1
}

Write-Host "`nStep 1/4: Resolving the official iTunes installer" -ForegroundColor Green
if (-not (Test-Path -LiteralPath $setupPath)) {
    # The download URL redirects to Apple's CDN with a fresh signed link.
    $setupPath = (Invoke-WebRequest -Uri $setupUrl -MaximumRedirection 0 -ErrorAction SilentlyContinue -PassThru).Headers.Location
    if (-not $setupPath) {
        Write-Host "ERROR: could not resolve the installer URL from $setupUrl" -ForegroundColor Red
        exit 1
    }
    Write-Step "Installer: $setupPath"
    Write-Step "Downloading (about 190 MB, this takes a while)..."
    Invoke-WebRequest -Uri $setupUrl -OutFile (Join-Path $workDir 'iTunes64Setup.exe') -MaximumRedirection 5
    $setupPath = Join-Path $workDir 'iTunes64Setup.exe'
} else {
    Write-Step "Reusing the installer already in $workDir"
}

if (-not (Test-Path -LiteralPath $setupPath)) {
    Write-Host "ERROR: installer download failed." -ForegroundColor Red
    exit 1
}

Write-Host "`nStep 2/4: Unpacking iTunes64.msi from the installer" -ForegroundColor Green
$msiPath = Join-Path $workDir 'iTunes64.msi'
if (-not (Test-Path -LiteralPath $msiPath)) {
    & $sevenZip e -y -o"$workDir" $setupPath 'iTunes64.msi' | Out-Null
}
if (-not (Test-Path -LiteralPath $msiPath)) {
    Write-Host "ERROR: could not extract iTunes64.msi." -ForegroundColor Red
    exit 1
}
Write-Step "Extracted $(Split-Path $msiPath -Leaf)"

Write-Host "`nStep 3/4: Extracting CoreFP.dll" -ForegroundColor Green
& $sevenZip e -y -o"$OutDir" $msiPath 'CoreFP.dll' | Select-String -Pattern 'CoreFP' | ForEach-Object { Write-Step $_.Line }
$coreFp = Join-Path $OutDir 'CoreFP.dll'

if (-not (Test-Path -LiteralPath $coreFp)) {
    Write-Host "ERROR: CoreFP.dll was not found inside iTunes64.msi." -ForegroundColor Red
    exit 1
}

$info = (Get-Item -LiteralPath $coreFp).VersionInfo
Write-Host "`nStep 4/4: Verifying" -ForegroundColor Green
Write-Host ("  Name     : {0}" -f (Split-Path $coreFp -Leaf))
Write-Host ("  Company  : {0}" -f $info.CompanyName)
Write-Host ("  Product  : {0}" -f $info.ProductName)
Write-Host ("  Version  : {0}" -f $info.FileVersion)
Write-Host ("  Size     : {0:N0} bytes" -f (Get-Item -LiteralPath $coreFp).Length)

if ($info.CompanyName -ne 'Apple Inc.') {
    Write-Host "`nWARNING: this DLL is not signed to Apple Inc. Do not install it." -ForegroundColor Red
    exit 1
}

Write-Host "`nCoreFP.dll is ready at:" -ForegroundColor Green
Write-Host "  $coreFp"
Write-Host "`nNext, run Install-CoreFP.ps1 as Administrator to place it and repair the registry." -ForegroundColor Green
exit 0
