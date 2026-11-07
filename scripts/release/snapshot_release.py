"""Replace the fixed snapshot only after saving a recoverable preimage."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

import artifact_catalog
from release_io import gh, release, write_json


TAG = "main-snapshot"


def api(path: str, *args: str, allow_missing: bool = False):
    result = subprocess.run(["gh", "api", f"repos/{os.environ['GITHUB_REPOSITORY']}/{path}", *args],
                            capture_output=True, text=True, encoding="utf-8")
    if result.returncode:
        if allow_missing and re.search(r"HTTP 404", result.stderr):
            return None
        raise RuntimeError("GitHub snapshot API failed")
    return json.loads(result.stdout) if result.stdout.strip() else None


def backup(directory: Path, recovery: Path, source_sha: str) -> dict:
    if not re.fullmatch(r"[0-9a-f]{40}", source_sha) or (recovery / "state.json").exists():
        raise ValueError("invalid snapshot identity or existing recovery journal")
    candidate = {path.name: artifact_catalog.sha256(path) for path in directory.iterdir() if path.is_file()}
    if not candidate:
        raise ValueError("empty snapshot candidate")
    ref = api(f"git/ref/tags/{TAG}", allow_missing=True)
    exists = api(f"releases/tags/{TAG}", allow_missing=True)
    previous = release(TAG) if exists else None
    assets = recovery / "assets"
    assets.mkdir(parents=True)
    if previous and previous["assets"]:
        gh("release", "download", TAG, "--dir", str(assets))
    hashes = {path.name: artifact_catalog.sha256(path) for path in assets.iterdir()}
    if previous and set(hashes) != {item["name"] for item in previous["assets"]}:
        raise ValueError("snapshot backup is incomplete")
    state = {"schema_version": 1, "source_sha": source_sha, "previous_ref": ref,
             "previous_release": previous, "previous_hashes": hashes, "candidate": candidate,
             "completed": [], "phase": "backed-up"}
    write_json(recovery / "state.json", state)
    return state


def set_ref(sha: str) -> None:
    if api(f"git/ref/tags/{TAG}", allow_missing=True):
        api(f"git/refs/tags/{TAG}", "--method", "PATCH", "-f", f"sha={sha}", "-F", "force=true")
    else:
        api("git/refs", "--method", "POST", "-f", f"ref=refs/tags/{TAG}", "-f", f"sha={sha}")


def publish(directory: Path, recovery: Path) -> None:
    state = json.loads((recovery / "state.json").read_text(encoding="utf-8"))
    expected = state["candidate"]
    if {path.name: artifact_catalog.sha256(path) for path in directory.iterdir()} != expected:
        raise ValueError("snapshot candidate changed after backup")
    current_ref = api(f"git/ref/tags/{TAG}", allow_missing=True)
    old_sha = state["previous_ref"]["object"]["sha"] if state["previous_ref"] else None
    if current_ref and current_ref["object"]["sha"] not in {old_sha, state["source_sha"]}:
        raise ValueError("snapshot tag changed after backup")
    if not api(f"releases/tags/{TAG}", allow_missing=True):
        gh("release", "create", TAG, "--draft", "--prerelease", "--target", state["source_sha"],
           "--title", "NyaTerm Main Snapshot", "--notes", "Snapshot upload in progress.")
    for name in sorted(expected):
        gh("release", "upload", TAG, str(directory / name), "--clobber")
        state["completed"].append(name)
        state["phase"] = "uploading"
        write_json(recovery / "state.json", state)
    with tempfile.TemporaryDirectory() as folder:
        for name, digest in expected.items():
            gh("release", "download", TAG, "--pattern", name, "--dir", folder, "--clobber")
            if artifact_catalog.sha256(Path(folder) / name) != digest:
                raise RuntimeError(f"snapshot upload readback differs: {name}")
        notes = Path(folder) / "notes.md"
        notes.write_text("Manually triggered snapshot of the main branch.\n\n"
                         f"Source commit: {state['source_sha']}\n\n"
                         "This prerelease is overwritten after every successful snapshot build and is intended for testing only.\n",
                         encoding="utf-8")
        set_ref(state["source_sha"])
        state["phase"] = "tag-updated"
        write_json(recovery / "state.json", state)
        gh("release", "edit", TAG, "--title", "NyaTerm Main Snapshot", "--notes-file", str(notes),
           "--prerelease", "--draft=false", "--latest=false")
    # Keep old files available until every new file and its tag have been verified.
    for item in release(TAG)["assets"]:
        if item["name"] not in expected:
            gh("release", "delete-asset", TAG, item["name"], "--yes")
    if api(f"git/ref/tags/{TAG}")["object"]["sha"] != state["source_sha"]:
        raise RuntimeError("snapshot tag readback differs")
    state["phase"] = "published"
    write_json(recovery / "state.json", state)


def restore(recovery: Path) -> None:
    state = json.loads((recovery / "state.json").read_text(encoding="utf-8"))
    previous = state["previous_release"]
    old_sha = state["previous_ref"]["object"]["sha"] if state["previous_ref"] else None
    ref = api(f"git/ref/tags/{TAG}", allow_missing=True)
    if ref and ref["object"]["sha"] not in {old_sha, state["source_sha"]}:
        raise ValueError("refusing to overwrite a later snapshot")
    for name, digest in state["previous_hashes"].items():
        if artifact_catalog.sha256(recovery / "assets" / name) != digest:
            raise ValueError("snapshot recovery backup is corrupted")
    if previous:
        for name in state["previous_hashes"]:
            gh("release", "upload", TAG, str(recovery / "assets" / name), "--clobber")
        for item in release(TAG)["assets"]:
            if item["name"] not in state["previous_hashes"]:
                if item["name"] not in state["candidate"]:
                    raise ValueError("unexpected snapshot asset during restore")
                gh("release", "delete-asset", TAG, item["name"], "--yes")
        notes = recovery / "previous-notes.md"
        notes.write_text(previous["body"], encoding="utf-8")
        gh("release", "edit", TAG, "--title", previous["name"], "--notes-file", str(notes),
           f"--prerelease={str(previous['isPrerelease']).lower()}", f"--draft={str(previous['isDraft']).lower()}")
    elif api(f"releases/tags/{TAG}", allow_missing=True):
        gh("release", "delete", TAG, "--yes")
    if old_sha:
        set_ref(old_sha)
    elif ref:
        api(f"git/refs/tags/{TAG}", "--method", "DELETE")
    if previous and state["previous_hashes"]:
        with tempfile.TemporaryDirectory() as folder:
            gh("release", "download", TAG, "--dir", folder)
            if {p.name: artifact_catalog.sha256(p) for p in Path(folder).iterdir()} != state["previous_hashes"]:
                raise RuntimeError("restored snapshot readback differs")
    elif previous and release(TAG)["assets"]:
        raise RuntimeError("restored empty snapshot still has assets")
    state["phase"] = "restored"
    write_json(recovery / "state.json", state)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("backup", "publish", "restore"))
    parser.add_argument("--dist", type=Path, default=Path("release-assets"))
    parser.add_argument("--recovery", type=Path, default=Path("snapshot-recovery"))
    parser.add_argument("--source-sha", default="")
    args = parser.parse_args()
    if args.operation == "backup":
        backup(args.dist, args.recovery, args.source_sha)
    elif args.operation == "publish":
        publish(args.dist, args.recovery)
    else:
        restore(args.recovery)


if __name__ == "__main__":
    main()
