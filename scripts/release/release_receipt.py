"""Resolve published versions for downstream channels without job-name coupling."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import tempfile

import artifact_catalog
import package_native
from release_io import command, gh, release, write_json


def api(path: str, *args: str) -> str:
    return command(["gh", "api", f"repos/{os.environ['GITHUB_REPOSITORY']}/{path}", *args])


def tag_sha(tag: str) -> str:
    value = json.loads(api(f"git/ref/tags/{tag}"))["object"]
    for _ in range(8):
        if value["type"] == "commit":
            if not re.fullmatch(r"[0-9a-f]{40}", value["sha"]):
                raise ValueError("invalid tag SHA")
            return value["sha"]
        if value["type"] != "tag":
            break
        value = json.loads(api(f"git/tags/{value['sha']}"))["object"]
    raise ValueError("tag does not resolve to a commit")


def validate_receipt(value: dict, *, run_id: str, tag: str, source_sha: str, checksum: str) -> None:
    version = package_native.validate_version(tag.removeprefix("v"))
    expected = {"schema_version": 1, "tag": tag, "version": version, "source_sha": source_sha,
                "channel": "preview" if package_native.parse_version(version)["pre"] else "stable",
                "run_id": run_id, "sha256sums_sha256": checksum}
    if value != expected:
        raise ValueError("published release receipt does not match run, tag or checksums")


def create(directory: Path, version: str, source_sha: str, run_id: str, output: Path) -> dict:
    if not re.fullmatch(r"[0-9a-f]{40}", source_sha) or not re.fullmatch(r"[1-9][0-9]*", run_id):
        raise ValueError("invalid publication identity")
    version = package_native.validate_version(version)
    tag = f"v{version}"
    if tag_sha(tag) != source_sha:
        raise ValueError("published tag does not match frozen source")
    info = release(tag)
    if info["isDraft"] or info["isPrerelease"] != bool(package_native.parse_version(version)["pre"]):
        raise ValueError("release is not published in its expected channel")
    value = {"schema_version": 1, "tag": tag, "version": version, "source_sha": source_sha,
             "channel": "preview" if package_native.parse_version(version)["pre"] else "stable",
             "run_id": run_id, "sha256sums_sha256": artifact_catalog.sha256(directory / "SHA256SUMS")}
    write_json(output, value)
    output.with_name("published-release-tag.txt").write_text(tag + "\n", encoding="utf-8")
    return value


def resolve(channel: str, *, run_id: str = "", version: str = "") -> dict[str, str]:
    with tempfile.TemporaryDirectory(prefix="nyaterm-receipt-") as folder:
        work = Path(folder)
        receipt = None
        if run_id:
            if not re.fullmatch(r"[1-9][0-9]*", run_id):
                raise ValueError("invalid source run ID")
            run = json.loads(api(f"actions/runs/{run_id}"))
            workflow = json.loads(api("actions/workflows/release.yml"))
            if run["workflow_id"] != workflow["id"] or run["status"] != "completed" or run["conclusion"] != "success":
                raise ValueError("source run is not a successful Release run")
            names = api(f"actions/runs/{run_id}/artifacts?per_page=100", "--paginate", "--jq",
                        '.artifacts[] | select(.expired == false) | .name').splitlines()
            if "published-release" in names:
                gh("run", "download", run_id, "--name", "published-release", "--dir", str(work))
                receipt = json.loads((work / "release-receipt.json").read_text(encoding="utf-8"))
                tag = receipt["tag"]
            elif "published-release-tag" in names:
                gh("run", "download", run_id, "--name", "published-release-tag", "--dir", str(work))
                tag = (work / "published-release-tag.txt").read_text(encoding="utf-8").strip()
            else:
                return {"publish": "false", "version": "", "tag": ""}
            if not isinstance(tag, str) or not tag.startswith("v"):
                raise ValueError("invalid receipt tag")
            version = package_native.validate_version(tag[1:])
            if tag != f"v{version}":
                raise ValueError("noncanonical receipt tag")
        else:
            version = package_native.validate_version(version)
            tag = f"v{version}"
        info = release(tag)
        prerelease = bool(package_native.parse_version(version)["pre"])
        if info["tagName"] != tag or info["isDraft"] or info["isPrerelease"] != prerelease:
            raise ValueError("release is not published in its expected channel")
        gh("release", "download", tag, "--pattern", "SHA256SUMS", "--dir", str(work), "--clobber")
        if receipt is not None:
            validate_receipt(receipt, run_id=run_id, tag=tag, source_sha=tag_sha(tag),
                             checksum=artifact_catalog.sha256(work / "SHA256SUMS"))
        return {"publish": str(not prerelease or channel == "gitee").lower(), "version": version, "tag": tag}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="operation", required=True)
    build = sub.add_parser("create")
    build.add_argument("--dist", type=Path, default=Path("dist-release"))
    build.add_argument("--version", required=True)
    build.add_argument("--source-sha", required=True)
    build.add_argument("--run-id", required=True)
    build.add_argument("--output", type=Path, default=Path("release-receipt.json"))
    read = sub.add_parser("resolve")
    read.add_argument("--channel", choices=("homebrew", "aur", "gitee"), required=True)
    args = parser.parse_args()
    if args.operation == "create":
        create(args.dist, args.version, args.source_sha, args.run_id, args.output)
    else:
        result = resolve(args.channel, run_id=os.environ.get("RUN_ID", ""),
                         version=os.environ.get("INPUT_VERSION", "") or os.environ.get("INPUT_TAG", ""))
        result["dry_run"] = os.environ.get("INPUT_DRY_RUN", "false") or "false"
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
            for name, value in result.items():
                stream.write(f"{name}={value}\n")


if __name__ == "__main__":
    main()
