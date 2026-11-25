#!/usr/bin/env python3
"""Bind package artifacts to the actual checkout, version and producing run."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from pathlib import Path

import generate_release_metadata
import package_native
import artifact_catalog


def identity(source_sha: str, version: str, run_id: str) -> dict[str, object]:
    if not re.fullmatch(r"[0-9a-f]{40}", source_sha):
        raise ValueError("source SHA must be a full commit SHA")
    if not re.fullmatch(r"[1-9][0-9]*", run_id):
        raise ValueError("run ID must be a positive integer")
    return {
        "schema_version": 1,
        "source_sha": source_sha,
        "version": package_native.validate_version(version),
        "run_id": run_id,
    }


def record(dist: Path, target: str, source_sha: str, version: str, run_id: str) -> dict:
    if target not in package_native.TARGETS:
        raise ValueError(f"unknown target: {target}")
    result = identity(source_sha, version, run_id)
    result["target"] = target
    result["artifacts"] = {
        name: generate_release_metadata.sha256(dist / name)
        for name in sorted(package_native.artifact_names(target, version))
    }
    return result


def verify(dist: Path, records: Path, source_sha: str, version: str, run_id: str) -> None:
    expected_records = {f"{target}.json" for target in package_native.TARGETS}
    if {p.name for p in records.iterdir()} != expected_records:
        raise ValueError("incomplete or unexpected provenance records; rebuild all packages")
    for target in package_native.TARGETS:
        actual = json.loads((records / f"{target}.json").read_text(encoding="utf-8"))
        if actual != record(dist, target, source_sha, version, run_id):
            raise ValueError(f"package provenance mismatch for {target}; rebuild packages")


def record_v2(dist: Path, target: str, source_sha: str, version: str, run_id: str,
              *, variant: str = "native", artifact_version: str | None = None,
              compiler: str, root: Path = artifact_catalog.ROOT) -> dict:
    label = artifact_version or version
    pinned = artifact_catalog.toolchain(root)
    if not compiler.startswith(f"rustc {pinned} "):
        raise ValueError("compiler does not match pinned Rust toolchain")
    result = identity(source_sha, version, run_id)
    result.update(schema_version=2, target=target, variant=variant, artifact_version=label,
                  compiler=compiler, toolchain=pinned,
                  lockfile_sha256=artifact_catalog.text_sha256(root / "Cargo.lock"),
                  build_config_sha256=artifact_catalog.build_config_digest(variant, root),
                  artifacts={name: artifact_catalog.sha256(dist / name)
                             for name in sorted(artifact_catalog.names(target, variant, label))})
    return result


def verify_v2(dist: Path, records: Path, source_sha: str, version: str, run_id: str,
              *, include_focal: bool = True, artifact_version: str | None = None,
              root: Path = artifact_catalog.ROOT) -> None:
    label = artifact_version or version
    packages = artifact_catalog.packages(label, include_focal=include_focal)
    expected = {f"{target}-{variant}.json" for target, variant in packages}
    if {path.name for path in records.iterdir()} != expected:
        raise ValueError("incomplete v2 provenance; rebuild all packages including required variants")
    for target, variant in packages:
        actual = json.loads((records / f"{target}-{variant}.json").read_text(encoding="utf-8"))
        expected_record = record_v2(dist, target, source_sha, version, run_id,
                                    variant=variant, artifact_version=label,
                                    compiler=actual.get("compiler", ""), root=root)
        if actual != expected_record:
            raise ValueError(f"package provenance mismatch for {target}/{variant}; rebuild packages")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("record", "verify"))
    parser.add_argument("--dist", type=Path, required=True)
    parser.add_argument("--records", type=Path, required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--target")
    parser.add_argument("--variant", choices=("native", "glibc231"), default="native")
    parser.add_argument("--artifact-version")
    parser.add_argument("--include-focal", action="store_true")
    parser.add_argument("--legacy", action="store_true", help="validate supported v1 records without permitting new publication")
    parser.add_argument("--compiler-file", type=Path, help="rustc -Vv captured inside an isolated builder")
    args = parser.parse_args()
    actual_source = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=artifact_catalog.ROOT, text=True).strip()
    if not args.legacy and actual_source != args.source_sha:
        raise ValueError("provenance source must match the actual frozen checkout")
    if args.operation == "record":
        compiler = (args.compiler_file.read_text(encoding="utf-8") if args.compiler_file else
                    subprocess.check_output(["rustc", "-Vv"], text=True)).strip()
        result = record_v2(args.dist, args.target, args.source_sha, args.version, args.run_id,
                           variant=args.variant, artifact_version=args.artifact_version, compiler=compiler)
        args.records.mkdir(parents=True, exist_ok=True)
        (args.records / f"{args.target}-{args.variant}.json").write_text(
            json.dumps(result, indent=2) + "\n", encoding="utf-8"
        )
    else:
        if args.legacy:
            verify(args.dist, args.records, args.source_sha, args.version, args.run_id)
        else:
            verify_v2(args.dist, args.records, args.source_sha, args.version, args.run_id,
                      include_focal=args.include_focal, artifact_version=args.artifact_version)


if __name__ == "__main__":
    main()
