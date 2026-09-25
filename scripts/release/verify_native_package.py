#!/usr/bin/env python3
"""Verify native NyaTerm release artifacts before they are published."""

from __future__ import annotations

import argparse
import json
import os
import plistlib
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path, PurePosixPath

import package_native


MIN_ARTIFACT_SIZE = 1024


def helper_filenames(target: str) -> list[str]:
    suffix = ".exe" if "windows" in target else ""
    return [f"{name}{suffix}" for name in package_native.HELPER_BINS]


def require_safe_archive_path(name: str) -> None:
    path = PurePosixPath(name.replace("\\", "/"))
    if path.is_absolute() or ".." in path.parts:
        raise RuntimeError(f"archive contains an unsafe path: {name}")


def verify_zip_paths(archive: zipfile.ZipFile) -> set[str]:
    names = set()
    for item in archive.infolist():
        require_safe_archive_path(item.filename)
        names.add(item.filename.rstrip("/"))
    return names


def verify_tar_paths(archive: tarfile.TarFile) -> set[str]:
    names = set()
    for item in archive.getmembers():
        require_safe_archive_path(item.name)
        names.add(item.name.rstrip("/"))
        if item.issym() or item.islnk():
            require_safe_archive_path(item.linkname)
    return names


def pe_machine(data: bytes) -> int:
    pe_offset = pe_header_offset(data)
    return struct.unpack_from("<H", data, pe_offset + 4)[0]


def pe_header_offset(data: bytes) -> int:
    if len(data) < 64 or data[:2] != b"MZ":
        raise RuntimeError("Windows executable is missing the MZ header")
    pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
    if len(data) < pe_offset + 6 or data[pe_offset : pe_offset + 4] != b"PE\0\0":
        raise RuntimeError("Windows executable is missing the PE header")
    return pe_offset


def pe_optional_header(data: bytes) -> tuple[int, int, int]:
    pe_offset = pe_header_offset(data)
    if len(data) < pe_offset + 24:
        raise RuntimeError("Windows executable has a truncated COFF header")
    section_count = struct.unpack_from("<H", data, pe_offset + 6)[0]
    optional_size = struct.unpack_from("<H", data, pe_offset + 20)[0]
    optional_offset = pe_offset + 24
    if optional_size < 70 or len(data) < optional_offset + optional_size:
        raise RuntimeError("Windows executable has a truncated optional header")
    return optional_offset, optional_size, section_count


def pe_subsystem(data: bytes) -> int:
    optional_offset, _, _ = pe_optional_header(data)
    return struct.unpack_from("<H", data, optional_offset + 68)[0]


def pe_resource_type_ids(data: bytes) -> set[int]:
    optional_offset, optional_size, section_count = pe_optional_header(data)
    magic = struct.unpack_from("<H", data, optional_offset)[0]
    if magic == 0x10B:
        data_directories_offset = optional_offset + 96
    elif magic == 0x20B:
        data_directories_offset = optional_offset + 112
    else:
        raise RuntimeError(f"Windows executable has unknown optional header 0x{magic:04x}")
    resource_entry = data_directories_offset + 2 * 8
    if resource_entry + 8 > optional_offset + optional_size:
        return set()
    resource_rva, resource_size = struct.unpack_from("<II", data, resource_entry)
    if resource_rva == 0 or resource_size < 16:
        return set()

    section_offset = optional_offset + optional_size
    resource_offset = None
    for index in range(section_count):
        entry = section_offset + index * 40
        if entry + 40 > len(data):
            raise RuntimeError("Windows executable has a truncated section table")
        virtual_size, virtual_address, raw_size, raw_offset = struct.unpack_from(
            "<IIII", data, entry + 8
        )
        extent = max(virtual_size, raw_size)
        if virtual_address <= resource_rva < virtual_address + extent:
            resource_offset = raw_offset + resource_rva - virtual_address
            break
    if resource_offset is None or resource_offset + 16 > len(data):
        raise RuntimeError("Windows executable resource directory is out of bounds")

    named_count, id_count = struct.unpack_from("<HH", data, resource_offset + 12)
    entries_offset = resource_offset + 16
    entries_end = entries_offset + (named_count + id_count) * 8
    if entries_end > len(data):
        raise RuntimeError("Windows executable has a truncated resource directory")
    ids = set()
    for index in range(named_count, named_count + id_count):
        resource_id = struct.unpack_from("<I", data, entries_offset + index * 8)[0]
        if resource_id & 0x8000_0000 == 0:
            ids.add(resource_id)
    return ids


def verify_windows_application_binary(data: bytes, artifact: str) -> None:
    if pe_subsystem(data) != 2:
        raise RuntimeError(f"{artifact} application executable is not Windows GUI subsystem")
    resource_ids = pe_resource_type_ids(data)
    missing = {3, 14} - resource_ids
    if missing:
        raise RuntimeError(
            f"{artifact} application executable is missing Windows icon resources: "
            + ", ".join(str(value) for value in sorted(missing))
        )


def elf_machine(data: bytes) -> int:
    if len(data) < 20 or data[:4] != b"\x7fELF":
        raise RuntimeError("Linux executable is missing the ELF header")
    byte_order = "little" if data[5] == 1 else "big"
    return int.from_bytes(data[18:20], byte_order)


def macho_cpu_type(data: bytes) -> int:
    if len(data) < 8:
        raise RuntimeError("macOS executable is too short")
    magic = data[:4]
    if magic == b"\xcf\xfa\xed\xfe":
        byte_order = "little"
    elif magic == b"\xfe\xed\xfa\xcf":
        byte_order = "big"
    else:
        raise RuntimeError("macOS executable is not a 64-bit Mach-O file")
    return int.from_bytes(data[4:8], byte_order)


def verify_macos_url_scheme(
    plist: dict[str, object], artifact: str,
    identity: package_native.PackageIdentity = package_native.STABLE_IDENTITY,
) -> None:
    url_types = plist.get("CFBundleURLTypes")
    if not isinstance(url_types, list) or len(url_types) != 1:
        raise RuntimeError(f"{artifact} must register exactly one macOS URL type")
    url_type = url_types[0]
    if not isinstance(url_type, dict):
        raise RuntimeError(f"{artifact} contains an invalid macOS URL type")
    schemes = url_type.get("CFBundleURLSchemes")
    if schemes != [identity.desktop_id]:
        raise RuntimeError(
            f"{artifact} must register only the {identity.desktop_id} URL scheme"
        )


def verify_linux_desktop(
    content: str, expected_executable: str, artifact: str,
    identity: package_native.PackageIdentity = package_native.STABLE_IDENTITY,
) -> None:
    fields: dict[str, str] = {}
    for line in content.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith(("#", "[")):
            continue
        if "=" not in stripped:
            raise RuntimeError(f"{artifact} contains an invalid desktop entry line")
        key, value = stripped.split("=", 1)
        if key in fields:
            raise RuntimeError(f"{artifact} contains duplicate desktop field {key}")
        fields[key] = value

    expected = {
        "Type": "Application",
        "Name": identity.display_name,
        "Icon": identity.desktop_id,
        "StartupWMClass": identity.desktop_id,
        "Exec": f"{expected_executable} %U",
        "MimeType": f"x-scheme-handler/{identity.desktop_id};",
    }
    for key, value in expected.items():
        if fields.get(key) != value:
            raise RuntimeError(
                f"{artifact} desktop field {key} is {fields.get(key)!r}, expected {value!r}"
            )


def read_rpm_member(path: Path, member: str) -> bytes:
    rpm2cpio = shutil.which("rpm2cpio")
    if not rpm2cpio:
        raise RuntimeError("rpm2cpio is required to verify RPM contents")
    archive = subprocess.check_output([rpm2cpio, str(path)])
    offset = 0
    while offset < len(archive):
        header = archive[offset : offset + 110]
        if len(header) != 110 or header[:6] not in (b"070701", b"070702"):
            raise RuntimeError(f"{path.name} contains an invalid RPM cpio payload")
        try:
            fields = [
                int(header[6 + index * 8 : 14 + index * 8], 16)
                for index in range(13)
            ]
        except ValueError as error:
            raise RuntimeError(
                f"{path.name} contains an invalid RPM cpio header"
            ) from error
        file_size = fields[6]
        name_size = fields[11]
        offset += 110
        name_end = offset + name_size
        if name_size < 1 or name_end > len(archive):
            raise RuntimeError(f"{path.name} contains an invalid RPM cpio name")
        name = archive[offset : name_end - 1].decode("utf-8")
        offset = (name_end + 3) & ~3
        data_end = offset + file_size
        if data_end > len(archive):
            raise RuntimeError(f"{path.name} contains a truncated RPM cpio entry")
        if name == "TRAILER!!!":
            break
        if name.removeprefix("./") == member.removeprefix("/"):
            return archive[offset:data_end]
        offset = (data_end + 3) & ~3
    raise RuntimeError(f"{path.name} is missing {member}")


def verify_windows_portable(path: Path, target: str, version: str) -> None:
    root = "NyaTerm-portable"
    executables = ["NyaTerm.exe", *helper_filenames(target)]
    required = {
        f"{root}/{name}" for name in executables
    } | {
        f"{root}/nyaterm-portable",
        f"{root}/LICENSE",
        f"{root}/VERSION",
        f"{root}/data/.keep",
    }
    with zipfile.ZipFile(path) as archive:
        names = verify_zip_paths(archive)
        missing = required - names
        if missing:
            raise RuntimeError(f"{path.name} is missing: {', '.join(sorted(missing))}")
        packaged_version = archive.read(f"{root}/VERSION").decode("utf-8").strip()
        if packaged_version != version:
            raise RuntimeError(f"{path.name} contains version {packaged_version}, expected {version}")
        binaries = {name: archive.read(f"{root}/{name}") for name in executables}
        machines = {name: pe_machine(data) for name, data in binaries.items()}
        verify_windows_application_binary(binaries["NyaTerm.exe"], path.name)
    expected_machine = {
        "x86_64-pc-windows-msvc": 0x8664,
        "aarch64-pc-windows-msvc": 0xAA64,
    }[target]
    for name, machine in machines.items():
        if machine != expected_machine:
            raise RuntimeError(
                f"{path.name} contains PE machine 0x{machine:04x} for {name}, "
                f"expected 0x{expected_machine:04x}"
            )


def find_7zip() -> str:
    for name in ("7z", "7z.exe"):
        found = shutil.which(name)
        if found:
            return found
    raise RuntimeError("7-Zip is required to verify the NSIS installer")


def verify_windows_installer_script(content: str, version: str) -> None:
    identity = package_native.release_identity(version)
    required = [
        f'Name "{identity.display_name}"',
        rf'InstallDir "$LOCALAPPDATA\Programs\{identity.display_name}"',
        f'InstallDirRegKey HKCU "{identity.windows_registry_key}" "InstallDir"',
        f'VIProductVersion "{package_native.windows_numeric_version(version)}"',
        f'VIFileVersion "{package_native.windows_numeric_version(version)}"',
        f'VIAddVersionKey "ProductName" "{identity.display_name}"',
        f'VIAddVersionKey "ProductVersion" "{version}"',
        f'VIAddVersionKey "FileVersion" "{version}"',
        f'VIAddVersionKey "FileDescription" "{identity.display_name} native GPUI terminal"',
        r'!define MUI_FINISHPAGE_RUN "$INSTDIR\NyaTerm.exe"',
        f'!define MUI_FINISHPAGE_RUN_TEXT "Launch {identity.display_name}"',
        f'WriteRegStr HKCU "{identity.windows_registry_key}" "InstallDir" "$INSTDIR"',
        f'WriteRegStr HKCU "{identity.windows_uninstall_key}" "DisplayName" "{identity.display_name}"',
        rf'WriteRegStr HKCU "Software\Classes\{identity.desktop_id}" "URL Protocol" ""',
        rf'CreateDirectory "$SMPROGRAMS\{identity.display_name}"',
        rf'CreateShortcut "$SMPROGRAMS\{identity.display_name}\{identity.display_name}.lnk" "$INSTDIR\NyaTerm.exe" "" "$INSTDIR\icon.ico"',
        rf'CreateShortcut "$DESKTOP\{identity.display_name}.lnk" "$INSTDIR\NyaTerm.exe" "" "$INSTDIR\icon.ico"',
        f'DeleteRegKey HKCU "{identity.windows_registry_key}"',
        f'DeleteRegKey HKCU "{identity.windows_uninstall_key}"',
        rf'DeleteRegKey HKCU "Software\Classes\{identity.desktop_id}"',
        rf'Delete "$DESKTOP\{identity.display_name}.lnk"',
        rf'RMDir "$SMPROGRAMS\{identity.display_name}"',
    ]
    directives = {line.strip() for line in content.splitlines()}
    for line in required:
        if line not in directives:
            raise RuntimeError(f"NSIS identity directive missing: {line}")
    other = (package_native.STABLE_IDENTITY if identity == package_native.PREVIEW_IDENTITY
             else package_native.PREVIEW_IDENTITY)
    for forbidden in [
        f'"{other.windows_registry_key}"', f'"{other.windows_uninstall_key}"',
        rf'"Software\Classes\{other.desktop_id}"',
        rf'"$LOCALAPPDATA\Programs\{other.display_name}"',
        rf'"$SMPROGRAMS\{other.display_name}"',
        rf'"$DESKTOP\{other.display_name}.lnk"',
    ]:
        if forbidden in content:
            raise RuntimeError(f"NSIS script targets the other application: {forbidden}")


def verify_windows_installer(path: Path, target: str, version: str) -> None:
    identity = package_native.release_identity(version)
    script = package_native.WORK_DIR / "nyaterm-installer.nsi"
    verify_windows_installer_script(script.read_text(encoding="utf-8"), version)
    if sys.platform == "win32":
        # Pass the filename separately, never interpolate it into PowerShell.
        metadata = json.loads(subprocess.check_output([
            "powershell.exe", "-NoProfile", "-NonInteractive", "-Command",
            "$v = (Get-Item -LiteralPath $env:NYATERM_VERIFY_INSTALLER).VersionInfo; "
            "$v | Select-Object ProductName,FileDescription,ProductVersion,FileVersion | ConvertTo-Json -Compress",
        ], text=True, env={**os.environ, "NYATERM_VERIFY_INSTALLER": str(path.resolve())}))
        expected_strings = {
            "ProductName": identity.display_name,
            "FileDescription": f"{identity.display_name} native GPUI terminal",
        }
        for key, expected in expected_strings.items():
            if metadata.get(key) != expected:
                raise RuntimeError(
                    f"{path.name} contains the wrong Windows version metadata: "
                    f"{key}={metadata.get(key)!r}, expected {expected!r}"
                )
        numeric_version = package_native.windows_numeric_version(version)
        for key in ("ProductVersion", "FileVersion"):
            if metadata.get(key) not in {version, numeric_version}:
                raise RuntimeError(
                    f"{path.name} contains the wrong Windows version metadata: "
                    f"{key}={metadata.get(key)!r}, expected {version!r} or {numeric_version!r}"
                )
    with path.open("rb") as handle:
        header = handle.read(2)
    if header != b"MZ":
        raise RuntimeError(f"{path.name} is not a Windows executable")
    with tempfile.TemporaryDirectory() as directory:
        output = Path(directory) / "installer"
        subprocess.run(
            [find_7zip(), "x", "-y", f"-o{output}", str(path)],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        names = {candidate.name for candidate in output.rglob("*") if candidate.is_file()}
        version_files = list(output.rglob("VERSION"))
        if len(version_files) != 1 or version_files[0].read_text(encoding="utf-8").strip() != version:
            raise RuntimeError(f"{path.name} contains the wrong installed version")
    required = {"NyaTerm.exe", "LICENSE", "VERSION", "Uninstall.exe"}
    required.update(helper_filenames(target))
    missing = required - names
    if missing:
        raise RuntimeError(f"{path.name} is missing installed files: {', '.join(sorted(missing))}")


def verify_macos_archive(path: Path, target: str, version: str) -> None:
    identity = package_native.release_identity(version)
    bundle = identity.macos_bundle_name
    executable = f"{bundle}/Contents/MacOS/NyaTerm"
    helpers = [f"{bundle}/Contents/MacOS/{name}" for name in helper_filenames(target)]
    info_plist = f"{bundle}/Contents/Info.plist"
    version_file = f"{bundle}/Contents/Resources/VERSION"
    required = {
        executable,
        *helpers,
        info_plist,
        version_file,
        f"{bundle}/Contents/Resources/LICENSE",
        f"{bundle}/Contents/Resources/icon.icns",
    }
    with tarfile.open(path, "r:gz") as archive:
        names = verify_tar_paths(archive)
        missing = required - names
        if missing:
            raise RuntimeError(f"{path.name} is missing: {', '.join(sorted(missing))}")
        packaged_version = archive.extractfile(version_file).read().decode().strip()  # type: ignore[union-attr]
        plist = plistlib.loads(archive.extractfile(info_plist).read())  # type: ignore[union-attr]
        binary = archive.extractfile(executable).read()  # type: ignore[union-attr]
        helper_binaries = {
            name: archive.extractfile(name).read()  # type: ignore[union-attr]
            for name in helpers
        }
    if packaged_version != version or plist.get("CFBundleShortVersionString") != version:
        raise RuntimeError(f"{path.name} contains inconsistent version metadata")
    if plist.get("CFBundleIdentifier") != identity.macos_identifier:
        raise RuntimeError(f"{path.name} contains the wrong bundle identifier")
    for field in ("CFBundleDisplayName", "CFBundleName"):
        if plist.get(field) != identity.display_name:
            raise RuntimeError(f"{path.name} contains the wrong {field}")
    if plist.get("CFBundleExecutable") != "NyaTerm" or plist.get("CFBundleVersion") != version:
        raise RuntimeError(f"{path.name} contains inconsistent bundle metadata")
    verify_macos_url_scheme(plist, path.name, identity)
    expected_cpu = {
        "x86_64-apple-darwin": 0x01000007,
        "aarch64-apple-darwin": 0x0100000C,
    }[target]
    for name, data in {executable: binary, **helper_binaries}.items():
        actual_cpu = macho_cpu_type(data)
        if actual_cpu != expected_cpu:
            raise RuntimeError(
                f"{path.name} contains Mach-O CPU 0x{actual_cpu:08x} for {name}, "
                f"expected 0x{expected_cpu:08x}"
            )


def verify_dmg(path: Path, version: str) -> None:
    identity = package_native.release_identity(version)
    if sys.platform != "darwin":
        return
    result = subprocess.run(
        ["hdiutil", "attach", "-plist", "-readonly", "-nobrowse", str(path)],
        check=True,
        stdout=subprocess.PIPE,
    )
    payload = plistlib.loads(result.stdout)
    entities = payload.get("system-entities", [])
    mount_point = next(
        (item.get("mount-point") for item in entities if item.get("mount-point")), None
    )
    device = next(
        (item.get("dev-entry") for item in reversed(entities) if item.get("dev-entry")), None
    )
    try:
        if not mount_point:
            raise RuntimeError(f"{path.name} did not expose a mounted volume")
        executable = Path(mount_point) / identity.macos_bundle_name / "Contents" / "MacOS" / "NyaTerm"
        bundle = Path(mount_point) / identity.macos_bundle_name
        plist = plistlib.loads((bundle / "Contents" / "Info.plist").read_bytes())
        if plist.get("CFBundleIdentifier") != identity.macos_identifier or plist.get("CFBundleDisplayName") != identity.display_name:
            raise RuntimeError(f"{path.name} contains the wrong bundle identity")
        verify_macos_url_scheme(plist, path.name, identity)
        if not (Path(mount_point) / "Applications").is_symlink() or os.readlink(Path(mount_point) / "Applications") != "/Applications":
            raise RuntimeError(f"{path.name} is missing the Applications link")
        for name in helper_filenames("aarch64-apple-darwin"):
            if not (bundle / "Contents" / "MacOS" / name).is_file():
                raise RuntimeError(f"{path.name} is missing {name}")
        if (bundle / "Contents" / "Resources" / "VERSION").read_text().strip() != version:
            raise RuntimeError(f"{path.name} contains the wrong version")
        if not executable.is_file():
            raise RuntimeError(f"{path.name} does not contain the NyaTerm application")
    finally:
        if device:
            subprocess.run(["hdiutil", "detach", device], check=True)


def verify_appimage(path: Path, target: str, version: str) -> None:
    identity = package_native.release_identity(version)
    expected_machine = {
        "x86_64-unknown-linux-gnu": 62,
        "aarch64-unknown-linux-gnu": 183,
    }[target]
    with path.open("rb") as handle:
        machine = elf_machine(handle.read(64))
    if machine != expected_machine:
        raise RuntimeError(f"{path.name} contains ELF machine {machine}, expected {expected_machine}")
    if not os.access(path, os.X_OK):
        raise RuntimeError(f"{path.name} is not executable")

    with tempfile.TemporaryDirectory() as directory:
        subprocess.run(
            [str(path.resolve()), "--appimage-extract"],
            cwd=directory,
            check=True,
            stdout=subprocess.DEVNULL,
            env={**os.environ, "APPIMAGE_EXTRACT_AND_RUN": "1"},
        )
        root = Path(directory) / "squashfs-root"
        executables = [
            root / "usr" / "bin" / name
            for name in ("nyaterm", *helper_filenames(target))
        ]
        version_file = root / "usr" / "share" / "doc" / identity.desktop_id / "VERSION"
        desktop_file = root / "usr" / "share" / "applications" / identity.linux_desktop_file
        required = [
            root / "AppRun",
            *executables,
            desktop_file,
            root / identity.linux_desktop_file,
            root / f"{identity.desktop_id}.png",
            root / "usr" / "share" / "doc" / identity.desktop_id / "LICENSE",
            version_file,
        ]
        missing = [item for item in required if not item.exists()]
        if missing:
            raise RuntimeError(f"{path.name} is missing AppImage entries: {missing}")
        packaged_version = version_file.read_text(encoding="utf-8").strip()
        desktop_content = desktop_file.read_text(encoding="utf-8")
        verify_linux_desktop((root / identity.linux_desktop_file).read_text(encoding="utf-8"), package_native.APP_BIN, path.name, identity)
        for size in ("32x32", "64x64", "128x128", "256x256"):
            icon = root / "usr" / "share" / "icons" / "hicolor" / size / "apps" / f"{identity.desktop_id}.png"
            if not icon.is_file():
                raise RuntimeError(f"{path.name} is missing {icon.name}")
        machines = {}
        for executable in executables:
            with executable.open("rb") as handle:
                machines[executable.name] = elf_machine(handle.read(64))
    if packaged_version != version:
        raise RuntimeError(f"{path.name} contains inconsistent version")
    verify_linux_desktop(desktop_content, package_native.APP_BIN, path.name, identity)
    for name, binary_machine in machines.items():
        if binary_machine != expected_machine:
            raise RuntimeError(
                f"{path.name} contains ELF machine {binary_machine} for {name}, "
                f"expected {expected_machine}"
            )


def verify_deb(path: Path, target: str, version: str) -> None:
    identity = package_native.release_identity(version)
    expected_arch = package_native.linux_deb_arch(target)
    fields = subprocess.check_output(
        ["dpkg-deb", "--field", str(path), "Package", "Version", "Architecture"],
        text=True,
    )
    metadata = dict(line.split(": ", 1) for line in fields.splitlines())
    if metadata.get("Package") != identity.desktop_id:
        raise RuntimeError(f"{path.name} has the wrong Debian package name")
    if metadata.get("Version") != version.replace("-", "~"):
        raise RuntimeError(f"{path.name} has the wrong Debian version")
    if metadata.get("Architecture") != expected_arch:
        raise RuntimeError(f"{path.name} has the wrong Debian architecture")
    contents = subprocess.check_output(["dpkg-deb", "--contents", str(path)], text=True)
    for required in (
        f"./opt/{identity.desktop_id}/nyaterm",
        f"./opt/{identity.desktop_id}/VERSION",
        f"./usr/share/applications/{identity.linux_desktop_file}",
        *(f"./opt/{identity.desktop_id}/{name}" for name in helper_filenames(target)),
    ):
        if required not in contents:
            raise RuntimeError(f"{path.name} is missing {required}")
    with tempfile.TemporaryDirectory() as directory:
        subprocess.run(
            ["dpkg-deb", "--extract", str(path), directory],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        root = Path(directory)
        desktop_content = (
            root / "usr" / "share" / "applications" / identity.linux_desktop_file
        ).read_text(encoding="utf-8")
        if (root / "opt" / identity.desktop_id / "VERSION").read_text(encoding="utf-8").strip() != version:
            raise RuntimeError(f"{path.name} contains the wrong installed version")
        for name in (package_native.APP_BIN, *helper_filenames(target)):
            if not (root / "opt" / identity.desktop_id / name).is_file():
                raise RuntimeError(f"{path.name} is missing {name}")
        for size in ("32x32", "64x64", "128x128", "256x256"):
            if not (root / "usr" / "share" / "icons" / "hicolor" / size / "apps" / f"{identity.desktop_id}.png").is_file():
                raise RuntimeError(f"{path.name} is missing its {size} icon")
    verify_linux_desktop(desktop_content, f"/opt/{identity.desktop_id}/nyaterm", path.name, identity)


def verify_rpm(path: Path, target: str, version: str) -> None:
    identity = package_native.release_identity(version)
    rpm_version, rpm_release = package_native.linux_rpm_version(version)
    expected = f"{identity.desktop_id}|{rpm_version}|{rpm_release}|{package_native.linux_rpm_arch(target)}"
    actual = subprocess.check_output(
        ["rpm", "-qp", "--qf", "%{NAME}|%{VERSION}|%{RELEASE}|%{ARCH}", str(path)],
        text=True,
    )
    if actual != expected:
        raise RuntimeError(f"{path.name} has RPM metadata {actual!r}, expected {expected!r}")
    contents = subprocess.check_output(["rpm", "-qlp", str(path)], text=True)
    for required in (
        f"/opt/{identity.desktop_id}/nyaterm",
        f"/opt/{identity.desktop_id}/VERSION",
        f"/usr/share/applications/{identity.linux_desktop_file}",
        *(f"/opt/{identity.desktop_id}/{name}" for name in helper_filenames(target)),
        *(f"/usr/share/icons/hicolor/{size}/apps/{identity.desktop_id}.png"
          for size in ("32x32", "64x64", "128x128", "256x256")),
    ):
        if required not in contents.splitlines():
            raise RuntimeError(f"{path.name} is missing {required}")
    if read_rpm_member(path, f"/opt/{identity.desktop_id}/VERSION").decode("utf-8").strip() != version:
        raise RuntimeError(f"{path.name} contains the wrong installed version")
    desktop_content = read_rpm_member(
        path, f"/usr/share/applications/{identity.linux_desktop_file}"
    ).decode("utf-8")
    verify_linux_desktop(desktop_content, f"/opt/{identity.desktop_id}/nyaterm", path.name, identity)


def verify_release(
    dist: Path, target: str, version: str, artifact_version: str | None = None
) -> dict[str, object]:
    version = package_native.validate_version(version)
    artifact_version = package_native.validate_artifact_version(
        artifact_version or version
    )
    expected_names = package_native.artifact_names(target, artifact_version)
    actual_names = {path.name for path in dist.iterdir() if path.is_file()}
    missing = expected_names - actual_names
    unexpected = actual_names - expected_names
    if missing:
        raise RuntimeError(f"missing release artifacts: {', '.join(sorted(missing))}")
    if unexpected:
        raise RuntimeError(f"unexpected release artifacts: {', '.join(sorted(unexpected))}")
    for name in expected_names:
        if (dist / name).stat().st_size < MIN_ARTIFACT_SIZE:
            raise RuntimeError(f"release artifact is unexpectedly small: {name}")

    info = package_native.target_info(target)
    prefix = f"{package_native.APP_NAME}_{artifact_version}_{info.label}"
    if info.os_name == "windows":
        verify_windows_portable(dist / f"{prefix}_portable.zip", target, version)
        verify_windows_installer(dist / f"{prefix}-setup.exe", target, version)
    elif info.os_name == "macos":
        verify_macos_archive(dist / f"{prefix}.app.tar.gz", target, version)
        verify_dmg(dist / f"{prefix}.dmg", version)
    else:
        verify_appimage(dist / f"{prefix}.AppImage", target, version)
        verify_deb(dist / f"{prefix}.deb", target, version)
        verify_rpm(dist / f"{prefix}.rpm", target, version)
    return {"target": target, "version": version, "artifacts": sorted(expected_names)}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--artifact-version")
    parser.add_argument("--dist", type=Path, default=Path("dist"))
    args = parser.parse_args()
    summary = verify_release(
        args.dist.resolve(), args.target, args.version, args.artifact_version
    )
    print(
        f"Verified {len(summary['artifacts'])} artifacts for "
        f"{summary['target']} ({summary['version']})"
    )


if __name__ == "__main__":
    main()
