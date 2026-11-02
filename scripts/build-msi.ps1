# Builds the release app and packages it as dist/BareClip-<version>.msi
$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
& (Join-Path $PSScriptRoot 'setup-ffmpeg.ps1')
& (Join-Path $PSScriptRoot 'setup-wix.ps1')

& (Join-Path $PSScriptRoot 'cargo.ps1') build --release -p bareclip-app --locked
if ($LASTEXITCODE -ne 0) { throw 'The release build failed.' }

$Manifest = Get-Content (Join-Path $Root 'Cargo.toml') -Raw
if ($Manifest -notmatch 'version = "(\d+\.\d+\.\d+)"') { throw 'No version found in Cargo.toml.' }
$Version = $Matches[1]

$Wix = Join-Path $Root 'vendor\wix'
$Dist = Join-Path $Root 'dist'
$Work = Join-Path $Root 'target\msi'
New-Item -ItemType Directory -Force $Dist, $Work | Out-Null

$Object = Join-Path $Work 'bareclip.wixobj'
$Msi = Join-Path $Dist "BareClip-$Version.msi"

& (Join-Path $Wix 'candle.exe') -nologo -arch x64 -ext WixUIExtension `
    "-dVersion=$Version" `
    "-dRepoDir=$Root" `
    "-dBrandDir=$(Join-Path $Root 'assets\brand')" `
    "-dInstallerDir=$(Join-Path $Root 'installer')" `
    "-dExePath=$(Join-Path $Root 'target\release\bareclip.exe')" `
    "-dFfmpegDir=$(Join-Path $Root 'vendor\ffmpeg')" `
    "-dFfmpegBin=$(Join-Path $Root 'vendor\ffmpeg\bin')" `
    -out $Object (Join-Path $Root 'installer\bareclip.wxs')
if ($LASTEXITCODE -ne 0) { throw 'WiX could not compile the installer.' }

& (Join-Path $Wix 'light.exe') -nologo -ext WixUIExtension -cultures:en-us `
    -out $Msi $Object
if ($LASTEXITCODE -ne 0) { throw 'WiX could not link the installer.' }

$Size = (Get-Item $Msi).Length / 1MB
Write-Host ("Built {0} ({1:N1} MB)" -f $Msi, $Size)
