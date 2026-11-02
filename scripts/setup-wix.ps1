# Downloads the pinned WiX toolset into vendor/wix. It needs no system install.
$ErrorActionPreference = 'Stop'

$Tag = 'wix3141rtm'
$Name = 'wix314-binaries'
$Sha256 = '6ac824e1642d6f7277d0ed7ea09411a508f6116ba6fae0aa5f2c7daa2ff43d31'

$Root = Split-Path -Parent $PSScriptRoot
$Vendor = Join-Path $Root 'vendor'
$Target = Join-Path $Vendor 'wix'
$Marker = Join-Path $Target '.build-name'

if ((Test-Path $Marker) -and ((Get-Content $Marker -Raw).Trim() -eq $Tag)) {
    Write-Host "WiX $Tag is already set up."
    return
}

New-Item -ItemType Directory -Force $Vendor | Out-Null
$Zip = Join-Path $Vendor "$Name.zip"

try {
    Invoke-WebRequest "https://github.com/wixtoolset/wix3/releases/download/$Tag/$Name.zip" -OutFile $Zip

    $Actual = (Get-FileHash $Zip -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($Actual -ne $Sha256) {
        throw "Checksum mismatch: expected $Sha256, got $Actual"
    }

    if (Test-Path $Target) { Remove-Item -Recurse -Force $Target }
    Expand-Archive $Zip -DestinationPath $Target
    Set-Content $Marker $Tag
    Write-Host "WiX $Tag is ready in $Target."
}
finally {
    if (Test-Path $Zip) { Remove-Item -Force $Zip }
}
