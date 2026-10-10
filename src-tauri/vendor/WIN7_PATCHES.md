# Windows 7 Compatibility Vendor Patches

This file records dependency patches that are only present because the isolated
`x86_64-win7-windows-msvc` compatibility build found a concrete blocker.

## windows-core 0.62.2

- Source: crates.io `windows-core` 0.62.2.
- Reason: the Win7 PE import audit found `combase.dll` imports from the
  `windows 0.62.2` dependency group used by Pageant, SSPI, and IronRDP.
- Patch: link `CoTaskMemAlloc`, `CoTaskMemFree`, and
  `CoCreateFreeThreadedMarshaler` from `ole32.dll`, matching the Win7-safe
  import library used by `windows-core` 0.61.x.
- Scope: no normal release workflow is changed; this patch is exercised through
  Cargo's `[patch.crates-io]` resolution and must remain validated by the
  `.github/workflows/windows-7-compat.yml` PE audit.

## webview2-com-sys 0.38.2 / WebView2 Loader SDK 1.0.1020.30

- Source: crates.io `webview2-com-sys` 0.38.2 plus NuGet
  `Microsoft.Web.WebView2` 1.0.1020.30 for the x64 static loader.
- Reason: the Win7 PE import audit found `EventSetInformation` in newer
  WebView2 static loaders. The original 1.0.1054.31 loader pin still contains
  that import; 1.0.1020.30 is the newest stable SDK version checked below that
  boundary without the symbol.
- Patch: the crate is vendored so raw Win7 probe builds link the known-good x64
  loader from source. `.github/scripts/prepare-webview2-win7-loader.ps1` also
  verifies/reinstalls that loader and replaces already-copied Cargo
  `target/**/out/x64` build outputs with the same pinned binary.

## arboard 3.6.1

- Source: crates.io `arboard` 3.6.1 (MIT OR Apache-2.0).
- Reason: its Windows `CF_HDROP` file-list writer calls
  `PathCchStripPrefix` from `api-ms-win-core-path-l1-1-0.dll`, which is
  unavailable on Windows 7 and causes a load-time failure.
- Patch: `.github/scripts/enable-win7-cargo-patches.ps1` verifies the exact
  registry source version and `src/platform/windows.rs` SHA-256, then patches
  a temporary crate copy selected by Win7-only `[patch.crates-io]`.
  `GetFinalPathNameByHandleW` (supported since Vista) still retrieves the
  final UTF-16 path. The temporary patch strips the extended `\\?\` prefix
  from DOS drive paths and converts `\\?\UNC\` to `\\` for file-list
  clipboard consumers, preserving Unicode and a terminating NUL.
- Scope: regular Windows 10/11 and other platform builds continue to use the
  unmodified registry package. The Win7 PE audit rejects both the unsupported
  API-set DLL and `PathCchStripPrefix` before NSIS packaging.
