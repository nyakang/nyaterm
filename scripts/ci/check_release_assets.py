#!/usr/bin/env python3
"""Verify a release directory holds exactly the artifacts every target produces.

The per-target contents are already checked by
``scripts/release/verify_native_package.py`` inside each packaging job; this is the
cross-job completeness gate that catches a matrix leg whose upload went missing.
Expected names come from ``package_native`` itself, so adding a target does not mean
remembering to update a count here.

Run from the repository root:
    python3 scripts/ci/check_release_assets.py --dist dist-release --version 2.0.0
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

ROOT_DIR = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT_DIR / "scripts" / "release"))

import package_native  # noqa: E402
import artifact_catalog  # noqa: E402


def expected_artifacts(version: str, *, include_focal: bool = False) -> set[str]:
    return artifact_catalog.expected(version, include_focal=include_focal)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Validate release assets")
    parser.add_argument(
        "--dist",
        type=Path,
        default=Path("dist-release"),
        help="directory holding the downloaded release artifacts",
    )
    parser.add_argument(
        "--version",
        required=True,
        help="release version; must match the Cargo workspace version",
    )
    parser.add_argument(
        "--artifact-version",
        help="optional public filename label, for example main-snapshot",
    )
    parser.add_argument("--include-focal", action="store_true")
    args = parser.parse_args(argv)

    directory = args.dist.resolve()
    if not directory.is_dir():
        print(f"release directory not found: {directory}", file=sys.stderr)
        return 1

    package_native.validate_version(args.version)
    artifact_version = package_native.validate_artifact_version(
        args.artifact_version or args.version
    )
    expected = expected_artifacts(artifact_version, include_focal=args.include_focal)
    actual = {entry.name for entry in directory.iterdir() if entry.is_file()}

    problems = [f"missing artifact: {name}" for name in sorted(expected - actual)]
    problems += [f"unexpected artifact: {name}" for name in sorted(actual - expected)]
    if problems:
        for problem in problems:
            print(f"- {problem}", file=sys.stderr)
        return 1

    print(f"release ok: all {len(expected)} artifacts present in {directory}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
