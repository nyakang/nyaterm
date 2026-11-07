"""Narrow subprocess adapters; never echo credentials or remote error bodies."""

from __future__ import annotations

import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

import artifact_catalog


def command(args: list[str]) -> str:
    result = subprocess.run(args, capture_output=True, text=True, encoding="utf-8")
    if result.returncode:
        raise RuntimeError(f"{args[0]} {args[1]} failed (exit {result.returncode})")
    return result.stdout


def gh(*args: str) -> str:
    return command(["gh", *args, "--repo", os.environ["GITHUB_REPOSITORY"]])


def release(tag: str) -> dict:
    return json.loads(gh("release", "view", tag, "--json", "tagName,isDraft,isPrerelease,body,name,targetCommitish,assets"))


def write_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(path)


class R2:
    def __init__(self):
        self.bucket = os.environ.get("R2_BUCKET", "nyaterm")
        self.endpoint = os.environ["R2_ENDPOINT"]
        if not re.fullmatch(r"https://[a-zA-Z0-9]+\.r2\.cloudflarestorage\.com", self.endpoint):
            raise ValueError("invalid R2 endpoint")

    def call(self, operation: str, key: str, *args: str):
        return subprocess.run(["aws", "s3api", operation, "--bucket", self.bucket, "--key", key,
                               "--endpoint-url", self.endpoint, *args],
                              capture_output=True, text=True, encoding="utf-8")

    def fetch(self, key: str, destination: Path) -> bool:
        destination.parent.mkdir(parents=True, exist_ok=True)
        result = self.call("get-object", key, str(destination))
        if result.returncode == 0:
            return True
        if re.search(r"\((?:NoSuchKey|404|NotFound)\)", result.stderr):
            destination.unlink(missing_ok=True)
            return False
        raise RuntimeError("R2 read failed; absence was not established")

    def put(self, key: str, path: Path, *, immutable: bool = False) -> None:
        with tempfile.TemporaryDirectory(prefix="nyaterm-r2-") as directory:
            existing = Path(directory) / "object"
            if immutable and self.fetch(key, existing):
                if artifact_catalog.sha256(existing) != artifact_catalog.sha256(path):
                    raise ValueError(f"immutable asset differs: {key}")
                return
            content_type = ("application/json; charset=utf-8" if path.suffix == ".json" else
                            "text/plain; charset=utf-8" if path.name == "SHA256SUMS" else
                            {".msi": "application/x-msi", ".deb": "application/vnd.debian.binary-package",
                             ".dmg": "application/x-apple-diskimage", ".zip": "application/zip",
                             ".exe": "application/vnd.microsoft.portable-executable",
                             ".rpm": "application/x-rpm", ".gz": "application/gzip"}.get(path.suffix, "application/octet-stream"))
            options = ["--body", str(path), "--content-type", content_type, "--cache-control",
                       "public, max-age=31536000, immutable" if immutable else "public, max-age=60, must-revalidate"]
            if immutable:
                options += ["--if-none-match", "*"]
            result = self.call("put-object", key, *options)
            if result.returncode:
                # Another writer may have won the conditional create. Accept only identical bytes.
                if not immutable or not self.fetch(key, existing) or artifact_catalog.sha256(existing) != artifact_catalog.sha256(path):
                    raise RuntimeError(f"R2 upload failed: {key}")
            if not self.fetch(key, existing) or artifact_catalog.sha256(existing) != artifact_catalog.sha256(path):
                raise RuntimeError(f"R2 readback differs: {key}")

    def delete(self, key: str) -> None:
        result = self.call("delete-object", key)
        if result.returncode:
            raise RuntimeError(f"R2 removal failed: {key}")
