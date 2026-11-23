"""Verify SPICE resources as read from any native release archive."""
from __future__ import annotations

import hashlib
import json
from pathlib import PurePosixPath
import re
import subprocess

import prepare_spice
import windows_portable
from native_headers import elf_machine, macho_cpu_type, pe_machine, verify_macos_minimum_os


def linux_glibc_requirement(runtime) -> str:
    """The private closure bundles every ELF dependency except glibc/its loader."""
    manifest = json.loads((runtime / "SOURCE.json").read_bytes())
    libraries = [runtime / name for name in manifest["files"]
                 if not name.startswith("licenses/") and (name.endswith(".so") or ".so." in name)]
    if not libraries:
        raise RuntimeError("SPICE ELF runtime is empty")
    output = subprocess.check_output(["readelf", "--version-info", "--wide", *map(str, libraries)], text=True)
    versions = re.findall(r"Name: GLIBC_([\w.]+)", output)
    if not versions or any(not re.fullmatch(r"\d+(?:\.\d+)+", version) for version in versions):
        raise RuntimeError("SPICE runtime has missing or private glibc requirements")
    return ".".join(map(str, max(tuple(map(int, version.split("."))) for version in versions)))


def locked_files(target: str) -> dict[str, str]:
    entry = json.loads(prepare_spice.LOCK_PATH.read_text(encoding="utf-8"))["targets"][target]
    return {name: item["sha256"] for name, item in entry["files"].items()}


def verify_resources(read, target: str, *, names=None) -> None:
    try:
        manifest = json.loads(read("spice/SOURCE.json"))
    except (KeyError, OSError, ValueError) as error:
        raise RuntimeError("SPICE resource manifest is missing or invalid") from error
    sources = json.loads(prepare_spice.SOURCES_PATH.read_text(encoding="utf-8"))
    if (manifest.get("schema") != 1 or manifest.get("target") != target
            or manifest.get("sources_sha256") != prepare_spice.sha256(prepare_spice.SOURCES_PATH.read_bytes())):
        raise RuntimeError("SPICE resource provenance does not match pinned sources")
    files = manifest.get("files")
    if not isinstance(files, dict) or not 1 <= len(files) <= 768:
        raise RuntimeError("Invalid SPICE resource inventory")
    if names is not None:
        actual = {name[len("spice/"):] for name in names if name.startswith("spice/")}
        if actual != set(files) | {"SOURCE.json"}:
            raise RuntimeError("SPICE artifact contains missing or unexpected runtime files")
    windows = "windows" in target
    macos = "apple" in target
    if windows:
        entry = json.loads(prepare_spice.LOCK_PATH.read_text(encoding="utf-8"))["targets"][target]
        if (manifest.get("lock_sha256") != prepare_spice.sha256(prepare_spice.LOCK_PATH.read_bytes())
                or manifest.get("packages") != entry["packages"]):
            raise RuntimeError("SPICE resource provenance does not match pinned Windows packages")
        required = {name[len("spice/"):] for name in windows_portable.LAYOUT["spice_files"][target]} - {"SOURCE.json"}
        if files.keys() != required:
            raise RuntimeError("SPICE resource inventory differs from the portable contract")
        for name, digest in locked_files(target).items():
            if files.get(name) != digest:
                raise RuntimeError("SPICE resource differs from locked package: " + name)
    else:
        if manifest.get("native_source") != sources["unix_git"]:
            raise RuntimeError("SPICE native source revision mismatch")
        windows_entry = json.loads(prepare_spice.LOCK_PATH.read_text(encoding="utf-8"))["targets"]["x86_64-pc-windows-msvc"]
        ca = windows_entry["files"]["ca-bundle.pem"]
        if (manifest.get("ca_package") != windows_entry["packages"][ca["package"]]
                or files.get("ca-bundle.pem") != locked_files("x86_64-pc-windows-msvc")["ca-bundle.pem"]):
            raise RuntimeError("SPICE CA snapshot differs from the reviewed package")
        required = {"ca-bundle.pem", "NOTICE.txt", "openssl.cnf"}
        required |= {f"lib{name}.0.dylib" if macos else f"lib{name}.so.0"
                     for name in ("glib-2.0", "gobject-2.0", "gio-2.0", "gstreamer-1.0")}
        required.add("libspice-client-glib-2.0.8.dylib" if macos else "libspice-client-glib-2.0.so.8")
        suffix = ".dylib" if macos else ".so"
        for plugin in prepare_spice.VIDEO_PLUGINS:
            name = f"gstreamer-1.0/libgst{plugin}{suffix}"
            if plugin == "videoconvertscale" and name not in files:
                required |= {f"gstreamer-1.0/libgst{part}{suffix}" for part in ("videoconvert", "videoscale")}
            else:
                required.add(name)
        if not required <= files.keys():
            raise RuntimeError("SPICE resource inventory is incomplete")
        for plugin in prepare_spice.SASL_PLUGINS:
            if not any(name.startswith(f"sasl2/lib{plugin}.") for name in files):
                raise RuntimeError("SPICE SASL plugin is missing: " + plugin)
    for name, digest in files.items():
        try:
            prepare_spice.safe_relative(name)
        except ValueError as error:
            raise RuntimeError("Unsafe SPICE resource path") from error
        path = PurePosixPath(name)
        if len(path.parts) > 1 and path.parts[0] not in ("gstreamer-1.0", "sasl2", "licenses"):
            raise RuntimeError("Unexpected SPICE resource directory")
        data = read("spice/" + name)
        if hashlib.sha256(data).hexdigest() != digest:
            raise RuntimeError("SPICE resource checksum mismatch: " + name)
        if name.endswith(".dll"):
            if not windows or pe_machine(data) != (0xAA64 if target.startswith("aarch64") else 0x8664):
                raise RuntimeError("SPICE resource PE architecture mismatch: " + name)
        elif not name.startswith("licenses/") and (name.endswith((".dylib", ".so")) or ".so." in name):
            actual = macho_cpu_type(data) if macos else elf_machine(data)
            expected = (0x0100000C if target.startswith("aarch64") else 0x01000007) if macos else (183 if target.startswith("aarch64") else 62)
            if windows or actual != expected:
                raise RuntimeError("SPICE resource native architecture mismatch: " + name)
            if macos:
                verify_macos_minimum_os(data)
