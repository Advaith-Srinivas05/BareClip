# Checks that a newer MSI replaces an older install instead of sitting beside it, and that an
# older MSI won't replace a newer install. Run scripts/build-msi.ps1 first.
$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\BareClip'
$Log = Join-Path $env:TEMP 'bareclip-msi-upgrade.log'
$Work = Join-Path $Root 'target\msi\upgrade'
$UpgradeCode = '{6F2B7C1E-3A94-4D58-9E21-7C0D5A8B4F13}'

if (Test-Path $InstallDir) { throw "$InstallDir already exists, so this test will not touch it." }

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

$ExitCode = Invoke-Msi @('/i', "`"$Old`"", '/qn', '/l*v', "`"$Log`"")
Assert-That ($ExitCode -eq 0) "the older version installed (exit $ExitCode)"
try {
    $Installed = @(Get-Installed)
    Assert-That ($Installed.Count -eq 1) 'one copy is installed'
    Assert-That ($Installed[0].Version -like '0.1.0*') "it is 0.1.0, found $($Installed[0].Version)"

    $ExitCode = Invoke-Msi @('/i', "`"$New`"", '/qn', '/l*v', "`"$Log`"")
    Assert-That ($ExitCode -eq 0) "the newer version installed over it (exit $ExitCode)"
    $Installed = @(Get-Installed)
    Assert-That ($Installed.Count -eq 1) 'there is still only one copy'
    Assert-That ($Installed[0].Version -like '0.1.1*') "it is now 0.1.1, found $($Installed[0].Version)"
    Assert-That (Test-Path (Join-Path $InstallDir 'bareclip.exe')) 'the app is still there'

    $ExitCode = Invoke-Msi @('/i', "`"$Old`"", '/qn', '/l*v', "`"$Log`"")
    Assert-That ($ExitCode -ne 0) "the older version refused to replace the newer one (exit $ExitCode)"
    $Installed = @(Get-Installed)
    Assert-That ($Installed.Count -eq 1 -and $Installed[0].Version -like '0.1.1*') 'the newer version is still installed'
}
finally {
    # Whatever went wrong, nothing from this test stays installed.
    foreach ($Product in Get-Installed) {
        $ExitCode = Invoke-Msi @('/x', $Product.Code, '/qn', '/l*v', "`"$Log`"")
    }
}
Assert-That ($ExitCode -eq 0) "it uninstalled (exit $ExitCode)"
Assert-That (@(Get-Installed).Count -eq 0) 'nothing is left installed'
Assert-That (-not (Test-Path (Join-Path $InstallDir 'bareclip.exe'))) 'the app is gone'

# An upgrade stays in the chosen folder and keeps the shortcut choices, without being told.
$Parent = Join-Path $env:TEMP "bareclip upgrade $PID"
$Desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) 'BareClip.lnk'
$StartMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\BareClip.lnk'
New-Item -ItemType Directory -Force $Parent | Out-Null
$ExitCode = Invoke-Msi @('/i', "`"$Old`"", '/qn', '/l*v', "`"$Log`"", "INSTALLPARENT=`"$Parent\`"", 'ADD_DESKTOP=""')
Assert-That ($ExitCode -eq 0) "the older version installed in a chosen folder (exit $ExitCode)"
try {
    $ExitCode = Invoke-Msi @('/i', "`"$New`"", '/qn', '/l*v', "`"$Log`"")
    Assert-That ($ExitCode -eq 0) "the newer version installed over it (exit $ExitCode)"
    $Installed = @(Get-Installed)
    Assert-That ($Installed.Count -eq 1 -and $Installed[0].Version -like '0.1.1*') 'there is one copy, and it is the newer one'
    Assert-That (Test-Path (Join-Path $Parent 'BareClip\bareclip.exe')) 'it stayed in the chosen folder'
    Assert-That (-not (Test-Path $InstallDir)) 'nothing went to the usual folder'
    Assert-That (Test-Path $StartMenu) 'the Start menu shortcut stayed'
    Assert-That (-not (Test-Path $Desktop)) 'the declined desktop shortcut did not come back'
}
finally {
    foreach ($Product in Get-Installed) {
        $ExitCode = Invoke-Msi @('/x', $Product.Code, '/qn', '/l*v', "`"$Log`"")
    }
}
Assert-That ($ExitCode -eq 0) "it uninstalled (exit $ExitCode)"
Assert-That (-not (Test-Path (Join-Path $Parent 'BareClip'))) 'the BareClip folder is gone'
Assert-That (-not (Test-Path 'HKCU:\Software\BareClip')) 'the registry entries are gone'

# An upgrade stays in a whole path chosen in the one folder box, which needn't be inside a parent.
$Chosen = Join-Path $Parent 'somewhere else\BareClip'
$ExitCode = Invoke-Msi @('/i', "`"$Old`"", '/qn', '/l*v', "`"$Log`"", "INSTALLFOLDER=`"$Chosen\`"")
Assert-That ($ExitCode -eq 0) "the older version installed in a whole chosen path (exit $ExitCode)"
try {
    $ExitCode = Invoke-Msi @('/i', "`"$New`"", '/qn', '/l*v', "`"$Log`"")
    Assert-That ($ExitCode -eq 0) "the newer version installed over it (exit $ExitCode)"
    $Installed = @(Get-Installed)
    Assert-That ($Installed.Count -eq 1 -and $Installed[0].Version -like '0.1.1*') 'there is one copy, and it is the newer one'
    Assert-That (Test-Path (Join-Path $Chosen 'bareclip.exe')) 'it stayed in the chosen path'
    Assert-That (-not (Test-Path $InstallDir)) 'nothing went to the usual folder'
}
finally {
    foreach ($Product in Get-Installed) {
        $ExitCode = Invoke-Msi @('/x', $Product.Code, '/qn', '/l*v', "`"$Log`"")
    }
}
Assert-That ($ExitCode -eq 0) "it uninstalled (exit $ExitCode)"
Assert-That (-not (Test-Path $Chosen)) 'the chosen folder is gone'

# The real first release, if it is here, only knows about the parent folder. Upgrading from it has
# to find its install and stay there.
$Released = Join-Path $Root 'dist\BareClip-0.1.0.msi'
$Current = Join-Path $Root 'dist\BareClip.msi'
if (Test-Path $Released) {
    $ExitCode = Invoke-Msi @('/i', "`"$Released`"", '/qn', '/l*v', "`"$Log`"", "INSTALLPARENT=`"$Parent\`"")
    Assert-That ($ExitCode -eq 0) "the real 0.1.0 installed (exit $ExitCode)"
    try {
        $ExitCode = Invoke-Msi @('/i', "`"$Current`"", '/qn', '/l*v', "`"$Log`"")
        Assert-That ($ExitCode -eq 0) "this version installed over the real 0.1.0 (exit $ExitCode)"
        $Installed = @(Get-Installed)
        Assert-That ($Installed.Count -eq 1 -and $Installed[0].Version -notlike '0.1.0*') "there is one copy, now $($Installed[0].Version)"
        Assert-That (Test-Path (Join-Path $Parent 'BareClip\bareclip.exe')) 'it stayed where 0.1.0 was'
        Assert-That (-not (Test-Path $InstallDir)) 'nothing went to the usual folder'
    }
    finally {
        foreach ($Product in Get-Installed) {
            $ExitCode = Invoke-Msi @('/x', $Product.Code, '/qn', '/l*v', "`"$Log`"")
        }
    }
    Assert-That ($ExitCode -eq 0) "it uninstalled (exit $ExitCode)"
}
else {
    Write-Host 'skipped: dist\BareClip-0.1.0.msi is not here, so the real first release was not tried.'
}
Assert-That (-not (Test-Path 'HKCU:\Software\BareClip')) 'the registry entries are gone'
Remove-Item -Recurse -Force $Parent
Write-Host 'The upgrade test passed.'
