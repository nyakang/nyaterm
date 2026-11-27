#!/usr/bin/env python3
"""Validate canonical release manifests and optionally probe their assets."""

from __future__ import annotations

import argparse
import json
import urllib.request
from pathlib import Path

import generate_release_metadata


def load_manifest(path: Path) -> dict[str, object]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"release manifest must be an object: {path}")
    return value


def probe_asset(url: str) -> None:
    request = urllib.request.Request(
        url,
        headers={"Range": "bytes=0-0", "User-Agent": "nyaterm-release-verifier"},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        if response.status not in (200, 206):
            raise RuntimeError(f"release asset returned HTTP {response.status}: {url}")
        response.read(1)


def verify(
    directory: Path,
    *,
    version: str,
    tag: str,
    base_url: str,
    check_assets: bool,
) -> set[str]:
    updater = load_manifest(directory / "latest.json")
    downloads = load_manifest(directory / "downloads.json")
    urls = generate_release_metadata.validate_release_manifest(
        updater,
        version=version,
        tag=tag,
        base_url=base_url,
        artifacts=generate_release_metadata.updater_artifacts(version, include_focal="linux-x86_64-deb-glibc231" in updater.get("platforms", {})),
        integrity_field="signature",
    )
    urls.update(
        generate_release_metadata.validate_release_manifest(
            downloads,
            version=version,
            tag=tag,
            base_url=base_url,
            artifacts=generate_release_metadata.download_artifacts(version),
            integrity_field="sha256",
        )
    )
    if check_assets:
        for url in sorted(urls):
            probe_asset(url)
    return urls


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--check-assets", action="store_true")
    args = parser.parse_args()
    urls = verify(
        args.directory,
        version=args.version,
        tag=args.tag,
        base_url=args.base_url,
        check_assets=args.check_assets,
    )
    print(f"Verified release metadata and {len(urls)} unique assets")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
