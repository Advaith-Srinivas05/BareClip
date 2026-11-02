# Installs the MSI for the current user, checks the installed app really runs on its
# own, then uninstalls it and checks nothing was left behind.
$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
$Msi = Get-ChildItem (Join-Path $Root 'dist') -Filter 'BareClip-*.msi' |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $Msi) { throw 'No MSI found. Run scripts/build-msi.ps1 first.' }

$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\BareClip'
$Shortcut = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\BareClip.lnk'
$Log = Join-Path $env:TEMP 'bareclip-msi-test.log'

if (Test-Path $InstallDir) { throw "$InstallDir already exists, so this test will not touch it." }

function Assert-That([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw "FAILED: $Message" }
    Write-Host "ok: $Message"
}

function Invoke-Msi([string[]]$Arguments) {
    $Process = Start-Process msiexec.exe -ArgumentList $Arguments -Wait -PassThru
    return $Process.ExitCode
}

$ExitCode = Invoke-Msi @('/i', "`"$($Msi.FullName)`"", '/qn', '/l*v', "`"$Log`"")
Assert-That ($ExitCode -eq 0) "the installer finished without administrator rights (exit $ExitCode)"

try {
    foreach ($File in 'bareclip.exe', 'avcodec-63.dll', 'avformat-63.dll', 'avutil-61.dll', 'swresample-7.dll', 'swscale-10.dll') {
        Assert-That (Test-Path (Join-Path $InstallDir $File)) "$File is installed"
    }
    foreach ($File in 'BareClip-MIT.txt', 'THIRD-PARTY-NOTICES.txt', 'FFmpeg-LGPL-3.0.txt', 'GPL-3.0.txt', 'HankenGrotesk-OFL.txt', 'Lucide-ISC.txt') {
        Assert-That (Test-Path (Join-Path $InstallDir "licenses\$File")) "the licence $File is installed"
    }
    Assert-That (Test-Path $Shortcut) 'the Start menu shortcut exists'

    # Hiding the developer's tools from the app proves the install is self-contained.
    $SavedPath = $env:PATH
    $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
    try {
        $App = Start-Process (Join-Path $InstallDir 'bareclip.exe') -PassThru
        $Clock = [Diagnostics.Stopwatch]::StartNew()
        while ($Clock.ElapsedMilliseconds -lt 20000 -and -not $App.HasExited) {
            $App.Refresh()
            if ($App.MainWindowHandle -ne 0 -and $App.MainWindowTitle -eq 'BareClip') { break }
            Start-Sleep -Milliseconds 20
        }
        Assert-That (-not $App.HasExited) 'the installed app starts'
        Assert-That ($App.MainWindowTitle -eq 'BareClip') "the installed app shows its window after $($Clock.ElapsedMilliseconds) ms"
        Start-Sleep -Seconds 1
        [void]$App.CloseMainWindow()
        Assert-That ($App.WaitForExit(6000)) 'the installed app closes cleanly'
    }
    finally {
        $env:PATH = $SavedPath
    }
}
finally {
    $ExitCode = Invoke-Msi @('/x', "`"$($Msi.FullName)`"", '/qn')
    Assert-That ($ExitCode -eq 0) "the uninstaller finished (exit $ExitCode)"
}

Assert-That (-not (Test-Path $InstallDir)) 'the install folder is gone'
Assert-That (-not (Test-Path $Shortcut)) 'the Start menu shortcut is gone'
Assert-That (-not (Test-Path 'HKCU:\Software\BareClip')) 'the registry entries are gone'
Write-Host 'The MSI passed.'
