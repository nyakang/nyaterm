[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$BinaryPath,

  [string]$ReportPath = "win7-pe-imports.txt"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Find-Dumpbin {
  $command = Get-Command dumpbin.exe -ErrorAction SilentlyContinue
  if ($null -ne $command) {
    return $command.Source
  }

  $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
  if (!(Test-Path -LiteralPath $vswhere -PathType Leaf)) {
    throw "Unable to find vswhere.exe while locating dumpbin.exe."
  }

  $visualStudio = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
  if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($visualStudio)) {
    throw "vswhere.exe could not locate a Visual Studio installation with x64 VC tools."
  }

  $toolsRoot = Join-Path $visualStudio "VC\Tools\MSVC"
  $dumpbin = Get-ChildItem -LiteralPath $toolsRoot -Filter dumpbin.exe -Recurse |
    Where-Object { $_.FullName -match '\\bin\\Hostx64\\x64\\dumpbin\.exe$' } |
    Sort-Object FullName -Descending |
    Select-Object -First 1

  if ($null -eq $dumpbin) {
    throw "Unable to find dumpbin.exe under $toolsRoot."
  }

  $dumpbin.FullName
}

if (!(Test-Path -LiteralPath $BinaryPath -PathType Leaf)) {
  throw "Windows 7 PE audit target does not exist: $BinaryPath"
}

$resolvedBinary = (Resolve-Path -LiteralPath $BinaryPath).Path
$resolvedReport = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($ReportPath)
$dumpbinPath = Find-Dumpbin

Write-Host "Auditing PE imports for Windows 7 compatibility:"
Write-Host "  Binary: $resolvedBinary"
Write-Host "  dumpbin: $dumpbinPath"
Write-Host "  Report: $resolvedReport"

$imports = (& $dumpbinPath /nologo /imports $resolvedBinary 2>&1 | Out-String)
if ($LASTEXITCODE -ne 0) {
  throw "dumpbin failed while auditing ${resolvedBinary}:`n$imports"
}

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $resolvedReport) | Out-Null
Set-Content -LiteralPath $resolvedReport -Value $imports -Encoding UTF8

$forbiddenImports = [ordered]@{
  "combase.dll" = "COMBASE is only available starting with Windows 8; use Win7-safe OLE32 imports instead."
  "api-ms-win-core-winrt-" = "WinRT API sets are unavailable on Windows 7."
  "api-ms-win-core-path-l1-1-0.dll" = "The Path API set is unavailable on Windows 7; remove the mandatory import."
  "PathCchStripPrefix" = "PathCchStripPrefix requires Windows 8; normalize extended file paths without this API."
  "CoIncrementMTAUsage" = "CoIncrementMTAUsage is unavailable on Windows 7."
  "EventSetInformation" = "EventSetInformation is unavailable on Windows 7; use a legacy WebView2 static loader."
  "GetSystemTimePreciseAsFileTime" = "GetSystemTimePreciseAsFileTime is unavailable on Windows 7."
  "GetDpiForWindow" = "GetDpiForWindow is unavailable on Windows 7."
  "GetSystemMetricsForDpi" = "GetSystemMetricsForDpi is unavailable on Windows 7."
  "SetThreadDpiAwarenessContext" = "SetThreadDpiAwarenessContext is unavailable on Windows 7."
  "VCRUNTIME140.dll" = "The Win7 package must not require a separately installed VC++ runtime."
  "VCRUNTIME140_1.dll" = "The Win7 package must not require a separately installed VC++ runtime."
  "MSVCP140.dll" = "The Win7 package must not require a separately installed VC++ runtime."
  "ucrtbase.dll" = "The Win7 package must link the Universal CRT statically."
  "api-ms-win-crt-" = "The Win7 package must not require separately installed Universal CRT API sets."
}

$violations = @()
foreach ($entry in $forbiddenImports.GetEnumerator()) {
  if ($imports -match [regex]::Escape($entry.Key)) {
    $violations += "$($entry.Key): $($entry.Value)"
  }
}

if ($violations.Count -gt 0) {
  Write-Host "Full PE import table for diagnosis:"
  Write-Host $imports
  throw "Windows 7 incompatible PE imports detected in ${resolvedBinary}:`n$($violations -join "`n")"
}

Write-Host "Windows 7 PE import audit passed: $resolvedBinary"
