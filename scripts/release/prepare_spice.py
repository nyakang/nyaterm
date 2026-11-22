#!/usr/bin/env python3
"""Prepare the private SPICE runtime from reviewed, SHA-256 pinned archives.

Normal builds never query a rolling repository. --refresh-windows-lock is an
explicit maintainer operation; review the resulting lock before committing it.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import io
import json
import os
import re
from pathlib import Path, PurePosixPath
import shutil
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
STAGING = Path(os.environ.get("NYATERM_SPICE_DEPS_DIR", ROOT / "target/spice-deps")).resolve()
LOCK_PATH = Path(__file__).with_name("spice-windows-lock.json")
SOURCES_PATH = Path(__file__).with_name("spice-sources.json")
WINDOWS_TARGETS = {
    "x86_64-pc-windows-msvc": ("clang64", 0x8664),
    "aarch64-pc-windows-msvc": ("clangarm64", 0xAA64),
}
VIDEO_PLUGINS = (
    "coreelements", "app", "playback", "typefindfunctions", "videoconvertscale",
    "jpeg", "vpx", "openh264", "videoparsersbad",
)
SASL_PLUGINS = ("anonymous", "crammd5", "digestmd5", "plain", "scram")
# OS components only. An unknown import must resolve to a pinned archive;
# checking the build host's System32 would hide accidentally omitted DLLs.
WINDOWS_SYSTEM_DLLS = frozenset(name + ".dll" for name in (
    "advapi32", "avrt", "bcrypt", "bcryptprimitives", "cabinet", "cfgmgr32",
    "comctl32", "comdlg32", "crypt32", "cryptbase", "cryptnet", "d2d1",
    "d3d11", "d3d12", "d3d9", "dbghelp", "dnsapi", "dsound", "dwmapi",
    "dwrite", "dxgi", "gdi32", "hid", "imm32", "iphlpapi", "kernel32",
    "ksuser", "mf", "mfplat", "mfreadwrite", "mfuuid", "mpr", "msimg32",
    "msvcrt", "mswsock", "ncrypt", "netapi32", "normaliz", "ntdll", "ole32", "oleacc",
    "oleaut32", "opengl32", "powrprof", "propsys", "psapi", "rpcrt4",
    "secur32", "setupapi", "shell32", "shlwapi", "strmiids", "ucrtbase",
    "user32", "userenv", "usp10", "uxtheme", "version", "winhttp",
    "wininet", "winmm", "winspool", "wintrust", "wldap32", "ws2_32", "wtsapi32",
))


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def safe_relative(name: str) -> PurePosixPath:
    path = PurePosixPath(name)
    if not name or "\\" in name or ":" in name or path.is_absolute() or any(
        part in (".", "..", "") for part in name.split("/")
    ):
        raise ValueError("Unsafe SPICE runtime path")
    return path


def download(url: str, name: str, expected: str | None) -> Path:
    safe_relative(name)
    if "/" in name:
        raise ValueError("Archive name must be a basename")
    path = STAGING / "downloads" / name
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.exists():
        with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as output:
            partial = Path(output.name)
            try:
                with urllib.request.urlopen(url, timeout=90) as response:
                    shutil.copyfileobj(response, output)
            except BaseException:
                output.close()
                partial.unlink(missing_ok=True)
                raise
        if expected and sha256(partial.read_bytes()) != expected:
            partial.unlink()
            raise ValueError("SPICE archive checksum mismatch: " + name)
        partial.replace(path)
    if expected and sha256(path.read_bytes()) != expected:
        raise ValueError("SPICE cached archive checksum mismatch: " + name)
    return path


def archive(path: Path):
    import zstandard
    return tarfile.open(fileobj=io.BytesIO(
        zstandard.ZstdDecompressor().stream_reader(io.BytesIO(path.read_bytes())).read()
    ))


def pe_imports(data: bytes, machine: int) -> list[str]:
    import pefile
    with pefile.PE(data=data, fast_load=True) as image:
        if image.FILE_HEADER.Machine != machine:
            raise ValueError("Wrong SPICE DLL architecture")
        image.parse_data_directories(directories=[
            pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"],
            pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT"],
        ])
        return sorted({entry.dll.decode("ascii").lower()
                       for attribute in ("DIRECTORY_ENTRY_IMPORT", "DIRECTORY_ENTRY_DELAY_IMPORT")
                       for entry in getattr(image, attribute, [])})


def system_dll(name: str) -> bool:
    return name in WINDOWS_SYSTEM_DLLS or name.startswith(("api-ms-win-", "ext-ms-win-"))


def refresh_windows_target(target: str) -> dict:
    prefix, machine = WINDOWS_TARGETS[target]
    base = f"https://repo.msys2.org/mingw/{prefix}/"
    # Deliberate refresh: an index is evidence for this lock operation, never a
    # build input. Package bytes must match its independently listed digest.
    with urllib.request.urlopen(base + prefix + ".files", timeout=90) as response:
        index_bytes = response.read()
    index = STAGING / (prefix + ".files")
    index.parent.mkdir(parents=True, exist_ok=True)
    index.write_bytes(index_bytes)
    records, owners = {}, {}
    with archive(index) as handle:
        for member in handle:
            if member.name.endswith("/desc"):
                fields = handle.extractfile(member).read().decode().strip().split("\n\n")
                records[member.name.rsplit("/", 1)[0]] = {
                    field.splitlines()[0].strip("%"): field.splitlines()[1:] for field in fields
                }
            elif member.name.endswith("/files"):
                for filename in handle.extractfile(member).read().decode().splitlines():
                    if filename.startswith(prefix + "/"):
                        owners[filename.lower()] = member.name.rsplit("/", 1)[0]

    desired = {prefix + "/bin/" + name: name for name in (
        "libspice-client-glib-2.0-8.dll", "libglib-2.0-0.dll",
        "libgobject-2.0-0.dll", "libgio-2.0-0.dll",
    )}
    desired.update({f"{prefix}/lib/gstreamer-1.0/libgst{name}.dll":
                    f"gstreamer-1.0/libgst{name}.dll" for name in VIDEO_PLUGINS})
    desired.update({f"{prefix}/lib/sasl2/lib{name}-3.dll":
                    f"sasl2/lib{name}-3.dll" for name in SASL_PLUGINS})
    desired[f"{prefix}/etc/ssl/certs/ca-bundle.crt"] = "ca-bundle.pem"
    acquired, packages, output, seen = {}, {}, {}, set()

    def acquire(key):
        record = records[key]
        name, digest = record["FILENAME"][0], record["SHA256SUM"][0]
        path = download(base + name, name, digest)
        files = {}
        with archive(path) as handle:
            for member in handle:
                if (member.isfile() or member.issym() or member.islnk()) and member.name.startswith(prefix + "/"):
                    if member.name.lower().endswith((".dll", ".pem", ".crt")) or "/share/licenses/" in member.name:
                        # Resolve links within the archive; nothing is extracted
                        # through a package-supplied filesystem path.
                        files[member.name.lower()] = (member.name, handle.extractfile(member).read())
        return key, files, {
            "archive": name, "url": base + name, "sha256": digest,
            "name": record["NAME"][0], "version": record["VERSION"][0],
            "licenses": record.get("LICENSE", []), "upstream": record.get("URL", [""])[0],
            "recipe": "https://github.com/msys2/MINGW-packages/tree/master/" + record["BASE"][0],
        }

    while desired:
        needed = {owners[name.lower()] for name in desired if name.lower() not in seen}
        with ThreadPoolExecutor(max_workers=6) as pool:
            for key, files, package in pool.map(acquire, sorted(needed - acquired.keys())):
                acquired[key], packages[key] = files, package
                print("Verified", package["name"], package["version"], flush=True)
                for original, data in files.values():
                    if "/share/licenses/" in original:
                        destination = "licenses/" + original.split("/share/licenses/", 1)[1]
                        output[destination] = {"package": key, "member": original, "sha256": sha256(data)}
        batch, desired = desired, {}
        for member, destination in sorted(batch.items()):
            if member.lower() in seen:
                continue
            seen.add(member.lower())
            key = owners[member.lower()]
            original, data = acquired[key][member.lower()]
            output[destination] = {"package": key, "member": original, "sha256": sha256(data)}
            if destination.endswith(".dll"):
                for imported in pe_imports(data, machine):
                    if system_dll(imported):
                        continue
                    dependency = f"{prefix}/bin/{imported}"
                    if dependency.lower() not in seen:
                        if dependency.lower() not in owners:
                            raise ValueError("Unresolved SPICE import: " + imported)
                        desired[dependency] = imported
    return {"repository": base, "index_sha256": sha256(index_bytes),
            "machine": machine, "packages": dict(sorted(packages.items())),
            "files": dict(sorted(output.items()))}


def verify_runtime(directory: Path, target: str) -> dict:
    manifest = json.loads((directory / "SOURCE.json").read_text(encoding="utf-8"))
    if manifest.get("schema") != 1 or manifest.get("target") != target:
        raise ValueError("SPICE runtime target/schema mismatch")
    if manifest.get("sources_sha256") != sha256(SOURCES_PATH.read_bytes()):
        raise ValueError("SPICE runtime provenance does not match the reviewed lock")
    if target in WINDOWS_TARGETS:
        entry = json.loads(LOCK_PATH.read_text(encoding="utf-8"))["targets"][target]
        if (manifest.get("lock_sha256") != sha256(LOCK_PATH.read_bytes())
                or manifest.get("packages") != entry["packages"]):
            raise ValueError("SPICE runtime provenance does not match the reviewed lock")
        for name, item in entry["files"].items():
            if manifest["files"].get(name) != item["sha256"]:
                raise ValueError("SPICE runtime differs from a locked package member")
    for name, digest in manifest["files"].items():
        safe_relative(name)
        path = directory / name
        if (any(p.is_symlink() for p in [path, *path.parents] if p != directory.parent)
                or not path.is_file() or sha256(path.read_bytes()) != digest):
            raise ValueError("SPICE runtime missing or modified file: " + name)
        if (target.endswith("apple-darwin") and not name.startswith("licenses/")
                and (name.endswith((".dylib", ".so")) or ".so." in name)):
            from spice_unix import check_architecture
            check_architecture(path, target)
    actual = {p.relative_to(directory).as_posix() for p in directory.rglob("*") if p.is_file()}
    if actual != set(manifest["files"]) | {"SOURCE.json"}:
        raise ValueError("Unexpected file in private SPICE runtime")
    return manifest


def source_notices(staging: Path, entry: dict, required_sources=("spice-gtk",)) -> dict:
    sources = json.loads(SOURCES_PATH.read_text(encoding="utf-8"))["sources"]
    required = set(required_sources)
    for key, package in entry["packages"].items():
        if any(name.startswith("licenses/") and item["package"] == key for name, item in entry["files"].items()):
            continue
        name = re.sub(r"^mingw-w64-clang-(?:x86_64|aarch64)-", "", package["name"])
        name = "gst-plugins-bad" if name == "gst-plugins-bad-libs" else name
        if name not in sources or sources[name]["version"] != package["version"].rsplit("-", 1)[0]:
            raise ValueError("Missing matching license source for " + package["name"])
        required.add(name)

    def copy(name):
        metadata = sources[name]
        path = download(metadata["url"], metadata["url"].rsplit("/", 1)[-1], metadata["sha256"])
        count = 0
        with tarfile.open(path) as handle:
            for member in handle:
                basename = PurePosixPath(member.name).name.lower()
                if member.isfile() and (basename.startswith(("copying", "license", "copyright"))
                                         or basename in ("authors", "patents")
                                         or "/licenses/" in member.name.lower()):
                    relative = safe_relative(member.name)
                    destination = staging / "licenses/sources" / name / relative
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    destination.write_bytes(handle.extractfile(member).read())
                    count += 1
        if not count:
            raise ValueError("No license notices in " + name)
    with ThreadPoolExecutor(max_workers=6) as pool:
        list(pool.map(copy, sorted(required)))
    return {name: sources[name] for name in sorted(required)}


def prepare_windows(target: str) -> Path:
    lock = json.loads(LOCK_PATH.read_text(encoding="utf-8"))
    entry = lock["targets"][target]
    # New recipes get separate caches; never overwrite a usable older runtime.
    fingerprint = sha256(LOCK_PATH.read_bytes() + SOURCES_PATH.read_bytes() + Path(__file__).read_bytes())
    output = STAGING / "runtime" / fingerprint[:16] / target
    if output.exists():
        manifest = verify_runtime(output, target)
        if manifest.get("lock_sha256") == sha256(LOCK_PATH.read_bytes()):
            return output
        raise ValueError("SPICE runtime cache uses another lock; use a fresh NYATERM_SPICE_DEPS_DIR")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".prepare-", dir=output.parent) as temporary:
        staging = Path(temporary) / "spice"
        staging.mkdir()
        wanted = {}
        for destination, item in entry["files"].items():
            safe_relative(destination)
            wanted.setdefault(item["package"], {})[item["member"]] = (destination, item["sha256"])

        def extract(key):
            package = entry["packages"][key]
            path = download(package["url"], package["archive"], package["sha256"])
            with archive(path) as handle:
                for member_name, (destination, digest) in wanted[key].items():
                    data = handle.extractfile(member_name).read()
                    if sha256(data) != digest:
                        raise ValueError("SPICE package member checksum mismatch")
                    path = staging / destination
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(data)
        with ThreadPoolExecutor(max_workers=6) as pool:
            list(pool.map(extract, sorted(wanted)))
        sources = source_notices(staging, entry)
        (staging / "openssl.cnf").write_text(
            "# Use only OpenSSL's builtin default provider.\n", encoding="utf-8")
        (staging / "NOTICE.txt").write_text(
            "NyaTerm SPICE native runtime\n\n"
            "Libraries are dynamically loaded from this directory and can be replaced\n"
            "with ABI-compatible builds. SOURCE.json records archive hashes, licenses,\n"
            "upstream projects and MSYS2 build recipes. License texts are in licenses/.\n"
            "The CA bundle is a release-pinned trust snapshot, not the Windows trust store.\n"
            "spice-gtk source: https://www.spice-space.org/download/gtk/spice-gtk-0.43.tar.xz\n",
            encoding="utf-8")
        manifest = {"schema": 1, "target": target, "lock_sha256": sha256(LOCK_PATH.read_bytes()),
                    "sources_sha256": sha256(SOURCES_PATH.read_bytes()), "sources": sources,
                    "packages": entry["packages"],
                    "files": {p.relative_to(staging).as_posix(): sha256(p.read_bytes())
                              for p in sorted(staging.rglob("*")) if p.is_file()}}
        (staging / "SOURCE.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        verify_runtime(staging, target)
        staging.rename(output)
    return output


def main() -> None:
    from spice_unix import TARGETS as UNIX_TARGETS
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=(*WINDOWS_TARGETS, *UNIX_TARGETS), default="x86_64-pc-windows-msvc")
    parser.add_argument("--refresh-windows-lock", action="store_true")
    parser.add_argument("--write-portable-contract", action="store_true",
                        help="Maintainer operation: update the shared Windows SPICE inventory from both verified runtimes")
    args = parser.parse_args()
    if args.refresh_windows_lock and args.write_portable_contract:
        parser.error("Review the refreshed dependency lock before updating the portable contract")
    if args.refresh_windows_lock:
        lock = {"schema": 1, "targets": {target: refresh_windows_target(target) for target in WINDOWS_TARGETS}}
        LOCK_PATH.write_text(json.dumps(lock, indent=2) + "\n", encoding="utf-8")
    elif args.write_portable_contract:
        contract = ROOT / "crates/nyaterm-core/src/packaging/windows-portable.json"
        data = json.loads(contract.read_text(encoding="utf-8"))
        inventories = {}
        for target in WINDOWS_TARGETS:
            runtime = prepare_windows(target)
            manifest = verify_runtime(runtime, target)
            inventories[target] = sorted("spice/" + name for name in [*manifest["files"], "SOURCE.json"])
        data["spice_files"] = inventories
        if "nyaterm-spice-helper.exe" not in data["helper_files"]:
            data["helper_files"].append("nyaterm-spice-helper.exe")
        contract.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    else:
        print("NYATERM_SPICE_RUNTIME=" + str(prepare(args.target)))


def prepare(target: str) -> Path:
    if target in WINDOWS_TARGETS:
        return prepare_windows(target)
    from spice_unix import prepare as prepare_unix
    return prepare_unix(target)


if __name__ == "__main__":
    main()
