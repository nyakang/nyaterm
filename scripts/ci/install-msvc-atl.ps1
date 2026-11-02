$ErrorActionPreference = 'Stop'
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$installation = & $vswhere -latest -products '*' -property installationPath
if (-not $installation) { throw 'Visual Studio installation not found' }
$components = @('Microsoft.VisualStudio.Component.VC.ATL', 'Microsoft.VisualStudio.Component.VC.ATL.ARM64', 'Microsoft.VisualStudio.Component.VC.Tools.ARM64')
$available = & $vswhere -latest -products '*' -requires $components -property installationPath
if (-not $available) {
    $installer = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\setup.exe"
    $arguments = @('modify', '--installPath', ('"' + $installation + '"'), '--quiet', '--norestart')
    foreach ($component in $components) { $arguments += @('--add', $component) }
    $process = Start-Process -FilePath $installer -ArgumentList $arguments -Wait -PassThru -WindowStyle Hidden
    if ($process.ExitCode -notin @(0, 3010)) { throw "ATL installation failed: $($process.ExitCode)" }
}
