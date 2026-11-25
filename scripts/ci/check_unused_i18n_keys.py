#!/usr/bin/env python3
"""Guard the translation catalogs against keys that no call site reaches.

A catalog key that no `t!` call can produce is dead weight: it ships to every
user in every language and still never shows up. This script finds those keys
and fails, so a catalog stays in step with the interface.

A key counts as used when its literal appears anywhere in the sources. That
covers the direct `t!("key")` calls as well as the key-producing helpers
(`label_key()` and friends) that the dynamic call sites rely on. Keys that a
`format!` builds at runtime are covered by their literal prefix instead.

Run from the repository root:

    python3 scripts/ci/check_unused_i18n_keys.py           # report
    python3 scripts/ci/check_unused_i18n_keys.py --remove  # drop them
"""

from __future__ import annotations

import argparse
import io
import json
import os
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
LOCALES = REPO_ROOT / "crates" / "nyaterm-i18n" / "locales"
CATALOG = LOCALES / "en.json"
SOURCE_ROOTS = ["crates"]
SKIP_DIRS = {"target", ".git", "node_modules", ".workbuddy", "temp"}

# A Rust string literal: "..." with escapes, or r"..." / r#"..."# raw strings.
LITERAL_RE = re.compile(
    r'r#*"(?:[^"]|"(?![#]*"))*"#*|"(?:\\.|[^"\\])*"',
    re.S,
)
# format!("prefix.{...}") builds a key at runtime.
FORMAT_RE = re.compile(r'format!\(\s*"((?:[^"\\]|\\.)*)\{')
# A prefix only builds a key when it ends on a segment boundary.
BOUNDARY_RE = re.compile(r"[A-Za-z0-9][._-]$")


def flatten(node: dict, prefix: str, out: dict) -> None:
    """Collect every leaf of the catalog as a dotted path."""
    for key, value in node.items():
        path = prefix + key
        if isinstance(value, dict):
            flatten(value, path + ".", out)
        else:
            out[path] = value


def source_files() -> list[Path]:
    files: list[Path] = []
    for root in SOURCE_ROOTS:
        for dirpath, dirnames, filenames in os.walk(REPO_ROOT / root):
            dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
            for name in filenames:
                if name.endswith(".rs"):
                    files.append(Path(dirpath) / name)
    return files


def used_keys(catalog: dict) -> set[str]:
    """Return the catalog keys that a call site can still produce."""
    literals: set[str] = set()
    prefixes: set[str] = set()
    for path in source_files():
        src = io.open(path, encoding="utf-8").read()
        for match in LITERAL_RE.finditer(src):
            raw = match.group(0)
            if raw.startswith("r"):
                literals.add(raw[raw.index('"') + 1: -1])
                continue
            body = raw[1:-1]
            try:
                literals.add(json.loads('"' + body + '"'))
            except Exception:
                literals.add(body)
        for match in FORMAT_RE.finditer(src):
            prefix = match.group(1)
            if prefix and BOUNDARY_RE.search(prefix):
                prefixes.add(prefix)

    used = set()
    for key in catalog:
        if key in literals:
            used.add(key)
            continue
        if any(key.startswith(prefix) for prefix in prefixes):
            used.add(key)
    return used


def drop(node: dict, key: str) -> bool:
    """Remove a dotted key from the catalog, reporting whether it was there."""
    parts = key.split(".")
    cursor = node
    for part in parts[:-1]:
        if not isinstance(cursor, dict) or part not in cursor:
            return False
        cursor = cursor[part]
    if not isinstance(cursor, dict) or parts[-1] not in cursor:
        return False
    del cursor[parts[-1]]
    return True


def prune_empty(node: dict) -> None:
    """Drop the sections that no key is left in."""
    for key in list(node):
        value = node[key]
        if not isinstance(value, dict):
            continue
        prune_empty(value)
        if not value:
            del node[key]


def write_catalog(path: Path, catalog: dict) -> None:
    """Write the catalog back in the exact shape the repository ships."""
    text = json.dumps(catalog, indent=2, ensure_ascii=False).replace("\n", "\r\n") + "\r\n"
    io.open(path, "wb").write(text.encode("utf-8"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--remove",
        action="store_true",
        help="drop the unused keys from every catalog instead of only reporting them",
    )
    args = parser.parse_args()

    catalog = json.loads(io.open(CATALOG, encoding="utf-8").read())
    flat: dict = {}
    flatten(catalog, "", flat)

    used = used_keys(flat)
    unused = sorted(key for key in flat if key not in used)

    if not args.remove:
        if not unused:
            print(f"all {len(flat)} catalog keys are reachable")
            return 0
        print(f"{len(unused)} of {len(flat)} catalog keys are unreachable:")
        for key in unused:
            print(f"  {key}")
        return 1

    catalogs = sorted(LOCALES.glob("*.json"))
    for path in catalogs:
        other = json.loads(io.open(path, encoding="utf-8").read())
        for key in unused:
            drop(other, key)
        prune_empty(other)
        write_catalog(path, other)
    print(f"removed {len(unused)} unused keys from {len(catalogs)} catalogs")
    return 0


if __name__ == "__main__":
    sys.exit(main())
