# Installs the MSI for the current user, checks the installed app really runs on its
# own, then uninstalls it and checks nothing was left behind.
# -MsiPath tests a particular file instead of the newest in dist.
param([string]$MsiPath)
$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
$Msi = if ($MsiPath) { Get-Item $MsiPath } else {
    Get-ChildItem (Join-Path $Root 'dist') -Filter 'BareClip-*.msi' |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
}
if (-not $Msi) { throw 'No MSI found. Run scripts/build-msi.ps1 first.' }

$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\BareClip'
$Shortcut = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\BareClip.lnk'
$Desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) 'BareClip.lnk'
$Log = Join-Path $env:TEMP 'bareclip-msi-test.log'
# The installer's right-click entry, which a test in text.rs holds to what the app takes.
$EditTypes = 'mp4', 'mkv', 'mov', 'webm', 'avi', 'mp3', 'wav', 'flac', 'm4a', 'aac', 'ogg', 'opus'

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

    # "Edit with BareClip" sits beside whatever opens videos and sound, and projects open in BareClip.
    $Classes = 'HKCU:\Software\Classes'
    $Exe = Join-Path $InstallDir 'bareclip.exe'
    $EditCommand = "`"$Exe`" --edit `"%1`""
    foreach ($Type in $EditTypes) {
        $Verb = "$Classes\SystemFileAssociations\.$Type\shell\BareClip.Edit"
        Assert-That ((Get-ItemProperty $Verb -ErrorAction SilentlyContinue).MUIVerb -eq 'Edit with BareClip') ".$Type has Edit with BareClip"
        Assert-That ((Get-Item "$Verb\command").GetValue('') -eq $EditCommand) "  and it runs $EditCommand"
    }
    foreach ($Type in 'ts', 'png', 'jpg', 'bareclip') {
        Assert-That (-not (Test-Path "$Classes\SystemFileAssociations\.$Type\shell\BareClip.Edit")) ".$Type has no Edit with BareClip"
    }
    Assert-That ((Get-Item "$Classes\.bareclip").GetValue('') -eq 'BareClip.Project') '.bareclip files belong to BareClip'
    Assert-That ((Get-Item "$Classes\BareClip.Project\shell\open\command").GetValue('') -eq "`"$Exe`" `"%1`"") '  and open in it'
    Assert-That ((Get-Item "$Classes\BareClip.Project\DefaultIcon").GetValue('') -eq "$Exe,0") '  with its icon'

    $Media = Join-Path $env:TEMP "bareclip edit test $PID"
    $AppData = Join-Path $env:TEMP "bareclip edit appdata $PID"
    New-Item -ItemType Directory -Force $Media, $AppData | Out-Null
    $Ffmpeg = Join-Path $Root 'vendor\ffmpeg\bin\ffmpeg.exe'
    foreach ($Name in 'one', 'two', 'three') {
        & $Ffmpeg -hide_banner -loglevel error -y -f lavfi -i 'testsrc=size=320x180:rate=30:duration=1' -pix_fmt yuv420p (Join-Path $Media "$Name.mp4")
    }
    & $Ffmpeg -hide_banner -loglevel error -y -f lavfi -i 'sine=frequency=440:duration=2' (Join-Path $Media 'song.m4a')
    Set-Content (Join-Path $Media 'code.ts') 'export {}'
    Set-Content (Join-Path $Media 'photo.png') ''

    # The shell reads the same entries Explorer's menu does.
    $Folder = (New-Object -ComObject Shell.Application).Namespace($Media)
    $VerbsOf = { param($Name) @($Folder.ParseName($Name).Verbs() | ForEach-Object Name) }
    Assert-That ((& $VerbsOf 'one.mp4') -contains 'Edit with BareClip') 'the shell offers Edit with BareClip on a video'
    Assert-That ((& $VerbsOf 'song.m4a') -contains 'Edit with BareClip') '  and on sound'
    Assert-That (-not ((& $VerbsOf 'code.ts') -contains 'Edit with BareClip')) '  but not on TypeScript'
    Assert-That (-not ((& $VerbsOf 'photo.png') -contains 'Edit with BareClip')) '  or pictures'

    # The window's recovery copy shows what reached it. The app's folder is moved to a temporary
    # one for this, so your own settings and recovery copies are never touched.
    $ClipsIn = {
        param($Window)
        $Copy = Get-ChildItem (Join-Path $AppData 'BareClip\recovery') -Filter "$($Window.Id)-*" -ErrorAction SilentlyContinue |
            Where-Object Extension -ne '.lock' | Select-Object -First 1
        if (-not $Copy) { return @(-1, -1) }
        try { $Record = Get-Content $Copy.FullName -Raw | ConvertFrom-Json } catch { return @(-1, -1) }
        @(@($Record.project.tracks | ForEach-Object { $_ }).Count, @($Record.project.audio_tracks | ForEach-Object { $_ }).Count)
    }
    $WaitForClips = {
        param($Window, $Pictures, $Sounds)
        $Clock = [Diagnostics.Stopwatch]::StartNew()
        do {
            $Counts = & $ClipsIn $Window
            if ($Counts[0] -eq $Pictures -and $Counts[1] -eq $Sounds) { return $true }
            Start-Sleep -Milliseconds 250
        } while ($Clock.Elapsed.TotalSeconds -lt 30)
        Write-Host "  last seen: $($Counts[0]) pictures and $($Counts[1]) sounds"
        return $false
    }
    $SavedPath = $env:PATH
    $SavedAppData = $env:LOCALAPPDATA
    $Launches = @()
    try {
        $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
        $env:LOCALAPPDATA = $AppData
        # Explorer starts one launch per chosen file, all at once.
        $Launches = foreach ($Name in 'one.mp4', 'two.mp4', 'song.m4a') {
            Start-Process $Exe -ArgumentList '--edit', "`"$(Join-Path $Media $Name)`"" -PassThru
        }
        foreach ($Launch in $Launches) { $null = $Launch.Handle }
        $Clock = [Diagnostics.Stopwatch]::StartNew()
        do {
            $Open = @($Launches | Where-Object { -not $_.HasExited })
            if ($Open.Count -gt 1) { Start-Sleep -Milliseconds 100 }
        } while ($Open.Count -gt 1 -and $Clock.Elapsed.TotalSeconds -lt 20)
        Assert-That ($Open.Count -eq 1) "three launches at once left one window open after $($Clock.ElapsedMilliseconds) ms"
        $Gone = @($Launches | Where-Object HasExited)
        Assert-That (@($Gone | Where-Object ExitCode -ne 0).Count -eq 0) '  and the others left cleanly'
        $Window = $Open[0]
        Assert-That (& $WaitForClips $Window 2 1) '  holding both videos and the sound on its timeline'

        $Late = Start-Process $Exe -ArgumentList '--edit', "`"$(Join-Path $Media 'three.mp4')`"" -PassThru
        $null = $Late.Handle
        $Launches += $Late
        Assert-That ($Late.WaitForExit(10000)) 'a launch while BareClip is open hands its file over and leaves'
        Assert-That (& $WaitForClips $Window 3 1) '  and the open window puts it on the end'
    }
    finally {
        $env:PATH = $SavedPath
        $env:LOCALAPPDATA = $SavedAppData
        # The project has unsaved changes, so closing would ask, and nothing here needs keeping.
        foreach ($Launch in $Launches) { if (-not $Launch.HasExited) { Stop-Process -Id $Launch.Id -Force } }
        Start-Sleep -Milliseconds 500
        Remove-Item -Recurse -Force $Media, $AppData -ErrorAction SilentlyContinue
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
foreach ($Type in $EditTypes) {
    Assert-That (-not (Test-Path "HKCU:\Software\Classes\SystemFileAssociations\.$Type\shell\BareClip.Edit")) "Edit with BareClip is gone from .$Type"
}
Assert-That (-not (Test-Path 'HKCU:\Software\Classes\.bareclip')) '.bareclip is no longer registered'
Assert-That (-not (Test-Path 'HKCU:\Software\Classes\BareClip.Project')) '  and neither is the project type'

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
