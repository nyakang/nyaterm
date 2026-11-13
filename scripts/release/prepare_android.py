#!/usr/bin/env python3
"""Prepare pinned Android native dependencies; never modify reference checkouts."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import re
import shlex
import subprocess
import tarfile
import tempfile
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[2]
METADATA_PATH = Path(__file__).with_name("android-package.json")
METADATA = json.loads(METADATA_PATH.read_text())
STAGING = Path(os.environ.get("NYATERM_ANDROID_DEPS_DIR", ROOT / "target" / "android-deps")).resolve()
WINDOWS_FFMPEG_LIBRARIES = {"avcodec": 62, "avformat": 62, "avutil": 60, "swscale": 9}


def is_coff_archive(path: Path) -> bool:
    if not path.is_file():
        return False
    with path.open("rb") as stream:
        return stream.read(8) == b"!<arch>\n" and bool(stream.read(1))


def find_msvc_lib() -> Path:
    # Prefer the librarian beside the selected compiler, including cross tools.
    compiler = shutil.which("cl.exe")
    if compiler:
        librarian = Path(compiler).resolve().with_name("lib.exe")
        if librarian.is_file():
            return librarian
    librarian = shutil.which("lib.exe")
    if librarian:
        return Path(librarian).resolve()
    raise FileNotFoundError("MSVC lib.exe not found; open a Visual Studio Developer PowerShell for the target architecture")


def ensure_windows_import_libraries(prefix: Path, target: str) -> None:
    machines = {"x86_64-pc-windows-msvc": "X64", "aarch64-pc-windows-msvc": "ARM64"}
    if target not in machines:
        raise ValueError(f"Unsupported Windows FFmpeg target: {target}")
    machine = machines[target]
    library_dir = prefix / "lib"
    library_dir.mkdir(parents=True, exist_ok=True)
    librarian = None
    for name, major in WINDOWS_FFMPEG_LIBRARIES.items():
        destination = library_dir / f"{name}.lib"
        dll = prefix / "bin" / f"{name}-{major}.dll"
        definition = library_dir / f"{name}-{major}.def"
        try:
            if not dll.is_file() or dll.stat().st_size == 0:
                raise FileNotFoundError(f"Missing or empty FFmpeg DLL: {dll}")
            if is_coff_archive(destination):
                continue
            installed = prefix / "bin" / destination.name
            # MSVC FFmpeg installs import libraries in bin, while Rust searches lib.
            # lib.exe can also emit an .exp; keep all intermediate outputs private.
            with tempfile.TemporaryDirectory(prefix=f".{name}-", dir=library_dir) as directory:
                temporary = Path(directory) / destination.name
                if is_coff_archive(installed):
                    shutil.copy2(installed, temporary)
                else:
                    if not definition.is_file() or definition.stat().st_size == 0:
                        raise FileNotFoundError(f"Missing or empty FFmpeg definition: {definition}")
                    if librarian is None:
                        librarian = find_msvc_lib()
                    subprocess.run([str(librarian), "/NOLOGO", f"/MACHINE:{machine}",
                                    f"/DEF:{definition.resolve()}", f"/NAME:{dll.name}",
                                    f"/OUT:{temporary.resolve()}"], cwd=directory, check=True)
                if not is_coff_archive(temporary):
                    raise ValueError(f"Invalid COFF import library output: {temporary}")
                temporary.replace(destination)
        except (OSError, ValueError, subprocess.CalledProcessError) as error:
            raise RuntimeError(f"Cannot prepare {destination} for {target} ({machine}): {error}. "
                               f"If the cached installation is incomplete, remove {prefix} and rerun prepare_android.py "
                               "in a Visual Studio Developer PowerShell for the target architecture.") from error


def download(metadata: dict, name: str) -> Path:
    output = STAGING / "downloads" / name
    output.parent.mkdir(parents=True, exist_ok=True)
    if not output.is_file():
        partial = output.with_suffix(output.suffix + ".partial")
        with urllib.request.urlopen(metadata["url"], timeout=60) as response, partial.open("wb") as destination:
            shutil.copyfileobj(response, destination)
        partial.replace(output)
    expected = metadata.get("sha256")
    if expected and hashlib.sha256(output.read_bytes()).hexdigest() != expected:
        output.unlink()
        raise ValueError(f"Android dependency checksum mismatch: {name}")
    return output


def extract_tar(archive: Path, destination: Path) -> Path:
    destination.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive) as handle:
        if hasattr(tarfile, "data_filter"):
            handle.extractall(destination, filter="data")
        else:
            # Older focal Python has no extraction filters. Source archives only
            # need directories and regular files; reject links and special files.
            root = destination.resolve()
            members = handle.getmembers()
            for member in members:
                path = (root / member.name).resolve()
                if (root not in path.parents or not (member.isfile() or member.isdir())
                        or "\\" in member.name):
                    raise ValueError("Unsafe source archive member: " + member.name)
            handle.extractall(destination, members=members)
    directories = [entry for entry in destination.iterdir() if entry.is_dir()]
    if len(directories) != 1:
        raise ValueError("Expected one source archive root")
    return directories[0]


def adb_protoc(build: Path) -> Path:
    compiler = shutil.which("protoc")
    if compiler is None:
        raise FileNotFoundError("protoc is required to build bundled ADB")
    version = subprocess.check_output([compiler, "--version"], text=True).strip()
    match = re.fullmatch(r"libprotoc (\d+)\.(\d+)(?:\.\d+)?", version)
    if not match:
        raise ValueError("Unrecognized protoc version: " + version)
    release = tuple(map(int, match.groups()))
    if release < (3, 12):
        raise ValueError("ADB requires protoc >= 3.12")
    if release >= (3, 15):
        return Path(compiler)
    build.mkdir(parents=True, exist_ok=True)
    wrapper = build / "protoc-optional"
    wrapper.write_text("#!/bin/sh\nexec " + shlex.quote(compiler)
                       + ' --experimental_allow_proto3_optional "$@"\n', encoding="utf-8")
    wrapper.chmod(0o755)
    return wrapper.resolve()


def debian_library_package(path: Path) -> str:
    # ldd and dpkg can use different spellings on usr-merged distributions.
    # Query the canonical file as well as aliases, but never match by basename:
    # provenance must identify the library we actually copied.
    candidates = [path.resolve(), path]
    for candidate in list(candidates):
        name = str(candidate)
        alias = Path(name[4:]) if name.startswith("/usr/lib/") else (
            Path("/usr" + name) if name.startswith("/lib/") else None)
        if alias is not None and alias.exists() and alias.samefile(path):
            candidates.append(alias)
    for candidate in dict.fromkeys(candidates):
        result = subprocess.run(["dpkg-query", "-S", str(candidate)],
                                text=True, capture_output=True)
        if result.returncode == 1:
            continue
        result.check_returncode()
        owners = {line.rsplit(": ", 1)[0] for line in result.stdout.splitlines()
                  if ": " in line and line.rsplit(": ", 1)[1] == str(candidate)}
        if len(owners) != 1:
            raise ValueError(f"Ambiguous package ownership for {candidate}")
        return owners.pop()
    raise ValueError(f"No Debian package owns bundled library {path}")


def prepare_adb(target: str) -> Path:
    key = "windows" if "windows" in target else "macos" if "apple" in target else "linux"
    output = STAGING / "adb" / target
    output.mkdir(parents=True, exist_ok=True)
    if target == "aarch64-unknown-linux-gnu":
        package = download(METADATA["archives"]["linux_arm64"], "android-tools-37.0.0p1.tar.xz")
        source = extract_tar(package, STAGING / "android-tools-source")
        build = STAGING / "android-tools-build" / target
        protoc = adb_protoc(build)
        subprocess.run(["cmake", "-S", str(source), "-B", str(build), "-G", "Ninja",
                        f"-DProtobuf_PROTOC_EXECUTABLE={protoc}",
                        "-DCMAKE_BUILD_TYPE=Release", "-DANDROID_TOOLS_USE_BUNDLED_FMT=ON", "-DANDROID_TOOLS_PATCH_VENDOR=OFF",
                        "-DANDROID_TOOLS_USE_BUNDLED_LIBUSB=ON", "-DANDROID_TOOLS_ADB_ENABLE_MDNS=OFF"], check=True)
        subprocess.run(["cmake", "--build", str(build), "--target", "adb", "--parallel", "4"], check=True)
        binary = next(build.rglob("adb"))
        shutil.copy2(binary, output / "adb")
        dependencies = subprocess.check_output(["ldd", str(binary)], text=True)
        libraries = output / "lib"
        libraries.mkdir(exist_ok=True)
        library_provenance = []
        library_notices = []
        for line in dependencies.splitlines():
            if "=> /" not in line:
                continue
            name, rest = line.split("=>", 1)
            path = Path(rest.strip().split()[0])
            if name.strip() in {"libc.so.6", "libm.so.6", "libpthread.so.0", "libdl.so.2", "librt.so.1"}:
                continue
            staged_library = libraries / path.name
            shutil.copy2(path.resolve(), staged_library)
            subprocess.run(["patchelf", "--set-rpath", "$ORIGIN", str(staged_library)], check=True)
            package = debian_library_package(path)
            version = subprocess.check_output(["dpkg-query", "-W", "-f=${Version}", package], text=True)
            copyright_file = Path("/usr/share/doc") / package.split(":")[0] / "copyright"
            if copyright_file.is_file(): library_notices.append(copyright_file.read_text())
            library_provenance.append({"file": f"lib/{path.name}", "package":package, "version":version, "source":"Ubuntu 22.04 ARM64 source packages: https://archive.ubuntu.com/ubuntu/pool/"})
        subprocess.run(["patchelf", "--set-rpath", "$ORIGIN/lib", str(output / "adb")], check=True)
        notices = [path for path in [source / "LICENSE", source / "vendor/adb/NOTICE", source / "vendor/adb/LICENSE", source / "vendor/libusb/COPYING", source / "vendor/fmt/LICENSE"] if path.is_file()]
        (output / "NOTICE.txt").write_text("\n\n".join([*[path.read_text(errors="replace") for path in notices], *library_notices]), encoding="utf-8")
    else:
        package = download(METADATA["archives"][key], f"platform-tools-37.0.1-{key}.zip")
        files = ["adb.exe", "AdbWinApi.dll", "AdbWinUsbApi.dll", "NOTICE.txt"] if key == "windows" else ["adb", "NOTICE.txt"]
        with zipfile.ZipFile(package) as archive:
            for name in files:
                data = archive.read(f"platform-tools/{name}")
                if name.endswith((".exe", ".dll")):
                    offset = struct.unpack_from("<I", data, 0x3C)[0]
                    if struct.unpack_from("<H", data, offset + 4)[0] != 0x14C:
                        raise ValueError(f"Unexpected official Windows ADB architecture: {name}")
                (output / name).write_bytes(data)
        if key != "windows":
            (output / "adb").chmod(0o755)
    (output / "SOURCE.json").write_text(json.dumps({"target": target, "source": METADATA["archives"].get("linux_arm64" if target == "aarch64-unknown-linux-gnu" else key), "runtime_libraries": library_provenance if target == "aarch64-unknown-linux-gnu" else []}, indent=2) + "\n")
    return output


def prepare_scrcpy() -> Path:
    package = download(METADATA["scrcpy"], "scrcpy-server-v5.0.1")
    output = ROOT / "crates" / "nyaterm-android" / "assets"
    output.mkdir(parents=True, exist_ok=True)
    shutil.copy2(package, output / "scrcpy-server-v5.0.1")
    return output


def prepare_ffmpeg(target: str) -> Path:
    prefix = STAGING / "ffmpeg" / target
    stamp = prefix / "SOURCE.json"
    archive = download(METADATA["ffmpeg"], "ffmpeg-8.0.1.tar.gz")
    source = extract_tar(archive, STAGING / "ffmpeg-source")
    build = STAGING / "ffmpeg-build" / target
    build.mkdir(parents=True, exist_ok=True)
    options = [str(source / "configure"), f"--prefix={prefix}", "--disable-autodetect", "--disable-everything",
               "--enable-avcodec", "--enable-avutil", "--enable-avformat", "--enable-swscale", "--disable-avdevice", "--disable-avfilter", "--disable-swresample", "--enable-decoder=h264",
               "--enable-parser=h264", "--enable-shared", "--disable-static", "--disable-programs",
               "--disable-doc", "--disable-network", "--disable-gpl", "--disable-nonfree", "--disable-x86asm"]
    if "windows" in target:
        options = ["sh", *options, "--toolchain=msvc", "--target-os=win32", "--extra-cflags=-MT",
                   "--arch=aarch64" if target.startswith("aarch64") else "--arch=x86_64"]
        if target.startswith("aarch64"):
            # Release runners execute on x64. Never execute ARM64 configure probes;
            # LLVM assembles FFmpeg's GNU-style ARM64 sources directly to COFF.
            # -MT in extra-cflags also reaches Clang's assembler, where it
            # consumes the following -c as a dependency target. Keep the CRT
            # option on the MSVC compiler commands instead.
            options.remove("--extra-cflags=-MT")
            options.extend(["--cc=cl.exe -MT", "--cxx=cl.exe -MT", "--enable-cross-compile",
                            "--as=clang --target=aarch64-pc-windows-msvc"])
    elif "apple" in target:
        options.extend(["--install-name-dir=@rpath", "--arch=aarch64" if target.startswith("aarch64") else "--arch=x86_64"])
    provenance = {**METADATA["ffmpeg"], "target": target, "configure": [option.replace(str(prefix), "${FFMPEG_DIR}").replace(str(source), "${FFMPEG_SOURCE}") for option in options]}
    if stamp.is_file() and json.loads(stamp.read_text()) == provenance:
        if "windows" in target:
            ensure_windows_import_libraries(prefix, target)
            license_path = prefix / "LICENSE.txt"
            if not license_path.is_file() or license_path.stat().st_size == 0:
                shutil.copy2(source / "COPYING.LGPLv2.1", license_path)
        return prefix
    if prefix.exists():
        shutil.rmtree(prefix)
    if build.exists():
        shutil.rmtree(build)
    build.mkdir(parents=True)
    subprocess.run(options, cwd=build, check=True)
    subprocess.run(["make", "-j4"], cwd=build, check=True)
    subprocess.run(["make", "install"], cwd=build, check=True)
    prefix.mkdir(parents=True, exist_ok=True)
    if "windows" in target:
        ensure_windows_import_libraries(prefix, target)
    shutil.copy2(source / "COPYING.LGPLv2.1", prefix / "LICENSE.txt")
    stamp.write_text(json.dumps(provenance, indent=2) + "\n")
    return prefix


def stage_runtime(adb: Path | None, ffmpeg: Path | None, destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    if adb:
        shutil.copytree(adb, destination / "adb", dirs_exist_ok=True)
    if ffmpeg:
        output = destination / "android-libs"
        if output.exists():
            shutil.rmtree(output)
        output.mkdir(exist_ok=True)
        source = ffmpeg / ("bin" if os.name == "nt" else "lib")
        for path in source.iterdir():
            if re.fullmatch(r"lib(?:avcodec|avformat|avutil|swscale)\.so\.\d+|lib(?:avcodec|avformat|avutil|swscale)\.\d+\.dylib|(?:avcodec|avformat|avutil|swscale)-\d+\.dll", path.name):
                # Windows resolves dependent DLLs before main(), beside the helper.
                shutil.copy2(path.resolve(), (destination if os.name == "nt" else output) / path.name)
        for name in ["SOURCE.json", "LICENSE.txt"]:
            shutil.copy2(ffmpeg / name, output / name)


def write_resource_manifest(destination: Path, target: str) -> list[Path]:
    files = [path for directory in (destination / "adb", destination / "android-libs")
             for path in directory.rglob("*") if path.is_file()]
    if "windows" in target:
        files.extend(destination / f"{name}-{major}.dll"
                     for name, major in WINDOWS_FFMPEG_LIBRARIES.items())
    manifest = {"schema": 1, "target": target, "dependencies": METADATA,
                "files": {path.relative_to(destination).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
                          for path in sorted(files)}}
    output = destination / "android-resources.json"
    output.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return [*files, output]


def prepare(target: str, *, adb_only: bool = False) -> tuple[Path, Path | None]:
    adb = prepare_adb(target)
    prepare_scrcpy()
    ffmpeg = None if adb_only else prepare_ffmpeg(target)
    target_dir = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    for profile in ["debug", "release"]:
        stage_runtime(adb, ffmpeg, target_dir / profile)
        stage_runtime(adb, ffmpeg, target_dir / target / profile)
    return adb, ffmpeg


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", default=subprocess.check_output(["rustc", "-vV"], text=True).split("host: ")[1].splitlines()[0])
    parser.add_argument("--adb-only", action="store_true")
    arguments = parser.parse_args()
    adb, ffmpeg = prepare(arguments.target, adb_only=arguments.adb_only)
    print(f"Prepared bundled ADB at {adb}")
    if ffmpeg:
        print(f"FFMPEG_DIR={ffmpeg}")
