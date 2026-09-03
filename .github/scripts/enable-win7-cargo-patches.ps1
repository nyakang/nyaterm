[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$knownCtorVersion = "0.8.0"
$knownCtorMacrosSha256 = "86ec55f4670e68dbd0fb6f400be0374ba14ac9b621ba041561de7dc629e22fcc"

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$tauriManifest = Join-Path $repositoryRoot "src-tauri\Cargo.toml"
$cargoConfigDirectory = Join-Path $repositoryRoot ".cargo"
$cargoConfigPath = Join-Path $cargoConfigDirectory "config.toml"
$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) "nyaterm-win7-cargo-patches-$([Guid]::NewGuid())"
$patchedCtorRoot = Join-Path $temporaryRoot "ctor-$knownCtorVersion"

Push-Location $repositoryRoot
try {
  $metadataJson = & cargo metadata --manifest-path $tauriManifest --locked --format-version 1
  if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed while locating ctor $knownCtorVersion."
  }

  $metadata = $metadataJson | ConvertFrom-Json
  $ctorPackages = @($metadata.packages | Where-Object {
      $_.name -eq "ctor" -and $_.version -eq $knownCtorVersion
    })
  if ($ctorPackages.Count -ne 1) {
    throw "Expected exactly one ctor $knownCtorVersion package, found $($ctorPackages.Count)."
  }

  $ctorRoot = Split-Path -Parent $ctorPackages[0].manifest_path
  $ctorMacrosPath = Join-Path $ctorRoot "src\macros\mod.rs"
  if (!(Test-Path -LiteralPath $ctorMacrosPath -PathType Leaf)) {
    throw "ctor macros file does not exist: $ctorMacrosPath"
  }

  $actualCtorMacrosSha256 = (Get-FileHash -LiteralPath $ctorMacrosPath -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($actualCtorMacrosSha256 -ne $knownCtorMacrosSha256) {
    throw "Unexpected ctor $knownCtorVersion macros SHA256. Expected $knownCtorMacrosSha256, got $actualCtorMacrosSha256."
  }

  New-Item -ItemType Directory -Path $temporaryRoot -Force | Out-Null
  Copy-Item -LiteralPath $ctorRoot -Destination $patchedCtorRoot -Recurse -Force

  $patchedMacrosPath = Join-Path $patchedCtorRoot "src\macros\mod.rs"
  $patchedMacros = Get-Content -LiteralPath $patchedMacrosPath -Raw
  $unsupportedVendorCondition = 'target_vendor = "pc"'
  $replacementCount = ([regex]::Matches($patchedMacros, [regex]::Escape($unsupportedVendorCondition))).Count
  if ($replacementCount -ne 3) {
    throw "Expected three Windows vendor checks in ctor $knownCtorVersion, found $replacementCount."
  }

  # The built-in Win7 targets use target_vendor="win7" but retain the normal
  # Windows MSVC CRT constructor sections. This is the same compatibility fix
  # released upstream in ctor 1.0.4, kept on 0.8.0 for Tauri's version range.
  $patchedMacros = $patchedMacros.Replace($unsupportedVendorCondition, 'target_os = "windows"')
  Set-Content -LiteralPath $patchedMacrosPath -Value $patchedMacros -Encoding UTF8 -NoNewline

  $cargoCtorPath = $patchedCtorRoot.Replace("\", "/").Replace("'", "''")
  New-Item -ItemType Directory -Path $cargoConfigDirectory -Force | Out-Null
  @"
[patch.crates-io]
ctor = { path = '$cargoCtorPath' }
webview2-com-sys = { path = "src-tauri/vendor/webview2-com-sys" }
windows-core = { path = "src-tauri/vendor/windows-core" }
"@ | Set-Content -LiteralPath $cargoConfigPath -Encoding UTF8

  $patchedMetadataJson = & cargo metadata --manifest-path $tauriManifest --format-version 1
  if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed after enabling the Windows 7 Cargo patches."
  }

  $patchedMetadata = $patchedMetadataJson | ConvertFrom-Json
  $activeCtorPackages = @($patchedMetadata.packages | Where-Object {
      $_.name -eq "ctor" -and $_.version -eq $knownCtorVersion -and
      (Split-Path -Parent $_.manifest_path) -eq $patchedCtorRoot
    })
  if ($activeCtorPackages.Count -ne 1) {
    throw "The patched ctor $knownCtorVersion package was not selected by Cargo."
  }

  Write-Host "Enabled Win7-only Cargo patches:"
  Get-Content -LiteralPath $cargoConfigPath
  Write-Host "Patched ctor source: $patchedCtorRoot"
}
finally {
  Pop-Location
}
