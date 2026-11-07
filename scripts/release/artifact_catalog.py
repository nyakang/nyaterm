"""Authoritative package inventory, including variants sharing a Rust target."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import tomllib

import package_native


ROOT = Path(__file__).resolve().parents[2]
FOCAL_TARGET = "x86_64-unknown-linux-gnu"


def packages(artifact_version: str, *, include_focal: bool = False) -> dict[tuple[str, str], set[str]]:
    result = {(target, "native"): package_native.artifact_names(target, artifact_version)
              for target in package_native.TARGETS}
    if include_focal:
        result[(FOCAL_TARGET, "glibc231")] = names(FOCAL_TARGET, "glibc231", artifact_version)
    return result


def names(target: str, variant: str, artifact_version: str) -> set[str]:
    package_native.validate_artifact_version(artifact_version)
    if variant == "native":
        return package_native.artifact_names(target, artifact_version)
    if (target, variant) != (FOCAL_TARGET, "glibc231") or artifact_version == "main-snapshot":
        raise ValueError("unsupported package target/variant")
    prefix = f"NyaTerm_{artifact_version}_linux_x64_glibc231"
    return {prefix + suffix for suffix in (".tar.gz", ".tar.gz.sha256", ".deb", ".deb.sha256")}


def expected(artifact_version: str, *, include_focal: bool = False) -> set[str]:
    return set().union(*packages(artifact_version, include_focal=include_focal).values())


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def toolchain(root: Path = ROOT) -> str:
    with (root / "rust-toolchain.toml").open("rb") as stream:
        return tomllib.load(stream)["toolchain"]["channel"]


def text_sha256(path: Path) -> str:
    # Git's Windows checkout may use CRLF; build identity describes repository text.
    return hashlib.sha256(path.read_bytes().replace(b"\r\n", b"\n")).hexdigest()


def build_config_digest(variant: str, root: Path = ROOT) -> str:
    # Hash tracked build contracts in the frozen source tree, never cache outputs.
    paths = {root / "Cargo.toml", root / "Cargo.lock", root / "rust-toolchain.toml"}
    for folder in ("scripts/release", "scripts/compat/linux-focal", ".github/actions", ".github/workflows"):
        paths.update(path for path in (root / folder).rglob("*")
                     if path.is_file() and path.suffix in {".py", ".sh", ".ps1", ".json", ".toml", ".txt", ".yml"})
    paths.add(root / "scripts/compat/linux-focal/Dockerfile")
    value = {path.relative_to(root).as_posix(): text_sha256(path) for path in sorted(paths)}
    payload = {"variant": variant, "files": value, "profile": "release", "locked": True,
               "gist_client_sha256": hashlib.sha256(os.environ.get("NYATERM_GITHUB_GIST_CLIENT_ID", "").encode()).hexdigest()}
    return hashlib.sha256(json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
