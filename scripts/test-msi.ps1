# Installs the MSI for the current user, checks the installed app really runs on its
# own, then uninstalls it and checks nothing was left behind.
$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
$Msi = Get-ChildItem (Join-Path $Root 'dist') -Filter 'BareClip-*.msi' |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $Msi) { throw 'No MSI found. Run scripts/build-msi.ps1 first.' }

$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\BareClip'
$Shortcut = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\BareClip.lnk'
$Desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) 'BareClip.lnk'
$Log = Join-Path $env:TEMP 'bareclip-msi-test.log'

if (Test-Path $InstallDir) { throw "$InstallDir already exists, so this test will not touch it." }
if (Test-Path $Desktop) { throw "$Desktop already exists, so this test will not touch it." }

function Assert-That([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw "FAILED: $Message" }
    Write-Host "ok: $Message"
}

# Windows can report a hidden helper window as the app's main one, so this looks through them
# all for the window with the app's title. The title is BareClip until a project name is added.
Add-Type -Namespace BareClipTest -Name Windows -MemberDefinition @'
public delegate bool Visit(IntPtr window, IntPtr unused);
[DllImport("user32.dll")] public static extern bool EnumWindows(Visit visit, IntPtr unused);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr window, System.Text.StringBuilder text, int size);
[DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, IntPtr w, IntPtr l);
public static IntPtr Find(uint process, string title) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((window, unused) => {
        uint owner;
        GetWindowThreadProcessId(window, out owner);
        var text = new System.Text.StringBuilder(256);
        GetWindowText(window, text, 256);
        if (owner == process && text.ToString() == title) { found = window; }
        return true;
    }, IntPtr.Zero);
    return found;
}
'@
$WM_CLOSE = 0x10

function Invoke-Msi([string[]]$Arguments) {
    # Start-Process -Wait can go on waiting after the installer has gone. The handle keeps the exit code.
    $Process = Start-Process msiexec.exe -ArgumentList $Arguments -PassThru
    $null = $Process.Handle
    $Process.WaitForExit()
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
    Assert-That (Test-Path $Desktop) 'the desktop shortcut exists'

    # Hiding the developer's tools from the app proves the install is self-contained.
    $SavedPath = $env:PATH
    $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
    try {
        $App = Start-Process (Join-Path $InstallDir 'bareclip.exe') -PassThru
        $Clock = [Diagnostics.Stopwatch]::StartNew()
        $Window = [IntPtr]::Zero
        while ($Clock.ElapsedMilliseconds -lt 20000 -and -not $App.HasExited -and $Window -eq [IntPtr]::Zero) {
            $Window = [BareClipTest.Windows]::Find($App.Id, 'BareClip')
            if ($Window -eq [IntPtr]::Zero) { $Window = [BareClipTest.Windows]::Find($App.Id, 'untitled - BareClip') }
            Start-Sleep -Milliseconds 20
        }
        Assert-That (-not $App.HasExited) 'the installed app starts'
        Assert-That ($Window -ne [IntPtr]::Zero) "the installed app shows its window after $($Clock.ElapsedMilliseconds) ms"
        # The title changes once the first frame has been drawn.
        $Clock.Restart()
        do {
            $Named = [BareClipTest.Windows]::Find($App.Id, 'untitled - BareClip')
            if ($Named -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 50 }
        } while ($Named -eq [IntPtr]::Zero -and $Clock.Elapsed.TotalSeconds -lt 15)
        Assert-That ($Named -ne [IntPtr]::Zero) "its title names the new project untitled after $($Clock.ElapsedMilliseconds) ms"
        $Window = $Named
        [void][BareClipTest.Windows]::PostMessage($Window, $WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero)
        Assert-That ($App.WaitForExit(6000)) 'the installed app closes cleanly'
    }
    finally {
        $env:PATH = $SavedPath
        # A failed check must not leave the app running while it is uninstalled.
        if ($App -and -not $App.HasExited) { Stop-Process -Id $App.Id -Force }
    }
}
finally {
    $ExitCode = Invoke-Msi @('/x', "`"$($Msi.FullName)`"", '/qn')
    Assert-That ($ExitCode -eq 0) "the uninstaller finished (exit $ExitCode)"
}

Assert-That (-not (Test-Path $InstallDir)) 'the install folder is gone'
Assert-That (-not (Test-Path $Shortcut)) 'the Start menu shortcut is gone'
Assert-That (-not (Test-Path $Desktop)) 'the desktop shortcut is gone'
Assert-That (-not (Test-Path 'HKCU:\Software\BareClip')) 'the registry entries are gone'

# A chosen folder holds a BareClip folder, and unticked shortcuts are left out. The space
# in the name is on purpose.
$Parent = Join-Path $env:TEMP "bareclip chosen $PID"
New-Item -ItemType Directory -Force $Parent | Out-Null
$ExitCode = Invoke-Msi @('/i', "`"$($Msi.FullName)`"", '/qn', '/l*v', "`"$Log`"", "INSTALLPARENT=`"$Parent\`"", 'ADD_START_MENU=""', 'ADD_DESKTOP=""')
Assert-That ($ExitCode -eq 0) "it installed in a chosen folder (exit $ExitCode)"
try {
    Assert-That (Test-Path (Join-Path $Parent 'BareClip\bareclip.exe')) 'the app is in a BareClip folder inside the chosen one'
    Assert-That (Test-Path (Join-Path $Parent 'BareClip\licenses\GPL-3.0.txt')) 'and so are the licences'
    Assert-That (-not (Test-Path $InstallDir)) 'nothing went to the usual folder'
    Assert-That (-not (Test-Path $Shortcut)) 'the Start menu shortcut was left out'
    Assert-That (-not (Test-Path $Desktop)) 'the desktop shortcut was left out'
}
finally {
    $ExitCode = Invoke-Msi @('/x', "`"$($Msi.FullName)`"", '/qn')
    Assert-That ($ExitCode -eq 0) "the uninstaller finished (exit $ExitCode)"
}
Assert-That (-not (Test-Path (Join-Path $Parent 'BareClip'))) 'the BareClip folder is gone'
Assert-That (Test-Path $Parent) 'the chosen folder itself was left alone'
Assert-That (-not (Test-Path 'HKCU:\Software\BareClip')) 'the registry entries are gone'

# Only one of the two boxes can be ticked.
$ExitCode = Invoke-Msi @('/i', "`"$($Msi.FullName)`"", '/qn', '/l*v', "`"$Log`"", "INSTALLPARENT=`"$Parent\`"", 'ADD_DESKTOP=""')
Assert-That ($ExitCode -eq 0) "it installed with one shortcut (exit $ExitCode)"
try {
    Assert-That (Test-Path $Shortcut) 'the Start menu shortcut was added'
    Assert-That (-not (Test-Path $Desktop)) 'the desktop shortcut was left out'
}
finally {
    $ExitCode = Invoke-Msi @('/x', "`"$($Msi.FullName)`"", '/qn')
    Assert-That ($ExitCode -eq 0) "the uninstaller finished (exit $ExitCode)"
}

# Everyone needs administrator rights, and a refusal leaves nothing behind.
$IsAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $IsAdmin) {
    $ExitCode = Invoke-Msi @('/i', "`"$($Msi.FullName)`"", '/qn', '/l*v', "`"$Log`"", "INSTALLPARENT=`"$Parent\`"", 'MSIINSTALLPERUSER=""')
    Assert-That ($ExitCode -ne 0) "installing for everyone without rights was refused (exit $ExitCode)"
    Assert-That ((Get-Content $Log -Encoding Unicode -Raw) -match 'Error 1925') 'because it needs administrator rights'
    Assert-That (-not (Test-Path (Join-Path $Parent 'BareClip'))) 'and it left nothing behind'
}
Remove-Item -Recurse -Force $Parent
Write-Host 'The MSI passed.'
