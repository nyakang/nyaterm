"""Stage immutable assets and commit a serialized, recoverable release channel."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import tempfile
import time
import urllib.error
import urllib.request

import artifact_catalog
import generate_release_metadata
import package_native
from release_io import R2, gh, release, write_json
import verify_release_metadata


def compare_versions(left: str, right: str) -> int:
    a, b = package_native.parse_version(left), package_native.parse_version(right)
    core_a, core_b = tuple(int(a[x]) for x in ("major", "minor", "patch")), tuple(int(b[x]) for x in ("major", "minor", "patch"))
    if core_a != core_b:
        return (core_a > core_b) - (core_a < core_b)
    pre_a, pre_b = a["pre"], b["pre"]
    if pre_a is None or pre_b is None:
        return (pre_a is None) - (pre_b is None)
    for x, y in zip(pre_a.split("."), pre_b.split(".")):
        if x == y:
            continue
        if x.isdigit() and y.isdigit():
            return (int(x) > int(y)) - (int(x) < int(y))
        if x.isdigit() != y.isdigit():
            return -1 if x.isdigit() else 1
        return (x > y) - (x < y)
    return (len(pre_a.split(".")) > len(pre_b.split("."))) - (len(pre_a.split(".")) < len(pre_b.split(".")))


def checksums(directory: Path) -> dict[str, str]:
    result = {}
    for line in (directory / "SHA256SUMS").read_text(encoding="utf-8").splitlines():
        digest, name = line.split("  ", 1)
        if len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest) or Path(name).name != name or "/" in name or "\\" in name or name in result:
            raise ValueError("invalid or duplicate checksum entry")
        if artifact_catalog.sha256(directory / name) != digest:
            raise ValueError(f"checksum mismatch: {name}")
        result[name] = digest
    return result


def validate_candidate(directory: Path, version: str) -> None:
    actual = checksums(directory)
    if set(actual) != artifact_catalog.expected(version, include_focal=True):
        raise ValueError("candidate checksum inventory is incomplete or unexpected")
    expected = set(actual) | {"SHA256SUMS", "latest.json", "downloads.json"}
    if {path.name for path in directory.iterdir()} != expected:
        raise ValueError("candidate directory has missing or unexpected files")
    verify_release_metadata.verify(directory, version=version, tag=f"v{version}",
                                   base_url="https://downloads.nyaterm.app", check_assets=False)
    downloads = json.loads((directory / "downloads.json").read_text(encoding="utf-8"))
    for platform, template in generate_release_metadata.download_artifacts(version).items():
        if downloads["platforms"][platform]["sha256"] != actual[template.format(version=version)]:
            raise ValueError("download manifest does not match candidate checksums")


def stage(store, directory: Path, version: str) -> None:
    validate_candidate(directory, version)
    for path in sorted(directory.iterdir()):
        store.put(f"releases/v{version}/{path.name}", path, immutable=True)


def stage_github(directory: Path, version: str) -> None:
    validate_candidate(directory, version)
    tag = f"v{version}"
    info = release(tag)
    if info["tagName"] != tag or info["isPrerelease"] != bool(package_native.parse_version(version)["pre"]) or not info["body"].strip():
        raise ValueError("expected a populated matching release")
    expected = {path.name for path in directory.iterdir()}
    existing_names = {item["name"] for item in info["assets"]}
    if existing_names - expected:
        raise ValueError("release contains unexpected assets")
    with tempfile.TemporaryDirectory() as folder:
        for path in sorted(directory.iterdir()):
            if path.name not in existing_names:
                if not info["isDraft"]:
                    raise ValueError("published release is missing candidate assets")
                gh("release", "upload", tag, str(path))
            gh("release", "download", tag, "--pattern", path.name, "--dir", folder, "--clobber")
            if artifact_catalog.sha256(Path(folder) / path.name) != artifact_catalog.sha256(path):
                raise ValueError(f"GitHub asset readback differs: {path.name}")


def backup(store, directory: Path, recovery: Path, version: str) -> dict:
    validate_candidate(directory, version)
    if (recovery / "state.json").exists():
        raise ValueError("recovery directory already contains a journal")
    state = {"schema_version": 1, "version": version, "phase": "backed-up", "completed": [], "objects": {}}
    for name, key in generate_release_metadata.channel_manifest_destinations(version).items():
        path = recovery / name
        exists = store.fetch(key, path)
        if exists:
            current = json.loads(path.read_text(encoding="utf-8"))
            order = compare_versions(version, current["version"])
            if order < 0:
                raise ValueError("channel downgrade refused")
            if order == 0 and path.read_bytes() != (directory / name).read_bytes():
                raise ValueError("same channel version has different candidate bytes")
        state["objects"][name] = {"key": key, "existed": exists,
                                  "sha256": artifact_catalog.sha256(path) if exists else None}
    write_json(recovery / "state.json", state)
    return state


def verify_public(directory: Path, version: str) -> None:
    with tempfile.TemporaryDirectory(prefix="nyaterm-public-") as folder:
        public = Path(folder)
        for name, key in generate_release_metadata.channel_manifest_destinations(version).items():
            for attempt in range(12):
                request = urllib.request.Request(
                    f"https://downloads.nyaterm.app/{key}?release={version}&run={os.environ.get('GITHUB_RUN_ID', 'local')}&attempt={attempt}",
                    headers={"Cache-Control": "no-cache", "User-Agent": "nyaterm-release-verifier"})
                try:
                    with urllib.request.urlopen(request, timeout=30) as response:
                        value = response.read()
                    if value == (directory / name).read_bytes():
                        (public / name).write_bytes(value)
                        break
                except (OSError, urllib.error.URLError):
                    pass
                if attempt == 11:
                    raise RuntimeError(f"public channel did not converge: {name}")
                time.sleep(5)
        verify_release_metadata.verify(public, version=version, tag=f"v{version}",
                                       base_url="https://downloads.nyaterm.app", check_assets=True)
        for name in artifact_catalog.names(artifact_catalog.FOCAL_TARGET, "glibc231", version):
            verify_release_metadata.probe_asset(f"https://downloads.nyaterm.app/releases/v{version}/{name}")


def commit(store, directory: Path, recovery: Path, version: str, *, source_sha: str | None = None,
           publish=gh, view=release, public=verify_public) -> dict:
    validate_candidate(directory, version)
    state = json.loads((recovery / "state.json").read_text(encoding="utf-8"))
    if state["version"] != version:
        raise ValueError("recovery journal belongs to another release")
    tag = f"v{version}"
    if source_sha is not None:
        from release_receipt import tag_sha
        if tag_sha(tag) != source_sha:
            raise ValueError("formal tag changed after preflight")
    info = view(tag)
    if info["tagName"] != tag or info["isPrerelease"] != bool(package_native.parse_version(version)["pre"]) or not info["body"].strip():
        raise ValueError("release metadata does not match candidate")
    with tempfile.TemporaryDirectory() as folder:
        for name, item in state["objects"].items():
            existing = Path(folder) / name
            exists = store.fetch(item["key"], existing)
            desired = directory / name
            if exists and existing.read_bytes() == desired.read_bytes():
                continue
            if exists != item["existed"] or (exists and artifact_catalog.sha256(existing) != item["sha256"]):
                raise ValueError("channel changed since backup; start a fresh serialized promotion")
            if not info["isDraft"]:
                raise ValueError("published release cannot be changed by recovery")
        for name, item in state["objects"].items():
            store.put(item["key"], directory / name)
            if name not in state["completed"]:
                state["completed"].append(name)
            state["phase"] = "channel-written"
            write_json(recovery / "state.json", state)
    public(directory, version)
    state["phase"] = "public-verified"
    write_json(recovery / "state.json", state)
    if info["isDraft"]:
        publish("release", "edit", tag, "--draft=false", f"--prerelease={str(info['isPrerelease']).lower()}")
    if view(tag)["isDraft"]:
        raise RuntimeError("GitHub release is not public")
    state["phase"] = "published"
    write_json(recovery / "state.json", state)
    return state


def verify_restored_public(recovery: Path, objects: dict) -> None:
    for name, item in objects.items():
        expected = (recovery / name).read_bytes() if item["existed"] else None
        for attempt in range(12):
            request = urllib.request.Request(
                f"https://downloads.nyaterm.app/{item['key']}?restore={attempt}&run={os.environ.get('GITHUB_RUN_ID', 'local')}",
                headers={"Cache-Control": "no-cache"})
            try:
                with urllib.request.urlopen(request, timeout=30) as response:
                    if response.read() == expected:
                        break
            except urllib.error.HTTPError as error:
                if expected is None and error.code == 404:
                    break
            except OSError:
                pass
            if attempt == 11:
                raise RuntimeError("restored public channel did not converge")
            time.sleep(5)


def restore(store, recovery: Path, *, public=verify_restored_public) -> None:
    state = json.loads((recovery / "state.json").read_text(encoding="utf-8"))
    with tempfile.TemporaryDirectory() as folder:
        for name, item in state["objects"].items():
            existing = Path(folder) / name
            if store.fetch(item["key"], existing):
                current = json.loads(existing.read_text(encoding="utf-8"))["version"]
                if compare_versions(current, state["version"]) > 0:
                    raise ValueError("refusing to restore over a newer release")
        # Validate every backup before the first write, including recorded absence.
        for name, item in state["objects"].items():
            if item["existed"] and artifact_catalog.sha256(recovery / name) != item["sha256"]:
                raise ValueError("recovery backup is corrupted")
        for name, item in state["objects"].items():
            if item["existed"]:
                store.put(item["key"], recovery / name)
            else:
                store.delete(item["key"])
        for name, item in state["objects"].items():
            existing = Path(folder) / name
            exists = store.fetch(item["key"], existing)
            if exists != item["existed"] or (exists and artifact_catalog.sha256(existing) != item["sha256"]):
                raise RuntimeError("restored channel readback differs")
    state["phase"] = "restore-origin-written"
    write_json(recovery / "state.json", state)
    public(recovery, state["objects"])
    state["phase"] = "restored"
    write_json(recovery / "state.json", state)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("stage", "stage-github", "backup", "commit", "restore"))
    parser.add_argument("--dist", type=Path, default=Path("dist-release"))
    parser.add_argument("--recovery", type=Path, default=Path("channel-recovery"))
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    if args.operation == "stage-github":
        stage_github(args.dist, args.version)
        return
    store = R2()
    if args.operation == "stage":
        stage(store, args.dist, args.version)
    elif args.operation == "backup":
        backup(store, args.dist, args.recovery, args.version)
    elif args.operation == "commit":
        commit(store, args.dist, args.recovery, args.version, source_sha=os.environ["SOURCE_SHA"])
    else:
        if json.loads((args.recovery / "state.json").read_text(encoding="utf-8"))["version"] != args.version:
            raise ValueError("restore version does not match the recovery journal")
        restore(store, args.recovery)


if __name__ == "__main__":
    main()
