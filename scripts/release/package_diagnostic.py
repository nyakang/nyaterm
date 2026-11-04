#!/usr/bin/env python3
"""Build a frozen Windows x64 source with symbols and bounded panic probes.

Ordinary release sources, packaging contracts and user data remain untouched.
The source worktree and exact instrumentation patch are retained for review.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
import zipfile
from datetime import datetime, timezone
from pathlib import Path

import diagnostic_source
import prepare_conpty
import verify_native_package


ROOT = Path(__file__).resolve().parents[2]
TARGET = "x86_64-pc-windows-msvc"
ORIGINAL_RUSTC = "58cf8f9c35b355199727627d07ae34a4e2846d67"


def run(command: list[str], *, cwd: Path, env: dict[str, str] | None = None) -> None:
    print("Running:", subprocess.list2cmdline(command), flush=True)
    subprocess.run(command, cwd=cwd, env=env, check=True)


def output(command: list[str], cwd: Path) -> str:
    return subprocess.check_output(command, cwd=cwd, text=True, encoding="utf-8").strip()


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for block in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def portable_contract(source: Path) -> dict:
    contract = source / "crates/nyaterm-core/src/packaging/windows-portable.json"
    if contract.is_file():
        return json.loads(contract.read_text(encoding="utf-8"))
    # v2.0.1 predates the shared JSON contract. Ask its own packaging module
    # instead of silently using today's helper list (it included ActiveX).
    return json.loads(output([sys.executable, "-c",
        "import sys,json; sys.path.insert(0, 'scripts/release'); import package_native as p; "
        f"t={TARGET!r}; "
        "print(json.dumps(dict(root='NyaTerm-portable', marker=p.PORTABLE_MARKER, "
        "data_placeholder='data/.keep', helper_files=[n+'.exe' for n in p.helper_bins(t)], "
        "conpty_files={t: ['conpty/'+n.as_posix() for n in p.conpty_files(t)]})))"], source))


def cargo_pdb_path(executable: Path) -> Path:
    # Cargo uses Rust crate identifiers for PDBs, retaining the executable's
    # hyphens only in the .exe name. Keep the CodeView PDB basename intact.
    return executable.with_name(executable.stem.replace("-", "_") + ".pdb")


def build_environment(build: Path, metadata: dict) -> dict[str, str]:
    environment = os.environ.copy()
    environment.pop("RUSTFLAGS", None)
    # Explicit codegen flags also cover dependencies even if a source ref has
    # package-specific Cargo profile overrides. PDBALTPATH makes symbol lookup
    # independent of the build machine's absolute directory.
    environment.update({
        "CARGO_TARGET_DIR": str(build / "cargo-target"),
        "CARGO_INCREMENTAL": "0",
        "CARGO_PROFILE_RELEASE_DEBUG": "2",
        "CARGO_PROFILE_RELEASE_STRIP": "none",
        "CARGO_PROFILE_RELEASE_OPT_LEVEL": "3",
        "CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS": "false",
        "CARGO_PROFILE_RELEASE_PANIC": "unwind",
        "CARGO_ENCODED_RUSTFLAGS": "\x1f".join([
            "-C", "target-feature=+crt-static", "-C", "debuginfo=2",
            "-C", "strip=none", "-C", "link-arg=/PDBALTPATH:%_PDB%",
        ]),
        "NYATERM_DIAGNOSTIC_ID": metadata["diagnostic_id"],
        "NYATERM_DIAGNOSTIC_SOURCE": metadata["source_sha"],
        "NYATERM_DIAGNOSTIC_PATCH": metadata["instrumentation_sha256"],
        "NYATERM_DIAGNOSTIC_RUSTC": metadata["rustc"].splitlines()[0],
    })
    return environment


def verify_symbols(binary: Path, pdb: Path, build: Path, expected_id: str) -> str:
    # Hide the original build PDB as well, proving that lookup works using only
    # the files the user receives. Always restore it, including on timeout.
    hidden = pdb.with_suffix(".pdb.symbol-test-hidden")
    if hidden.exists():
        raise FileExistsError(hidden)
    with tempfile.TemporaryDirectory(prefix="relocated-symbol-test-", dir=build) as directory:
        relocated = Path(directory)
        shutil.copy2(binary, relocated / "NyaTerm.exe")
        shutil.copy2(pdb, relocated / pdb.name)
        environment = os.environ.copy()
        for name in ["_NT_SYMBOL_PATH", "_NT_ALT_SYMBOL_PATH", "RUST_BACKTRACE", "RUST_LIB_BACKTRACE"]:
            environment.pop(name, None)
        pdb.rename(hidden)
        try:
            subprocess.run([str(relocated / "NyaTerm.exe"), "--nyaterm-diagnostic-self-test",
                            str(relocated / "reports")], cwd=relocated, env=environment,
                           check=True, timeout=90, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            reports = list((relocated / "reports").glob("panic-*.log"))
            if len(reports) != 1:
                raise RuntimeError(f"symbol self-test expected one report, got {len(reports)}")
            report = reports[0].read_text(encoding="utf-8")
            for required in ["nyaterm_app::diagnostic_build::symbol_probe",
                             f"Diagnostic-Build: {expected_id}",
                             "save_requested: count=1 first=1 last=1"]:
                if required not in report:
                    raise RuntimeError(f"relocated symbol self-test missing {required}")
            if not re.search(r"(?m)^\s+at .*diagnostic_build\.rs:\d+", report):
                raise RuntimeError("relocated symbol self-test has no source line in backtrace")
            return report
        finally:
            hidden.rename(pdb)


def package(source: Path, build: Path, destination: Path, metadata: dict) -> Path:
    contract = portable_contract(source)
    stage = destination / contract["root"]
    stage.mkdir()
    binaries = build / "cargo-target" / TARGET / "release"
    executables = [("nyaterm.exe", "NyaTerm.exe"),
                   *((name, name) for name in contract["helper_files"])]
    for original, packaged in executables:
        executable = binaries / original
        symbols = cargo_pdb_path(executable)
        if not executable.is_file() or not symbols.is_file():
            raise FileNotFoundError(f"missing executable/PDB pair: {executable}")
        if prepare_conpty.pe_machine(executable.read_bytes()) != 0x8664:
            raise RuntimeError(f"diagnostic executable is not x64: {executable}")
        shutil.copy2(executable, stage / packaged)
        shutil.copy2(symbols, stage / symbols.name)
    verify_native_package.verify_windows_application_binary(
        (stage / "NyaTerm.exe").read_bytes(), "NyaTerm.exe")
    verify_native_package.verify_windows_application_version(stage / "NyaTerm.exe", metadata["version"])
    run([sys.executable, "scripts/release/prepare_conpty.py"], cwd=source)
    conpty = source / "target/conpty-package/resources/windows/conpty"
    for name in contract["conpty_files"][TARGET]:
        target = stage / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(conpty / Path(name).relative_to("conpty"), target)
    shutil.copy2(source / "scripts/release/conpty-license.txt", stage / "conpty/LICENSE.txt")
    shutil.copy2(source / "LICENSE", stage / "LICENSE")
    (stage / "VERSION").write_text(metadata["version"] + "\n", encoding="utf-8")
    (stage / contract["marker"]).touch()
    placeholder = stage / contract["data_placeholder"]
    placeholder.parent.mkdir(parents=True)
    placeholder.touch()
    shutil.copy2(diagnostic_source.TEMPLATES / "README-user.txt", stage / "README-diagnostic.txt")
    report = verify_symbols(binaries / "nyaterm.exe", binaries / "nyaterm.pdb", build,
                            metadata["diagnostic_id"])
    (destination / "symbol-self-test.log").write_text(report, encoding="utf-8")
    metadata["relocated_symbol_self_test"] = "passed"
    metadata["payload_sha256"] = {
        path.relative_to(stage).as_posix(): sha256(path)
        for path in sorted(stage.rglob("*")) if path.is_file()
    }
    (stage / "build-info.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    archive = destination / f'NyaTerm_{metadata["version"]}_windows_x64_diagnostic_{metadata["diagnostic_id"]}.zip'
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as file:
        for path in sorted(stage.rglob("*")):
            if path.is_file():
                file.write(path, f'{contract["root"]}/{path.relative_to(stage).as_posix()}')
    archive.with_suffix(".zip.sha256").write_text(f"{sha256(archive)}  {archive.name}\n", encoding="utf-8")
    return archive


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-ref", default="v2.0.1", help="frozen source ref (default: v2.0.1)")
    parser.add_argument("--toolchain", default="1.99.1", help="installed Rust toolchain (default: 1.99.1)")
    parser.add_argument("--id", default="issue1065-" + datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ"))
    parser.add_argument("--prepare-only", action="store_true", help="prepare source and patch without compiling")
    parser.add_argument("--resume", action="store_true", help="resume a prepared build with the same ID/ref/toolchain")
    parser.add_argument("--jobs", type=int, default=4, help="Cargo build jobs (default: 4, limits symbol-build memory)")
    args = parser.parse_args()
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,79}", args.id):
        parser.error("--id must be 1-80 ASCII letters, digits, dots, underscores or hyphens")
    if args.jobs < 1:
        parser.error("--jobs must be positive")
    if not args.prepare_only and sys.platform != "win32":
        parser.error("build on Windows in an x64 MSVC developer shell")
    source_sha = output(["git", "rev-parse", "--verify", "--end-of-options", f"{args.source_ref}^{{commit}}"], ROOT)
    build = ROOT / "target/diagnostic-build" / args.id
    destination = ROOT / "dist" / f"diagnostic-{args.id}"
    if (build.exists() or destination.exists()) and not args.resume:
        raise FileExistsError("diagnostic build ID already exists; choose a new --id")
    source = build / "source"
    rustc = output(["rustup", "run", args.toolchain, "rustc", "-Vv"], ROOT)
    cargo_lock = source / "Cargo.lock"
    if args.resume:
        metadata = json.loads((destination / "build-info.json").read_text(encoding="utf-8"))
        patch = subprocess.check_output(["git", "diff", "--binary", "HEAD"], cwd=source)
        if (metadata["source_sha"] != source_sha or metadata["rustc"] != rustc
                or metadata["diagnostic_id"] != args.id
                or output(["git", "rev-parse", "HEAD"], source) != source_sha
                or hashlib.sha256(patch).hexdigest() != metadata["instrumentation_sha256"]
                or sha256(cargo_lock) != metadata["cargo_lock_sha256"]):
            raise RuntimeError("prepared diagnostic sources or build inputs changed; use a new ID")
        if (destination / "NyaTerm-portable").exists():
            raise FileExistsError("packaging already started; use a new ID")
    else:
        build.mkdir(parents=True)
        destination.mkdir(parents=True)
        run(["git", "worktree", "add", "--detach", str(source), source_sha], cwd=ROOT)
        changed = diagnostic_source.apply_overlay(source)
        run(["rustup", "run", args.toolchain, "rustfmt", "--edition", "2024",
             "--config", "skip_children=true", *changed], cwd=source)
        # Intent-to-add makes the new modules part of the exact, reviewable patch.
        run(["git", "add", "--intent-to-add", "--", *changed], cwd=source)
        patch = subprocess.check_output(["git", "diff", "--binary", "HEAD"], cwd=source)
        (destination / "diagnostic.patch").write_bytes(patch)
        metadata = {
            "diagnostic_id": args.id, "source_ref": args.source_ref, "source_sha": source_sha,
            "instrumentation_sha256": hashlib.sha256(patch).hexdigest(),
            "instrumentation_base_sha": output(["git", "rev-parse", "HEAD"], ROOT),
            "toolchain": args.toolchain, "rustc": rustc,
            "matches_issue1065_compiler": f"commit-hash: {ORIGINAL_RUSTC}" in rustc,
            "target": TARGET,
            "version": tomllib.loads((source / "Cargo.toml").read_text())["workspace"]["package"]["version"],
            "cargo_lock_sha256": sha256(cargo_lock), "profile": "release", "debuginfo": 2, "strip": "none",
            "created_utc": datetime.now(timezone.utc).isoformat(),
        }
    (destination / "build-info.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    print(f"Prepared diagnostic source: {source}", flush=True)
    if args.prepare_only:
        return
    environment = build_environment(build, metadata)
    contract = portable_contract(source)
    command = ["rustup", "run", args.toolchain, "cargo", "build", "--release", "--locked", "--jobs", str(args.jobs), "--target", TARGET,
               "-p", "nyaterm-app", "--bin", "nyaterm"]
    for helper in contract["helper_files"]:
        command.extend(["-p", Path(helper).stem, "--bin", Path(helper).stem])
    log = destination / "build.log"
    print(f"Building frozen sources; compiler output: {log}", flush=True)
    with log.open("a", encoding="utf-8") as writer:
        result = subprocess.run(command, cwd=source, env=environment, stdout=writer, stderr=subprocess.STDOUT)
    if result.returncode:
        raise RuntimeError(f"Cargo build failed ({result.returncode}); see {log}")
    if sha256(cargo_lock) != metadata["cargo_lock_sha256"]:
        raise RuntimeError("frozen Cargo.lock changed during build")
    archive = package(source, build, destination, metadata)
    (destination / "build-info.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    print(f"Diagnostic package verified: {archive}", flush=True)


if __name__ == "__main__":
    main()
