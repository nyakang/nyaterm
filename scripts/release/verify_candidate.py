"""Verify a complete signed candidate without rebuilding or resigning it."""

from __future__ import annotations

import argparse
import base64
import json
from pathlib import Path
import subprocess
import tempfile

import generate_release_metadata
import release_channel
import release_provenance


PUBLIC_KEY_PATH = Path(__file__).with_name("updater-public-key.txt")


def verify(directory: Path, records: Path, *, version: str, source_sha: str, run_id: str) -> None:
    release_provenance.verify_v2(directory, records, source_sha, version, run_id, include_focal=True)
    release_channel.validate_candidate(directory, version)
    manifest = json.loads((directory / "latest.json").read_text(encoding="utf-8"))
    with tempfile.TemporaryDirectory(prefix="nyaterm-candidate-signatures-") as folder:
        public = Path(folder) / "public.key"
        public.write_bytes(PUBLIC_KEY_PATH.read_bytes())
        for platform, template in generate_release_metadata.updater_artifacts(version).items():
            name = template.format(version=version)
            signature = Path(folder) / "signature"
            signature.write_bytes(base64.b64decode(manifest["platforms"][platform]["signature"], validate=True))
            subprocess.run(["minisign", "-Vm", str(directory / name), "-p", str(public), "-x", str(signature)], check=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dist", type=Path, default=Path("dist-release"))
    parser.add_argument("--records", type=Path, default=Path("provenance"))
    parser.add_argument("--version", required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--run-id", required=True)
    args = parser.parse_args()
    verify(args.dist, args.records, version=args.version, source_sha=args.source_sha, run_id=args.run_id)


if __name__ == "__main__":
    main()
