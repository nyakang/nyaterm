[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$knownCtorVersion = "0.8.0"
$knownCtorMacrosSha256 = "86ec55f4670e68dbd0fb6f400be0374ba14ac9b621ba041561de7dc629e22fcc"
$knownArboardVersion = "3.6.1"
$knownArboardWindowsSha256 = "bbc0a5a0fdfa3a79c3745b73793eb97bcbe28bd889185ae02971c19e2ce23423"

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$tauriManifest = Join-Path $repositoryRoot "src-tauri\Cargo.toml"
$cargoConfigDirectory = Join-Path $repositoryRoot ".cargo"
$cargoConfigPath = Join-Path $cargoConfigDirectory "config.toml"
$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) "nyaterm-win7-cargo-patches-$([Guid]::NewGuid())"
$patchedCtorRoot = Join-Path $temporaryRoot "ctor-$knownCtorVersion"
$patchedArboardRoot = Join-Path $temporaryRoot "arboard-$knownArboardVersion"

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

  $arboardPackages = @($metadata.packages | Where-Object {
      $_.name -eq "arboard" -and $_.version -eq $knownArboardVersion
    })
  if ($arboardPackages.Count -ne 1) {
    throw "Expected exactly one arboard $knownArboardVersion package, found $($arboardPackages.Count)."
  }

  $arboardRoot = Split-Path -Parent $arboardPackages[0].manifest_path
  $arboardWindowsPath = Join-Path $arboardRoot "src\platform\windows.rs"
  if (!(Test-Path -LiteralPath $arboardWindowsPath -PathType Leaf)) {
    throw "arboard Windows platform source does not exist: $arboardWindowsPath"
  }
  $actualArboardWindowsSha256 = (Get-FileHash -LiteralPath $arboardWindowsPath -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($actualArboardWindowsSha256 -ne $knownArboardWindowsSha256) {
    throw "Unexpected arboard $knownArboardVersion Windows SHA256. Expected $knownArboardWindowsSha256, got $actualArboardWindowsSha256."
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

  # arboard's CF_HDROP implementation calls PathCchStripPrefix, a mandatory
  # Windows 8+ import. Keep GetFinalPathNameByHandleW (Vista+) and normalize
  # only its extended DOS/UNC paths in a Win7-only copy of arboard 3.6.1.
  Copy-Item -LiteralPath $arboardRoot -Destination $patchedArboardRoot -Recurse -Force
  $patchedArboardWindowsPath = Join-Path $patchedArboardRoot "src\platform\windows.rs"
  $arboardWindows = (Get-Content -LiteralPath $patchedArboardWindowsPath -Raw).Replace("`r`n", "`n")

  $arboardReplacements = [ordered]@{
    'Foundation::{GetLastError, GlobalFree, HANDLE, HGLOBAL, POINT, S_OK}' = 'Foundation::{GetLastError, GlobalFree, HANDLE, HGLOBAL, POINT}'
    'UI::Shell::{PathCchStripPrefix, DROPFILES}' = 'UI::Shell::DROPFILES'
  }
  foreach ($replacement in $arboardReplacements.GetEnumerator()) {
    $count = ([regex]::Matches($arboardWindows, [regex]::Escape($replacement.Key))).Count
    if ($count -ne 1) {
      throw "Expected one arboard import '$($replacement.Key)', found $count."
    }
    $arboardWindows = $arboardWindows.Replace($replacement.Key, $replacement.Value)
  }

  $oldPrefixConversion = @'
		|buf| {
			let mut wide = Vec::with_capacity(buf.len() + 1);
			wide.extend_from_slice(buf);
			wide.push(0);

			let hr = unsafe { PathCchStripPrefix(wide.as_mut_ptr(), wide.len()) };
			// On success truncate invalid data
			if hr == S_OK {
				if let Some(end) = wide.iter().position(|c| *c == 0) {
					// Retain NULL character
					wide.truncate(end + 1)
				}
			}
			wide
		},
'@
  $oldPrefixConversion = $oldPrefixConversion.Replace("`r`n", "`n")
  $newPrefixConversion = @'
		|buf| win7_normalize_final_path_wide(buf),
'@
  $count = ([regex]::Matches($arboardWindows, [regex]::Escape($oldPrefixConversion))).Count
  if ($count -ne 1) {
    throw "Expected one PathCchStripPrefix call block in arboard $knownArboardVersion, found $count."
  }
  $arboardWindows = $arboardWindows.Replace($oldPrefixConversion, $newPrefixConversion)

  $prefixHelper = @'
// Windows 7 has no PathCchStripPrefix. GetFinalPathNameByHandleW returns
// extended UTF-16 paths, while CF_HDROP consumers expect DOS/UNC paths.
fn win7_normalize_final_path_wide(path: &[u16]) -> Vec<u16> {
	let length = path.iter().position(|&ch| ch == 0).unwrap_or(path.len());
	let path = &path[..length];
	let slash = b'\\' as u16;
	let extended = path.starts_with(&[slash, slash, b'?' as u16, slash]);
	let mut wide = Vec::with_capacity(path.len() + 1);
	if extended
		&& path.len() >= 8
		&& (path[4] | 0x20) == b'u' as u16
		&& (path[5] | 0x20) == b'n' as u16
		&& (path[6] | 0x20) == b'c' as u16
		&& path[7] == slash
	{
		wide.extend_from_slice(&[slash, slash]);
		wide.extend_from_slice(&path[8..]);
	} else if extended
		&& path.len() >= 7
		&& ((b'A' as u16..=b'Z' as u16).contains(&path[4])
			|| (b'a' as u16..=b'z' as u16).contains(&path[4]))
		&& path[5] == b':' as u16
		&& path[6] == slash
	{
		wide.extend_from_slice(&path[4..]);
	} else {
		wide.extend_from_slice(path);
	}
	wide.push(0);
	wide
}

'@
  $prefixHelper = $prefixHelper.Replace("`r`n", "`n")
  $helperAnchor = "/// Given a file path attempt to open it and call GetFinalPathNameByHandleW,"
  $count = ([regex]::Matches($arboardWindows, [regex]::Escape($helperAnchor))).Count
  if ($count -ne 1) {
    throw "Expected one arboard path helper anchor, found $count."
  }
  $arboardWindows = $arboardWindows.Replace($helperAnchor, "$prefixHelper`n$helperAnchor")
  if ($arboardWindows -match '\bPathCchStripPrefix\s*\(') {
    throw "arboard still references the Windows 8 PathCchStripPrefix function."
  }
  Set-Content -LiteralPath $patchedArboardWindowsPath -Value $arboardWindows -Encoding UTF8 -NoNewline

  $cargoCtorPath = $patchedCtorRoot.Replace("\", "/").Replace("'", "''")
  $cargoArboardPath = $patchedArboardRoot.Replace("\", "/").Replace("'", "''")
  New-Item -ItemType Directory -Path $cargoConfigDirectory -Force | Out-Null
  @"
[patch.crates-io]
ctor = { path = '$cargoCtorPath' }
arboard = { path = '$cargoArboardPath' }
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

  $activeArboardPackages = @($patchedMetadata.packages | Where-Object {
      $_.name -eq "arboard" -and $_.version -eq $knownArboardVersion -and
      (Split-Path -Parent $_.manifest_path) -eq $patchedArboardRoot
    })
  if ($activeArboardPackages.Count -ne 1) {
    throw "The patched arboard $knownArboardVersion package was not selected by Cargo."
  }

  Write-Host "Enabled Win7-only Cargo patches:"
  Get-Content -LiteralPath $cargoConfigPath
  Write-Host "Patched ctor source: $patchedCtorRoot"
  Write-Host "Patched arboard source: $patchedArboardRoot"
}
finally {
  Pop-Location
}
