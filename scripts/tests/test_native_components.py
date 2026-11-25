from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "release"))
import native_components
import package_native
from scripts.tests.test_verify_native_package import fake_pe


class NativeComponentsTests(unittest.TestCase):
    def package(self, directory, component="vnc"):
        root = Path(directory) / "payload"
        root.mkdir()
        (root / f"nyaterm-{component}-helper.exe").write_bytes(fake_pe(0x8664))
        (root / "LICENSE").write_bytes(b"license")
        output = Path(directory) / f"NyaTerm_2.0.8_windows_x64_component_{component}.zip"
        native_components.write_package(root, output, component, "2.0.8", "a" * 40, "x86_64-pc-windows-msvc")
        return output

    def test_each_component_has_an_exact_independent_inventory(self):
        for component in native_components.IDS:
            with self.subTest(component=component), tempfile.TemporaryDirectory() as directory:
                output = self.package(directory, component)
                manifest = native_components.verify(output, signed=False)
                self.assertEqual(manifest["component"], component)
                self.assertEqual(manifest["build_id"], "a" * 40)
                self.assertEqual(set(manifest["files"]), {f"nyaterm-{component}-helper.exe", "LICENSE"})
                with mock.patch.object(native_components, "verify_payload"), self.assertRaises(KeyError):
                    native_components.verify(output)

    def test_signature_addition_preserves_build_provenance_but_payload_change_does_not(self):
        with tempfile.TemporaryDirectory() as directory:
            output = self.package(directory)
            before = native_components.provenance_hash(output)
            with zipfile.ZipFile(output, "a") as archive:
                archive.writestr("component.minisig", b"not-a-valid-signature")
            self.assertEqual(native_components.provenance_hash(output), before)
            with mock.patch.object(native_components.subprocess, "run", side_effect=RuntimeError("invalid signature")):
                with self.assertRaisesRegex(RuntimeError, "invalid signature"):
                    native_components.verify(output)
            with zipfile.ZipFile(output) as archive:
                files = {name: archive.read(name) for name in archive.namelist()}
            files["nyaterm-vnc-helper.exe"] = b"changed"
            with zipfile.ZipFile(output, "w") as archive:
                for name, data in files.items(): archive.writestr(name, data)
            with self.assertRaisesRegex(ValueError, "checksum"):
                native_components.provenance_hash(output)

    def test_unlisted_dll_and_case_alias_are_rejected(self):
        for name in ("injected.dll", "license"):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                output = self.package(directory)
                with zipfile.ZipFile(output, "a") as archive: archive.writestr(name, b"unexpected")
                with self.assertRaises(ValueError): native_components.verify(output, signed=False)

    def test_component_publish_uses_immutable_build_and_platform_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            output = self.package(directory)
            manifest = native_components.verify(output, signed=False)
            store = mock.Mock()
            with mock.patch.object(native_components, "verify", return_value=manifest):
                native_components.publish(store, Path(directory))
            store.put.assert_called_once_with("components/" + "a" * 40 + "/x86_64-pc-windows-msvc/vnc.zip", output, immutable=True)

    def test_empty_component_publication_fails_without_writing(self):
        with tempfile.TemporaryDirectory() as directory:
            store = mock.Mock()
            with self.assertRaisesRegex(ValueError, "no native component"):
                native_components.publish(store, Path(directory))
            store.put.assert_not_called()

    def test_helper_architecture_is_verified_before_signing(self):
        with tempfile.TemporaryDirectory() as directory:
            output = self.package(directory)
            manifest, files = native_components.inspect(output)
            native_components.verify_payload(manifest, files)
            files[manifest["entry"]] = fake_pe(0xAA64)
            with self.assertRaisesRegex(ValueError, "architecture"):
                native_components.verify_payload(manifest, files)

    def test_base_contract_excludes_all_optional_helpers_and_runtime_files(self):
        for target in ("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"):
            files = package_native.windows_portable.payload_files(target)
            self.assertIn("nyaterm-mcp.exe", files)
            self.assertFalse(any("-helper" in name or name.startswith(("adb/", "spice/", "android-libs/")) for name in files))


if __name__ == "__main__":
    unittest.main()
