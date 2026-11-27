#!/usr/bin/env python3
"""Verify the base64 minisign signatures used by the legacy updater."""

from __future__ import annotations

import argparse
import base64
import subprocess
import tempfile
from pathlib import Path

import generate_release_metadata


def verify_file(artifact: Path, public_key: Path) -> None:
    signature = base64.b64decode(
        artifact.with_name(artifact.name + ".sig").read_text(encoding="utf-8").strip(),
        validate=True,
    )
    with tempfile.TemporaryDirectory(prefix="nyaterm-signature-") as directory:
        decoded = Path(directory) / "signature.minisig"
        decoded.write_bytes(signature)
        subprocess.run(
            ["minisign", "-Vm", str(artifact), "-p", str(public_key), "-x", str(decoded)],
            check=True,
        )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dist", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--public-key", type=Path, required=True)
    parser.add_argument("--include-focal", action="store_true")
    args = parser.parse_args()
    for name in sorted({
        template.format(version=args.version)
        for template in generate_release_metadata.updater_artifacts(args.version, include_focal=args.include_focal).values()
    }):
        verify_file(args.dist / name, args.public_key)


if __name__ == "__main__":
    main()
