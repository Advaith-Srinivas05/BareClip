# Downloads the pinned LGPL shared build of FFmpeg into vendor/ffmpeg.
$ErrorActionPreference = 'Stop'

$Tag = 'autobuild-2026-11-01-12-49'
$Name = 'ffmpeg-n9.0.2-42-gd7643a0fe3-win64-lgpl-shared-9.0'
$Sha256 = 'dd3d3791e1d9fe7bca2e62a4c2c916cebe6b46d21cc6b5e3706eb975a1bfe2d4'

$Root = Split-Path -Parent $PSScriptRoot
$Vendor = Join-Path $Root 'vendor'
$Target = Join-Path $Vendor 'ffmpeg'
$Marker = Join-Path $Target '.build-name'

if ((Test-Path $Marker) -and ((Get-Content $Marker -Raw).Trim() -eq $Name)) {
    Write-Host "FFmpeg $Name is already set up."
    return
}

New-Item -ItemType Directory -Force $Vendor | Out-Null
$Zip = Join-Path $Vendor "$Name.zip"
$Extracted = Join-Path $Vendor $Name

try {
    Invoke-WebRequest "https://github.com/BtbN/FFmpeg-Builds/releases/download/$Tag/$Name.zip" -OutFile $Zip

    $Actual = (Get-FileHash $Zip -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($Actual -ne $Sha256) {
        throw "Checksum mismatch: expected $Sha256, got $Actual"
    }

    if (Test-Path $Extracted) { Remove-Item -Recurse -Force $Extracted }
    Expand-Archive $Zip -DestinationPath $Vendor

    if (Test-Path $Target) { Remove-Item -Recurse -Force $Target }
    Move-Item $Extracted $Target
    Set-Content $Marker $Name
    Write-Host "FFmpeg $Name is ready in $Target."
}
finally {
    if (Test-Path $Zip) { Remove-Item -Force $Zip }
}
