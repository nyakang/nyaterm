"""Release inventory, tampering and architecture regressions for SPICE."""
from __future__ import annotations

import hashlib
import json
import os
import platform
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "release"))
import package_native
import prepare_spice
import spice_release
import windows_portable
import verify_native_package
from scripts.tests.test_verify_native_package import spice_fixture, fixture_spice_locked_files, fake_pe, stage_android_fixture


TARGETS = (*prepare_spice.WINDOWS_TARGETS, "x86_64-apple-darwin", "aarch64-apple-darwin",
           "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu")


class SpiceReleaseTests(unittest.TestCase):
    def test_mime_registration_requires_the_connection_type_and_extension(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            package_native.spice_associations.write_linux_mime(root, "nyaterm")
            data = (root / "usr/share/mime/packages/nyaterm-spice.xml").read_bytes()
            package_native.spice_associations.verify_linux_mime(data)
            for broken in (data.replace(b"*.vv", b"*.txt"),
                           data.replace(b"application/x-virt-viewer", b"text/plain")):
                with self.assertRaises(RuntimeError):
                    package_native.spice_associations.verify_linux_mime(broken)

    def test_private_runtime_glibc_floor_uses_numeric_versions_and_rejects_private_abi(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime = Path(directory)
            (runtime / "SOURCE.json").write_text(json.dumps({"files": {"libnative.so.0": "unused"}}))
            for output, expected in (("Name: GLIBC_2.9\nName: GLIBC_2.35", "2.35"),
                                     ("Name: GLIBC_PRIVATE", None), ("", None)):
                with self.subTest(output=output), mock.patch.object(spice_release.subprocess, "check_output", return_value=output):
                    if expected:
                        self.assertEqual(spice_release.linux_glibc_requirement(runtime), expected)
                    else:
                        with self.assertRaises(RuntimeError):
                            spice_release.linux_glibc_requirement(runtime)

    def test_every_release_target_has_a_complete_runtime_and_helper_contract(self):
        for target in TARGETS:
            with self.subTest(target=target):
                self.assertNotIn("nyaterm-spice-helper", package_native.helper_bins(target))
                self.assertTrue(any("component_spice" in name for name in package_native.component_artifact_names(target, "2.0.8")))
                entries = spice_fixture(target)
                with mock.patch.object(spice_release, "locked_files", side_effect=fixture_spice_locked_files):
                    spice_release.verify_resources(entries.__getitem__, target)
                if "windows" in target:
                    self.assertEqual(set(entries), set(windows_portable.LAYOUT["spice_files"][target]))
                    self.assertTrue(set(entries).isdisjoint(windows_portable.payload_files(target)))

    def test_artifact_rejects_missing_plugins_modified_bytes_and_wrong_target(self):
        target = "x86_64-pc-windows-msvc"
        name = "spice/gstreamer-1.0/libgstvpx.dll"
        for kind in ("missing", "modified", "wrong-target", "forged-manifest"):
            with self.subTest(kind=kind), mock.patch.object(spice_release, "locked_files", side_effect=fixture_spice_locked_files):
                entries = spice_fixture(target)
                manifest = json.loads(entries["spice/SOURCE.json"])
                if kind == "missing":
                    manifest["files"].pop(name.removeprefix("spice/"))
                    entries.pop(name)
                elif kind == "modified":
                    entries[name] += b"changed"
                elif kind == "wrong-target":
                    manifest["target"] = "aarch64-pc-windows-msvc"
                else:
                    entries[name] = fake_pe(0xAA64)
                    manifest["files"][name.removeprefix("spice/")] = hashlib.sha256(entries[name]).hexdigest()
                entries["spice/SOURCE.json"] = json.dumps(manifest).encode()
                with self.assertRaises(RuntimeError):
                    spice_release.verify_resources(entries.__getitem__, target)

    @mock.patch.object(spice_release, "locked_files", side_effect=fixture_spice_locked_files)
    def test_artifact_rejects_unmanifested_native_plugins_on_every_platform(self, _locked):
        for target in TARGETS:
            with self.subTest(target=target):
                entries = spice_fixture(target)
                spice_release.verify_resources(entries.__getitem__, target, names=entries)
                entries["spice/gstreamer-1.0/unexpected-plugin.dll"] = b"untracked"
                with self.assertRaisesRegex(RuntimeError, "unexpected runtime files"):
                    spice_release.verify_resources(entries.__getitem__, target, names=entries)

    @mock.patch.object(spice_release, "locked_files", side_effect=fixture_spice_locked_files)
    def test_unix_architecture_and_source_revision_are_checked_in_final_artifacts(self, _locked):
        target = "aarch64-apple-darwin"
        entries = spice_fixture(target)
        manifest = json.loads(entries["spice/SOURCE.json"])
        name = "libspice-client-glib-2.0.8.dylib"
        entries["spice/" + name] = b"\xcf\xfa\xed\xfe" + (0x01000007).to_bytes(4, "little") + bytes(100)
        manifest["files"][name] = hashlib.sha256(entries["spice/" + name]).hexdigest()
        entries["spice/SOURCE.json"] = json.dumps(manifest).encode()
        with self.assertRaisesRegex(RuntimeError, "native architecture"):
            spice_release.verify_resources(entries.__getitem__, target)
        entries = spice_fixture(target)
        manifest = json.loads(entries["spice/SOURCE.json"])
        manifest["native_source"]["revision"] = "0" * 40
        entries["spice/SOURCE.json"] = json.dumps(manifest).encode()
        with self.assertRaisesRegex(RuntimeError, "source revision"):
            spice_release.verify_resources(entries.__getitem__, target)

    @mock.patch.object(spice_release, "locked_files", side_effect=fixture_spice_locked_files)
    def test_macos_package_rejects_newer_os_library_even_with_matching_hash(self, _locked):
        target = "aarch64-apple-darwin"
        entries = spice_fixture(target)
        name = "libspice-client-glib-2.0.8.dylib"
        library = bytearray(entries["spice/" + name])
        struct.pack_into("<I", library, 44, 14 << 16)
        entries["spice/" + name] = bytes(library)
        manifest = json.loads(entries["spice/SOURCE.json"])
        manifest["files"][name] = hashlib.sha256(library).hexdigest()
        entries["spice/SOURCE.json"] = json.dumps(manifest).encode()
        with self.assertRaisesRegex(RuntimeError, "requires macOS 14"):
            spice_release.verify_resources(entries.__getitem__, target)

    @unittest.skipUnless(os.environ.get("NYATERM_TEST_SPICE_RUNTIME"), "requires a prepared native runtime")
    def test_actual_runtime_stages_and_verifies_without_changing_library_bytes(self):
        runtime = Path(os.environ["NYATERM_TEST_SPICE_RUNTIME"])
        manifest = json.loads((runtime / "SOURCE.json").read_text(encoding="utf-8"))
        target = manifest["target"]
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(package_native, "spice_runtime", return_value=runtime):
            root = Path(directory)
            copied = package_native.copy_spice_resources(root, target)
            spice_release.verify_resources(lambda name: (root / name).read_bytes(), target)
            for path in copied:
                relative = path.relative_to(root / "spice")
                self.assertEqual(path.read_bytes(), (runtime / relative).read_bytes())

    @unittest.skipUnless(os.environ.get("NYATERM_TEST_SPICE_RUNTIME") and sys.platform == "linux"
                         and all(shutil.which(tool) for tool in ("cc", "rpmbuild", "rpm2cpio", "dpkg-deb")),
                         "requires a prepared Linux runtime and native package tools")
    def test_actual_runtime_preserves_its_bytes_in_real_rpm_and_deb_packages(self):
        runtime = Path(os.environ["NYATERM_TEST_SPICE_RUNTIME"])
        manifest = json.loads((runtime / "SOURCE.json").read_bytes())
        target = manifest["target"]
        expected = "aarch64" if platform.machine() in ("aarch64", "arm64") else "x86_64"
        self.assertEqual(target, expected + "-unknown-linux-gnu")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "probe.c"
            source.write_text("int main(void) { return 0; }\n")
            binary = root / "nyaterm"
            subprocess.run(["cc", str(source), "-o", str(binary)], check=True)
            original = binary.read_bytes()
            helpers = []
            for name in package_native.helper_bins(target):
                helper = root / name
                shutil.copy2(binary, helper)
                helpers.append(helper)

            def android(destination, _):
                import prepare_android
                stage_android_fixture(destination, target)
                for path in [destination / "adb/adb", *list((destination / "android-libs").glob("*.so.*"))]:
                    path.write_bytes(original)
                return prepare_android.write_resource_manifest(destination, target)

            dist = root / "dist"
            dist.mkdir()
            with mock.patch.object(package_native, "WORK_DIR", root / "work"), \
                    mock.patch.object(package_native, "DIST_DIR", dist), \
                    mock.patch.object(package_native, "helper_binary_paths", return_value=helpers), \
                    mock.patch.object(package_native, "spice_runtime", return_value=runtime), \
                    mock.patch.object(package_native, "copy_android_resources", side_effect=android):
                info = package_native.target_info(target)
                package_native.create_linux_rpm(binary, info, "2.0.0", "2.0.0")
                package_native.create_linux_deb(binary, info, "2.0.0", "2.0.0")
            rpm = next(dist.glob("*.rpm"))
            floor = spice_release.linux_glibc_requirement(runtime)
            provides = subprocess.check_output(["rpm", "-qp", "--provides", str(rpm)], text=True)
            requires = subprocess.check_output(["rpm", "-qp", "--requires", str(rpm)], text=True)
            self.assertNotIn("libspice-client-glib", provides)
            self.assertNotIn("libcrypto.so", provides)
            self.assertNotIn("libgst", provides)
            self.assertNotIn("libspice-client-glib", requires)
            self.assertNotIn("libcrypto.so", requires)
            self.assertIn("glibc >= " + floor, requires)
            depends = subprocess.check_output(["dpkg-deb", "-f", str(next(dist.glob("*.deb"))), "Depends"], text=True)
            self.assertIn("libc6 (>= " + floor + ")", depends)
            verify_native_package.verify_rpm(rpm, target, "2.0.0")
            verify_native_package.verify_deb(next(dist.glob("*.deb")), target, "2.0.0")
            payload = verify_native_package.read_rpm_payload(rpm)
            for name in manifest["files"]:
                self.assertEqual(payload["opt/nyaterm/spice/" + name], (runtime / name).read_bytes())
