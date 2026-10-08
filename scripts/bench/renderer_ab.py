"""Run synthetic native GPUI A/B probes, optionally against an exact Git revision."""
from __future__ import annotations

import argparse
import ctypes
from datetime import datetime, timezone
import hashlib
import json
import os
import platform
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
PACKAGE = Path("crates/nyaterm-terminal-gpui")


def command(args: list[str], cwd: Path = ROOT) -> str:
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def binary_digest(path: Path) -> str:
    with path.open("rb") as binary:
        return hashlib.file_digest(binary, "sha256").hexdigest()


def working_set(pid: int) -> int | None:
    if os.name != "nt":
        return None
    from ctypes import wintypes

    class Counters(ctypes.Structure):
        _fields_ = [("cb", wintypes.DWORD), ("faults", wintypes.DWORD)] + [
            (name, ctypes.c_size_t) for name in (
                "peak_working_set", "working_set", "peak_paged", "paged",
                "peak_nonpaged", "nonpaged", "pagefile", "peak_pagefile",
            )
        ]

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    psapi.GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD]
    handle = kernel.OpenProcess(0x1000 | 0x10, False, pid)
    if not handle:
        return None
    try:
        counters = Counters()
        counters.cb = ctypes.sizeof(counters)
        if psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
            return counters.peak_working_set
        return None
    finally:
        kernel.CloseHandle(handle)


def build(repo: Path, output: Path, label: str, profile: str) -> Path:
    # Cargo workspace artifacts can collide across detached worktrees even
    # when freshness metadata differs. Keep historical builds fully isolated.
    target = repo / "target"
    subprocess.run([
        "cargo", "build", "--locked", "-p", "nyaterm-terminal-gpui", "--example",
        "renderer_probe", "--features", "renderer-bench", "--profile", profile,
        "--target-dir", str(target),
    ], cwd=repo, check=True)
    suffix = ".exe" if os.name == "nt" else ""
    source = target / ("debug" if profile == "dev" else profile) / "examples" / f"renderer_probe{suffix}"
    destination = output / f"{label}{suffix}"
    shutil.copy2(source, destination)
    return destination


def baseline_worktree(revision: str, existing: Path | None = None) -> tuple[Path, str]:
    resolved = command(["git", "rev-parse", f"{revision}^{{commit}}"])
    # Keep the detached worktree for inspection. No automatic recursive deletion.
    if existing is None:
        path = Path(tempfile.mkdtemp(prefix="nyaterm-renderer-baseline-"))
        subprocess.run(["git", "worktree", "add", "--detach", str(path), resolved], cwd=ROOT, check=True)
    else:
        path = existing.resolve()
        if command(["git", "rev-parse", "HEAD"], cwd=path) != resolved:
            raise ValueError("existing baseline worktree has a different revision")
        changed = command(["git", "diff", "--name-only"], cwd=path).splitlines()
        if set(changed) - {"Cargo.lock", (PACKAGE / "Cargo.toml").as_posix()}:
            raise ValueError("existing baseline has source changes")
    source = (ROOT / PACKAGE / "examples/renderer_probe.rs").read_text(encoding="utf-8")
    source = source.replace(".with_cell_glyph_rendering(self.cell_graphics)", "")
    source = source.replace('"font-control"', '"historical-font"')
    source = source.replace("font-control is not a historical build", "historical renderer; only benchmark harness and dependencies added")
    example = path / PACKAGE / "examples/renderer_probe.rs"
    example.parent.mkdir(parents=True, exist_ok=True)
    example.write_text(source, encoding="utf-8")
    manifest = path / PACKAGE / "Cargo.toml"
    text = manifest.read_text(encoding="utf-8")
    if "[features]" in text and not (existing and "renderer-bench" in text):
        raise ValueError("baseline already defines features; use the pre-cell-renderer revision")
    if "renderer-bench" not in text:
        text = text.replace("[dependencies]", '[dependencies]\ngpui_platform = { workspace = true, optional = true, features = ["test-support"] }\nserde_json = { workspace = true, optional = true }')
        text += '\n[features]\nrenderer-bench = ["gpui/test-support", "gpui/profiler", "dep:gpui_platform", "dep:serde_json"]\n\n[[example]]\nname = "renderer_probe"\nrequired-features = ["renderer-bench"]\n'
    manifest.write_text(text, encoding="utf-8")
    shutil.copy2(ROOT / "Cargo.lock", path / "Cargo.lock")
    # The comparison must compile the historical renderer without source edits.
    changed = command(["git", "diff", "--name-only"], cwd=path).splitlines()
    assert set(changed) <= {"Cargo.lock", (PACKAGE / "Cargo.toml").as_posix()}, changed
    return path, resolved


def run_probe(binary: Path, args: list[str]) -> dict:
    peak = None
    started = time.monotonic()
    process = subprocess.Popen([str(binary), *args], cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    while True:
        value = working_set(process.pid)
        if value is not None:
            peak = max(peak or 0, value)
        try:
            stdout, stderr = process.communicate(timeout=0.05)
            break
        except subprocess.TimeoutExpired:
            if time.monotonic() - started > 45:
                process.kill()
                process.communicate()
                raise TimeoutError(f"probe timed out: {args}")
    if process.returncode:
        raise RuntimeError(f"probe failed: {stderr}")
    result = json.loads(stdout.strip().splitlines()[-1])
    result["peak_process_working_set_bytes"] = peak
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", help="exact pre-cell-renderer revision, e.g. a39b82d99")
    parser.add_argument("--baseline-worktree", type=Path, help="reuse a worktree prepared by a previous probe run")
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/renderer-ab")
    parser.add_argument("--profile", choices=("dev", "release"), default="release")
    parser.add_argument("--scales", nargs="+", default=["native", "1.25", "1.5"])
    parser.add_argument("--fixtures", nargs="+", default=["text", "mixed", "blocks", "shade", "decorations"])
    parser.add_argument("--cache", nargs="+", choices=("hot", "cold"), default=["hot", "cold"])
    parser.add_argument("--repeats", type=int, default=3)
    args = parser.parse_args()
    assert args.repeats > 0
    if args.baseline_worktree and not args.baseline:
        parser.error("--baseline-worktree requires --baseline")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    current = build(ROOT, output, "current", args.profile)
    variants = [("current-cell", current, "cell"), ("current-font-control", current, "font")]
    metadata = {"current_revision": command(["git", "rev-parse", "HEAD"]), "current_dirty": bool(command(["git", "status", "--porcelain"])), "profile": args.profile, "started_utc": datetime.now(timezone.utc).isoformat(), "platform": platform.platform(), "rustc": command(["rustc", "--version"])}
    if args.baseline:
        baseline, resolved = baseline_worktree(args.baseline, args.baseline_worktree)
        metadata.update(baseline_revision=resolved, baseline_worktree=str(baseline))
        print(f"Historical worktree: {baseline}", flush=True)
        variants.append(("historical-font", build(baseline, output, "baseline", args.profile), "font"))
    metadata["binary_sha256"] = {
        label: binary_digest(binary)
        for label, binary, _ in variants
    }
    results = []
    for repeat in range(args.repeats):
        # Alternate order to reduce a consistent warmup/thermal bias.
        order = variants if repeat % 2 == 0 else variants[::-1]
        for kind in args.fixtures:
            for scale in args.scales:
                for cache in args.cache:
                    for label, binary, mode in order:
                        print(f"{label}: {kind} {scale} {cache}, repeat {repeat + 1}", flush=True)
                        probe_args = [mode, kind, cache, scale]
                        if repeat == 0 and kind == "decorations":
                            probe_args.append(str(output / f"{label}-{scale}-{cache}.png"))
                        result = run_probe(binary, probe_args)
                        result.update(variant=label, repeat=repeat + 1)
                        results.append(result)
                        (output / "results.json").write_text(json.dumps({"metadata": metadata, "results": results}, ensure_ascii=False, indent=2), encoding="utf-8")


if __name__ == "__main__":
    main()
