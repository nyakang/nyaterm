#!/usr/bin/env python3
"""Reject new violations of NyaTerm's crate and runtime architecture rules."""

from __future__ import annotations

import json
import re
import sys
from collections import Counter
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
ALLOWLIST_PATH = ROOT / "scripts" / "ci" / "architecture_allowlist.json"
RUST_FILES = tuple((ROOT / "crates").glob("*/src/**/*.rs"))


def relative(path: Path) -> str:
    return path.relative_to(ROOT).as_posix()


def matching_counts(pattern: re.Pattern[str], files: tuple[Path, ...]) -> Counter[str]:
    counts: Counter[str] = Counter()
    for path in files:
        text = path.read_text(encoding="utf-8")
        count = len(pattern.findall(text))
        if count:
            counts[relative(path)] = count
    return counts


def load_allowlist() -> dict[str, list[dict[str, object]]]:
    document = json.loads(ALLOWLIST_PATH.read_text(encoding="utf-8"))
    if not isinstance(document, dict):
        raise ValueError("architecture allowlist root must be an object")
    for category, entries in document.items():
        if not isinstance(entries, list):
            raise ValueError(f"allowlist category {category!r} must be a list")
        seen: set[str] = set()
        for entry in entries:
            path = entry.get("path")
            reason = entry.get("reason")
            maximum = entry.get("max_count")
            if not isinstance(path, str) or not path or path in seen:
                raise ValueError(f"{category}: every path must be unique and non-empty")
            if not isinstance(reason, str) or not reason.strip():
                raise ValueError(f"{category}: {path} must explain why it is allowed")
            if not isinstance(maximum, int) or maximum < 1:
                raise ValueError(f"{category}: {path} max_count must be a positive integer")
            seen.add(path)
    return document


def check_budget(
    category: str,
    actual: Counter[str],
    allowlist: dict[str, list[dict[str, object]]],
) -> list[str]:
    errors: list[str] = []
    entries = {entry["path"]: entry for entry in allowlist.get(category, [])}
    for path, count in actual.items():
        entry = entries.get(path)
        if entry is None:
            errors.append(f"{category}: unapproved occurrence in {path} ({count})")
        elif count > entry["max_count"]:
            errors.append(
                f"{category}: {path} has {count}, exceeding baseline {entry['max_count']}"
            )
    for path in entries.keys() - actual.keys():
        errors.append(f"{category}: stale allowlist entry for {path}")
    return errors


def dependency_errors() -> list[str]:
    errors: list[str] = []
    dependency = re.compile(r"(?m)^\s*(gpui|gpui_platform|gpui-kit)\s*=")
    low_level = (
        "nyaterm-core",
        "nyaterm-transport",
        "nyaterm-terminal",
        "nyaterm-store",
        "nyaterm-remote-desktop",
    )
    for crate in low_level:
        manifest = ROOT / "crates" / crate / "Cargo.toml"
        if dependency.search(manifest.read_text(encoding="utf-8")):
            errors.append(f"crate_boundary: {crate} must remain independent of GPUI")
    desktop_manifest = (ROOT / "crates" / "nyaterm-desktop" / "Cargo.toml").read_text(
        encoding="utf-8"
    )
    if re.search(r"(?m)^\s*(gpui-kit|gpui_component)\s*=", desktop_manifest):
        errors.append("crate_boundary: nyaterm-desktop must use nyaterm-ui wrappers")
    return errors


def main() -> int:
    try:
        allowlist = load_allowlist()
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"architecture check configuration error: {error}", file=sys.stderr)
        return 2

    errors: list[str] = []
    path_declarations = matching_counts(re.compile(r"#\s*\[\s*path\s*="), RUST_FILES)
    for path, count in path_declarations.items():
        errors.append(f"module_tree: #[path] is forbidden in {path} ({count})")

    wildcard_imports = matching_counts(re.compile(r"\buse\s+super::\*\s*;"), RUST_FILES)
    errors.extend(check_budget("wildcard_super_import", wildcard_imports, allowlist))

    feature_files = tuple(
        path
        for path in RUST_FILES
        if relative(path).startswith("crates/nyaterm-desktop/src/features/")
    )
    thread_spawns = matching_counts(
        re.compile(r"\b(?:std::thread|thread)::spawn\s*\("), feature_files
    )
    errors.extend(check_budget("desktop_feature_thread_spawn", thread_spawns, allowlist))

    desktop_files = tuple(
        path
        for path in RUST_FILES
        if relative(path).startswith("crates/nyaterm-desktop/src/")
    )
    thread_builders = matching_counts(
        re.compile(r"\b(?:std::)?thread::Builder::new\s*\(\s*\)"), desktop_files
    )
    errors.extend(check_budget("desktop_thread_builder", thread_builders, allowlist))

    raw_scrolls = matching_counts(
        re.compile(r"\.overflow_(?:x_|y_)?scroll\s*\(\s*\)"), RUST_FILES
    )
    errors.extend(check_budget("raw_scroll_container", raw_scrolls, allowlist))
    errors.extend(dependency_errors())

    if errors:
        print("Architecture check failed:", file=sys.stderr)
        for error in sorted(errors):
            print(f"- {error}", file=sys.stderr)
        return 1
    print("Architecture check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
