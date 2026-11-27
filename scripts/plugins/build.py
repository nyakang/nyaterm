"""Build SDK guests, turn their core modules into components, validate and package.

Requires Rust's wasm32-unknown-unknown target. No cargo-component, WASI adapter,
network access by the guest, or desktop linkage is needed.
"""
from pathlib import Path
import argparse
import json
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def run(*args):
    subprocess.run(args, cwd=ROOT, check=True)


def cargo_target(manifest):
    metadata = subprocess.check_output(
        ["cargo", "metadata", "--manifest-path", str(manifest), "--locked", "--no-deps", "--format-version", "1"],
        cwd=ROOT, text=True,
    )
    return Path(json.loads(metadata)["target_directory"])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--fixtures", action="store_true", help="Also build adversarial SDK test fixtures")
    parser.add_argument("--check", action="store_true", help="Run key contract and host tests after building")
    options = parser.parse_args()
    run("cargo", "build", "--manifest-path", "examples/plugins/guests/Cargo.toml", "--locked", "--release", "--target", "wasm32-unknown-unknown")
    run("cargo", "build", "--locked", "-p", "nyaterm-plugin-host", "--bin", "nyaterm-plugin-tool")
    tool = cargo_target(ROOT / "Cargo.toml") / "debug/nyaterm-plugin-tool"
    if not tool.exists():
        tool = tool.with_suffix(".exe")
    output = ROOT / "target/plugin-examples"
    output.mkdir(parents=True, exist_ok=True)
    guest_target = cargo_target(ROOT / "examples/plugins/guests/Cargo.toml")
    for name in ["templates", "diagnostic-command", "text-tools", "remote-logs", "upload-deploy"]:
        destination = output / name
        shutil.copytree(ROOT / "examples/plugins" / name, destination, dirs_exist_ok=True)
        if name != "templates":
            guest_name = "remote_tools" if name in {"remote-logs", "upload-deploy"} else name.replace("-", "_")
            core = guest_target / "wasm32-unknown-unknown/release" / (guest_name + ".wasm")
            run(str(tool), "component", str(core), str(destination / "plugin.wasm"))
        run(str(tool), "pack", str(destination), str(output / (name + ".zip")))
    if options.fixtures or options.check:
        destination = ROOT / "crates/nyaterm-plugin-host/tests/fixtures"
        destination.mkdir(parents=True, exist_ok=True)
        core = guest_target / "wasm32-unknown-unknown/release/lifecycle_fixture.wasm"
        run(str(tool), "component", str(core), str(destination / "lifecycle.wasm"))
        for name in ["diagnostic-command", "text-tools"]:
            shutil.copyfile(output / name / "plugin.wasm", destination / (name + ".wasm"))
    if options.check:
        run("cargo", "test", "--locked", "-p", "nyaterm-core", "--test", "plugins")
        run("cargo", "test", "--locked", "-p", "nyaterm-store", "plugin_preferences")
        run("cargo", "test", "--locked", "-p", "nyaterm-plugin-host")


if __name__ == "__main__":
    main()
