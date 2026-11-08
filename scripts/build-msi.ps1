# Builds the release app and packages it as dist/BareClip-<version>.msi, with a copy named
# dist/BareClip.msi. GitHub's "latest" download link needs a name that never changes.
# Set CARGO_TARGET_DIR to build beside a copy of the app that is still running.
param(
    [string]$Version,
    [string]$OutDir,
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
& (Join-Path $PSScriptRoot 'setup-ffmpeg.ps1')
& (Join-Path $PSScriptRoot 'setup-wix.ps1')

if (-not $SkipBuild) {
    & (Join-Path $PSScriptRoot 'cargo.ps1') build --release -p bareclip-app --locked
    if ($LASTEXITCODE -ne 0) { throw 'The release build failed.' }
}
$Target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }

$Manifest = Get-Content (Join-Path $Root 'Cargo.toml') -Raw
if ($Manifest -notmatch 'version = "(\d+\.\d+\.\d+)"') { throw 'No version found in Cargo.toml.' }
if (-not $Version) { $Version = $Matches[1] }

$Wix = Join-Path $Root 'vendor\wix'
$Dist = if ($OutDir) { $OutDir } else { Join-Path $Root 'dist' }
$Work = Join-Path $Root 'target\msi'
New-Item -ItemType Directory -Force $Dist, $Work | Out-Null

$Object = Join-Path $Work 'bareclip.wixobj'
$Msi = Join-Path $Dist "BareClip-$Version.msi"

& (Join-Path $Wix 'candle.exe') -nologo -arch x64 -ext WixUIExtension -ext WixUtilExtension `
    "-dVersion=$Version" `
    "-dRepoDir=$Root" `
    "-dBrandDir=$(Join-Path $Root 'assets\brand')" `
    "-dInstallerDir=$(Join-Path $Root 'installer')" `
    "-dExePath=$(Join-Path $Target 'release\bareclip.exe')" `
    "-dFfmpegDir=$(Join-Path $Root 'vendor\ffmpeg')" `
    "-dFfmpegBin=$(Join-Path $Root 'vendor\ffmpeg\bin')" `
    -out $Object (Join-Path $Root 'installer\bareclip.wxs')
if ($LASTEXITCODE -ne 0) { throw 'WiX could not compile the installer.' }

# ICE38, ICE43 and ICE57 assume an install is for one user or for everyone, never either.
& (Join-Path $Wix 'light.exe') -nologo -ext WixUIExtension -ext WixUtilExtension -cultures:en-us `
    -sice:ICE38 -sice:ICE43 -sice:ICE57 `
    -out $Msi $Object
if ($LASTEXITCODE -ne 0) { throw 'WiX could not link the installer.' }

if (-not $OutDir) { Copy-Item $Msi (Join-Path $Dist 'BareClip.msi') -Force }

$Size = (Get-Item $Msi).Length / 1MB
Write-Host ("Built {0} ({1:N1} MB)" -f $Msi, $Size)
