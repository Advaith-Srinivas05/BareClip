# Walks the installer's real pages the way a person would and checks the result. Run
# scripts/build-msi.ps1 first, in Windows PowerShell. The installer's windows open on screen
# for about a minute, so leave them alone. Pass -Pictures to save each page. Pass
# -InstallForEveryone on an administrator account to press Install on the Everyone path too,
# which asks for administrator permission unless Windows is set not to.
param([string]$Pictures, [switch]$InstallForEveryone)
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Drawing

$Root = Split-Path -Parent $PSScriptRoot
$Msi = Join-Path $Root 'dist\BareClip.msi'
if (-not (Test-Path $Msi)) { throw 'No MSI found. Run scripts/build-msi.ps1 first.' }

$Parent = Join-Path $env:TEMP "bareclip screens $PID"
$InstallDir = Join-Path $Parent 'BareClip'
$StartMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\BareClip.lnk'
$Desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) 'BareClip.lnk'
$Log = Join-Path $env:TEMP 'bareclip-msi-screens.log'
$Ui = [Windows.Automation.AutomationElement]
$Tree = [Windows.Automation.TreeScope]

# The installer's controls can't be pressed through UI Automation, so they get Windows messages.
Add-Type -Namespace BareClipTest -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, IntPtr w, IntPtr l);
[DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr window, uint message, IntPtr w, IntPtr l);
[DllImport("user32.dll", CharSet = CharSet.Unicode, EntryPoint = "SendMessageW")] public static extern IntPtr SendText(IntPtr window, uint message, IntPtr w, string text);
[DllImport("user32.dll", CharSet = CharSet.Unicode, EntryPoint = "SendMessageW")] public static extern IntPtr ReadText(IntPtr window, uint message, IntPtr w, System.Text.StringBuilder text);
[DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr window, IntPtr canvas, uint flags);
'@
$BM_GETCHECK = 0xF0
$BM_CLICK = 0xF5
$WM_SETTEXT = 0x0C
$WM_GETTEXT = 0x0D

function Assert-That([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw "FAILED: $Message" }
    Write-Host "ok: $Message"
}

# The installer's window, which is replaced whenever a new page opens.
function Get-Page {
    param([string]$Title = 'BareClip Setup')
    $Clock = [Diagnostics.Stopwatch]::StartNew()
    while ($Clock.Elapsed.TotalSeconds -lt 30) {
        $Name = New-Object Windows.Automation.PropertyCondition($Ui::NameProperty, $Title)
        $Window = $Ui::RootElement.FindFirst($Tree::Children, $Name)
        if ($Window) { return $Window }
        Start-Sleep -Milliseconds 100
    }
    throw "FAILED: no window called $Title appeared"
}

function Find-Control($Page, [string]$Name) {
    $Condition = New-Object Windows.Automation.PropertyCondition($Ui::NameProperty, $Name)
    $Page.FindFirst($Tree::Descendants, $Condition)
}

# Waits for a control, since a button or text only exists once its page has opened.
function Wait-Control([string]$Name, [string]$Title = 'BareClip Setup') {
    $Clock = [Diagnostics.Stopwatch]::StartNew()
    while ($Clock.Elapsed.TotalSeconds -lt 60) {
        try {
            $Found = Find-Control (Get-Page $Title) $Name
            if ($Found -and $Found.Current.IsEnabled -and -not $Found.Current.IsOffscreen) { return $Found }
        }
        catch [Windows.Automation.ElementNotAvailableException] { }
        Start-Sleep -Milliseconds 100
    }
    throw "FAILED: $Name never appeared"
}

function Invoke-Button([string]$Name, [string]$Title = 'BareClip Setup') {
    $Button = Wait-Control $Name $Title
    [void][BareClipTest.Native]::PostMessage([IntPtr]$Button.Current.NativeWindowHandle, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 500
}

function Set-Tick([string]$Name, [bool]$Ticked) {
    if ((Get-Tick $Name) -ne $Ticked) { Invoke-Button $Name }
}

function Get-Tick([string]$Name) {
    $Box = Wait-Control $Name
    [BareClipTest.Native]::SendMessage([IntPtr]$Box.Current.NativeWindowHandle, $BM_GETCHECK, [IntPtr]::Zero, [IntPtr]::Zero) -eq [IntPtr]1
}

function Get-AllText($Page) {
    $Found = $Page.FindAll($Tree::Descendants, [Windows.Automation.Condition]::TrueCondition)
    @($Found | ForEach-Object {
        $_.Current.Name
        if ($_.Current.ClassName -match 'Edit') {
            $Typed = New-Object Text.StringBuilder 1024
            [void][BareClipTest.Native]::ReadText([IntPtr]$_.Current.NativeWindowHandle, $WM_GETTEXT, [IntPtr]1024, $Typed)
            $Typed.ToString()
        }
    }) -join "`n"
}

# Types into the page's last text box, which on the browse window is the folder name.
function Set-FolderText([string]$Path) {
    $Page = Get-Page
    $Boxes = @($Page.FindAll($Tree::Descendants, [Windows.Automation.Condition]::TrueCondition) | Where-Object { $_.Current.ClassName -match 'Edit' -and $_.Current.IsEnabled })
    $Box = $Boxes[$Boxes.Count - 1]
    [void][BareClipTest.Native]::SendText([IntPtr]$Box.Current.NativeWindowHandle, $WM_SETTEXT, [IntPtr]::Zero, $Path)
    Start-Sleep -Milliseconds 200
}

function Save-Picture([string]$Name, [string]$Inside) {
    if (-not $Pictures) { return }
    New-Item -ItemType Directory -Force $Pictures | Out-Null
    Start-Sleep -Milliseconds 300
    # Asks the window to draw itself, so nothing else on the screen ends up in the picture.
    $Page = Get-Page
    if ($Inside) {
        $Walker = [Windows.Automation.TreeWalker]::RawViewWalker
        $Page = $Walker.GetParent((Wait-Control $Inside))
    }
    $Box = $Page.Current.BoundingRectangle
    $Image = New-Object Drawing.Bitmap ([int]$Box.Width), ([int]$Box.Height)
    $Canvas = [Drawing.Graphics]::FromImage($Image)
    $Surface = $Canvas.GetHdc()
    [void][BareClipTest.Native]::PrintWindow([IntPtr]$Page.Current.NativeWindowHandle, $Surface, 2)
    $Canvas.ReleaseHdc($Surface)
    $Image.Save((Join-Path $Pictures "$Name.png"))
    $Canvas.Dispose()
    $Image.Dispose()
}

if ((Test-Path $StartMenu) -or (Test-Path $Desktop)) { throw 'A BareClip shortcut already exists, so this test will not touch it.' }
if (Test-Path 'HKCU:\Software\BareClip') { throw 'BareClip is already installed, so this test will not touch it.' }
if (Test-Path 'HKLM:\Software\BareClip') { throw 'BareClip is already installed for everyone, so this test will not touch it.' }
New-Item -ItemType Directory -Force $Parent | Out-Null

# Everyone needs administrator permission to finish, so this walk normally stops at the last page.
$EveryoneLog = Join-Path $env:TEMP 'bareclip-msi-everyone-pages.log'
$Installer = Start-Process msiexec.exe -ArgumentList @('/i', "`"$Msi`"", '/l*v', "`"$EveryoneLog`"") -PassThru
try {
    [void](Wait-Control 'I accept the terms in the License Agreement')
    Set-Tick 'I accept the terms in the License Agreement' $true
    Invoke-Button 'Next'
    [void](Wait-Control 'Everyone on this PC')
    Invoke-Button 'Everyone on this PC'
    Assert-That (Get-Tick 'Everyone on this PC') 'Everyone can be chosen'
    Invoke-Button 'Next'
    [void](Wait-Control 'Add to the Start menu')
    Assert-That ((Get-AllText (Get-Page)) -like "*$env:ProgramW6432\BareClip\*") 'Everyone starts in Program Files'
    Save-Picture '8-everyone-options'
    Invoke-Button 'Next'
    [void](Wait-Control 'Install')
    Save-Picture '9-everyone-ready'
    $Text = Get-AllText (Get-Page)
    Assert-That ($Text -like "*$env:ProgramW6432\BareClip\*") 'Program Files is not refused for Everyone'
    Assert-That ($Text -like '*available to everyone*') 'the last page says it is for everyone'

    if ($InstallForEveryone) {
        $ProgramFilesDir = Join-Path $env:ProgramW6432 'BareClip'
        $SharedStartMenu = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\BareClip.lnk'
        $SharedDesktop = Join-Path $env:PUBLIC 'Desktop\BareClip.lnk'
        Assert-That (-not (Test-Path $ProgramFilesDir)) 'Program Files has no BareClip yet'
        try {
            Invoke-Button 'Install'
            [void](Wait-Control 'Finish')
            Invoke-Button 'Finish'
            Assert-That ($Installer.WaitForExit(30000)) 'the installer closes'
            Assert-That ($Installer.ExitCode -eq 0) "the installer succeeded (exit $($Installer.ExitCode))"
            Assert-That (Test-Path (Join-Path $ProgramFilesDir 'bareclip.exe')) 'the app is in Program Files'
            Assert-That (Test-Path 'HKLM:\Software\BareClip') 'its choices are in the machine registry'
            Assert-That (-not (Test-Path 'HKCU:\Software\BareClip')) 'and not in this user''s'
            Assert-That (Test-Path $SharedStartMenu) 'the shared Start menu shortcut exists'
            Assert-That (Test-Path $SharedDesktop) 'the shared desktop shortcut exists'
            Assert-That (-not (Test-Path $StartMenu)) 'there is no separate shortcut for this user'

            # The wizard works out for itself that an upgrade is for everyone, and skips the choice.
            # The version only has to stay above whatever is being released.
            $Work = Join-Path $Root 'target\msi\everyone-pages'
            [void](& (Join-Path $PSScriptRoot 'build-msi.ps1') -Version '9.9.9' -OutDir $Work -SkipBuild)
            $Newer = Join-Path $Work 'BareClip-9.9.9.msi'
            $UpgradeLog = Join-Path $env:TEMP 'bareclip-msi-everyone-upgrade.log'
            $Installer = Start-Process msiexec.exe -ArgumentList @('/i', "`"$Newer`"", '/l*v', "`"$UpgradeLog`"") -PassThru
            [void](Wait-Control 'I accept the terms in the License Agreement')
            Set-Tick 'I accept the terms in the License Agreement' $true
            Invoke-Button 'Next'
            [void](Wait-Control 'Add to the Start menu')
            Assert-That ((Get-AllText (Get-Page)) -like "*$ProgramFilesDir\*") 'the upgrade stays in Program Files without asking who it is for'
            Invoke-Button 'Next'
            [void](Wait-Control 'Install')
            Assert-That ((Get-AllText (Get-Page)) -like '*available to everyone*') 'the upgrade says it is for everyone'
            Invoke-Button 'Install'
            [void](Wait-Control 'Finish')
            Invoke-Button 'Finish'
            Assert-That ($Installer.WaitForExit(30000)) 'the upgrade closes'
            Assert-That ($Installer.ExitCode -eq 0) "the upgrade succeeded (exit $($Installer.ExitCode))"
            $Windows = New-Object -ComObject WindowsInstaller.Installer
            $Installed = @($Windows.RelatedProducts('{6F2B7C1E-3A94-4D58-9E21-7C0D5A8B4F13}'))
            Assert-That ($Installed.Count -eq 1) 'there is still only one copy'
            Assert-That ($Windows.ProductInfo($Installed[0], 'VersionString') -like '9.9.9*') 'and it is the newer one'
            Assert-That (Test-Path (Join-Path $ProgramFilesDir 'bareclip.exe')) 'it stayed in Program Files'
            Assert-That (-not (Test-Path 'HKCU:\Software\BareClip')) 'and did not also install for this user'
        }
        finally {
            if (-not $Installer.HasExited) { Stop-Process -Id $Installer.Id -Force -ErrorAction SilentlyContinue; Start-Sleep -Seconds 2 }
            $Windows = New-Object -ComObject WindowsInstaller.Installer
            foreach ($Product in @($Windows.RelatedProducts('{6F2B7C1E-3A94-4D58-9E21-7C0D5A8B4F13}'))) {
                $Removed = Start-Process msiexec.exe -ArgumentList @('/x', $Product, '/qb') -Wait -PassThru
                Write-Host "uninstall exit $($Removed.ExitCode)"
            }
        }
        Assert-That (-not (Test-Path $ProgramFilesDir)) 'the uninstaller removed it from Program Files'
        Assert-That (-not (Test-Path $SharedStartMenu)) 'the shared Start menu shortcut is gone'
        Assert-That (-not (Test-Path $SharedDesktop)) 'the shared desktop shortcut is gone'
        Assert-That (-not (Test-Path 'HKLM:\Software\BareClip')) 'the registry entries are gone'
        Write-Host 'The install for everyone passed.'
        exit 0
    }

    # Changing your mind puts the folder back.
    Invoke-Button 'Back'
    [void](Wait-Control 'Add to the Start menu')
    Invoke-Button 'Back'
    [void](Wait-Control 'Just me')
    Invoke-Button 'Just me'
    Invoke-Button 'Next'
    [void](Wait-Control 'Add to the Start menu')
    Assert-That ((Get-AllText (Get-Page)) -like "*$env:LOCALAPPDATA\Programs\BareClip\*") 'going back to Just me puts the folder back'

    Invoke-Button 'Cancel'
    Invoke-Button 'Yes'
    [void](Wait-Control 'Finish')
    Invoke-Button 'Finish'
    Assert-That ($Installer.WaitForExit(30000)) 'cancelling closes the installer'
    Assert-That (-not (Test-Path 'HKCU:\Software\BareClip')) 'and installs nothing'
}
finally {
    if (-not $Installer.HasExited) { Stop-Process -Id $Installer.Id -Force -ErrorAction SilentlyContinue; Start-Sleep -Seconds 2 }
}

$Installer = Start-Process msiexec.exe -ArgumentList @('/i', "`"$Msi`"", '/l*v', "`"$Log`"") -PassThru
try {
    # The agreement.
    [void](Wait-Control 'I accept the terms in the License Agreement')
    Save-Picture '1-agreement'
    Assert-That (-not (Find-Control (Get-Page) 'Print')) 'the agreement page has no Print button'
    Assert-That (-not (Find-Control (Get-Page) '&Print')) 'not under its shortcut name either'
    Set-Tick 'I accept the terms in the License Agreement' $true
    Invoke-Button 'Next'

    # Who it is for.
    [void](Wait-Control 'Everyone on this PC')
    Save-Picture '2-who'
    Assert-That (Get-Tick 'Just me') 'Just me starts chosen'
    Invoke-Button 'Next'

    # The folder and shortcuts.
    [void](Wait-Control 'Add to the Start menu')
    Assert-That (Get-Tick 'Add to the Start menu') 'the Start menu box starts ticked'
    Assert-That (Get-Tick 'Add a desktop shortcut') 'the desktop box starts ticked'
    $Text = Get-AllText (Get-Page)
    Assert-That ($Text -like "*$env:LOCALAPPDATA\Programs\BareClip\*") 'the folder starts in your own Programs folder'
    Save-Picture '3-options'
    $Boxes = @((Get-Page).FindAll($Tree::Descendants, [Windows.Automation.Condition]::TrueCondition) | Where-Object { $_.Current.ClassName -match 'Edit' })
    Assert-That ($Boxes.Count -eq 1) 'the page has one folder box'

    # A path typed without BareClip on the end gets it, with or without a final slash.
    Set-FolderText "$Parent\typed"
    Invoke-Button 'Next'
    [void](Wait-Control 'Install')
    Assert-That ((Get-AllText (Get-Page)) -like "*$Parent\typed\BareClip\*") 'a typed folder gets BareClip on the end'
    Invoke-Button 'Back'
    [void](Wait-Control 'Add to the Start menu')

    # A protected folder is refused before anything is installed.
    Set-FolderText "$env:ProgramW6432\"
    Invoke-Button 'Next'
    [void](Wait-Control 'OK')
    Save-Picture '4-needs-admin' 'OK'
    Assert-That ((Get-AllText (Get-Page)) -like '*Only administrators*') 'Program Files explains that it needs Everyone'
    Invoke-Button 'OK'

    # Browsing to a folder puts the app in a BareClip folder inside it.
    [void](Wait-Control 'Add to the Start menu')
    Invoke-Button 'Browse...'
    [void](Wait-Control 'OK')
    Set-FolderText "$Parent\"
    Invoke-Button 'OK'
    [void](Wait-Control 'Add to the Start menu')
    Start-Sleep -Milliseconds 500
    $Text = Get-AllText (Get-Page)
    Assert-That ($Text -like "*$InstallDir\*") 'the page shows a BareClip folder inside the chosen one'
    Save-Picture '5-browsed'

    # A folder already called BareClip isn't given another one.
    Invoke-Button 'Browse...'
    [void](Wait-Control 'OK')
    Set-FolderText "$InstallDir\"
    Invoke-Button 'OK'
    [void](Wait-Control 'Add to the Start menu')
    Start-Sleep -Milliseconds 500
    $Text = Get-AllText (Get-Page)
    Assert-That ($Text -like "*$InstallDir\*") 'a folder called BareClip stays as it is'
    Assert-That ($Text -notlike '*BareClip\BareClip*') 'and has no second BareClip added'

    Set-Tick 'Add a desktop shortcut' $false
    Invoke-Button 'Next'

    # Ready to go.
    [void](Wait-Control 'Install')
    Save-Picture '6-ready'
    $Text = Get-AllText (Get-Page)
    Assert-That ($Text -like "*$InstallDir\*") 'the last page names the final folder'
    Assert-That ($Text -like '*available to you only*') 'the last page says it is for you only'
    Invoke-Button 'Install'

    [void](Wait-Control 'Finish')
    Save-Picture '7-finished'
    Invoke-Button 'Finish'
    Assert-That ($Installer.WaitForExit(30000)) 'the installer closes'
    Assert-That ($Installer.ExitCode -eq 0) "the installer succeeded without administrator rights (exit $($Installer.ExitCode))"

    $Logged = Get-Content $Log -Encoding Unicode -Raw
    Assert-That ($Logged -match 'dual mode package and is being installed in Per-User mode') 'Windows did not ask for administrator permission'
    Assert-That (Test-Path (Join-Path $InstallDir 'bareclip.exe')) 'the app is in the BareClip folder inside the chosen one'
    Assert-That (Test-Path $StartMenu) 'the Start menu shortcut was added'
    Assert-That (-not (Test-Path $Desktop)) 'the desktop shortcut was left out, as unticked'
}
finally {
    if (-not $Installer.HasExited) { Stop-Process -Id $Installer.Id -Force -ErrorAction SilentlyContinue; Start-Sleep -Seconds 2 }
    $Removed = Start-Process msiexec.exe -ArgumentList @('/x', "`"$Msi`"", '/qn') -Wait -PassThru
    Write-Host "uninstall exit $($Removed.ExitCode)"
}
Assert-That (-not (Test-Path $InstallDir)) 'the uninstaller removed the BareClip folder'
Assert-That (Test-Path $Parent) 'and left the chosen folder alone'
Assert-That (-not (Test-Path $StartMenu)) 'the Start menu shortcut is gone'
Assert-That (-not (Test-Path 'HKCU:\Software\BareClip')) 'the registry entries are gone'
Remove-Item -Recurse -Force $Parent
Write-Host 'The installer pages passed.'
