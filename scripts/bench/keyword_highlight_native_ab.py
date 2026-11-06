"""Measure real GPUI surfaces with synthetic output; Windows desktop required.

Each process opens a native window. These measurements include the scheduler
and renderer, but do not represent physical keyboard or monitor latency.
"""
from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import time

from renderer_ab import working_set

ROOT = Path(__file__).resolve().parents[2]
PACKAGE = Path("crates/nyaterm-desktop")
MODULE = PACKAGE / "src/features/terminal/terminal_surface_entity"


def command(args, cwd=ROOT):
    return subprocess.check_output(args, cwd=cwd, text=True, encoding="utf-8").strip()


def build(repo: Path, label: str, output: Path, target: Path, profile: str) -> Path:
    if repo != ROOT:
        source = (ROOT / MODULE / "native_probe.rs").read_text(encoding="utf-8")
        module = repo / MODULE / "mod.rs"
        text = module.read_text(encoding="utf-8")
        if "line_roles: bool" not in text:
            source = source.replace("surface.line_roles = roles; // HISTORICAL ROLE ADAPTER", "let _ = roles;")
        if "merge_keyword_highlight_rules_for_paint_with_ansi" not in (repo / "crates/nyaterm-core/src/keyword_highlight_presets.rs").read_text(encoding="utf-8"):
            start = source.index("nyaterm_core::keyword_highlight_presets::merge_keyword_highlight_rules_for_paint_with_ansi(")
            end = source.index("));", start)
            source = source[:start] + 'nyaterm_core::keyword_highlight_presets::get_builtin_keyword_rules(theme != "github-light")' + source[end+1:]
        (repo / MODULE / "native_probe.rs").write_text(source, encoding="utf-8")
        if "mod native_probe;" not in text:
            module.write_text(text + "\n#[cfg(test)]\nmod native_probe;\n", encoding="utf-8")
        manifest = repo / PACKAGE / "Cargo.toml"
        text = manifest.read_text(encoding="utf-8").replace('gpui = { workspace = true, features = ["test-support"] }', 'gpui = { workspace = true, features = ["test-support", "profiler"] }')
        if 'gpui_platform = { workspace = true, features = ["test-support"] }' not in text:
            text = text.replace("[dev-dependencies]", '[dev-dependencies]\ngpui_platform = { workspace = true, features = ["test-support"] }')
        manifest.write_text(text, encoding="utf-8")
    packages = json.loads(command(["cargo", "metadata", "--no-deps", "--format-version", "1"], repo))["packages"]
    # Clean only workspace packages to prevent same-version worktree artifacts
    # being reused; third-party dependencies keep their normal Cargo cache.
    if os.environ.get("NYATERM_BENCH_RESUME") != label:
        subprocess.run(["cargo", "clean", "--profile", profile, "--target-dir", str(target), *[flag for package in packages for flag in ("-p", package["name"])]], cwd=repo, check=True)
    build_result = subprocess.run(["cargo", "test", "--profile", profile, "--jobs", "2", "-p", "nyaterm-desktop", "--lib", "--no-run", "--message-format=json", "--target-dir", str(target)], cwd=repo, text=True, encoding="utf-8", stdout=subprocess.PIPE)
    build_output = build_result.stdout
    (output / f"{label}-build.jsonl").write_text(build_output, encoding="utf-8")
    if build_result.returncode:
        for line in build_output.splitlines():
            item = json.loads(line)
            if item.get("reason") == "compiler-message":
                print(item["message"].get("rendered", ""), flush=True)
        build_result.check_returncode()
    executable = next(Path(item["executable"]) for line in build_output.splitlines() if (item := json.loads(line)).get("reason") == "compiler-artifact" and item.get("executable") and item["target"]["name"] == "nyaterm_desktop")
    destination = output / f"{label}.exe"
    shutil.copy2(executable, destination)
    return destination


def cpu_time(process) -> float | None:
    from ctypes import wintypes
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.GetProcessTimes.argtypes = [wintypes.HANDLE, *[ctypes.POINTER(wintypes.FILETIME)] * 4]
    values = [wintypes.FILETIME() for _ in range(4)]
    if not kernel.GetProcessTimes(int(process._handle), *[ctypes.byref(value) for value in values]):
        return None
    return sum((value.dwHighDateTime << 32) | value.dwLowDateTime for value in values[2:]) / 10_000_000


def probe(binary, case, roles, theme, screenshot=None):
    env = dict(os.environ, NYATERM_HIGHLIGHT_PROBE_CASE=case, NYATERM_HIGHLIGHT_PROBE_ROLES=str(int(roles)), NYATERM_HIGHLIGHT_PROBE_THEME=theme)
    if screenshot:
        env["NYATERM_HIGHLIGHT_PROBE_SCREENSHOT"] = str(screenshot)
    started = time.monotonic()
    process = subprocess.Popen([str(binary), "keyword_highlight_native_probe", "--ignored", "--nocapture", "--test-threads=1"], cwd=ROOT, env=env, text=True, encoding="utf-8", stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    peak = 0
    while True:
        peak = max(peak, working_set(process.pid) or 0)
        try:
            stdout, stderr = process.communicate(timeout=0.05)
            break
        except subprocess.TimeoutExpired:
            if time.monotonic() - started > 90:
                process.kill()
                process.communicate()
                raise TimeoutError(f"native probe timed out: {binary} {case}")
    if process.returncode:
        raise RuntimeError(f"native probe failed: {stdout}\n{stderr}")
    result = next(json.loads(line[line.index('{'):]) for line in stdout.splitlines() if '"synthetic_frame_to_present"' in line)
    result.update(cpu_seconds=cpu_time(process), wall_seconds=time.monotonic() - started, peak_working_set_bytes=peak)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ancestor", type=Path, required=True)
    parser.add_argument("--original", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=ROOT / "target/pr1098/native-ab")
    parser.add_argument("--target", type=Path, default=ROOT / "target/pr1098/keyword-ab/cargo-target")
    parser.add_argument("--profile", choices=["dev", "release"], default="release")
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--build-only", action="store_true")
    parser.add_argument("--run-only", action="store_true")
    parser.add_argument("--resume", action="store_true", help="retain successful samples and rerun missing combinations")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    variants = [("ancestor", args.ancestor.resolve()), ("original", args.original.resolve()), ("optimized", ROOT)]
    metadata = {"platform": platform.platform(), "rustc": command(["rustc", "--version"]), "profile": args.profile, "repeats": args.repeats, "variants": {}}
    for label, repo in variants:
        binary = output / f"{label}.exe" if args.run_only else build(repo, label, output, args.target.resolve(), args.profile)
        metadata["variants"][label] = {"revision": command(["git", "rev-parse", "HEAD"], repo), "cargo_lock_sha256": hashlib.sha256((repo / "Cargo.lock").read_bytes()).hexdigest(), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
    (output / "metadata.json").write_text(json.dumps(metadata, indent=2), encoding="utf-8")
    if args.build_only:
        return
    result_path = output / "results.json"
    records = json.loads(result_path.read_text(encoding="utf-8")) if args.resume and result_path.exists() else []
    completed = {(item["variant"], item["repeat"], item["case"], item["roles"]) for item in records}
    for repeat in range(args.repeats):
        for label, _ in variants if repeat % 2 == 0 else reversed(variants):
            for case in ["typing", "output", "scroll"]:
                for roles in [False, True] if repeat % 2 == 0 else [True, False]:
                    if (label, repeat, case, roles) in completed:
                        continue
                    result = probe(output / f"{label}.exe", case, roles, "github-dark")
                    records.append(dict(result, variant=label, repeat=repeat))
                    result_path.write_text(json.dumps(records, indent=2), encoding="utf-8")
    for theme in ["github-dark", "github-light"]:
        probe(output / "optimized.exe", "typing", True, theme, output / f"{theme}.png")
    print(output / "results.json")


if __name__ == "__main__":
    main()
