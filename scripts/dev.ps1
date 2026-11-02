# Runs the dev app, then rebuilds and reopens it whenever code or assets change.
# Press Ctrl+C here to stop. Needs watchexec: winget install watchexec.watchexec
$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
& (Join-Path $PSScriptRoot 'setup-ffmpeg.ps1')

$env:PATH = "$(Join-Path $Root 'vendor\ffmpeg\bin');$env:USERPROFILE\.cargo\bin;$env:PATH"

$Watchexec = Get-Command watchexec -ErrorAction SilentlyContinue
if ($Watchexec) {
    $Watchexec = $Watchexec.Source
} else {
    # A terminal opened before watchexec was installed will not have it on PATH yet.
    $Packages = Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Packages'
    $Watchexec = Get-ChildItem $Packages -Recurse -Filter watchexec.exe -ErrorAction SilentlyContinue |
        Select-Object -First 1 -ExpandProperty FullName
    if (-not $Watchexec) {
        throw 'watchexec is not installed. Run: winget install watchexec.watchexec'
    }
}

Set-Location $Root
& $Watchexec --restart --watch crates --watch assets --watch Cargo.toml `
    --exts rs,toml,png,ttf,json --shell none -- cargo run -p bareclip-app
exit $LASTEXITCODE
