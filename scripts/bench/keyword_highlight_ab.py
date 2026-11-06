"""Build immutable release probes from three worktrees and compare keyword paths."""
from __future__ import annotations
import argparse
import csv
import hashlib
import json
import platform
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PACKAGE = Path("crates/nyaterm-terminal-gpui")

def run(args, cwd=ROOT):
    return subprocess.check_output(args, cwd=cwd, text=True, encoding="utf-8").strip()

def build(repo: Path, label: str, output: Path, target: Path) -> Path:
    source = (ROOT / PACKAGE / "examples/keyword_highlight_probe.rs").read_text(encoding="utf-8")
    keywords = (repo / PACKAGE / "src/keywords.rs").read_text(encoding="utf-8")
    if "struct TerminalKeywordHighlightConfig" not in keywords:
        start, end = source.index("// ADAPTER BEGIN"), source.index("// ADAPTER END")
        argument = ", roles" if "    line_roles: bool," in keywords else ""
        adapter = "fn highlighter(rules: &[ResolvedKeywordHighlightRule], roles: bool) -> TerminalKeywordHighlighter {\n    let _ = roles;\n    compile_terminal_keyword_highlighter(rules" + argument + ")\n}\n"
        source = source[:start] + adapter + source[end:]
    example = repo / PACKAGE / "examples/keyword_highlight_probe.rs"
    if repo != ROOT:
        example.write_text(source, encoding="utf-8")
    # Cargo path-package fingerprints collide across worktrees with the same
    # version. Remove workspace artifacts before each variant; dependency
    # artifacts remain shared and each resulting executable is copied at once.
    packages = json.loads(run(["cargo", "metadata", "--no-deps", "--format-version", "1"], repo))["packages"]
    subprocess.run(["cargo", "clean", "--release", "--target-dir", str(target), *[flag for package in packages for flag in ("-p", package["name"])]], cwd=repo, check=True)
    subprocess.run(["cargo", "build", "--release", "--locked", "--jobs", "4", "-p", "nyaterm-terminal-gpui", "--example", "keyword_highlight_probe", "--target-dir", str(target)], cwd=repo, check=True)
    suffix = ".exe" if platform.system() == "Windows" else ""
    destination = output / f"{label}{suffix}"
    shutil.copy2(target / "release/examples" / f"keyword_highlight_probe{suffix}", destination)
    return destination

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ancestor", type=Path, required=True)
    parser.add_argument("--original", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=ROOT / "target/pr1098/keyword-ab")
    parser.add_argument("--build-only", action="store_true")
    parser.add_argument("--run-only", action="store_true")
    parser.add_argument("--rebuild", nargs="+", choices=["ancestor", "original", "optimized"], default=["ancestor", "original", "optimized"])
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    variants = [("ancestor", args.ancestor.resolve()), ("original", args.original.resolve()), ("optimized", ROOT)]
    metadata = {"platform": platform.platform(), "rustc": run(["rustc", "--version"]), "profile": "release", "rounds": 15, "variants": {}}
    for label, repo in variants:
        suffix = ".exe" if platform.system() == "Windows" else ""
        binary = output / f"{label}{suffix}"
        if not args.run_only and label in args.rebuild:
            binary = build(repo, label, output, output / "cargo-target")
        metadata["variants"][label] = {"revision": run(["git", "rev-parse", "HEAD"], repo), "dirty": bool(run(["git", "status", "--porcelain"], repo)), "cargo_lock_sha256": hashlib.sha256((repo / "Cargo.lock").read_bytes()).hexdigest(), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
    (output / "metadata.json").write_text(json.dumps(metadata, indent=2), encoding="utf-8")
    if args.build_only:
        return
    # Interleave variants across 15 rounds; each process alternates role order.
    records = []
    for round_index in range(15):
        for label, _ in variants if round_index % 2 == 0 else reversed(variants):
            suffix = ".exe" if platform.system() == "Windows" else ""
            raw = run([str(output / f"{label}{suffix}"), "1", str(round_index)])
            records.extend(dict(row, variant=label) for row in csv.DictReader(raw.splitlines()))
    with (output / "results.csv").open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=list(records[0]))
        writer.writeheader()
        writer.writerows(records)
    print(output / "results.csv")

if __name__ == "__main__":
    main()
