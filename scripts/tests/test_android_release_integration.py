"""Regressions for Android resources in native release packaging."""
from __future__ import annotations

import importlib.util
import io
import os
import platform
import shutil
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/release"))
import prepare_android
import verify_native_package
from scripts.tests.test_verify_native_package import stage_android_fixture
from scripts.tests.test_package_native import staged_package
import package_native

spec = importlib.util.spec_from_file_location("android_focal_package", ROOT / "scripts/compat/linux-focal/package.py")
focal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(focal)


class AndroidReleaseTests(unittest.TestCase):
    @unittest.skipUnless(os.name == "posix" and all(shutil.which(tool) for tool in
                         ("cc", "patchelf", "readelf")), "native ELF tools required")
    def test_focal_removes_adb_build_tree_paths_before_audit_and_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, stage = root / "source", root / "stage"
            stage_android_fixture(source, "x86_64-unknown-linux-gnu")
            stage.mkdir()
            code = root / "probe.c"
            code.write_text("int main(void) { return 0; }\n")
            adb = source / "adb/adb"
            subprocess.run(["cc", str(code), "-Wl,-rpath,$ORIGIN/:$ORIGIN/../../../lib64",
                            "-o", str(adb)], check=True)
            original = adb.read_bytes()
            for name in focal.ANDROID_LIBRARIES:
                subprocess.run(["cc", "-shared", "-fPIC", str(code), "-o",
                                str(source / "android-libs" / name)], check=True)
            def inspect(binary, **kwargs):
                dynamic = subprocess.check_output(["readelf", "-d", str(binary)], text=True)
                if binary.name == "adb":
                    self.assertNotIn("(RUNPATH)", dynamic)
                    self.assertNotIn("(RPATH)", dynamic)
                else:
                    self.assertIn("[$ORIGIN]", dynamic)
                return {}
            with mock.patch.object(focal, "audit", side_effect=inspect):
                focal.stage_android(source, stage)
            self.assertEqual(adb.read_bytes(), original)
            self.assertNotEqual((stage / "adb/adb").read_bytes(), original)
            verify_native_package.verify_android_resources(
                lambda name: (stage / name).read_bytes(), "x86_64-unknown-linux-gnu")

    @unittest.skipUnless(os.name == "posix" and all(shutil.which(tool) for tool in
                         ("cc", "dpkg-shlibdeps")), "Debian ELF tools required")
    def test_focal_deb_resolves_private_ffmpeg_and_retains_system_dependencies(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            libraries = root / "android-libs"
            libraries.mkdir()
            source = root / "probe.c"
            source.write_text('#include <stdio.h>\nvoid probe(void) { puts("probe"); }\n')
            for name in focal.ANDROID_LIBRARIES:
                subprocess.run(["cc", "-shared", "-fPIC", str(source), "-Wl,-soname," + name,
                                "-o", str(libraries / name)], check=True)
            source.write_text("void probe(void); int main(void) { probe(); return 0; }\n")
            helper = root / "nyaterm-android-helper"
            subprocess.run(["cc", str(source), "-L" + str(libraries), "-l:libavcodec.so.62",
                            "-Wl,-rpath,$ORIGIN/android-libs", "-o", str(helper)], check=True)
            (root / "adb").mkdir()
            shutil.copy2("/bin/true", root / "adb/adb")
            dependencies = focal.deb_dependencies([helper])
            self.assertIn("libc6", dependencies)
            self.assertNotIn("nyaterm-compat", dependencies)
            self.assertNotIn("libavcodec", dependencies)

    @unittest.skipUnless(os.name == "posix" and platform.machine() == "x86_64" and all(shutil.which(tool) for tool in
                         ("rpmbuild", "rpm2cpio", "cc")), "native RPM and C tools required")
    def test_real_rpm_preserves_android_resource_bytes(self):
        target = "x86_64-unknown-linux-gnu"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "probe.c"
            source.write_text("int main(void) { return 0; }\n")
            binary = root / "nyaterm"
            subprocess.run(["cc", "-g", str(source), "-o", str(binary)], check=True)
            original = binary.read_bytes()
            helpers = []
            for name in package_native.helper_bins(target):
                helper = root / name
                shutil.copy2(binary, helper)
                helpers.append(helper)
            dist = root / "dist"
            dist.mkdir()
            def resources(destination, _):
                stage_android_fixture(destination, target)
                for path in [destination / "adb/adb", *[destination / "android-libs" / name for name in focal.ANDROID_LIBRARIES]]:
                    path.write_bytes(original)
                return prepare_android.write_resource_manifest(destination, target)
            with mock.patch.object(package_native, "WORK_DIR", root / "work"), \
                    mock.patch.object(package_native, "DIST_DIR", dist), \
                    mock.patch.object(package_native, "helper_binary_paths", return_value=helpers), \
                    mock.patch.object(package_native, "copy_android_resources", side_effect=resources):
                package_native.create_linux_rpm(binary, package_native.target_info(target), "2.0.0", "2.0.0")
            artifact = next(dist.glob("*.rpm"))
            def read(name):
                return verify_native_package.read_rpm_member(artifact, "/opt/nyaterm/" + name)
            self.assertEqual(read("adb/adb"), original)
            verify_native_package.verify_android_resources(read, target)
            with self.assertRaisesRegex(RuntimeError, "checksum mismatch"):
                verify_native_package.verify_android_resources(
                    lambda name: read(name) + (b"tampered" if name == "adb/adb" else b""), target)

    def test_old_protoc_gets_optional_flag_without_replacing_system_libraries(self):
        with tempfile.TemporaryDirectory() as directory:
            for version in ("3.12.4", "3.14.0", "3.15.0", "25.1"):
                with self.subTest(version=version), mock.patch.object(prepare_android.shutil, "which", return_value="/tools with spaces/protoc"), \
                        mock.patch.object(prepare_android.subprocess, "check_output", return_value="libprotoc " + version):
                    compiler = prepare_android.adb_protoc(Path(directory))
                    if version in ("3.12.4", "3.14.0"):
                        self.assertIn("exec '/tools with spaces/protoc' --experimental_allow_proto3_optional \"$@\"", compiler.read_text())
                    else:
                        self.assertEqual(compiler, Path("/tools with spaces/protoc"))
            for version in ("libprotoc 3.11.4", "unexpected"):
                with mock.patch.object(prepare_android.shutil, "which", return_value="/usr/bin/protoc"), \
                        mock.patch.object(prepare_android.subprocess, "check_output", return_value=version), self.assertRaises(ValueError):
                    prepare_android.adb_protoc(Path(directory))

    def test_python38_archive_fallback_rejects_escape_links_and_special_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, kind in (("../escape", tarfile.REGTYPE), ("/escape", tarfile.REGTYPE),
                               ("src/link", tarfile.SYMTYPE), ("src/hard", tarfile.LNKTYPE),
                               ("src/fifo", tarfile.FIFOTYPE)):
                archive = root / "test.tar"
                with tarfile.open(archive, "w") as handle:
                    member = tarfile.TarInfo(name)
                    member.type = kind
                    member.linkname = "../../escape"
                    handle.addfile(member)
                with mock.patch.object(tarfile, "data_filter", create=True):
                    del tarfile.data_filter
                    with self.subTest(name=name), self.assertRaises(ValueError):
                        prepare_android.extract_tar(archive, root / "output")
            with tarfile.open(archive, "w") as handle:
                member = tarfile.TarInfo("src/file")
                member.size = 2
                handle.addfile(member, io.BytesIO(b"ok"))
            with mock.patch.object(tarfile, "data_filter", create=True):
                del tarfile.data_filter
                self.assertEqual((prepare_android.extract_tar(archive, root / "safe") / "file").read_bytes(), b"ok")

    def test_shared_manifest_round_trips_and_detects_modified_adb(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for target in ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu",
                           "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc",
                           "x86_64-apple-darwin", "aarch64-apple-darwin"):
                with self.subTest(target=target):
                    stage = root / target
                    stage_android_fixture(stage, target)
                    prepare_android.write_resource_manifest(stage, target)
                    read = lambda name: (stage / name).read_bytes()
                    verify_native_package.verify_android_resources(read, target)
                    adb = stage / ("adb/adb.exe" if "windows" in target else "adb/adb")
                    adb.write_bytes(adb.read_bytes() + b"changed by packaging")
                    with self.assertRaisesRegex(RuntimeError, "checksum mismatch"):
                        verify_native_package.verify_android_resources(read, target)

    def test_rpm_keeps_hashed_binaries_and_other_postprocessing(self):
        target = "x86_64-unknown-linux-gnu"
        with staged_package(target) as (root, binary, info), \
                mock.patch.object(package_native, "copy_android_resources", side_effect=lambda path, _: stage_android_fixture(path, target)):
            package_native.create_linux_rpm(binary, info, "2.0.0", "2.0.0")
            commands = [call.args[0] for call in package_native.run.call_args_list]
            rpm = next(command for command in commands if command[0] == "rpmbuild")
            self.assertIn("__strip /bin/true", rpm)
            self.assertNotIn("__os_install_post %{nil}", rpm)

    def test_focal_stages_then_audits_and_hashes_android_runtime(self):
        with tempfile.TemporaryDirectory() as directory:
            source, stage = Path(directory) / "source", Path(directory) / "stage"
            stage_android_fixture(source, "x86_64-unknown-linux-gnu")
            stage.mkdir()
            def patch_library(command, **kwargs):
                path = Path(command[-1])
                path.write_bytes(path.read_bytes() + b"rpath")
            with mock.patch.object(focal.subprocess, "run", side_effect=patch_library), \
                    mock.patch.object(focal, "audit", return_value={}) as audit:
                focal.stage_android(source, stage)
                self.assertEqual(audit.call_count, 5)
                self.assertTrue(all(stage in call.args[0].parents for call in audit.call_args_list))
            verify_native_package.verify_android_resources(lambda name: (stage / name).read_bytes(), "x86_64-unknown-linux-gnu")

    def test_focal_rejects_missing_libraries_and_bundled_system_libc(self):
        for changed in ("missing", "private-libc"):
            with self.subTest(changed=changed), tempfile.TemporaryDirectory() as directory:
                source, stage = Path(directory) / "source", Path(directory) / "stage"
                stage_android_fixture(source, "x86_64-unknown-linux-gnu")
                if changed == "missing":
                    (source / "android-libs/libavcodec.so.62").unlink()
                else:
                    (source / "adb/libc.so.6").write_bytes(b"must not ship")
                stage.mkdir()
                with self.assertRaisesRegex(ValueError, "runtime files"):
                    focal.stage_android(source, stage)

    def test_focal_allows_only_android_relative_search_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            stage_android_fixture(root, "x86_64-unknown-linux-gnu")
            binary = root / "adb/adb"
            interpreter = "[Requesting program interpreter: /lib64/ld-linux-x86-64.so.2]"
            for android, path, accepted in ((False, "$ORIGIN/android-libs", False),
                                             (True, "$ORIGIN/android-libs:$ORIGIN/../android-libs", True),
                                             (True, "/build/ffmpeg", False),
                                             (True, "$ORIGIN/../../private", False),
                                             (True, "$ORIGIN/android-libs:", False)):
                with self.subTest(android=android, path=path), mock.patch.object(focal, "command", side_effect=[
                        "Name: GLIBC_2.31", interpreter,
                        "(RUNPATH) Library runpath: [" + path + "]\n(NEEDED) Shared library: [libavcodec.so.62]", ""]):
                    if accepted:
                        focal.audit(binary, android=android)
                    else:
                        with self.assertRaises(ValueError):
                            focal.audit(binary, android=android)


if __name__ == "__main__":
    unittest.main()
