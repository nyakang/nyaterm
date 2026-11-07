#!/usr/bin/env python3
"""Generate deterministic release checksums and download/update manifests."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path
from urllib.parse import urlsplit

import package_native
import artifact_catalog


DOWNLOAD_ARTIFACTS = {
    "windows-x86_64": "NyaTerm_{version}_windows_x64-setup.exe",
    "windows-aarch64": "NyaTerm_{version}_windows_arm64-setup.exe",
    "windows-x86_64-portable": "NyaTerm_{version}_windows_x64_portable.zip",
    "windows-aarch64-portable": "NyaTerm_{version}_windows_arm64_portable.zip",
    "linux-x86_64": "NyaTerm_{version}_linux_x64.AppImage",
    "linux-aarch64": "NyaTerm_{version}_linux_arm64.AppImage",
    "darwin-x86_64": "NyaTerm_{version}_macos_x64.dmg",
    "darwin-aarch64": "NyaTerm_{version}_macos_arm64.dmg",
}

UPDATER_ARTIFACTS = {
    "darwin-x86_64": "NyaTerm_{version}_macos_x64.app.tar.gz",
    "darwin-aarch64": "NyaTerm_{version}_macos_arm64.app.tar.gz",
    "linux-x86_64": "NyaTerm_{version}_linux_x64.AppImage",
    "linux-aarch64": "NyaTerm_{version}_linux_arm64.AppImage",
    "windows-x86_64": "NyaTerm_{version}_windows_x64-setup.exe",
    "windows-x86_64-nsis": "NyaTerm_{version}_windows_x64-setup.exe",
    "windows-aarch64": "NyaTerm_{version}_windows_arm64-setup.exe",
    "windows-aarch64-nsis": "NyaTerm_{version}_windows_arm64-setup.exe",
    "windows-x86_64-portable": "NyaTerm_{version}_windows_x64_portable.zip",
    "windows-aarch64-portable": "NyaTerm_{version}_windows_arm64_portable.zip",
}

MSI_ARTIFACTS = {
    "windows-x86_64-msi": "NyaTerm_{version}_windows_x64.msi",
    "windows-aarch64-msi": "NyaTerm_{version}_windows_arm64.msi",
}


def download_artifacts(version: str) -> dict[str, str]:
    msi = MSI_ARTIFACTS if package_native.includes_windows_msi(version) else {}
    return {**DOWNLOAD_ARTIFACTS, **msi}


def updater_artifacts(version: str) -> dict[str, str]:
    msi = MSI_ARTIFACTS if package_native.includes_windows_msi(version) else {}
    return {**UPDATER_ARTIFACTS, **msi}


CHANNEL_MANIFEST_NAMES = ("latest.json", "downloads.json")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def expected_artifacts(version: str, *, include_focal: bool = False) -> set[str]:
    return artifact_catalog.expected(version, include_focal=include_focal)


def canonical_base_url(base_url: str) -> str:
    parsed = urlsplit(base_url)
    if (
        parsed.scheme != "https"
        or parsed.hostname != "downloads.nyaterm.app"
        or parsed.port is not None
        or parsed.username is not None
        or parsed.password is not None
        or parsed.path not in ("", "/")
        or parsed.query
        or parsed.fragment
    ):
        raise ValueError(
            "release base URL must be the canonical origin "
            "https://downloads.nyaterm.app"
        )
    return "https://downloads.nyaterm.app"


def artifact_url(base_url: str, tag: str, filename: str) -> str:
    return f"{canonical_base_url(base_url)}/releases/{tag}/{filename}"


def validate_release_manifest(
    manifest: dict[str, object],
    *,
    version: str,
    tag: str,
    base_url: str,
    artifacts: dict[str, str],
    integrity_field: str,
) -> set[str]:
    version = package_native.validate_version(version)
    base_url = canonical_base_url(base_url)
    if tag != f"v{version}":
        raise ValueError(f"release tag {tag} does not match version {version}")
    if manifest.get("version") != version:
        raise ValueError("release manifest version does not match the release")
    platforms = manifest.get("platforms")
    if not isinstance(platforms, dict) or set(platforms) != set(artifacts):
        raise ValueError("release manifest platform table is incomplete")

    urls: set[str] = set()
    for platform, template in artifacts.items():
        entry = platforms.get(platform)
        if not isinstance(entry, dict):
            raise ValueError(f"invalid release manifest entry: {platform}")
        filename = template.format(version=version)
        expected_url = artifact_url(base_url, tag, filename)
        if entry.get("url") != expected_url:
            raise ValueError(f"noncanonical release URL for {platform}")
        integrity = entry.get(integrity_field)
        if not isinstance(integrity, str) or not integrity.strip():
            raise ValueError(f"missing {integrity_field} for {platform}")
        urls.add(expected_url)
    return urls


def channel_manifest_destinations(version: str) -> dict[str, str]:
    version = package_native.validate_version(version)
    prefix = "channels/preview/" if package_native.parse_version(version).group("pre") else ""
    return {name: f"{prefix}{name}" for name in CHANNEL_MANIFEST_NAMES}


def generate(
    directory: Path,
    *,
    version: str,
    tag: str,
    base_url: str,
    notes: str,
    pub_date: str,
    include_focal: bool = False,
) -> tuple[dict[str, object], dict[str, object]]:
    version = package_native.validate_version(version)
    base_url = canonical_base_url(base_url)
    if tag != f"v{version}":
        raise ValueError(f"release tag {tag} does not match version {version}")

    expected = expected_artifacts(version, include_focal=include_focal)
    missing = sorted(name for name in expected if not (directory / name).is_file())
    if missing:
        raise RuntimeError(f"missing release artifacts: {', '.join(missing)}")

    hashes = {name: sha256(directory / name) for name in sorted(expected)}
    checksum_text = "".join(
        f"{digest}  {name}\n" for name, digest in hashes.items()
    )
    (directory / "SHA256SUMS").write_text(checksum_text, encoding="utf-8")

    downloads: dict[str, dict[str, str]] = {}
    for platform, template in download_artifacts(version).items():
        filename = template.format(version=version)
        downloads[platform] = {
            "url": artifact_url(base_url, tag, filename),
            "sha256": hashes[filename],
        }

    updater: dict[str, dict[str, str]] = {}
    signature_cache: dict[str, str] = {}
    for platform, template in updater_artifacts(version).items():
        filename = template.format(version=version)
        if filename not in signature_cache:
            signature_path = directory / f"{filename}.sig"
            if not signature_path.is_file():
                raise RuntimeError(f"missing updater signature: {signature_path.name}")
            signature = signature_path.read_text(encoding="utf-8").strip()
            if not signature:
                raise RuntimeError(f"empty updater signature: {signature_path.name}")
            signature_cache[filename] = signature
        updater[platform] = {
            "url": artifact_url(base_url, tag, filename),
            "signature": signature_cache[filename],
        }

    downloads_manifest: dict[str, object] = {
        "version": version,
        "notes": notes,
        "pub_date": pub_date,
        "platforms": downloads,
    }
    updater_manifest: dict[str, object] = {
        "version": version,
        "notes": notes,
        "pub_date": pub_date,
        "platforms": updater,
    }
    for name, manifest in (
        ("downloads.json", downloads_manifest),
        ("latest.json", updater_manifest),
    ):
        (directory / name).write_text(
            json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
    return downloads_manifest, updater_manifest


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dist", type=Path, default=Path("dist-release"))
    parser.add_argument("--version", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--notes-file", type=Path, required=True)
    parser.add_argument("--pub-date", required=True)
    parser.add_argument("--include-focal", action="store_true")
    args = parser.parse_args(argv)

    directory = args.dist.resolve()
    if not directory.is_dir():
        print(f"release directory not found: {directory}", file=sys.stderr)
        return 1
    generate(
        directory,
        version=args.version,
        tag=args.tag,
        base_url=args.base_url,
        notes=args.notes_file.read_text(encoding="utf-8"),
        pub_date=args.pub_date,
        include_focal=args.include_focal,
    )
    print(f"Generated SHA256SUMS, downloads.json and latest.json in {directory}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
