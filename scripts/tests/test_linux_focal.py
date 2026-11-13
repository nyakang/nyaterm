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
import textwrap
import unittest
from unittest import mock
from scripts.tests.test_verify_native_package import stage_android_fixture


SCRIPT = Path(__file__).resolve().parents[1] / "compat/linux-focal/package.py"
spec = importlib.util.spec_from_file_location("focal_package", SCRIPT)
focal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(focal)


class FocalAbiTests(unittest.TestCase):
    @unittest.skipUnless(os.name == "posix" and shutil.which("dpkg-deb"), "dpkg-deb required")
    def test_compat_deb_uses_private_launcher_and_complete_readable_icons(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            stage = root / "stage"
            stage.mkdir()
            for name in (*focal.BINARIES, "nyaterm-portable", "start-nyaterm.sh"):
                (stage / name).touch()
            output = root / "dist"
            output.mkdir()
            with mock.patch.object(focal, "deb_dependencies", return_value="libc6 (>= 2.31), libudev1") as scan:
                deb = focal.create_deb(stage, output, "2.0.1")
                self.assertEqual([p.name for p in scan.call_args.args[0]], list(focal.BINARIES))
            depends = subprocess.check_output(["dpkg-deb", "-f", str(deb), "Depends"], text=True)
            self.assertIn("libudev1", depends)
            self.assertIn("bash", depends)
            extracted = root / "extracted"
            subprocess.run(["dpkg-deb", "--extract", str(deb), str(extracted)], check=True)
            self.assertFalse((extracted / "opt/nyaterm-compat/nyaterm-portable").exists())
            self.assertIn("/opt/nyaterm-compat/start-nyaterm.sh", (extracted / "usr/bin/nyaterm-compat").read_text())
            desktop = (extracted / "usr/share/applications/nyaterm-compat.desktop").read_text()
            self.assertIn("StartupWMClass=NyaTerm\n", desktop)
            self.assertIn("Exec=/usr/bin/nyaterm-compat %U\n", desktop)
            self.assertIn("Actions=SoftwareRendering;\n", desktop)
            self.assertIn("[Desktop Action SoftwareRendering]\n", desktop)
            self.assertIn("Exec=/usr/bin/nyaterm-compat --software %U\n", desktop)
            for size in focal.linux_icons.ICON_SIZES:
                icon = extracted / "usr/share/icons/hicolor" / size / "apps/nyaterm-compat.png"
                self.assertEqual(icon.stat().st_mode & 0o777, 0o644)
                self.assertEqual(struct.unpack(">II", icon.read_bytes()[16:24]), tuple(map(int, size.split("x"))))
            self.assertTrue(deb.with_name(deb.name + ".sha256").is_file())

    def test_deb_dependency_scan_includes_helpers_and_fails_without_metadata(self):
        binaries = [Path(name) for name in focal.BINARIES]
        with mock.patch.dict(os.environ, {"LD_LIBRARY_PATH": "/private/mesa"}), \
                mock.patch.object(focal.subprocess, "check_output", return_value="shlibs:Depends=libc6, libssl1.1\n") as scan:
            self.assertEqual(focal.deb_dependencies(binaries), "libc6, libssl1.1")
            args = scan.call_args.args[0]
            self.assertEqual(args[:2], ["dpkg-shlibdeps", "-O"])
            self.assertEqual(args[2:], [arg for p in binaries for arg in ("-e", str(p.resolve()))])
            self.assertNotIn("LD_LIBRARY_PATH", scan.call_args.kwargs["env"])
        for result in ("", "shlibs:Depends=\n"):
            with mock.patch.object(focal.subprocess, "check_output", return_value=result):
                with self.assertRaisesRegex(RuntimeError, "shlibs:Depends"):
                    focal.deb_dependencies(binaries)
        with mock.patch.object(focal.subprocess, "check_output", side_effect=subprocess.CalledProcessError(2, "dpkg-shlibdeps")):
            with self.assertRaises(subprocess.CalledProcessError):
                focal.deb_dependencies(binaries)

    @unittest.skipUnless(os.name == "posix" and shutil.which("g++") and shutil.which("dpkg-shlibdeps"), "Debian ELF tools required")
    def test_real_deb_dependency_scan_captures_helper_only_cpp_runtime(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "helper.cc"
            source.write_text('#include <iostream>\nint main() { std::cout << "helper"; }\n')
            binary = root / "nyaterm-rdp-helper"
            subprocess.run(["g++", str(source), "-o", str(binary)], check=True)
            depends = focal.deb_dependencies([Path("/bin/true"), binary])
            self.assertIn("libstdc++6", depends)
            self.assertIn("libc6", depends)

    @unittest.skipUnless(os.name == "posix" and shutil.which("cc"), "C compiler required")
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

    @unittest.skipUnless(os.name == "posix" and shutil.which("cc") and shutil.which("readelf"), "native ELF tools required")
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

    @unittest.skipUnless(os.name == "posix", "POSIX package permissions required")
    def test_archive_contains_all_helpers_diagnostics_and_integrity_report(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in focal.BINARIES:
                (root / name).write_bytes(name.encode())
                (root / name).chmod(0o600)
            runtime = root / "opengl"
            runtime.mkdir()
            stage_android_fixture(root, "x86_64-unknown-linux-gnu")
            with mock.patch.object(focal.os, "confstr", return_value="glibc 2.31"), \
                    mock.patch.object(focal, "audit", side_effect=lambda p, **kwargs: {"name": p.name}), \
                    mock.patch.object(focal.subprocess, "run"), \
                    mock.patch.object(focal, "audit_opengl", return_value=[]):
                previous_umask = os.umask(0o077)
                try:
                    archive = focal.create_package(root, root / "dist", "2.0.0", runtime)
                finally:
                    os.umask(previous_umask)
            with tarfile.open(archive) as handle:
                prefix = "NyaTerm_2.0.0_linux_x64_glibc231/"
                for member in handle.getmembers():
                    self.assertEqual((member.uid, member.gid, member.uname, member.gname),
                                     (0, 0, "root", "root"))
                    if member.isdir():
                        self.assertEqual(member.mode, 0o755)
                    elif member.isfile():
                        executable = Path(member.name).name in (*focal.BINARIES, "adb", "start-nyaterm.sh", "diagnose.sh")
                        self.assertEqual(member.mode, 0o755 if executable else 0o644)
                for name in (*focal.BINARIES, "nyaterm-portable", "diagnose.sh", "LICENSE",
                             "OFFLINE-README.zh-CN.md", "abi-report.json", "start-nyaterm.sh",
                             "opengl/opengl-lock.txt", "opengl/egl_desktop.c", "adb/adb",
                             "android-resources.json", "android-libs/libavcodec.so.62"):
                    self.assertIn(prefix + name, handle.getnames())
                report = json.load(handle.extractfile(prefix + "abi-report.json"))
                self.assertEqual(len(report["binaries"]), 5)
                self.assertEqual(len(report["android_runtime"]), 5)
            self.assertTrue(archive.with_name(archive.name + ".sha256").is_file())

    def test_packaging_refuses_validation_on_newer_host(self):
        with mock.patch.object(focal.os, "confstr", return_value="glibc 2.41", create=True):
            with self.assertRaisesRegex(ValueError, "glibc 2.31"):
                focal.create_package(Path("missing"), Path("dist"), "2.0.0", Path("missing"))


@unittest.skipUnless(os.name == "posix" and shutil.which("bash"), "POSIX bash required")
class FocalOpenGLCacheTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.scripts = self.root / "scripts/compat/linux-focal"
        self.scripts.mkdir(parents=True)
        shutil.copy2(SCRIPT.with_name("prepare-opengl.sh"), self.scripts)
        for name in ("opengl-lock.txt", "opengl-tools-lock.txt"):
            (self.scripts / name).write_text("initial " + name)
        tools = self.root / "target/linux-focal/opengl-tools"
        tools.mkdir(parents=True)
        mamba = tools / "micromamba"
        mamba.write_text("#!" + sys.executable + "\n" + textwrap.dedent('''\
            import os, pathlib, sys
            args = sys.argv
            prefix = pathlib.Path(args[args.index('-p') + 1])
            lock = args[args.index('--file') + 1]
            with open(os.environ['CREATE_LOG'], 'a') as log:
                log.write(lock + '\\n')
            (prefix / 'conda-meta').mkdir(parents=True)
            (prefix / 'lib').mkdir()
            (prefix / 'lib/libEGL.so.1').write_bytes(b'fixture')
            (prefix / 'bin').mkdir()
            tool = prefix / 'bin/patchelf'
            tool.write_text('#!/bin/sh\\nexit 0\\n')
            tool.chmod(0o755)
            '''))
        mamba.chmod(0o755)
        (self.scripts / "prepare_opengl.py").write_text(textwrap.dedent('''\
            import pathlib, shutil, sys
            source, output = map(pathlib.Path, sys.argv[1:])
            shutil.rmtree(output, ignore_errors=True)
            shutil.copytree(source / 'lib', output / 'lib')
            '''))
        fake_bin = self.root / "bin"
        fake_bin.mkdir()
        cc = fake_bin / "cc"
        cc.write_text('#!/bin/sh\nfor arg do output="$arg"; done\ntouch "$output"\n')
        cc.chmod(0o755)
        self.log = self.root / "creates.log"
        self.env = dict(os.environ, PATH=str(fake_bin) + os.pathsep + os.environ["PATH"],
                        CREATE_LOG=str(self.log))
        for key in ("NYATERM_OPENGL_RUNTIME", "NYATERM_PATCHELF"):
            self.env.pop(key, None)

    def prepare(self, **settings):
        return subprocess.run(["bash", str(self.scripts / "prepare-opengl.sh")],
                              env=dict(self.env, **settings), text=True, capture_output=True)

    def test_both_locks_invalidate_only_the_corresponding_cached_environment(self):
        for changed, count in ((None, 2), (None, 2), ("opengl-lock.txt", 3),
                               ("opengl-tools-lock.txt", 4)):
            with self.subTest(changed=changed, count=count):
                if changed:
                    (self.scripts / changed).write_text("changed " + changed)
                result = self.prepare()
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(len(self.log.read_text().splitlines()), count)

    def test_unverified_external_environment_is_rejected_without_modification(self):
        external = self.root / "external"
        (external / "conda-meta").mkdir(parents=True)
        sentinel = external / "keep"
        sentinel.write_text("untouched")
        result = self.prepare(NYATERM_OPENGL_RUNTIME=str(external))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("mismatched OpenGL cache", result.stderr)
        self.assertEqual(sentinel.read_text(), "untouched")
        self.assertFalse(self.log.exists())


if __name__ == "__main__":
    unittest.main()
