#!/usr/bin/env python3
"""Import declaration-only Carapace YAML; never evaluate completion actions.

Usage: python scripts/completion/generate_catalog.py --archive /path/source.tar.gz
The archive is pinned and verified before any YAML is parsed. --check compares
all generated artifacts without modifying them.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import tarfile
import unicodedata
import yaml

COMMIT = "6bc2b8d69a2474b6eeb626a85ef83254c2ab7d82"
ARCHIVE_SHA256 = "2a3838b27dcd56e9ee6a4f1008a260a1ff55422f6b726a559e29a2fafda8b393"
NAME = re.compile(r"^[A-Za-z0-9_.:+@%-]{1,64}$")
VALUE = re.compile(r"^[\w./:@%+,=-]{1,1024}$", re.UNICODE)


def clean(value):
    if not isinstance(value, str):
        return ""
    return " ".join("".join(c for c in value[:4096] if not unicodedata.category(c).startswith("C")).split())[:256]


def node(token):
    return dict(token=token, aliases=[], description="", hidden=False, flags=[], persistent=[], children=[])


def flag(spec, description):
    if not isinstance(spec, str):
        return None
    spec = spec.strip()
    trimmed = spec.rstrip("=*?&!")
    mods = spec[len(trimmed):]
    if "&" in mods:
        return None
    long = short = ""
    parts = [p.strip() for p in trimmed.split(",")]
    if len(parts) > 2:
        return None
    for i, part in enumerate(parts):
        if part.startswith("--") and i == len(parts) - 1 and not long:
            long = part[2:]
        elif part.startswith("-") and i == 0 and not short:
            short = part[1:]
        else:
            return None
    if not (long or short) or any(v and not NAME.fullmatch(v) for v in [long, short]):
        return None
    if isinstance(description, dict):
        description = description.get("description", "")
    return dict(long=long, short=short, description=clean(description), takes_value="=" in mods or "?" in mods,
                optional="?" in mods, repeatable="*" in mods, required="!" in mods, values=[])


def apply(target, source, stats, depth=0):
    if depth > 32:
        raise ValueError("command tree exceeds depth limit")
    if not isinstance(source, dict):
        stats["skipped_commands"] += 1
        return
    target["description"] = target["description"] or clean(source.get("description"))
    target["hidden"] |= source.get("hidden") is True
    for alias in source.get("aliases") or []:
        if isinstance(alias, str) and NAME.fullmatch(alias) and alias != target["token"]:
            target["aliases"].append(alias)
    for yaml_key, key in [("flags", "flags"), ("persistentflags", "persistent")]:
        definitions = source.get(yaml_key) or {}
        if isinstance(definitions, dict):
            definitions = [definitions]
        if not isinstance(definitions, list):
            stats["skipped_flags"] += 1
            continue
        for definitions_map in definitions:
            if not isinstance(definitions_map, dict):
                stats["skipped_flags"] += 1
                continue
            for spec, desc in definitions_map.items():
                parsed = flag(spec, desc)
                if parsed is None:
                    stats["skipped_flags"] += 1
                    continue
                existing = next((f for f in target[key] if (f["long"], f["short"]) == (parsed["long"], parsed["short"])), None)
                if existing:
                    for field in ["takes_value", "optional", "repeatable", "required"]:
                        existing[field] |= parsed[field]
                    existing["description"] = existing["description"] or parsed["description"]
                else:
                    target[key].append(parsed)
    completion = source.get("completion") or {}
    values = completion.get("flag", {}) if isinstance(completion, dict) else {}
    if isinstance(values, dict):
        for key, items in sorted(values.items()):
            if not isinstance(items, list):
                continue
            for item in items:
                text = item.split("\t", 1)[0] if isinstance(item, str) else ""
                if not VALUE.fullmatch(text) or text.startswith("$"):
                    stats["skipped_values"] += 1
                    continue
                for f in target["flags"] + target["persistent"]:
                    if key in (f["long"], f["short"]) and text not in f["values"] and len(f["values"]) < 128:
                        f["values"].append(text)
    for child in source.get("commands") or []:
        token = child.get("name", "").split(" ")[0] if isinstance(child, dict) else ""
        if not NAME.fullmatch(token):
            stats["skipped_commands"] += 1
            continue
        existing = next((n for n in target["children"] if n["token"] == token), None)
        if existing is None:
            existing = node(token)
            target["children"].append(existing)
        apply(existing, child, stats, depth + 1)


def normalize(target, stats):
    stats["nodes"] += 1
    stats["flags"] += len(target["flags"]) + len(target["persistent"])
    target["aliases"] = sorted(set(target["aliases"]))
    for key in ["flags", "persistent"]:
        target[key].sort(key=lambda f: (f["long"], f["short"]))
    target["children"].sort(key=lambda n: n["token"])
    for child in target["children"]:
        normalize(child, stats)


def generate(archive_path):
    if archive_path.stat().st_size > 64 * 1024 * 1024:
        raise ValueError("archive exceeds size limit")
    digest = hashlib.sha256(archive_path.read_bytes()).hexdigest()
    if digest != ARCHIVE_SHA256:
        raise ValueError("source archive digest does not match pinned Carapace revision")
    sources = {}
    license_data = None
    stats = dict(files=0, roots=0, nodes=0, flags=0, skipped_files=0, skipped_flags=0, skipped_commands=0, skipped_values=0)
    with tarfile.open(archive_path, "r:gz") as archive:
        total = 0
        for member in archive:
            total += member.size
            if total > 256 * 1024 * 1024:
                raise ValueError("archive expands beyond limit")
            path = PurePosixPath(member.name)
            if path.is_absolute() or ".." in path.parts:
                raise ValueError("unsafe source path")
            if not member.isfile():
                continue
            if len(path.parts) == 2 and path.name == "LICENSE":
                license_data = archive.extractfile(member).read()
            if len(path.parts) < 5 or path.parts[1:3] != ("man", "cmd") or path.suffix != ".yaml":
                continue
            if member.size > 1024 * 1024 or len(sources) >= 20000:
                raise ValueError("YAML input exceeds limits")
            relative = "/".join(path.parts[3:])
            if relative in sources:
                raise ValueError("duplicate source path")
            data = archive.extractfile(member).read()
            try:
                # Fixed source data still receives bounded alias-free parsing.
                if any(isinstance(t, (yaml.tokens.AliasToken, yaml.tokens.AnchorToken)) for t in yaml.scan(data)):
                    raise ValueError("YAML aliases are not supported")
                document = yaml.safe_load(data)
            except (yaml.YAMLError, ValueError):
                stats["skipped_files"] += 1
                continue
            sources[relative] = document
    roots = []
    for directory in sorted({key.split("/")[0] for key in sources}):
        root_source = sources.get(f"{directory}/{directory}.yaml")
        if not isinstance(root_source, dict) or not NAME.fullmatch(directory):
            stats["skipped_commands"] += 1
            continue
        root = node(directory)
        for relative, document in sorted(sources.items()):
            if relative.split("/")[0] != directory or not isinstance(document, dict):
                continue
            path = Path(relative).stem.split(".")
            if path[0] != directory or any(not NAME.fullmatch(t) for t in path):
                stats["skipped_commands"] += 1
                continue
            target = root
            for token in path[1:]:
                child = next((n for n in target["children"] if n["token"] == token), None)
                if child is None:
                    child = node(token)
                    target["children"].append(child)
                target = child
            apply(target, document, stats)
            stats["files"] += 1
        # Carapace's man tree stores Docker's root aliases under container.
        # These public Docker spellings share the same declaration data.
        if directory == "docker":
            container = next((n for n in root["children"] if n["token"] == "container"), None)
            if container:
                shortcuts = {"run", "create", "start", "stop", "restart", "rm", "kill", "attach", "logs", "inspect", "exec", "cp", "diff", "export", "port", "rename", "stats", "top", "update", "wait", "commit", "pause", "unpause"}
                existing = {n["token"] for n in root["children"]}
                for child in container["children"]:
                    if child["token"] in shortcuts and child["token"] not in existing:
                        root["children"].append(copy.deepcopy(child))
        normalize(root, stats)
        roots.append(root)
    stats["roots"] = len(roots)
    if not roots or license_data is None:
        raise ValueError("archive lacks command roots or upstream license")
    catalog = dict(version=1, roots=roots)
    body = (json.dumps(catalog, ensure_ascii=False, separators=(",", ":")) + "\n").encode()
    metadata = dict(source="carapace-bin", release="v1.8.1", commit=COMMIT, license="MIT", archive_sha256=digest,
                    catalog_sha256=hashlib.sha256(body).hexdigest(), statistics=stats)
    return {"catalog.json": body, "SOURCE.json": (json.dumps(metadata, indent=2) + "\n").encode(), "LICENSE.carapace": license_data}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--out", type=Path, default=Path("crates/nyaterm-core/assets/completion"))
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    for name, content in generate(args.archive).items():
        output = args.out / name
        if args.check:
            if not output.exists() or output.read_bytes() != content:
                raise SystemExit(f"generated artifact differs: {output}")
        else:
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(content)


if __name__ == "__main__":
    main()
