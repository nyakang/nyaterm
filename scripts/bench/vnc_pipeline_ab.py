"""Compare isolated VNC helper/CPU pipelines using synthetic loopback pixels."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import statistics
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXAMPLE = Path("crates/nyaterm-remote-desktop/examples/vnc_pipeline_probe.rs")
SUFFIX = ".exe" if platform.system() == "Windows" else ""


def run(command, cwd=ROOT, **kwargs):
    return subprocess.check_output(command, cwd=cwd, text=True, encoding="utf-8", **kwargs).strip()


def legacy_source(source: str) -> str:
    # The historical crate has no presentation API. Adapt only the consumer;
    # keep the same mock server, clock, cadence, pixel validation and JSON fields.
    start, end = source.index("    let pipeline ="), source.index("    let id =")
    source = source[:start] + "    let pipeline = false;\n    let manager = VncSessionManager::new();\n" + source[end:]
    source = source.replace("    manager.set_presentation_visible(&id, true, presentation_config);\n", "")
    source = source.replace("    let mut pipeline_pixels = None;\n", "")
    start, end = source.index("        if pipeline &&"), source.index("        processing +=")
    source = source[:start] + source[end:]
    start, end = source.index("    let pixels = if pipeline"), source.index("    let expected =")
    return source[:start] + "    let pixels = framebuffer.as_ref().unwrap().pixels();\n" + source[end:]


def build(repo: Path, destination: Path, legacy: bool):
    if legacy:
        example = repo / EXAMPLE
        if example.exists():
            raise SystemExit(f"Refusing to overwrite baseline probe: {example}; use --run-only or a fresh worktree")
        example.parent.mkdir(parents=True, exist_ok=True)
        example.write_text(legacy_source((ROOT / EXAMPLE).read_text(encoding="utf-8")), encoding="utf-8")
    target = destination / "cargo-target"
    # Separate target directories prevent same-version worktree fingerprints
    # from reusing the other implementation's helper or workspace libraries.
    for arguments in [
        ["-p", "nyaterm-vnc-helper", "--bin", "nyaterm-vnc-helper"],
        ["-p", "nyaterm-remote-desktop", "--example", "vnc_pipeline_probe"],
    ]:
        subprocess.run(["cargo", "build", "--release", "--locked", "--target-dir", str(target), *arguments], cwd=repo, check=True)
    shutil.copy2(target / "release" / f"nyaterm-vnc-helper{SUFFIX}", destination / f"helper{SUFFIX}")
    shutil.copy2(target / "release/examples" / f"vnc_pipeline_probe{SUFFIX}", destination / f"probe{SUFFIX}")


def fingerprint(path: Path):
    return {"sha256": hashlib.sha256(path.read_bytes()).hexdigest(), "name": path.name}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True, help="clean detached worktree at the pre-change revision")
    parser.add_argument("--output", type=Path, default=ROOT / "target/vnc-performance")
    parser.add_argument("--run-only", action="store_true")
    parser.add_argument("--baseline-probe", type=Path)
    parser.add_argument("--baseline-helper", type=Path)
    parser.add_argument("--probe", type=Path)
    parser.add_argument("--helper", type=Path)
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--large-rounds", type=int, default=1)
    parser.add_argument("--texture-probe", type=Path, help="optional native vnc_texture_probe; requires a desktop display")
    args = parser.parse_args()
    if args.rounds < 1 or args.large_rounds < 1:
        parser.error("round counts must be positive")
    output = args.output.resolve()
    variants = [("baseline", args.baseline.resolve(), args.baseline_probe, args.baseline_helper),
                ("pipeline", ROOT, args.probe, args.helper)]
    metadata = {"platform": platform.platform(), "rustc": run(["rustc", "--version"]), "profile": "release", "variants": {},
                "note": "elapsed includes helper startup and RFB handshake; consumer_cpu is timed work, not process CPU or GPU/display latency"}
    binaries = {}
    for label, repo, probe, helper in variants:
        destination = output / label
        destination.mkdir(parents=True, exist_ok=True)
        if not args.run_only:
            build(repo, destination, label == "baseline")
        probe = (probe or destination / f"probe{SUFFIX}").resolve()
        helper = (helper or destination / f"helper{SUFFIX}").resolve()
        binaries[label] = (probe, helper)
        metadata["variants"][label] = {"revision": run(["git", "rev-parse", "HEAD"], repo),
            "dirty": bool(run(["git", "status", "--porcelain"], repo)),
            "cargo_lock_sha256": hashlib.sha256((repo / "Cargo.lock").read_bytes()).hexdigest(),
            "probe": fingerprint(probe), "helper": fingerprint(helper)}
    records = []
    for count, interval, rounds in [(1000, 16, args.rounds), (1000, 33, args.rounds), (65535, 16, args.large_rounds)]:
        for round_index in range(rounds):
            labels = ["baseline", "pipeline"] if round_index % 2 == 0 else ["pipeline", "baseline"]
            for label in labels:
                probe, helper = binaries[label]
                env = dict(os.environ, NYATERM_VNC_HELPER=str(helper))
                command = [str(probe), str(count), str(interval)] + (["pipeline"] if label == "pipeline" else [])
                record = dict(json.loads(run(command, env=env, timeout=190)), variant=label, round=round_index)
                records.append(record)
                print(json.dumps(record), flush=True)
                (output / "results.json").write_text(json.dumps({"metadata": metadata, "records": records}, indent=2), encoding="utf-8")
    for count, interval in [(1000, 16), (1000, 33), (65535, 16)]:
        medians = {label: statistics.median(row["elapsed_ms"] for row in records
                   if row["variant"] == label and row["rectangles"] == count and row["interval_ms"] == interval)
                   for label in binaries}
        print(json.dumps({"rectangles": count, "interval_ms": interval, "median_elapsed_ms": medians,
                          "speedup": medians["baseline"] / medians["pipeline"]}))
    if args.texture_probe:
        native = []
        for width, height in [(1920, 1080), (3840, 2160)]:
            for fixture in ["full", "column"]:
                for mode in ["borrowed", "owned"]:
                    record = json.loads(run([str(args.texture_probe.resolve()), str(width), str(height), fixture, mode, "120"], timeout=90))
                    native.append(record)
                    print(json.dumps(record), flush=True)
        (output / "native.json").write_text(json.dumps({"platform": platform.platform(), "probe": fingerprint(args.texture_probe.resolve()),
                                                       "records": native}, indent=2), encoding="utf-8")
    print(output / "results.json")


if __name__ == "__main__":
    main()
