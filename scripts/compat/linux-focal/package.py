#!/usr/bin/env python3
"""Audit and package a native focal build; compatible with target Python 3.8."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import textwrap


ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "scripts/release"))
import linux_icons
import prepare_android

BINARIES = ("nyaterm", "nyaterm-mcp", "nyaterm-rdp-helper", "nyaterm-vnc-helper", "nyaterm-android-helper")
ANDROID_LIBRARIES = {"libavcodec.so.62", "libavformat.so.62", "libavutil.so.60", "libswscale.so.9"}
LIMITS = {"GLIBC": (2, 31), "GLIBCXX": (3, 4, 28), "CXXABI": (1, 3, 12)}
# These SONAMEs are supplied by the documented focal desktop. No newer SSL/FFI,
# private libc, or bundled replacement loader is accepted silently.
SYSTEM_LIBRARIES = {
    "libc.so.6", "libm.so.6", "libgcc_s.so.1", "libstdc++.so.6",
    "libpthread.so.0", "libdl.so.2", "librt.so.1", "libutil.so.1",
    "ld-linux-x86-64.so.2", "libdbus-1.so.3", "libgtk-3.so.0", "libgdk-3.so.0",
    "libgdk_pixbuf-2.0.so.0", "libgio-2.0.so.0", "libgobject-2.0.so.0",
    "libglib-2.0.so.0", "libxcb.so.1", "libxkbcommon.so.0", "libxkbcommon-x11.so.0",
    "libudev.so.1", "libssl.so.1.1", "libcrypto.so.1.1", "libz.so.1",
}


def command(*args, extra_env=None):
    return subprocess.check_output(args, text=True, stderr=subprocess.STDOUT,
                                   env=dict(os.environ, LC_ALL="C", **(extra_env or {})))


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def check_versions(output):
    requirements = sorted(set(re.findall(r"Name: ((?:GLIBC|GLIBCXX|CXXABI)_[\w.]+)", output)))
    if not any(value.startswith("GLIBC_") for value in requirements):
        raise ValueError("missing glibc version requirements")
    for requirement in requirements:
        family, number = requirement.split("_", 1)
        if not re.fullmatch(r"\d+(?:\.\d+)+", number):
            raise ValueError("unsupported private/ABI requirement: " + requirement)
        if tuple(map(int, number.split("."))) > LIMITS[family]:
            raise ValueError("exceeds focal ABI baseline: " + requirement)
    return requirements


def audit(binary, *, android=False, shared=False):
    with binary.open("rb") as handle:
        header = handle.read(20)
    if (len(header) < 20 or header[:6] != b"\x7fELF\x02\x01"
            or struct.unpack_from("<H", header, 18)[0] != 62):
        raise ValueError(str(binary) + ": expected little-endian ELF64 x86-64")
    version_output = command("readelf", "--version-info", "--wide", str(binary))
    versions = check_versions(version_output.partition("Version needs section")[2] if shared else version_output)
    program_headers = command("readelf", "--program-headers", "--wide", str(binary))
    if not shared and "[Requesting program interpreter: /lib64/ld-linux-x86-64.so.2]" not in program_headers:
        raise ValueError(str(binary) + ": expected the system x86-64 loader")
    dynamic = command("readelf", "--dynamic", "--wide", str(binary))
    allowed_paths = {"$ORIGIN"} if shared else {"$ORIGIN/android-libs", "$ORIGIN/../android-libs"}
    for search_path in re.findall(r"\((?:RUNPATH|RPATH)\).*\[([^]]*)\]", dynamic):
        if not android or not set(search_path.split(":")) <= allowed_paths:
            raise ValueError(str(binary) + ": unexpected library search path")
    needed = re.findall(r"\(NEEDED\).*Shared library: \[([^]]+)\]", dynamic)
    unknown = set(needed) - SYSTEM_LIBRARIES - (ANDROID_LIBRARIES if android else set())
    if unknown:
        raise ValueError("unreviewed runtime libraries: " + ", ".join(sorted(unknown)))
    # Checks transitive dependencies and unversioned GTK/GLib symbols too.
    linkage = command("ldd", "-r", str(binary.resolve()))
    if "not found" in linkage or "undefined symbol" in linkage:
        raise ValueError(str(binary) + ": unresolved runtime dependencies:\n" + linkage)
    return {"name": binary.name, "sha256": sha256(binary),
            "versions": versions, "needed": needed, "ldd": linkage}


def stage_android(binary_dir, stage):
    for name in ("adb", "android-libs"):
        shutil.copytree(binary_dir / name, stage / name)
    expected = {"adb/adb", "adb/NOTICE.txt", "adb/SOURCE.json", "android-libs/LICENSE.txt",
                "android-libs/SOURCE.json", *["android-libs/" + name for name in ANDROID_LIBRARIES]}
    actual = {path.relative_to(stage).as_posix() for directory in (stage / "adb", stage / "android-libs")
              for path in directory.rglob("*") if path.is_file()}
    if actual != expected:
        raise ValueError("unexpected or missing Android runtime files")
    (stage / "adb/adb").chmod(0o755)
    for name in expected - {"adb/adb"}:
        (stage / name).chmod(0o644)
    # Official x64 platform-tools carries Android build-tree lib64 search
    # paths. This ADB uses only system libraries; remove those paths from the
    # package copy before auditing and hashing, without relaxing the ABI gate.
    subprocess.run(["patchelf", "--remove-rpath", str(stage / "adb/adb")], check=True)
    # Resolve sibling FFmpeg dependencies without a global LD_LIBRARY_PATH.
    for name in sorted(ANDROID_LIBRARIES):
        subprocess.run(["patchelf", "--set-rpath", "$ORIGIN", str(stage / "android-libs" / name)], check=True)
    reports = [audit(stage / "adb/adb")]
    reports.extend(audit(stage / "android-libs" / name, android=True, shared=True)
                   for name in sorted(ANDROID_LIBRARIES))
    for name in ("adb/SOURCE.json", "adb/NOTICE.txt", "android-libs/SOURCE.json", "android-libs/LICENSE.txt"):
        if not (stage / name).is_file() or not (stage / name).stat().st_size:
            raise ValueError("missing Android provenance/license: " + name)
    prepare_android.write_resource_manifest(stage, "x86_64-unknown-linux-gnu")
    return reports


def audit_opengl(runtime):
    for required in ("lib/libEGL.so.1", "lib/libEGL_nyaterm.so.1",
                     "share/glvnd/egl_vendor.d/50_mesa.json", "packages.json", "licenses"):
        if not (runtime / required).exists():
            raise ValueError("incomplete OpenGL runtime: " + required)
    reports = []
    for library in sorted((runtime / "lib").rglob("*")):
        if not library.is_file() or library.is_symlink():
            continue
        with library.open("rb") as handle:
            if handle.read(4) != b"\x7fELF":
                continue
        if library.name.startswith(("libc.so", "ld-linux", "libpthread.so", "libdl.so")):
            raise ValueError("system libc/loader must not be bundled")
        version_output = command("readelf", "--version-info", "--wide", str(library))
        # For DSOs, inspect requirements, not the versions they themselves export.
        needs = version_output.partition("Version needs section")[2]
        glibc_needs = "\n".join(re.findall(r"Name: GLIBC_[\w.]+", needs))
        versions = check_versions(glibc_needs) if glibc_needs else []
        dynamic = command("readelf", "--dynamic", "--wide", str(library))
        for rpath in re.findall(r"\((?:RUNPATH|RPATH)\).*\[([^]]*)\]", dynamic):
            if any(not part.startswith("$ORIGIN") for part in rpath.split(":")):
                raise ValueError("non-relocatable OpenGL library: " + str(library))
        linkage = command("ldd", "-r", str(library.resolve()),
                          extra_env={"LD_LIBRARY_PATH": str((runtime / "lib").resolve())})
        if "not found" in linkage or "undefined symbol" in linkage:
            raise ValueError("unresolved OpenGL dependency: " + linkage)
        reports.append({"name": str(library.relative_to(runtime)), "sha256": sha256(library),
                        "glibc_requirements": versions})
    return reports


def create_deb(stage, output, version):
    """Package the audited runtime without touching an installed/released copy."""
    desktop_id = "nyaterm-compat"
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        app_root = root / "opt" / desktop_id
        shutil.copytree(stage, app_root, symlinks=True)
        # The compatibility build uses its own per-user configuration namespace;
        # /opt is read-only to ordinary users. The tarball remains portable.
        (app_root / "nyaterm-portable").unlink()
        launcher = root / "usr/bin" / desktop_id
        launcher.parent.mkdir(parents=True)
        launcher.write_text('#!/bin/sh\nexec /opt/nyaterm-compat/start-nyaterm.sh "$@"\n')
        launcher.chmod(0o755)
        desktop = root / "usr/share/applications" / (desktop_id + ".desktop")
        desktop.parent.mkdir(parents=True)
        wm_class = "NyaTerm Preview" if "-" in version.split("+", 1)[0] else "NyaTerm"
        desktop.write_text(textwrap.dedent("""\
            [Desktop Entry]
            Type=Application
            Name=NyaTerm (Compatibility)
            Comment=Ubuntu 20.04 terminal with GPU rendering and a CPU fallback
            Exec=/usr/bin/nyaterm-compat %U
            Icon=nyaterm-compat
            StartupWMClass={}
            Terminal=false
            Categories=Development;TerminalEmulator;Network;
            StartupNotify=true

            Actions=SoftwareRendering;

            [Desktop Action SoftwareRendering]
            Name=Start with CPU software rendering
            Name[zh_CN]=使用 CPU 软件渲染启动
            Exec=/usr/bin/nyaterm-compat --software %U
            """).format(wm_class), encoding="utf-8")
        desktop.chmod(0o644)
        linux_icons.copy_icons(root, ROOT / "crates/nyaterm-app/resources/icons", desktop_id)
        control = root / "DEBIAN"
        control.mkdir()
        linux_icons.write_postinst(control / "postinst", desktop_id)
        dependencies = deb_dependencies([app_root / name for name in BINARIES])
        (control / "control").write_text(textwrap.dedent("""\
            Package: nyaterm-compat
            Version: {}-1+glibc231
            Section: net
            Priority: optional
            Architecture: amd64
            Maintainer: NyaTerm compatibility packaging <noreply@github.com>
            Depends: bash, {}
            Recommends: libvulkan1 | mesa-vulkan-drivers
            Description: NyaTerm Ubuntu 20.04 compatibility build with GPU rendering and a CPU fallback
             Includes MCP/RDP/VNC/Android helpers, ADB, FFmpeg and a checksum-pinned Mesa runtime.
             Requires X11. Does not replace libc or system graphics drivers.
            """).format(version.replace("-", "~"), dependencies), encoding="utf-8")
        linux_icons.normalize_permissions(root)
        artifact = output / ("NyaTerm_{}_linux_x64_glibc231.deb".format(version))
        subprocess.run(["dpkg-deb", "--build", "--root-owner-group", str(root), str(artifact)],
                       check=True)
    artifact.with_name(artifact.name + ".sha256").write_text(
        sha256(artifact) + "  " + artifact.name + "\n", encoding="utf-8")
    print(artifact)
    return artifact


def deb_dependencies(binaries):
    """Resolve every shipped executable against focal's package database.

    Like the standard native packager, fail on missing dependency metadata;
    the ABI SONAME allowlist is not a substitute for Debian package ownership.
    Private Mesa is opt-in and must not affect the system-mode dependency scan.
    """
    with tempfile.TemporaryDirectory() as directory:
        scratch = Path(directory)
        (scratch / "debian").mkdir()
        (scratch / "debian/control").write_text(
            "Source: nyaterm-compat\nPackage: nyaterm-compat\nArchitecture: any\n"
            "Description: NyaTerm compatibility build\n", encoding="utf-8")
        args = ["dpkg-shlibdeps", "-O"]
        private_libraries = binaries[0].parent / "android-libs"
        if private_libraries.is_dir():
            # Give private FFmpeg SONAMEs local ownership, then exclude only our
            # own package. System dependencies of those DSOs are still scanned.
            shlibs = scratch / "android.shlibs"
            shlibs.write_text("".join(name.split(".so.")[0] + " " + name.split(".so.")[1]
                                      + " nyaterm-compat\n" for name in sorted(ANDROID_LIBRARIES)))
            args.extend(["-L" + str(shlibs), "-xnyaterm-compat", "-l" + str(private_libraries.resolve())])
            binaries = [*binaries, binaries[0].parent / "adb/adb",
                        *[private_libraries / name for name in sorted(ANDROID_LIBRARIES)]]
        for binary in binaries:
            args.extend(["-e", str(binary.resolve())])
        environment = dict(os.environ, LC_ALL="C")
        environment.pop("LD_LIBRARY_PATH", None)
        output = subprocess.check_output(args, cwd=scratch, text=True, env=environment)
    prefix = "shlibs:Depends="
    for line in output.splitlines():
        if line.startswith(prefix) and line[len(prefix):].strip():
            return line[len(prefix):].strip()
    raise RuntimeError("dpkg-shlibdeps did not report shlibs:Depends")


def create_package(binary_dir, output, version, opengl, deb=False):
    if os.confstr("CS_GNU_LIBC_VERSION") != "glibc 2.31":
        raise ValueError("package and resolve libraries inside the focal / glibc 2.31 image")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.+-]+)?", version):
        raise ValueError("invalid artifact version")
    graphics = audit_opengl(opengl)
    output.mkdir(parents=True, exist_ok=True)
    label = "NyaTerm_{}_linux_x64_glibc231".format(version)
    with tempfile.TemporaryDirectory() as directory:
        stage = Path(directory) / label
        stage.mkdir()
        for name in BINARIES:
            shutil.copy2(binary_dir / name, stage / name)
            (stage / name).chmod(0o755)
        android_reports = stage_android(binary_dir, stage)
        reports = [audit(stage / name, android=name == "nyaterm-android-helper") for name in BINARIES]
        # Portable mode keeps all data beside the application, without touching
        # installed NyaTerm data or depending on a fixed installation prefix.
        (stage / "nyaterm-portable").touch()
        for source in (ROOT / "LICENSE", Path(__file__).with_name("diagnose.sh"),
                       Path(__file__).with_name("start-nyaterm.sh"),
                       Path(__file__).with_name("OFFLINE-README.zh-CN.md")):
            shutil.copy2(source, stage / source.name)
            # NTFS/DrvFS checkouts can report every source file as executable.
            # Only the two launch scripts below are executable package inputs.
            (stage / source.name).chmod(0o644)
        (stage / "diagnose.sh").chmod(0o755)
        (stage / "start-nyaterm.sh").chmod(0o755)
        shutil.copytree(opengl, stage / "opengl", symlinks=True)
        for name in ("opengl-lock.txt", "egl_desktop.c"):
            shutil.copy2(Path(__file__).with_name(name), stage / "opengl" / name)
            (stage / "opengl" / name).chmod(0o644)
        (stage / "abi-report.json").write_text(json.dumps(
            {"baseline": "Ubuntu 20.04 x86_64 / glibc 2.31", "version": version,
             "binaries": reports, "opengl_libraries": graphics,
             "android_runtime": android_reports}, indent=2) + "\n", encoding="utf-8")
        linux_icons.normalize_permissions(stage)
        if deb:
            create_deb(stage, output, version)
        archive = output / (label + ".tar.gz")

        def archive_owner(info):
            # Do not retain the builder's identity or restrictive checkout modes.
            info.uid = info.gid = 0
            info.uname = info.gname = "root"
            return info

        with tarfile.open(archive, "w:gz", compresslevel=6) as handle:
            handle.add(stage, arcname=label, filter=archive_owner)
    digest = sha256(archive)
    archive.with_name(archive.name + ".sha256").write_text(
        digest + "  " + archive.name + "\n", encoding="utf-8")
    print(archive)
    return archive


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    parser.add_argument("--opengl", type=Path, required=True)
    parser.add_argument("--version")
    parser.add_argument("--deb", action="store_true", help="also build an installable compatibility deb")
    args = parser.parse_args()
    version = args.version
    if version is None:
        metadata = json.loads(command("cargo", "metadata", "--no-deps", "--offline", "--format-version=1"))
        version = next(p["version"] for p in metadata["packages"] if p["name"] == "nyaterm-app")
    create_package(args.binary_dir, args.output, version, args.opengl, deb=args.deb)


if __name__ == "__main__":
    main()
