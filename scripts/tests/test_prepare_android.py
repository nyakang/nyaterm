from __future__ import annotations

from contextlib import contextmanager
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts" / "release"))

import prepare_android  # noqa: E402
import verify_native_package  # noqa: E402


LIBRARIES = {"avcodec": 62, "avformat": 62, "avutil": 60, "swscale": 9}
ARCHIVE = b"!<arch>\nfixture member"
X64 = "x86_64-pc-windows-msvc"
ARM64 = "aarch64-pc-windows-msvc"


def install_fixture(prefix: Path, *, upstream_imports: bool = True) -> None:
    (prefix / "bin").mkdir(parents=True, exist_ok=True)
    (prefix / "lib").mkdir(parents=True, exist_ok=True)
    for name, major in LIBRARIES.items():
        (prefix / "bin" / f"{name}-{major}.dll").write_bytes(b"fixture DLL")
        (prefix / "lib" / f"{name}-{major}.def").write_text("EXPORTS\n    fixture_export\n")
        if upstream_imports:
            (prefix / "bin" / f"{name}.lib").write_bytes(ARCHIVE)


def write_import_output(command: list[str], **kwargs) -> None:
    output = Path(next(arg.removeprefix("/OUT:") for arg in command if arg.startswith("/OUT:")))
    output.write_bytes(ARCHIVE)
    output.with_suffix(".exp").write_bytes(b"temporary export object")


class PrepareAndroidTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory(prefix="android dependencies ")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.staging = self.root / "staging"
        self.prefix = self.staging / "ffmpeg" / X64
        self.source = self.root / "source"
        self.source.mkdir()
        (self.source / "COPYING.LGPLv2.1").write_text("fixture license\n")
        self.commands = []

    @contextmanager
    def preparation(self, *, upstream_imports: bool = True):
        def run(command, **kwargs):
            self.commands.append(command)
            if command == ["make", "install"]:
                self.assertFalse((self.prefix / "SOURCE.json").exists())
                install_fixture(self.prefix, upstream_imports=upstream_imports)
            elif any(arg.startswith("/OUT:") for arg in command):
                write_import_output(command, **kwargs)

        with (
            mock.patch.object(prepare_android, "STAGING", self.staging),
            mock.patch.object(prepare_android, "download", return_value=self.root / "archive.tar.gz"),
            mock.patch.object(prepare_android, "extract_tar", return_value=self.source),
            mock.patch.object(prepare_android.subprocess, "run", side_effect=run),
        ):
            yield

    def assert_complete(self, prefix: Path | None = None) -> None:
        prefix = prefix or self.prefix
        for name in LIBRARIES:
            self.assertTrue(prepare_android.is_coff_archive(prefix / "lib" / f"{name}.lib"))
        self.assertEqual(sorted(path.name for path in (prefix / "lib").iterdir()),
                         sorted([f"{name}.lib" for name in LIBRARIES] +
                                [f"{name}-{major}.def" for name, major in LIBRARIES.items()]))

    def test_rejects_empty_truncated_and_non_archive_files(self) -> None:
        path = self.root / "library.lib"
        self.assertFalse(prepare_android.is_coff_archive(path))
        for payload in (b"", b"!<arch>\n", b"MZ executable", b"not an archive"):
            with self.subTest(payload=payload):
                path.write_bytes(payload)
                self.assertFalse(prepare_android.is_coff_archive(path))
        path.write_bytes(ARCHIVE)
        self.assertTrue(prepare_android.is_coff_archive(path))

    def test_fresh_install_copies_upstream_imports_before_writing_stamp(self) -> None:
        with self.preparation(), mock.patch.object(prepare_android, "find_msvc_lib") as tool:
            self.assertEqual(prepare_android.prepare_ffmpeg(X64), self.prefix)
            tool.assert_not_called()
        self.assert_complete()
        self.assertEqual(self.commands[-1], ["make", "install"])
        self.assertEqual((self.prefix / "LICENSE.txt").read_text(), "fixture license\n")
        self.assertEqual(json.loads((self.prefix / "SOURCE.json").read_text())["target"], X64)

    def test_complete_cache_preserves_import_libraries_without_build_or_tools(self) -> None:
        with self.preparation():
            prepare_android.prepare_ffmpeg(X64)
            stamp = (self.prefix / "SOURCE.json").read_bytes()
            before = {name: (self.prefix / "lib" / f"{name}.lib").stat().st_mtime_ns for name in LIBRARIES}
            self.commands.clear()
            with mock.patch.object(prepare_android, "find_msvc_lib") as tool:
                prepare_android.prepare_ffmpeg(X64)
                tool.assert_not_called()
            self.assertEqual(self.commands, [])
            self.assertEqual(stamp, (self.prefix / "SOURCE.json").read_bytes())
            self.assertEqual(before, {name: (self.prefix / "lib" / f"{name}.lib").stat().st_mtime_ns for name in LIBRARIES})

    def test_cache_repairs_missing_and_invalid_imports_from_bin_without_rebuilding(self) -> None:
        with self.preparation():
            prepare_android.prepare_ffmpeg(X64)
            (self.prefix / "lib" / "avcodec.lib").unlink()
            (self.prefix / "lib" / "avutil.lib").write_bytes(b"")
            (self.prefix / "lib" / "avformat.lib").write_bytes(b"invalid archive")
            self.commands.clear()
            with mock.patch.object(prepare_android, "find_msvc_lib") as tool:
                prepare_android.prepare_ffmpeg(X64)
                tool.assert_not_called()
            self.assertEqual(self.commands, [])
        self.assert_complete()

    def test_cache_generates_import_when_both_installed_copies_are_missing(self) -> None:
        with self.preparation():
            prepare_android.prepare_ffmpeg(X64)
            for directory in ("bin", "lib"):
                (self.prefix / directory / "avcodec.lib").unlink()
            self.commands.clear()
            with mock.patch.object(prepare_android, "find_msvc_lib", return_value=self.root / "MSVC tools/lib.exe"):
                prepare_android.prepare_ffmpeg(X64)
            self.assertEqual(len(self.commands), 1)
            self.assertIn("/NAME:avcodec-62.dll", self.commands[0])
        self.assert_complete()

    def test_cache_with_missing_dll_fails_without_rebuilding_or_accepting_stamp(self) -> None:
        with self.preparation():
            prepare_android.prepare_ffmpeg(X64)
            stamp = (self.prefix / "SOURCE.json").read_bytes()
            (self.prefix / "bin/avcodec-62.dll").unlink()
            self.commands.clear()
            with self.assertRaisesRegex(RuntimeError, "Missing or empty FFmpeg DLL"):
                prepare_android.prepare_ffmpeg(X64)
            self.assertEqual(self.commands, [])
            self.assertEqual(stamp, (self.prefix / "SOURCE.json").read_bytes())

    def test_generation_uses_target_machine_versioned_dll_and_absolute_paths(self) -> None:
        for target, machine in ((X64, "X64"), (ARM64, "ARM64")):
            with self.subTest(target=target):
                prefix = self.root / target
                install_fixture(prefix, upstream_imports=False)
                librarian = self.root / "Visual Studio/bin/lib.exe"
                with (
                    mock.patch.object(prepare_android, "find_msvc_lib", return_value=librarian) as tool,
                    mock.patch.object(prepare_android.subprocess, "run", side_effect=write_import_output) as run,
                ):
                    prepare_android.ensure_windows_import_libraries(prefix, target)
                    tool.assert_called_once()
                    self.assertEqual(run.call_count, 4)
                    for call, (name, major) in zip(run.call_args_list, LIBRARIES.items()):
                        command = call.args[0]
                        self.assertEqual(command[0], str(librarian))
                        self.assertIn(f"/MACHINE:{machine}", command)
                        self.assertIn(f"/NAME:{name}-{major}.dll", command)
                        self.assertIn(f"/DEF:{(prefix / 'lib' / f'{name}-{major}.def').resolve()}", command)
                        output = Path(next(arg[5:] for arg in command if arg.startswith("/OUT:")))
                        self.assertTrue(output.is_absolute())
                        # Windows TMP can use an 8.3 user path; /OUT is canonical.
                        self.assertEqual(Path(call.kwargs["cwd"]).resolve(), output.parent.resolve())
                        self.assertTrue(call.kwargs["check"])
                self.assert_complete(prefix)

    def test_valid_lib_imports_do_not_require_definitions_or_bin_imports(self) -> None:
        install_fixture(self.prefix, upstream_imports=False)
        for name, major in LIBRARIES.items():
            (self.prefix / "lib" / f"{name}.lib").write_bytes(ARCHIVE)
            (self.prefix / "lib" / f"{name}-{major}.def").unlink()
        with mock.patch.object(prepare_android, "find_msvc_lib") as tool:
            prepare_android.ensure_windows_import_libraries(self.prefix, X64)
            tool.assert_not_called()

    def test_missing_definition_or_dll_reports_path_and_recovery(self) -> None:
        for relative in ("lib/avcodec-62.def", "bin/avcodec-62.dll"):
            for empty in (False, True):
                with self.subTest(relative=relative, empty=empty):
                    install_fixture(self.prefix, upstream_imports=False)
                    path = self.prefix / relative
                    if empty:
                        path.write_bytes(b"")
                    else:
                        path.unlink()
                    with self.assertRaises(RuntimeError) as raised:
                        prepare_android.ensure_windows_import_libraries(self.prefix, X64)
                    self.assertIn(str(path), str(raised.exception))
                    self.assertIn(f"{X64} (X64)", str(raised.exception))
                    self.assertIn("rerun prepare_android.py", str(raised.exception))
                    self.assertEqual(list((self.prefix / "lib").glob(".avcodec-*")), [])

    def test_missing_librarian_is_an_error_instead_of_preparation_success(self) -> None:
        install_fixture(self.prefix, upstream_imports=False)
        with (
            mock.patch.object(prepare_android, "find_msvc_lib", side_effect=FileNotFoundError("MSVC lib.exe not found")),
            self.assertRaisesRegex(RuntimeError, "MSVC lib.exe not found"),
        ):
            prepare_android.ensure_windows_import_libraries(self.prefix, X64)
        self.assertFalse((self.prefix / "lib/avcodec.lib").exists())

    def test_failed_generation_preserves_destination_and_cleans_all_intermediates(self) -> None:
        install_fixture(self.prefix, upstream_imports=False)
        destination = self.prefix / "lib/avcodec.lib"
        destination.write_bytes(b"existing invalid file")

        def fail(command, **kwargs):
            write_import_output(command, **kwargs)
            raise subprocess.CalledProcessError(7, command)

        with (
            mock.patch.object(prepare_android, "find_msvc_lib", return_value=self.root / "lib.exe"),
            mock.patch.object(prepare_android.subprocess, "run", side_effect=fail),
            self.assertRaisesRegex(RuntimeError, "exit status 7"),
        ):
            prepare_android.ensure_windows_import_libraries(self.prefix, X64)
        self.assertEqual(destination.read_bytes(), b"existing invalid file")
        self.assertEqual(list((self.prefix / "lib").glob(".avcodec-*")), [])
        self.assertEqual(list((self.prefix / "lib").rglob("*.exp")), [])

    def test_successful_tool_exit_with_missing_or_invalid_output_is_rejected(self) -> None:
        for payload in (None, b"", b"not a COFF archive"):
            with self.subTest(payload=payload):
                install_fixture(self.prefix, upstream_imports=False)

                def invalid(command, **kwargs):
                    if payload is not None:
                        Path(next(arg[5:] for arg in command if arg.startswith("/OUT:"))).write_bytes(payload)

                with (
                    mock.patch.object(prepare_android, "find_msvc_lib", return_value=self.root / "lib.exe"),
                    mock.patch.object(prepare_android.subprocess, "run", side_effect=invalid),
                    self.assertRaisesRegex(RuntimeError, "Invalid COFF import library output"),
                ):
                    prepare_android.ensure_windows_import_libraries(self.prefix, X64)
                self.assertFalse((self.prefix / "lib/avcodec.lib").exists())
                self.assertEqual(list((self.prefix / "lib").glob(".avcodec-*")), [])

    def test_fresh_failure_does_not_write_success_stamp(self) -> None:
        with self.preparation(upstream_imports=False):
            with (
                mock.patch.object(prepare_android, "find_msvc_lib", side_effect=FileNotFoundError("missing lib.exe")),
                self.assertRaises(RuntimeError),
            ):
                prepare_android.prepare_ffmpeg(X64)
        self.assertFalse((self.prefix / "SOURCE.json").exists())

    def test_missing_license_prevents_new_stamp_and_is_repaired_on_cache_hit(self) -> None:
        with self.preparation():
            (self.source / "COPYING.LGPLv2.1").unlink()
            with self.assertRaises(FileNotFoundError):
                prepare_android.prepare_ffmpeg(X64)
            self.assertFalse((self.prefix / "SOURCE.json").exists())
            (self.source / "COPYING.LGPLv2.1").write_text("fixture license\n")
            prepare_android.prepare_ffmpeg(X64)
            (self.prefix / "LICENSE.txt").unlink()
            self.commands.clear()
            prepare_android.prepare_ffmpeg(X64)
            self.assertEqual(self.commands, [])
            self.assertEqual((self.prefix / "LICENSE.txt").read_text(), "fixture license\n")

    def test_linux_and_macos_do_not_use_windows_import_library_preparation(self) -> None:
        with self.preparation(), mock.patch.object(prepare_android, "ensure_windows_import_libraries") as ensure:
            for target in ("x86_64-unknown-linux-gnu", "aarch64-apple-darwin"):
                prepare_android.prepare_ffmpeg(target)
                self.commands.clear()
                prepare_android.prepare_ffmpeg(target)
                self.assertEqual(self.commands, [])
            ensure.assert_not_called()

    def test_arm64_cross_configuration_invalidates_cached_native_configuration(self) -> None:
        self.prefix = self.staging / "ffmpeg" / ARM64
        with self.preparation():
            prepare_android.prepare_ffmpeg(ARM64)
            command = self.commands[0]
            self.assertIn("--enable-cross-compile", command)
            self.assertIn("--cc=cl.exe -MT", command)
            self.assertIn("--cxx=cl.exe -MT", command)
            self.assertNotIn("--extra-cflags=-MT", command)
            self.assertIn("--as=clang --target=aarch64-pc-windows-msvc", command)
            stamp = self.prefix / "SOURCE.json"
            previous = json.loads(stamp.read_text())
            previous["configure"] = [arg for arg in previous["configure"]
                                     if arg != "--enable-cross-compile" and not arg.startswith("--as=")]
            stamp.write_text(json.dumps(previous))
            self.commands.clear()
            prepare_android.prepare_ffmpeg(ARM64)
            self.assertIn("--enable-cross-compile", self.commands[0])
            self.commands.clear()
            prepare_android.prepare_ffmpeg(ARM64)
            self.assertEqual(self.commands, [])

    def test_x64_keeps_native_configuration(self) -> None:
        with self.preparation():
            prepare_android.prepare_ffmpeg(X64)
        self.assertNotIn("--enable-cross-compile", self.commands[0])
        self.assertFalse(any(arg.startswith("--as=") for arg in self.commands[0]))

    def test_unsupported_windows_target_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "Unsupported Windows FFmpeg target"):
            prepare_android.ensure_windows_import_libraries(self.prefix, "i686-pc-windows-msvc")


class DebianLibraryOwnershipTests(unittest.TestCase):
    def test_canonical_library_keeps_multiarch_package_identity(self):
        path = Path("/lib/aarch64-linux-gnu/libprotobuf.so.23")
        resolved = Path("/usr/lib/aarch64-linux-gnu/libprotobuf.so.23.0.4")
        with mock.patch.object(Path, "resolve", return_value=resolved), \
                mock.patch.object(Path, "exists", return_value=False), \
                mock.patch.object(prepare_android.subprocess, "run", return_value=subprocess.CompletedProcess(
                    [], 0, f"libprotobuf23:arm64: {resolved}\n", "")) as run:
            self.assertEqual(prepare_android.debian_library_package(path), "libprotobuf23:arm64")
            self.assertEqual(run.call_args.args[0], ["dpkg-query", "-S", str(resolved)])

    def test_original_path_is_used_when_database_owns_symlink(self):
        path, resolved = Path("/lib/libexample.so.1"), Path("/usr/lib/libexample.so.1.0")
        with mock.patch.object(Path, "resolve", return_value=resolved), \
                mock.patch.object(Path, "exists", return_value=False), \
                mock.patch.object(prepare_android.subprocess, "run", side_effect=[
                    subprocess.CompletedProcess([], 1, "", "missing"),
                    subprocess.CompletedProcess([], 0, f"libexample1: {path}\n", "")]):
            self.assertEqual(prepare_android.debian_library_package(path), "libexample1")

    def test_missing_or_ambiguous_ownership_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "library.so"
            path.touch()
            for code, output in ((1, ""), (0, f"one: {path}\ntwo: {path}\n")):
                with self.subTest(code=code), mock.patch.object(prepare_android.subprocess, "run",
                        return_value=subprocess.CompletedProcess([], code, output, "")), self.assertRaises(ValueError):
                    prepare_android.debian_library_package(path)


class MsvcToolDiscoveryTests(unittest.TestCase):
    def test_prefers_librarian_beside_selected_compiler(self) -> None:
        with tempfile.TemporaryDirectory(prefix="MSVC tools ") as directory:
            compiler = Path(directory) / "cl.exe"
            compiler.touch()
            librarian = compiler.with_name("lib.exe")
            librarian.touch()
            with mock.patch.object(prepare_android.shutil, "which", side_effect=lambda name: str(compiler) if name == "cl.exe" else "/other/lib.exe"):
                self.assertEqual(prepare_android.find_msvc_lib(), librarian.resolve())

    def test_falls_back_to_explicit_exe_on_path(self) -> None:
        with mock.patch.object(prepare_android.shutil, "which", side_effect=lambda name: "tools/lib.exe" if name == "lib.exe" else None):
            self.assertEqual(prepare_android.find_msvc_lib(), Path("tools/lib.exe").resolve())

    def test_missing_tool_explains_required_developer_environment(self) -> None:
        with mock.patch.object(prepare_android.shutil, "which", return_value=None):
            with self.assertRaisesRegex(FileNotFoundError, "Visual Studio Developer PowerShell"):
                prepare_android.find_msvc_lib()


@unittest.skipUnless(sys.platform == "win32", "requires Windows MSVC and prepared FFmpeg")
class WindowsFfmpegTests(unittest.TestCase):
    def test_real_install_and_cached_repairs_link_all_four_versioned_dlls(self) -> None:
        target = os.environ.get("TARGET") or next(line.removeprefix("host: ") for line in
            subprocess.check_output(["rustc", "-vV"], text=True).splitlines() if line.startswith("host: "))
        prepared = Path(os.environ.get("FFMPEG_DIR", prepare_android.STAGING / "ffmpeg" / target)).resolve()
        self.assertEqual(json.loads((prepared / "SOURCE.json").read_text())["target"], target)
        for name in LIBRARIES:
            self.assertTrue(prepare_android.is_coff_archive(prepared / "lib" / f"{name}.lib"))
        compiler = Path(shutil.which("cl.exe")).resolve()
        dumpbin = compiler.with_name("dumpbin.exe")
        self.assertTrue(dumpbin.is_file())
        with tempfile.TemporaryDirectory(prefix="FFmpeg native cache ") as directory:
            staging = Path(directory)
            prefix = staging / "ffmpeg" / target
            shutil.copytree(prepared, prefix)
            source = staging / "source"
            source.mkdir()
            shutil.copy2(prefix / "LICENSE.txt", source / "COPYING.LGPLv2.1")
            probe = staging / "imports.c"
            probe.write_text("\n".join(f"__declspec(dllimport) unsigned {name}_version(void);" for name in LIBRARIES) +
                             "\nint main(void) { return (int)(" + " + ".join(f"{name}_version()" for name in LIBRARIES) + "); }\n")
            executable = staging / "imports.exe"

            def link_probe():
                subprocess.run([str(compiler), "/nologo", "/MT", str(probe), f"/Fo{staging / 'imports.obj'}",
                                f"/Fe{executable}", "/link", f"/LIBPATH:{prefix / 'lib'}",
                                *[f"{name}.lib" for name in LIBRARIES]], cwd=staging, check=True)
                self.assertEqual(verify_native_package.pe_machine(executable.read_bytes()),
                                 0xAA64 if target == ARM64 else 0x8664)
                imports = subprocess.check_output([str(dumpbin), "/nologo", "/imports", str(executable)], text=True).lower()
                for name, major in LIBRARIES.items():
                    self.assertIn(f"{name}-{major}.dll", imports)

            link_probe()
            with (
                mock.patch.object(prepare_android, "STAGING", staging),
                mock.patch.object(prepare_android, "download", return_value=staging / "unused.tar.gz"),
                mock.patch.object(prepare_android, "extract_tar", return_value=source),
            ):
                stamp = (prefix / "SOURCE.json").read_bytes()
                (prefix / "lib/avcodec.lib").unlink()
                prepare_android.prepare_ffmpeg(target)
                link_probe()
                # Exercise the .def fallback for every real FFmpeg library, including
                # its target machine and DLL name, without modifying the CI install.
                for name in LIBRARIES:
                    for location in ("bin", "lib"):
                        (prefix / location / f"{name}.lib").unlink(missing_ok=True)
                prepare_android.prepare_ffmpeg(target)
                link_probe()
                self.assertEqual(stamp, (prefix / "SOURCE.json").read_bytes())
                self.assertEqual(list((prefix / "lib").glob(".*")), [])


if __name__ == "__main__":
    unittest.main()
