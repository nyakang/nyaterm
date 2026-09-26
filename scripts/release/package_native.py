#!/usr/bin/env python3
"""Build and package the native NyaTerm application for one Rust target."""

from __future__ import annotations

import os
import plistlib
import re
import shutil
import subprocess
import sys
import tarfile
import textwrap
import time
import tomllib
import zipfile
from dataclasses import dataclass
from pathlib import Path


ROOT_DIR = Path(__file__).resolve().parents[2]
DIST_DIR = ROOT_DIR / "dist"
WORK_DIR = ROOT_DIR / "target" / "native-package"
RESOURCE_DIR = ROOT_DIR / "crates" / "nyaterm-app" / "resources"
ICON_DIR = RESOURCE_DIR / "icons"
LICENSE_PATH = ROOT_DIR / "LICENSE"
APP_NAME = "NyaTerm"
APP_BIN = "nyaterm"
# Helper processes the application spawns at runtime. `resolve_helper_path()`
# in nyaterm-remote-desktop only looks beside the running executable, so each
# of these must be packaged next to the application binary in every format.
HELPER_BINS = ("nyaterm-rdp-helper", "nyaterm-vnc-helper", "nyaterm-mcp")
PORTABLE_MARKER = "nyaterm-portable"


@dataclass(frozen=True)
class PackageIdentity:
    display_name: str
    macos_identifier: str
    windows_registry_name: str
    desktop_id: str

    @property
    def macos_bundle_name(self) -> str:
        return f"{self.display_name}.app"

    @property
    def windows_registry_key(self) -> str:
        return rf"Software\{self.windows_registry_name}"

    @property
    def windows_uninstall_key(self) -> str:
        return rf"Software\Microsoft\Windows\CurrentVersion\Uninstall\{self.display_name}"

    @property
    def linux_desktop_file(self) -> str:
        return f"{self.desktop_id}.desktop"


STABLE_IDENTITY = PackageIdentity("NyaTerm", "com.kang.nyaterm", "NyaTerm", "nyaterm")
PREVIEW_IDENTITY = PackageIdentity(
    "NyaTerm Preview", "com.kang.nyaterm.preview", "NyaTermPreview", "nyaterm-preview"
)

# SemVer 2.0: prerelease numeric identifiers cannot have leading zeroes.
SEMVER = re.compile(
    r"(?P<major>0|[1-9][0-9]*)\.(?P<minor>0|[1-9][0-9]*)\.(?P<patch>0|[1-9][0-9]*)"
    r"(?:-(?P<pre>(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*))?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
)


def parse_version(raw: str) -> re.Match[str]:
    match = SEMVER.fullmatch(normalize_version(raw))
    if match is None:
        raise ValueError(f"invalid release version: {raw}")
    return match


def release_identity(version: str) -> PackageIdentity:
    return PREVIEW_IDENTITY if parse_version(version).group("pre") else STABLE_IDENTITY


@dataclass(frozen=True)
class TargetInfo:
    target: str
    os_name: str
    arch: str

    @property
    def label(self) -> str:
        return f"{self.os_name}_{self.arch}"


TARGETS = {
    "aarch64-apple-darwin": TargetInfo("aarch64-apple-darwin", "macos", "arm64"),
    "x86_64-apple-darwin": TargetInfo("x86_64-apple-darwin", "macos", "x64"),
    "aarch64-unknown-linux-gnu": TargetInfo(
        "aarch64-unknown-linux-gnu", "linux", "arm64"
    ),
    "x86_64-unknown-linux-gnu": TargetInfo(
        "x86_64-unknown-linux-gnu", "linux", "x64"
    ),
    "aarch64-pc-windows-msvc": TargetInfo(
        "aarch64-pc-windows-msvc", "windows", "arm64"
    ),
    "x86_64-pc-windows-msvc": TargetInfo(
        "x86_64-pc-windows-msvc", "windows", "x64"
    ),
}


def run(
    args: list[str],
    *,
    cwd: Path = ROOT_DIR,
    env: dict[str, str] | None = None,
) -> None:
    print("+", " ".join(args), flush=True)
    subprocess.run(args, cwd=cwd, env=env, check=True)


def run_with_retry(args: list[str], *, attempts: int = 3, delay_seconds: float = 2.0) -> None:
    if attempts < 1:
        raise ValueError("attempts must be at least 1")
    for attempt in range(1, attempts + 1):
        try:
            run(args)
            return
        except subprocess.CalledProcessError:
            if attempt == attempts:
                raise
            time.sleep(delay_seconds)


def require_tool(name: str) -> str:
    path = shutil.which(name)
    if not path:
        raise RuntimeError(f"required packaging tool not found: {name}")
    return path


def workspace_version() -> str:
    with (ROOT_DIR / "Cargo.toml").open("rb") as handle:
        return tomllib.load(handle)["workspace"]["package"]["version"]


def normalize_version(raw: str) -> str:
    return raw.strip().removeprefix("v").removeprefix("V")


def validate_version(raw: str, expected: str | None = None) -> str:
    version = normalize_version(raw)
    parse_version(version)
    if expected is not None and version != expected:
        raise ValueError(
            f"release version {version} does not match Cargo workspace version {expected}"
        )
    return version


def validate_artifact_version(raw: str) -> str:
    """Validate a public filename label without changing package metadata."""
    value = raw.strip()
    if value == "main-snapshot":
        return value
    return validate_version(value)


def target_info(target: str) -> TargetInfo:
    try:
        return TARGETS[target]
    except KeyError as error:
        raise ValueError(f"unsupported release target: {target}") from error


def artifact_names(target: str, version: str) -> set[str]:
    info = target_info(target)
    prefix = f"{APP_NAME}_{validate_artifact_version(version)}_{info.label}"
    if info.os_name == "macos":
        return {f"{prefix}.dmg", f"{prefix}.app.tar.gz"}
    if info.os_name == "linux":
        return {f"{prefix}.AppImage", f"{prefix}.deb", f"{prefix}.rpm"}
    return {f"{prefix}_portable.zip", f"{prefix}-setup.exe"}


def cargo_target_dir() -> Path:
    configured = os.environ.get("CARGO_TARGET_DIR")
    if not configured:
        return ROOT_DIR / "target"
    path = Path(configured)
    return path if path.is_absolute() else ROOT_DIR / path


def release_binary_path(target: str, name: str = APP_BIN) -> Path:
    suffix = ".exe" if "windows" in target else ""
    return cargo_target_dir() / target / "release" / f"{name}{suffix}"


def helper_binary_paths(target: str) -> list[Path]:
    return [release_binary_path(target, name) for name in HELPER_BINS]


def build_binary(package: str, name: str, target: str) -> Path:
    run(
        [
            "cargo",
            "build",
            "-p",
            package,
            "--bin",
            name,
            "--release",
            "--target",
            target,
            "--locked",
        ]
    )
    binary = release_binary_path(target, name)
    if not binary.is_file():
        raise FileNotFoundError(f"release executable not found: {binary}")
    return binary


def build_application(target: str) -> Path:
    binary = build_binary("nyaterm-app", APP_BIN, target)
    for name in HELPER_BINS:
        build_binary(name, name, target)
    return binary


def make_executable(path: Path) -> None:
    path.chmod(path.stat().st_mode | 0o111)


def copy_helpers(destination: Path, target: str) -> list[Path]:
    """Copy helper binaries beside the application and return the copies."""
    copied = []
    for source in helper_binary_paths(target):
        binary = destination / source.name
        shutil.copy2(source, binary)
        if "windows" not in target:
            make_executable(binary)
        copied.append(binary)
    return copied


def copy_release_documents(destination: Path, version: str) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    shutil.copy2(LICENSE_PATH, destination / "LICENSE")
    (destination / "VERSION").write_text(f"{version}\n", encoding="utf-8")


def reset_output() -> None:
    if DIST_DIR.exists():
        shutil.rmtree(DIST_DIR)
    DIST_DIR.mkdir()
    if WORK_DIR.exists():
        shutil.rmtree(WORK_DIR)
    WORK_DIR.mkdir(parents=True)


def archive_zip(source: Path, destination: Path) -> None:
    with zipfile.ZipFile(
        destination, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9
    ) as archive:
        for path in sorted(source.rglob("*")):
            archive.write(path, path.relative_to(source.parent))


def windows_numeric_version(version: str) -> str:
    parsed = parse_version(version)
    numeric = [int(parsed.group(key)) for key in ("major", "minor", "patch")] + [0]
    if any(part > 65535 for part in numeric):
        raise ValueError(f"Windows version component exceeds 65535: {version}")
    return ".".join(str(part) for part in numeric)


def nsis_path(path: Path) -> str:
    return str(path.resolve()).replace("/", "\\")


def find_makensis() -> str:
    found = shutil.which("makensis") or shutil.which("makensis.exe")
    if found:
        return found
    raise RuntimeError("makensis not found; install NSIS before packaging Windows")


def create_windows_packages(
    binary: Path, info: TargetInfo, version: str, artifact_version: str
) -> None:
    identity = release_identity(version)
    portable_root = WORK_DIR / "NyaTerm-portable"
    portable_root.mkdir()
    shutil.copy2(binary, portable_root / "NyaTerm.exe")
    copy_helpers(portable_root, info.target)
    (portable_root / PORTABLE_MARKER).touch()
    (portable_root / "data").mkdir()
    (portable_root / "data" / ".keep").touch()
    copy_release_documents(portable_root, version)
    portable_output = (
        DIST_DIR / f"{APP_NAME}_{artifact_version}_{info.label}_portable.zip"
    )
    archive_zip(portable_root, portable_output)

    installer_root = WORK_DIR / "windows-installer"
    installer_root.mkdir()
    shutil.copy2(binary, installer_root / "NyaTerm.exe")
    installer_helpers = copy_helpers(installer_root, info.target)
    copy_release_documents(installer_root, version)
    shutil.copy2(ICON_DIR / "icon.ico", installer_root / "icon.ico")

    nsis_indent = "\n" + " " * 14
    helper_install = nsis_indent.join(
        f'File "{nsis_path(path)}"' for path in installer_helpers
    )
    helper_uninstall = nsis_indent.join(
        f'Delete "$INSTDIR\\{path.name}"' for path in installer_helpers
    )

    output = DIST_DIR / f"{APP_NAME}_{artifact_version}_{info.label}-setup.exe"
    script = WORK_DIR / "nyaterm-installer.nsi"
    script.write_text(
        textwrap.dedent(
            rf"""
            Unicode true
            RequestExecutionLevel user
            SetCompressor /SOLID lzma

            !include "MUI2.nsh"
            !define MUI_ICON "{nsis_path(ICON_DIR / 'icon.ico')}"
            !define MUI_UNICON "{nsis_path(ICON_DIR / 'icon.ico')}"

            Name "{identity.display_name}"
            OutFile "{nsis_path(output)}"
            InstallDir "$LOCALAPPDATA\Programs\{identity.display_name}"
            InstallDirRegKey HKCU "{identity.windows_registry_key}" "InstallDir"
            VIProductVersion "{windows_numeric_version(version)}"
            VIFileVersion "{windows_numeric_version(version)}"
            VIAddVersionKey "ProductName" "{identity.display_name}"
            VIAddVersionKey "ProductVersion" "{version}"
            VIAddVersionKey "FileVersion" "{version}"
            VIAddVersionKey "FileDescription" "{identity.display_name} native GPUI terminal"
            VIAddVersionKey "LegalCopyright" "Copyright Kang"

            !insertmacro MUI_PAGE_WELCOME
            !insertmacro MUI_PAGE_DIRECTORY
            !insertmacro MUI_PAGE_INSTFILES
            !define MUI_FINISHPAGE_RUN "$INSTDIR\NyaTerm.exe"
            !define MUI_FINISHPAGE_RUN_TEXT "Launch {identity.display_name}"
            !insertmacro MUI_PAGE_FINISH
            !insertmacro MUI_UNPAGE_CONFIRM
            !insertmacro MUI_UNPAGE_INSTFILES
            !insertmacro MUI_LANGUAGE "English"

            Section "{identity.display_name}" SecMain
              SetOutPath "$INSTDIR"
              File "{nsis_path(installer_root / 'NyaTerm.exe')}"
              {helper_install}
              File "{nsis_path(installer_root / 'LICENSE')}"
              File "{nsis_path(installer_root / 'VERSION')}"
              File "{nsis_path(installer_root / 'icon.ico')}"
              WriteUninstaller "$INSTDIR\Uninstall.exe"
              WriteRegStr HKCU "{identity.windows_registry_key}" "InstallDir" "$INSTDIR"
              WriteRegStr HKCU "Software\Classes\{identity.desktop_id}" "" "URL:{identity.display_name} Protocol"
              WriteRegStr HKCU "Software\Classes\{identity.desktop_id}" "URL Protocol" ""
              WriteRegStr HKCU "Software\Classes\{identity.desktop_id}\DefaultIcon" "" "$INSTDIR\NyaTerm.exe,0"
              WriteRegStr HKCU "Software\Classes\{identity.desktop_id}\shell\open\command" "" "$\"$INSTDIR\NyaTerm.exe$\" $\"%1$\""
              WriteRegStr HKCU "{identity.windows_uninstall_key}" "DisplayName" "{identity.display_name}"
              WriteRegStr HKCU "{identity.windows_uninstall_key}" "DisplayVersion" "{version}"
              WriteRegStr HKCU "{identity.windows_uninstall_key}" "DisplayIcon" "$INSTDIR\NyaTerm.exe"
              WriteRegStr HKCU "{identity.windows_uninstall_key}" "UninstallString" "$\"$INSTDIR\Uninstall.exe$\""
              CreateDirectory "$SMPROGRAMS\{identity.display_name}"
              CreateShortcut "$SMPROGRAMS\{identity.display_name}\{identity.display_name}.lnk" "$INSTDIR\NyaTerm.exe" "" "$INSTDIR\icon.ico"
              CreateShortcut "$DESKTOP\{identity.display_name}.lnk" "$INSTDIR\NyaTerm.exe" "" "$INSTDIR\icon.ico"
            SectionEnd

            Section "Uninstall"
              Delete "$DESKTOP\{identity.display_name}.lnk"
              Delete "$SMPROGRAMS\{identity.display_name}\{identity.display_name}.lnk"
              RMDir "$SMPROGRAMS\{identity.display_name}"
              Delete "$INSTDIR\NyaTerm.exe"
              {helper_uninstall}
              Delete "$INSTDIR\LICENSE"
              Delete "$INSTDIR\VERSION"
              Delete "$INSTDIR\icon.ico"
              Delete "$INSTDIR\Uninstall.exe"
              RMDir "$INSTDIR"
              DeleteRegKey HKCU "Software\Classes\{identity.desktop_id}"
              DeleteRegKey HKCU "{identity.windows_uninstall_key}"
              DeleteRegKey HKCU "{identity.windows_registry_key}"
            SectionEnd
            """
        ).lstrip(),
        encoding="utf-8",
    )
    run([find_makensis(), str(script)])


def create_macos_packages(
    binary: Path, info: TargetInfo, version: str, artifact_version: str
) -> None:
    identity = release_identity(version)
    bundle = WORK_DIR / identity.macos_bundle_name
    macos_dir = bundle / "Contents" / "MacOS"
    resources_dir = bundle / "Contents" / "Resources"
    macos_dir.mkdir(parents=True)
    resources_dir.mkdir(parents=True)
    app_binary = macos_dir / "NyaTerm"
    shutil.copy2(binary, app_binary)
    make_executable(app_binary)
    helper_binaries = copy_helpers(macos_dir, info.target)
    shutil.copy2(ICON_DIR / "icon.icns", resources_dir / "icon.icns")
    copy_release_documents(resources_dir, version)

    plist = {
        "CFBundleDevelopmentRegion": "en",
        "CFBundleDisplayName": identity.display_name,
        "CFBundleExecutable": "NyaTerm",
        "CFBundleIconFile": "icon.icns",
        "CFBundleIdentifier": identity.macos_identifier,
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": identity.display_name,
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleURLTypes": [
            {
                "CFBundleTypeRole": "Viewer",
                "CFBundleURLName": identity.macos_identifier,
                "CFBundleURLSchemes": [identity.desktop_id],
            }
        ],
        "CFBundleVersion": version,
        "LSMinimumSystemVersion": "12.0",
        "NSHighResolutionCapable": True,
        "NSHumanReadableCopyright": "Copyright Kang",
    }
    with (bundle / "Contents" / "Info.plist").open("wb") as handle:
        plistlib.dump(plist, handle, sort_keys=True)

    codesign = require_tool("codesign")
    # Sign inside out. A bundle-only signature seals extra Mach-O files in
    # Contents/MacOS as resources instead of signing them as code, which makes
    # the helper fail to launch under Gatekeeper.
    for helper in helper_binaries:
        run([codesign, "--force", "--sign", "-", "--timestamp=none", str(helper)])
    run([codesign, "--force", "--deep", "--sign", "-", "--timestamp=none", str(bundle)])

    tar_output = DIST_DIR / f"{APP_NAME}_{artifact_version}_{info.label}.app.tar.gz"
    with tarfile.open(tar_output, "w:gz", compresslevel=9) as archive:
        archive.add(bundle, arcname=identity.macos_bundle_name)

    dmg_root = WORK_DIR / "dmg"
    dmg_root.mkdir()
    shutil.copytree(bundle, dmg_root / identity.macos_bundle_name, symlinks=True)
    (dmg_root / "Applications").symlink_to("/Applications")
    dmg_output = DIST_DIR / f"{APP_NAME}_{artifact_version}_{info.label}.dmg"
    run_with_retry(
        [
            require_tool("hdiutil"),
            "create",
            "-volname",
            identity.display_name,
            "-srcfolder",
            str(dmg_root),
            "-ov",
            "-format",
            "UDZO",
            str(dmg_output),
        ]
    )


def linux_deb_arch(target: str) -> str:
    return {
        "x86_64-unknown-linux-gnu": "amd64",
        "aarch64-unknown-linux-gnu": "arm64",
    }[target]


def linux_rpm_arch(target: str) -> str:
    return {
        "x86_64-unknown-linux-gnu": "x86_64",
        "aarch64-unknown-linux-gnu": "aarch64",
    }[target]


def linux_appimage_arch(target: str) -> str:
    return {
        "x86_64-unknown-linux-gnu": "x86_64",
        "aarch64-unknown-linux-gnu": "aarch64",
    }[target]


def linux_rpm_version(version: str) -> tuple[str, str]:
    parsed = parse_version(version)
    upstream = ".".join(parsed.group(key) for key in ("major", "minor", "patch"))
    prerelease = parsed.group("pre")
    if prerelease is None:
        return upstream, "1"
    normalized = re.sub(r"[^0-9A-Za-z]+", ".", prerelease).strip(".")
    return upstream, f"0.{normalized}"


def write_desktop_file(
    path: Path, executable: str, identity: PackageIdentity = STABLE_IDENTITY
) -> None:
    path.write_text(
        textwrap.dedent(
            f"""
            [Desktop Entry]
            Type=Application
            Name={identity.display_name}
            Comment=Native GPUI terminal and SSH client
            Exec={executable} %U
            Icon={identity.desktop_id}
            StartupWMClass={identity.desktop_id}
            Terminal=false
            Categories=Development;TerminalEmulator;Network;
            MimeType=x-scheme-handler/{identity.desktop_id};
            StartupNotify=true
            """
        ).lstrip(),
        encoding="utf-8",
    )


def copy_linux_icons(root: Path, identity: PackageIdentity = STABLE_IDENTITY) -> None:
    icons = {
        "32x32": "32x32.png",
        "64x64": "64x64.png",
        "128x128": "128x128.png",
        "256x256": "256x256.png",
    }
    for size, source in icons.items():
        destination = root / "usr" / "share" / "icons" / "hicolor" / size / "apps"
        destination.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ICON_DIR / source, destination / f"{identity.desktop_id}.png")


def parse_dpkg_dependencies(output: str) -> str:
    prefix = "shlibs:Depends="
    for line in output.splitlines():
        if line.startswith(prefix) and line[len(prefix) :].strip():
            return line[len(prefix) :].strip()
    raise RuntimeError("dpkg-shlibdeps did not report shlibs:Depends")


def linux_deb_dependencies(binaries: list[Path]) -> str:
    scratch = WORK_DIR / "shlibdeps"
    debian = scratch / "debian"
    debian.mkdir(parents=True)
    (debian / "control").write_text(
        "Source: nyaterm\nPackage: nyaterm\nArchitecture: any\nDescription: NyaTerm\n",
        encoding="utf-8",
    )
    # Helpers can pull shared libraries the application itself does not need.
    command = [require_tool("dpkg-shlibdeps"), "-O"]
    for binary in binaries:
        command += ["-e", str(binary)]
    output = subprocess.check_output(command, cwd=scratch, text=True)
    return parse_dpkg_dependencies(output)


def create_linux_appimage(
    binary: Path, info: TargetInfo, version: str, artifact_version: str
) -> None:
    identity = release_identity(version)
    appdir = WORK_DIR / "NyaTerm.AppDir"
    usr_bin = appdir / "usr" / "bin"
    usr_bin.mkdir(parents=True)
    app_binary = usr_bin / APP_BIN
    shutil.copy2(binary, app_binary)
    make_executable(app_binary)
    copy_helpers(usr_bin, info.target)
    copy_release_documents(appdir / "usr" / "share" / "doc" / identity.desktop_id, version)

    applications = appdir / "usr" / "share" / "applications"
    applications.mkdir(parents=True)
    write_desktop_file(applications / identity.linux_desktop_file, APP_BIN, identity)
    shutil.copy2(applications / identity.linux_desktop_file, appdir / identity.linux_desktop_file)
    shutil.copy2(ICON_DIR / "128x128.png", appdir / f"{identity.desktop_id}.png")
    copy_linux_icons(appdir, identity)

    apprun = appdir / "AppRun"
    apprun.write_text(
        '#!/bin/sh\nAPPDIR="${APPDIR:-$(dirname "$(readlink -f "$0")")}"\n'
        'exec "$APPDIR/usr/bin/nyaterm" "$@"\n',
        encoding="utf-8",
    )
    make_executable(apprun)

    output = DIST_DIR / f"{APP_NAME}_{artifact_version}_{info.label}.AppImage"
    env = os.environ.copy()
    env["ARCH"] = linux_appimage_arch(info.target)
    env.setdefault("APPIMAGE_EXTRACT_AND_RUN", "1")
    run([require_tool("appimagetool"), str(appdir), str(output)], env=env)


def create_linux_deb(
    binary: Path, info: TargetInfo, version: str, artifact_version: str
) -> None:
    identity = release_identity(version)
    root = WORK_DIR / "deb"
    app_root = root / "opt" / identity.desktop_id
    app_root.mkdir(parents=True)
    app_binary = app_root / APP_BIN
    shutil.copy2(binary, app_binary)
    make_executable(app_binary)
    helper_binaries = copy_helpers(app_root, info.target)
    copy_release_documents(app_root, version)
    copy_release_documents(root / "usr" / "share" / "doc" / identity.desktop_id, version)

    applications = root / "usr" / "share" / "applications"
    applications.mkdir(parents=True)
    write_desktop_file(applications / identity.linux_desktop_file, f"/opt/{identity.desktop_id}/nyaterm", identity)
    copy_linux_icons(root, identity)

    control_dir = root / "DEBIAN"
    control_dir.mkdir()
    dependencies = linux_deb_dependencies([app_binary, *helper_binaries])
    (control_dir / "control").write_text(
        textwrap.dedent(
            f"""
            Package: {identity.desktop_id}
            Version: {version.replace('-', '~')}
            Section: utils
            Priority: optional
            Architecture: {linux_deb_arch(info.target)}
            Maintainer: Kang <noreply@nyaterm.app>
            Depends: {dependencies}
            Recommends: libvulkan1 | mesa-vulkan-drivers
            Description: NyaTerm native GPUI terminal and SSH client
             Native terminal workspace with SSH, SFTP and remote operations.
            """
        ).lstrip(),
        encoding="utf-8",
    )
    output = DIST_DIR / f"{APP_NAME}_{artifact_version}_{info.label}.deb"
    run([require_tool("dpkg-deb"), "--build", "--root-owner-group", str(root), str(output)])


def create_linux_rpm(
    binary: Path, info: TargetInfo, version: str, artifact_version: str
) -> None:
    identity = release_identity(version)
    rpm_root = WORK_DIR / "rpm"
    top_dir = rpm_root / "rpmbuild"
    payload = rpm_root / "payload"
    for directory in ("BUILD", "BUILDROOT", "RPMS", "SOURCES", "SPECS", "SRPMS"):
        (top_dir / directory).mkdir(parents=True)

    app_root = payload / "opt" / identity.desktop_id
    app_root.mkdir(parents=True)
    app_binary = app_root / APP_BIN
    shutil.copy2(binary, app_binary)
    make_executable(app_binary)
    copy_helpers(app_root, info.target)
    copy_release_documents(app_root, version)
    copy_release_documents(payload / "usr" / "share" / "doc" / identity.desktop_id, version)
    applications = payload / "usr" / "share" / "applications"
    applications.mkdir(parents=True)
    write_desktop_file(applications / identity.linux_desktop_file, f"/opt/{identity.desktop_id}/nyaterm", identity)
    copy_linux_icons(payload, identity)

    rpm_version, rpm_release = linux_rpm_version(version)
    payload_path = str(payload.resolve()).replace("%", "%%")
    spec = textwrap.dedent(
        f"""
        Name: {identity.desktop_id}
        Version: {rpm_version}
        Release: {rpm_release}
        Summary: NyaTerm native GPUI terminal and SSH client
        License: Apache-2.0
        URL: https://nyaterm.app
        BuildArch: {linux_rpm_arch(info.target)}

        %description
        Native terminal workspace with SSH, SFTP and remote operations.

        %install
        rm -rf %{{buildroot}}
        mkdir -p %{{buildroot}}
        cp -a "{payload_path}/." %{{buildroot}}/

        %files
        /opt/{identity.desktop_id}
        /usr/share/applications/{identity.linux_desktop_file}
        /usr/share/icons/hicolor/*/apps/{identity.desktop_id}.png
        /usr/share/doc/{identity.desktop_id}
        """
    ).lstrip()
    spec_path = top_dir / "SPECS" / "nyaterm.spec"
    spec_path.write_text(spec, encoding="utf-8")
    run(
        [
            require_tool("rpmbuild"),
            "--define",
            f"_topdir {top_dir.resolve()}",
            "--define",
            "_build_id_links none",
            "--define",
            "debug_package %{nil}",
            "-bb",
            str(spec_path),
        ]
    )
    built = list((top_dir / "RPMS" / linux_rpm_arch(info.target)).glob("*.rpm"))
    if len(built) != 1:
        raise RuntimeError(f"expected one RPM artifact, found {len(built)}")
    shutil.copy2(
        built[0], DIST_DIR / f"{APP_NAME}_{artifact_version}_{info.label}.rpm"
    )


def create_linux_packages(
    binary: Path, info: TargetInfo, version: str, artifact_version: str
) -> None:
    create_linux_appimage(binary, info, version, artifact_version)
    create_linux_deb(binary, info, version, artifact_version)
    create_linux_rpm(binary, info, version, artifact_version)


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: package_native.py <rust-target>")
    info = target_info(sys.argv[1])
    expected_version = workspace_version()
    version = validate_version(
        os.environ.get("NYATERM_VERSION", expected_version), expected_version
    )
    artifact_version = validate_artifact_version(
        os.environ.get("NYATERM_ARTIFACT_VERSION", version)
    )
    reset_output()
    print(f"==> Packaging {APP_NAME} {version} for {info.target}", flush=True)
    binary = build_application(info.target)

    if info.os_name == "macos":
        create_macos_packages(binary, info, version, artifact_version)
    elif info.os_name == "linux":
        create_linux_packages(binary, info, version, artifact_version)
    else:
        create_windows_packages(binary, info, version, artifact_version)

    actual = {path.name for path in DIST_DIR.iterdir() if path.is_file()}
    expected = artifact_names(info.target, artifact_version)
    if actual != expected:
        raise RuntimeError(
            f"package output mismatch: expected {sorted(expected)}, got {sorted(actual)}"
        )
    for path in sorted(DIST_DIR.iterdir()):
        print(f"==> {path.name} ({path.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
