# Runs cargo with the FFmpeg DLLs on PATH, for example: ./scripts/cargo.ps1 test --workspace
# PowerShell drops a bare --, so quote it: ./scripts/cargo.ps1 clippy '--' -D warnings
$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
& (Join-Path $PSScriptRoot 'setup-ffmpeg.ps1')

$env:PATH = "$(Join-Path $Root 'vendor\ffmpeg\bin');$env:USERPROFILE\.cargo\bin;$env:PATH"
Set-Location $Root
& cargo @args
exit $LASTEXITCODE
