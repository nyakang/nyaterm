from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/release"))

import diagnostic_source  # noqa: E402
import package_diagnostic  # noqa: E402


SOURCE_FILES = [
    "crates/nyaterm-core/src/lib.rs",
    "crates/nyaterm-app/src/lib.rs",
    "crates/nyaterm-app/src/main.rs",
    "crates/nyaterm-app/src/crash_report.rs",
    "crates/nyaterm-desktop/src/features/connections/connection_runtime/editor.rs",
    "crates/nyaterm-desktop/src/features/connections/connection_runtime/window.rs",
    "crates/nyaterm-desktop/src/features/session/session_runtime/background.rs",
    "crates/nyaterm-transport/src/lib.rs",
    "crates/nyaterm-terminal-gpui/src/element/mod.rs",
]


def source_files(revision: str | None = None) -> list[str]:
    if revision is not None:
        return SOURCE_FILES
    return [name.replace("crates/nyaterm-transport/src/lib.rs", "crates/nyaterm-transport/src/session.rs")
            for name in SOURCE_FILES]


def populate(source: Path, revision: str | None = None) -> None:
    for name in source_files(revision):
        data = ((ROOT / name).read_bytes() if revision is None else subprocess.check_output(
            ["git", "show", f"{revision}:{name}"], cwd=ROOT))
        path = source / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)


class DiagnosticSourceTests(unittest.TestCase):
    def test_overlay_supports_original_release_and_current_source_without_manifest_changes(self) -> None:
        for revision in ["v2.0.1", None]:
            with self.subTest(revision=revision), tempfile.TemporaryDirectory() as directory:
                source = Path(directory)
                populate(source, revision)
                changed = diagnostic_source.apply_overlay(source)
                self.assertEqual(set(changed), set(source_files(revision)) | {
                    "crates/nyaterm-core/src/diagnostic_probe.rs",
                    "crates/nyaterm-app/src/diagnostic_build.rs",
                })
                element = (source / "crates/nyaterm-terminal-gpui/src/element/mod.rs").read_text(encoding="utf-8")
                self.assertEqual(element.count("Stage::TerminalShape);"), 3)
                self.assertEqual(element.count("Stage::TerminalShaped);"), 3)
                main = (source / "crates/nyaterm-app/src/main.rs").read_text(encoding="utf-8")
                self.assertLess(main.index("run_self_test_if_requested"), main.index("AppRuntime::resolve"))
                # Unknown revisions and accidental double application fail closed.
                with self.assertRaises(ValueError):
                    diagnostic_source.apply_overlay(source)

    def test_unknown_anchor_leaves_all_source_files_untouched(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)
            populate(source)
            file = source / SOURCE_FILES[-1]
            file.write_text(file.read_text(encoding="utf-8").replace("fn prepaint(", "fn renamed_prepaint("), encoding="utf-8")
            before = {name: (source / name).read_bytes() for name in source_files()}
            with self.assertRaisesRegex(ValueError, "prepaint"):
                diagnostic_source.apply_overlay(source)
            self.assertEqual(before, {name: (source / name).read_bytes() for name in source_files()})
            self.assertFalse((source / "crates/nyaterm-core/src/diagnostic_probe.rs").exists())

    def test_missing_or_duplicate_pty_implementation_leaves_sources_untouched(self) -> None:
        for duplicate in [False, True]:
            with self.subTest(duplicate=duplicate), tempfile.TemporaryDirectory() as directory:
                source = Path(directory)
                populate(source)
                transport = source / "crates/nyaterm-transport/src/session.rs"
                if duplicate:
                    transport.with_name("duplicate.rs").write_bytes(transport.read_bytes())
                else:
                    transport.write_text("// PTY implementation moved unexpectedly\n", encoding="utf-8")
                before = {path: path.read_bytes() for path in source.rglob("*.rs")}
                with self.assertRaisesRegex(ValueError, "expected one implementation"):
                    diagnostic_source.apply_overlay(source)
                self.assertEqual(before, {path: path.read_bytes() for path in source.rglob("*.rs")})

    def test_original_release_uses_its_own_helper_list_including_activex(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)
            for name in ["package_native.py", "prepare_conpty.py", "windows_msi.py"]:
                path = source / "scripts/release" / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(subprocess.check_output(
                    ["git", "show", f"v2.0.1:scripts/release/{name}"], cwd=ROOT))
            contract = package_diagnostic.portable_contract(source)
            self.assertIn("nyaterm-activex-helper.exe", contract["helper_files"])
            self.assertIn("conpty/x64/arm64/OpenConsole.exe", contract["conpty_files"][package_diagnostic.TARGET])

    def test_build_flags_override_inherited_symbol_stripping_for_dependencies(self) -> None:
        with patch.dict(os.environ, {"RUSTFLAGS": "-C strip=symbols", "CARGO_ENCODED_RUSTFLAGS": "-C\x1fdebuginfo=0"}):
            environment = package_diagnostic.build_environment(Path("diagnostic-build"), {
                "diagnostic_id": "test", "source_sha": "source", "instrumentation_sha256": "patch",
                "rustc": "rustc test\ncommit-hash: test",
            })
        self.assertNotIn("RUSTFLAGS", environment)
        self.assertIn("debuginfo=2", environment["CARGO_ENCODED_RUSTFLAGS"])
        self.assertIn("strip=none", environment["CARGO_ENCODED_RUSTFLAGS"])
        self.assertIn("target-feature=+crt-static", environment["CARGO_ENCODED_RUSTFLAGS"])


@unittest.skipUnless(shutil.which("rustc"), "requires Rust")
class DiagnosticRuntimeTests(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "win32" and shutil.which("cargo"), "requires Windows and Cargo")
    def test_helper_pdb_name_matches_real_cargo_output(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)
            (source / "src").mkdir()
            (source / "Cargo.toml").write_text(
                '[package]\nname="nyaterm-test-helper"\nversion="0.1.0"\nedition="2024"\n',
                encoding="utf-8")
            (source / "src/main.rs").write_text("fn main() {}\n", encoding="utf-8")
            subprocess.run(["cargo", "build", "--offline"], cwd=source,
                           env={**os.environ, "CARGO_TARGET_DIR": str(source / "target")},
                           check=True, capture_output=True, timeout=60)
            executable = source / "target/debug/nyaterm-test-helper.exe"
            self.assertTrue(executable.is_file())
            self.assertTrue(package_diagnostic.cargo_pdb_path(executable).is_file())

    def test_breadcrumbs_remain_bounded_and_survive_repeated_frames(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)
            shutil.copy2(diagnostic_source.TEMPLATES / "diagnostic_probe.rs", source)
            (source / "lib.rs").write_text("mod diagnostic_probe;\n", encoding="utf-8")
            executable = source / ("probe-tests.exe" if sys.platform == "win32" else "probe-tests")
            subprocess.run(["rustc", "--edition=2024", "--test", "lib.rs", "-o", str(executable)],
                           cwd=source, check=True, capture_output=True)
            subprocess.run([str(executable)], cwd=source, check=True, capture_output=True)

    def test_real_panic_hook_writes_metadata_and_probes_before_backtrace(self) -> None:
        # Compile the production hook and diagnostic modules without linking
        # GPUI. This checks the actual Rust API contracts, not a mock formatter.
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)
            populate(source)
            diagnostic_source.apply_overlay(source)
            shutil.copy2(source / "crates/nyaterm-app/src/crash_report.rs", source)
            shutil.copy2(diagnostic_source.TEMPLATES / "diagnostic_build.rs", source)
            shutil.copy2(diagnostic_source.TEMPLATES / "diagnostic_probe.rs", source)
            (source / "main.rs").write_text(
                "extern crate self as nyaterm_core;\n"
                "pub mod diagnostic_probe; mod diagnostic_build; mod crash_report;\n"
                "fn main() { assert!(diagnostic_build::run_self_test_if_requested().unwrap()); }\n",
                encoding="utf-8")
            environment = {**os.environ, "CARGO_PKG_VERSION": "2.0.1",
                           "NYATERM_DIAGNOSTIC_ID": "test-1065", "NYATERM_DIAGNOSTIC_SOURCE": "source",
                           "NYATERM_DIAGNOSTIC_PATCH": "patch", "NYATERM_DIAGNOSTIC_RUSTC": "test-rustc"}
            executable = source / ("probe.exe" if sys.platform == "win32" else "probe")
            subprocess.run(["rustc", "--edition=2024", "--crate-name", "nyaterm_app", "main.rs",
                            "-C", "debuginfo=2", "-o", str(executable)],
                           cwd=source, env=environment, check=True, capture_output=True)
            subprocess.run([str(executable), "--nyaterm-diagnostic-self-test", str(source / "reports")],
                           cwd=source, check=True, capture_output=True, timeout=60)
            reports = list((source / "reports").glob("panic-*.log"))
            self.assertEqual(len(reports), 1)
            report = reports[0].read_text(encoding="utf-8")
            self.assertIn("Diagnostic-Build: test-1065", report)
            self.assertIn("save_requested: count=1 first=1 last=1", report)
            self.assertLess(report.index("Diagnostic-Stages"), report.index("Backtrace:"))
            self.assertIn("diagnostic_build::symbol_probe", report)
            if sys.platform == "win32":
                # Exercise the same relocation gate used for the deliverable.
                relocated_report = package_diagnostic.verify_symbols(
                    executable, executable.with_suffix(".pdb"), source, "test-1065")
                self.assertIn("nyaterm_app::diagnostic_build::symbol_probe", relocated_report)
                self.assertTrue(executable.with_suffix(".pdb").is_file())


if __name__ == "__main__":
    unittest.main()
