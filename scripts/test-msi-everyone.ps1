# Checks installing for everyone on this PC: the shared folders and registry, upgrading in place
# and uninstalling. Run scripts/build-msi.ps1 first, on an administrator account but not from an
# elevated window. Each step shows the installer's progress bar and asks for permission, unless
# Windows is set not to.
$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
$Log = Join-Path $env:TEMP 'bareclip-msi-everyone.log'
$Work = Join-Path $Root 'target\msi\everyone'
$UpgradeCode = '{6F2B7C1E-3A94-4D58-9E21-7C0D5A8B4F13}'
$InstallDir = Join-Path $env:ProgramW6432 'BareClip'
$StartMenu = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\BareClip.lnk'
$Desktop = Join-Path $env:PUBLIC 'Desktop\BareClip.lnk'

if (Test-Path $InstallDir) { throw "$InstallDir already exists, so this test will not touch it." }
if (Test-Path 'HKCU:\Software\BareClip') { throw 'BareClip is installed for this user, so this test will not touch it.' }
if (Test-Path 'HKLM:\Software\BareClip') { throw 'BareClip is installed for everyone, so this test will not touch it.' }

function Assert-That([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw "FAILED: $Message" }
    Write-Host "ok: $Message"
}

function Invoke-Msi([string[]]$Arguments) {
    # Start-Process -Wait can go on waiting after the installer has gone. The handle keeps the exit code.
    $Process = Start-Process msiexec.exe -ArgumentList $Arguments -PassThru
    $null = $Process.Handle
    $Process.WaitForExit()
    return $Process.ExitCode
}

# Asks Windows Installer directly which versions of this app are installed.
function Get-Installed {
    $Installer = New-Object -ComObject WindowsInstaller.Installer
    @($Installer.RelatedProducts($UpgradeCode) | ForEach-Object {
        [pscustomobject]@{ Code = $_; Version = $Installer.ProductInfo($_, 'VersionString') }
    })
}

New-Item -ItemType Directory -Force $Work | Out-Null
foreach ($Version in '0.1.0', '0.1.1') {
    & (Join-Path $PSScriptRoot 'build-msi.ps1') -Version $Version -OutDir $Work -SkipBuild
}
$Old = Join-Path $Work 'BareClip-0.1.0.msi'
$New = Join-Path $Work 'BareClip-0.1.1.msi'

$ExitCode = Invoke-Msi @('/i', "`"$Old`"", '/qb', '/l*v', "`"$Log`"", 'MSIINSTALLPERUSER=""')
Assert-That ($ExitCode -eq 0) "the older version installed for everyone (exit $ExitCode)"
try {
    Assert-That (Test-Path (Join-Path $InstallDir 'bareclip.exe')) 'the app is in Program Files'
    Assert-That (Test-Path $StartMenu) 'the shared Start menu shortcut exists'
    Assert-That (Test-Path $Desktop) 'the shared desktop shortcut exists'
    Assert-That (Test-Path 'HKLM:\Software\BareClip') 'its choices are in the machine registry'
    Assert-That (-not (Test-Path 'HKCU:\Software\BareClip')) 'and not in this user''s'

    # An unattended upgrade that isn't told it's for everyone gets no administrator permission,
    # so it has to stop before it removes the copy that is there. Silent, as a progress bar would
    # wait on the message for someone to press OK.
    $ExitCode = Invoke-Msi @('/i', "`"$New`"", '/qn', '/l*v', "`"$Log`"")
    Assert-That ($ExitCode -eq 1603) "an upgrade not told it is for everyone was refused (exit $ExitCode)"
    $Logged = Get-Content $Log -Encoding Unicode -Raw
    Assert-That ($Logged -match 'updating it needs administrator permission') 'and it says why'
    $Installed = @(Get-Installed)
    Assert-That ($Installed.Count -eq 1 -and $Installed[0].Version -like '0.1.0*') 'the older version is still installed'
    Assert-That (Test-Path (Join-Path $InstallDir 'bareclip.exe')) 'and still in Program Files'

    # With no pages to work it out on, an unattended upgrade has to be told it's for everyone.
    $ExitCode = Invoke-Msi @('/i', "`"$New`"", '/qb', '/l*v', "`"$Log`"", 'MSIINSTALLPERUSER=""')
    Assert-That ($ExitCode -eq 0) "the newer version installed over it (exit $ExitCode)"
    $Installed = @(Get-Installed)
    Assert-That ($Installed.Count -eq 1) 'there is still only one copy'
    Assert-That ($Installed[0].Version -like '0.1.1*') "it is now 0.1.1, found $($Installed[0].Version)"
    Assert-That (Test-Path (Join-Path $InstallDir 'bareclip.exe')) 'it stayed in Program Files'
    Assert-That (-not (Test-Path 'HKCU:\Software\BareClip')) 'and did not also install for this user'
}
finally {
    foreach ($Product in Get-Installed) {
        $ExitCode = Invoke-Msi @('/x', $Product.Code, '/qb', '/l*v', "`"$Log`"")
    }
}
Assert-That ($ExitCode -eq 0) "it uninstalled (exit $ExitCode)"
Assert-That (@(Get-Installed).Count -eq 0) 'nothing is left installed'
Assert-That (-not (Test-Path $InstallDir)) 'the folder in Program Files is gone'
Assert-That (-not (Test-Path $StartMenu)) 'the shared Start menu shortcut is gone'
Assert-That (-not (Test-Path $Desktop)) 'the shared desktop shortcut is gone'
Assert-That (-not (Test-Path 'HKLM:\Software\BareClip')) 'the registry entries are gone'
Write-Host 'The install for everyone passed.'
