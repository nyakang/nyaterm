from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / "compat/linux-focal/package.py"
spec = importlib.util.spec_from_file_location("focal_package", SCRIPT)
focal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(focal)


class FocalAbiTests(unittest.TestCase):
    @unittest.skipUnless(shutil.which("cc"), "C compiler required")
    def test_egl_adapter_changes_only_desktop_only_config_filter(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fake = root / "real.c"
            fake.write_text('''
#include <stdlib.h>
const char *eglQueryString(void *display, int key) { return getenv("TEST_EGL_APIS"); }
unsigned int eglChooseConfig(void *display, const int *attrs, void **configs, int size, int *count) {
    *count = attrs[1]; return 1;
}
''')
            subprocess.run(["cc", "-shared", "-fPIC", str(fake), "-o",
                            str(root / "libEGL_nyaterm.so.1")], check=True)
            wrapper = root / "adapter.so"
            subprocess.run(["cc", "-shared", "-fPIC", str(SCRIPT.with_name("egl_desktop.c")),
                            "-ldl", "-lpthread", "-o", str(wrapper)], check=True)
            probe = '''
import ctypes,sys
c=ctypes
f=c.CDLL(sys.argv[1]).eglChooseConfig
f.argtypes=[c.c_void_p,c.POINTER(c.c_int),c.c_void_p,c.c_int,c.POINTER(c.c_int)]
f.restype=c.c_uint
attrs=(c.c_int*3)(0x3040,4,0x3038)
count=c.c_int()
assert f(None,attrs,None,0,c.byref(count)) == 1
print(count.value)
'''
            for apis, expected in (("OpenGL", "8"), ("OpenGL OpenGL_ES", "4"),
                                   ("OpenGL_ES", "4"), ("", "4")):
                with self.subTest(apis=apis):
                    actual = subprocess.check_output(
                        [sys.executable, "-c", probe, str(wrapper)], text=True,
                        env=dict(os.environ, LD_LIBRARY_PATH=str(root), TEST_EGL_APIS=apis))
                    self.assertEqual(actual.strip(), expected)

    def test_version_limits_include_patch_level_and_private_abi(self):
        self.assertEqual(focal.check_versions("Name: GLIBC_2.31\nName: GLIBCXX_3.4.28"),
                         ["GLIBCXX_3.4.28", "GLIBC_2.31"])
        # Never compare versions as strings (2.9 is older than 2.31).
        focal.check_versions("Name: GLIBC_2.9")
        for version in ("GLIBC_2.32", "GLIBC_2.34", "GLIBC_2.100", "GLIBCXX_3.4.29",
                        "CXXABI_1.3.13", "GLIBC_PRIVATE", "GLIBC_ABI_DT_RELR"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                focal.check_versions("Name: GLIBC_2.31\nName: " + version)

    def test_missing_version_information_is_not_a_pass(self):
        with self.assertRaises(ValueError):
            focal.check_versions("No version information found")

    def test_gate_rejects_new_sonames_build_rpaths_and_unresolved_glib_symbols(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "nyaterm"
            header = bytearray(20)
            header[:6] = b"\x7fELF\x02\x01"
            struct.pack_into("<H", header, 18, 62)
            binary.write_bytes(header)
            interpreter = "[Requesting program interpreter: /lib64/ld-linux-x86-64.so.2]"
            for dynamic, linkage in (
                ("(NEEDED) Shared library: [libssl.so.3]", ""),
                ("(RUNPATH) Library runpath: [/workspace/build]", ""),
                ("(NEEDED) Shared library: [libc.so.6]", "undefined symbol: g_memdup2"),
                ("(NEEDED) Shared library: [libc.so.6]", "libffi.so.8 => not found"),
            ):
                with self.subTest(dynamic=dynamic, linkage=linkage):
                    with mock.patch.object(focal, "command", side_effect=[
                            "Name: GLIBC_2.31", interpreter, dynamic, linkage]):
                        with self.assertRaises(ValueError):
                            focal.audit(binary)

    def test_gate_rejects_non_x64_elf(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "nyaterm"
            binary.write_bytes(b"MZ" + bytes(18))
            with self.assertRaisesRegex(ValueError, "ELF64 x86-64"):
                focal.audit(binary)

    @unittest.skipUnless(shutil.which("cc") and shutil.which("readelf"), "native ELF tools required")
    def test_real_compiler_output_cannot_hide_new_glibc_requirement(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "hello.c"
            source.write_text("int main(void) { return 0; }\n")
            binary = Path(directory) / "hello"
            subprocess.run(["cc", str(source), "-o", str(binary)], check=True)
            versions = focal.command("readelf", "--version-info", "--wide", str(binary))
            if "GLIBC_2.34" in versions:
                with self.assertRaisesRegex(ValueError, "GLIBC_2.34"):
                    focal.audit(binary)
            else:
                self.assertIn("libc.so.6", focal.audit(binary)["needed"])

    def test_archive_contains_all_helpers_diagnostics_and_integrity_report(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in focal.BINARIES:
                (root / name).write_bytes(name.encode())
            runtime = root / "opengl"
            runtime.mkdir()
            with (mock.patch.object(focal.os, "confstr", return_value="glibc 2.31"),
                  mock.patch.object(focal, "audit", side_effect=lambda p: {"name": p.name}),
                  mock.patch.object(focal, "audit_opengl", return_value=[])):
                archive = focal.create_package(root, root / "dist", "2.0.0", runtime)
            with tarfile.open(archive) as handle:
                prefix = "NyaTerm_2.0.0_linux_x64_glibc231/"
                for name in (*focal.BINARIES, "nyaterm-portable", "diagnose.sh", "LICENSE",
                             "OFFLINE-README.zh-CN.md", "abi-report.json", "start-nyaterm.sh",
                             "opengl/opengl-lock.txt", "opengl/egl_desktop.c"):
                    self.assertIn(prefix + name, handle.getnames())
                report = json.load(handle.extractfile(prefix + "abi-report.json"))
                self.assertEqual(len(report["binaries"]), 4)
            self.assertTrue(archive.with_name(archive.name + ".sha256").is_file())

    def test_packaging_refuses_validation_on_newer_host(self):
        with mock.patch.object(focal.os, "confstr", return_value="glibc 2.41"):
            with self.assertRaisesRegex(ValueError, "glibc 2.31"):
                focal.create_package(Path("missing"), Path("dist"), "2.0.0", Path("missing"))


if __name__ == "__main__":
    unittest.main()
