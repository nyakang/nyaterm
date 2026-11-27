from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "release"))

import linux_appimage
import package_native
from native_headers import elf_machine


class AppImageDependencyTests(unittest.TestCase):
    def test_missing_openssl_fails_even_when_ldd_exits_successfully(self):
        with mock.patch.object(subprocess, "check_output", return_value="libssl.so.3 => not found\n"):
            with self.assertRaisesRegex(RuntimeError, "unresolved dependency libssl.so.3"):
                linux_appimage.dependencies(Path("nyaterm"))

    def test_dependency_paths_can_contain_spaces(self):
        with tempfile.TemporaryDirectory(prefix="appimage libraries ") as directory:
            library = Path(directory) / "libcrypto.so.3"
            library.touch()
            output = f"libcrypto.so.3 => {library} (0x1234)\n"
            with mock.patch.object(subprocess, "check_output", return_value=output):
                self.assertEqual(linux_appimage.dependencies(Path("nyaterm")), {library.name: library})

    def test_build_shell_cannot_mask_missing_package_dependencies(self):
        overrides = {"LD_LIBRARY_PATH": "/build/lib", "LD_PRELOAD": "build.so", "LD_AUDIT": "audit.so"}
        with mock.patch.dict(os.environ, overrides):
            environment = linux_appimage.loader_environment()
        self.assertFalse({"LD_LIBRARY_PATH", "LD_PRELOAD", "LD_AUDIT"} & environment.keys())
        self.assertEqual(environment["LC_ALL"], "C")

    def test_no_openssl_dependency_needs_no_bundled_library(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with mock.patch.object(linux_appimage, "dependencies", return_value={}):
                linux_appimage.bundle_openssl(root, [root / "usr/bin/nyaterm"])
                linux_appimage.verify_openssl(root, [root / "usr/bin/nyaterm"], 62)
            self.assertFalse((root / "usr/lib").exists())


@unittest.skipUnless(
    sys.platform.startswith("linux") and all(shutil.which(tool) for tool in ("cc", "ldd", "patchelf")),
    "requires native Linux cc, ldd and patchelf",
)
class AppImageRuntimeTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="nyaterm appimage ")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.build = self.root / "build"
        self.build.mkdir()
        self.appdir = self.root / "NyaTerm.AppDir"
        (self.appdir / "usr/bin").mkdir(parents=True)
        # A private ABI avoids accidentally passing with the test host's OpenSSL.
        self.crypto = "libcrypto.so.1356"
        self.ssl = "libssl.so.1356"
        self.compile("crypto", "int fixture_crypto(void) { return 42; }", self.build / self.crypto,
                     "-shared", "-fPIC", f"-Wl,-soname,{self.crypto}")
        self.compile("ssl", "int fixture_crypto(void); int fixture_ssl(void) { return fixture_crypto(); }",
                     self.build / self.ssl, "-shared", "-fPIC", f"-Wl,-soname,{self.ssl}",
                     f"-L{self.build}", f"-l:{self.crypto}", f"-Wl,-rpath,{self.build}")
        # Exercise copying through a SONAME symlink, as on Linux distributions.
        actual = self.build / f"{self.crypto}.real"
        (self.build / self.crypto).rename(actual)
        (self.build / self.crypto).symlink_to(actual.name)
        self.binary = self.appdir / "usr/bin/nyaterm"
        self.compile("main", '#include <stdio.h>\n#include <stdlib.h>\n'
                     'int fixture_ssl(void); int main(void) { printf("%d\\n", fixture_ssl()); '
                     'return getenv("LD_LIBRARY_PATH") != NULL; }', self.binary,
                     f"-L{self.build}", f"-l:{self.ssl}", f"-Wl,-rpath,{self.build}")
        self.helper = self.appdir / "usr/bin/nyaterm-mcp"
        shutil.copy2(self.binary, self.helper)
        self.executables = [self.binary, self.helper]
        self.machine = elf_machine(self.binary.read_bytes()[:64])
        notice = self.build / "copyright"
        notice.write_text("OpenSSL fixture copyright\n")
        patcher = mock.patch.object(linux_appimage, "OPENSSL_COPYRIGHT", notice)
        patcher.start()
        self.addCleanup(patcher.stop)

    def compile(self, name, code, output, *flags):
        source = self.build / f"{name}.c"
        source.write_text(code)
        subprocess.run(["cc", str(source), "-o", str(output), *flags], check=True, capture_output=True)

    def test_relocated_application_and_helper_run_without_build_libraries(self):
        linux_appimage.bundle_openssl(self.appdir, self.executables)
        self.assertFalse((self.appdir / "usr/lib" / self.crypto).is_symlink())
        shutil.rmtree(self.build)
        relocated = self.root / "another mount point"
        self.appdir.rename(relocated)
        executables = [relocated / "usr/bin" / path.name for path in self.executables]
        linux_appimage.verify_openssl(relocated, executables, self.machine)
        for binary in executables:
            result = subprocess.run([str(binary)], env=linux_appimage.loader_environment(),
                                    capture_output=True, text=True, check=True)
            self.assertEqual(result.stdout, "42\n")

    def test_verifier_rejects_package_that_falls_back_to_build_host(self):
        linux_appimage.bundle_openssl(self.appdir, self.executables)
        (self.appdir / "usr/lib" / self.crypto).unlink()
        with self.assertRaisesRegex(RuntimeError, "outside its bundle"):
            linux_appimage.verify_openssl(self.appdir, self.executables, self.machine)

    def test_verifier_rejects_wrong_architecture(self):
        linux_appimage.bundle_openssl(self.appdir, self.executables)
        with self.assertRaisesRegex(RuntimeError, "architecture mismatch"):
            linux_appimage.verify_openssl(self.appdir, self.executables, 183 if self.machine == 62 else 62)

    def test_verifier_requires_openssl_notice(self):
        linux_appimage.bundle_openssl(self.appdir, self.executables)
        (self.appdir / "usr/share/doc/openssl/copyright").unlink()
        with self.assertRaisesRegex(RuntimeError, "copyright notice"):
            linux_appimage.verify_openssl(self.appdir, self.executables, self.machine)

    def test_appimage_packager_bundles_runtime_before_building_image(self):
        helper_source = self.build / self.helper.name
        shutil.copy2(self.helper, helper_source)
        with (
            mock.patch.object(package_native, "WORK_DIR", self.root / "package"),
            mock.patch.object(package_native, "helper_binary_paths", return_value=[helper_source]),
            mock.patch.object(package_native, "require_tool", return_value="appimagetool"),
            mock.patch.object(package_native, "run") as run,
        ):
            package_native.create_linux_appimage(
                self.binary,
                package_native.target_info(
                    "aarch64-unknown-linux-gnu" if self.machine == 183 else "x86_64-unknown-linux-gnu"
                ),
                "2.0.8", "2.0.8",
            )
        packaged = self.root / "package/NyaTerm.AppDir"
        executables = [packaged / "usr/bin" / path.name for path in self.executables]
        shutil.rmtree(self.build)
        linux_appimage.verify_openssl(packaged, executables, self.machine)
        self.assertEqual(run.call_args.args[0][0:2], ["appimagetool", str(packaged)])
        self.assertNotIn("LD_LIBRARY_PATH", (packaged / "AppRun").read_text())


if __name__ == "__main__":
    unittest.main()
