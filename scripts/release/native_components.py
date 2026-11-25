"""Build, sign and verify immutable official native component packages."""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import zipfile

IDS = ("spice", "rdp", "vnc", "android")
PUBLIC_KEY = Path(__file__).with_name("updater-public-key.txt")
MAX_ARCHIVE = 256 * 1024 * 1024
MAX_PAYLOAD = 1024 * 1024 * 1024


def is_component(path: Path) -> bool:
    return bool(re.search(r"_component_(spice|rdp|vnc|android)\.zip$", path.name))


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def write_package(root: Path, output: Path, component: str, version: str, build: str, target: str) -> None:
    if component not in IDS or not re.fullmatch(r"[0-9a-f]{40}(?:-[0-9]+-[0-9]+)?", build):
        raise ValueError("invalid component identity")
    files = {}
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            raise ValueError("component contains a symlink")
        if path.is_file():
            name = path.relative_to(root).as_posix()
            files[name] = {"size": path.stat().st_size, "sha256": digest(path.read_bytes()),
                           "executable": bool(path.stat().st_mode & 0o111)}
    entry = f"nyaterm-{component}-helper" + (".exe" if "windows" in target else "")
    if entry not in files:
        raise ValueError("missing component helper")
    manifest = {"schema_version": 1, "component": component, "app_version": version,
                "build_id": build, "target": target, "entry": entry, "files": files}
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        archive.writestr("component.json", json.dumps(manifest, sort_keys=True, indent=2).encode() + b"\n")
        for name in files:
            archive.write(root / name, name)


def build_packages(target: str, version: str, label: str, build: str) -> None:
    import package_native as package
    for component in IDS:
        root = package.WORK_DIR / ("component-" + component)
        root.mkdir()
        source = package.release_binary_path(target, "nyaterm-" + component + "-helper")
        shutil.copy2(source, root / source.name)
        if component == "spice":
            package.copy_spice_resources(root, target)
        if component == "android":
            package.copy_android_resources(root, target)
            for name in ("NOTICE.md", "LICENSE.scrcpy"):
                shutil.copy2(package.ROOT_DIR / "crates/nyaterm-android/assets" / name, root / name)
        shutil.copy2(package.LICENSE_PATH, root / "LICENSE")
        if "apple" in target:
            for path in root.rglob("*"):
                if path.is_file() and (path.name == source.name or path.name == "adb" or path.suffix in (".dylib", ".so")):
                    subprocess.run(["codesign", "--force", "--sign", "-", "--timestamp=none", str(path)], check=True)
            for name in ("android-resources.json", "spice/SOURCE.json"):
                path = root / name
                if path.is_file():
                    document = json.loads(path.read_bytes())
                    base = path.parent if name.startswith("spice/") else root
                    document["files"] = {file: digest((base / file).read_bytes()) for file in document["files"]}
                    path.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
        output = package.DIST_DIR / f"NyaTerm_{label}_{package.target_info(target).label}_component_{component}.zip"
        write_package(root, output, component, version, build, target)
        verify(output, signed=False)
        verify_payload(*inspect(output))


def inspect(path: Path) -> tuple[dict, dict[str, bytes]]:
    if path.stat().st_size > MAX_ARCHIVE:
        raise ValueError("component archive exceeds size limit")
    with zipfile.ZipFile(path) as archive:
        entries = archive.infolist()
        if len(entries) > 4098 or sum(entry.file_size for entry in entries) > MAX_PAYLOAD + 1048576 + 8192:
            raise ValueError("component inventory exceeds size limit")
        names = [entry.filename for entry in entries]
        if len({name.lower() for name in names}) != len(names):
            raise ValueError("duplicate component entries")
        for entry in entries:
            name = entry.filename
            if (entry.is_dir() or "\\" in name or ":" in name or name.startswith("/")
                    or any(part in ("", ".", "..") or part.endswith((".", " ")) for part in name.split("/"))
                    or ((entry.external_attr >> 16) & 0o170000) not in (0, 0o100000)):
                raise ValueError("unsafe component entry")
        if archive.getinfo("component.json").file_size > 1048576:
            raise ValueError("component manifest exceeds size limit")
        files = {name: archive.read(name) for name in names}
    manifest = json.loads(files["component.json"])
    if (manifest["schema_version"] != 1 or manifest["component"] not in IDS
            or not re.fullmatch(r"[0-9a-f]{40}(?:-[0-9]+-[0-9]+)?", manifest["build_id"])
            or not re.fullmatch(r"[a-z0-9_-]+", manifest["target"])):
        raise ValueError("invalid component identity")
    expected = manifest["files"]
    if set(files) - {"component.json", "component.minisig"} != set(expected):
        raise ValueError("component inventory mismatch")
    for name, entry in expected.items():
        if len(files[name]) != entry["size"] or digest(files[name]) != entry["sha256"]:
            raise ValueError("component file checksum mismatch")
    suffix = ".exe" if "windows" in manifest["target"] else ""
    if manifest["entry"] != f"nyaterm-{manifest['component']}-helper{suffix}" or manifest["entry"] not in expected:
        raise ValueError("invalid component entrypoint")
    return manifest, files


def verify_payload(manifest: dict, files: dict[str, bytes]) -> None:
    from native_headers import pe_machine, elf_machine, macho_cpu_type
    target = manifest["target"].removesuffix("-glibc231")
    arm = target.startswith("aarch64")
    data = files[manifest["entry"]]
    if "windows" in target:
        actual, expected = pe_machine(data), 0xAA64 if arm else 0x8664
    elif "apple" in target:
        actual, expected = macho_cpu_type(data), 0x0100000C if arm else 0x01000007
    else:
        actual, expected = elf_machine(data), 183 if arm else 62
    if actual != expected:
        raise ValueError("component helper architecture mismatch")
    if manifest["component"] == "spice":
        import spice_release
        spice_release.verify_resources(files.__getitem__, target, names=files)
    elif manifest["component"] == "android":
        from verify_native_package import verify_android_resources
        verify_android_resources(files.__getitem__, target)


def verify(path: Path, *, signed: bool = True) -> dict:
    manifest, files = inspect(path)
    if signed:
        verify_payload(manifest, files)
        with tempfile.TemporaryDirectory(prefix="nyaterm-component-verify-") as temporary:
            root = Path(temporary)
            (root / "manifest").write_bytes(files["component.json"])
            (root / "signature").write_bytes(files["component.minisig"])
            subprocess.run(["minisign", "-Vm", str(root / "manifest"), "-p", str(PUBLIC_KEY), "-x", str(root / "signature")], check=True)
    return manifest


def provenance_hash(path: Path) -> str:
    # Signing may add only component.minisig. Every build-produced byte stays bound
    # to the original receipt, independent of ZIP timestamps or recompression.
    _, files = inspect(path)
    hashes = {name: digest(data) for name, data in sorted(files.items()) if name != "component.minisig"}
    return digest(json.dumps(hashes, sort_keys=True, separators=(",", ":")).encode())


def sign(path: Path) -> None:
    _, files = inspect(path)
    if "component.minisig" in files:
        verify(path)
        return
    with tempfile.TemporaryDirectory(prefix="nyaterm-component-sign-") as temporary:
        root = Path(temporary)
        manifest = root / "component.json"
        manifest.write_bytes(files["component.json"])
        result = subprocess.run(["npx", "--yes", "@tauri-apps/cli@2.10.0", "signer", "sign", "--password",
                                 os.environ["TAURI_SIGNING_PRIVATE_KEY_PASSWORD"], str(manifest)], capture_output=True)
        if result.returncode:
            raise RuntimeError("Native component signing failed; signer diagnostics are withheld to protect credentials")
        files["component.minisig"] = base64.b64decode(manifest.with_suffix(".json.sig").read_bytes().strip(), validate=True)
        replacement = root / "signed.zip"
        with zipfile.ZipFile(replacement, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            for name, data in sorted(files.items()):
                archive.writestr(name, data)
        verify(replacement)
        shutil.copyfile(replacement, path)


def packages_in(directory: Path) -> list[Path]:
    paths = sorted(directory.glob("*_component_*.zip"))
    if not paths:
        raise ValueError("no native component packages found")
    return paths


def publish(store, directory: Path) -> None:
    for path in packages_in(directory):
        manifest = verify(path)
        key = f"components/{manifest['build_id']}/{manifest['target']}/{manifest['component']}.zip"
        store.put(key, path, immutable=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("sign", "verify", "publish"))
    parser.add_argument("--dist", type=Path, required=True)
    args = parser.parse_args()
    if args.operation == "publish":
        from release_io import R2
        publish(R2(), args.dist)
    else:
        for path in packages_in(args.dist):
            (sign if args.operation == "sign" else verify)(path)


if __name__ == "__main__":
    main()
