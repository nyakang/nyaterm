"""Check MSI release contracts; integration tests never install the packages."""

from __future__ import annotations

import copy
import shutil
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "release"))
import package_native
import verify_native_package
import windows_msi
from scripts.tests.test_verify_native_package import fake_pe


def stage_payload(root: Path, target: str) -> None:
    root.mkdir(parents=True)
    machine = 0x8664 if target.startswith("x86_64") else 0xAA64
    (root / "NyaTerm.exe").write_bytes(fake_pe(machine, subsystem=2, resource_ids=(3, 14, 16)))
    for name in package_native.HELPER_BINS:
        (root / f"{name}.exe").write_bytes(fake_pe(machine))
    (root / "VERSION").write_text("2.0.0\n", encoding="utf-8")
    (root / "LICENSE").write_text("test license", encoding="utf-8")
    shutil.copy2(package_native.ICON_DIR / "icon.ico", root / "icon.ico")
    for relative in package_native.conpty_files(target):
        path = root / "conpty" / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(fake_pe(0xAA64 if "arm64" in relative.parts else 0x8664))
    (root / "conpty/LICENSE.txt").write_text("ConPTY test license", encoding="utf-8")


class WindowsMsiTests(unittest.TestCase):
    def test_only_stable_releases_include_msi(self) -> None:
        for version, expected in (("2.0.0", True), ("2.0.0+build-with-hyphen", True),
                                  ("2.0.0-preview.1", False), ("2.0.0-rc.1", False),
                                  ("main-snapshot", False)):
            with self.subTest(version=version):
                self.assertEqual(package_native.includes_windows_msi(version), expected)
                for target in ("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"):
                    names = package_native.artifact_names(target, version)
                    self.assertEqual(sum(name.endswith(".msi") for name in names), int(expected))
                    self.assertEqual(len(names), 3 if expected else 2)

    def test_msi_version_obeys_three_field_windows_installer_limits(self) -> None:
        for version, expected in (("v2.0.0", "2.0.0"), ("2.0.0+build.7", "2.0.0"),
                                  ("255.255.65535", "255.255.65535")):
            self.assertEqual(package_native.windows_msi_version(version), expected)
        for version in ("256.0.0", "2.256.0", "2.0.65536", "2.0.0-preview.9", "main-snapshot"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                package_native.windows_msi_version(version)

    def test_wix_version_is_pinned_and_checked(self) -> None:
        with mock.patch.object(windows_msi.shutil, "which", return_value="wix"):
            for version, accepted in (("6.0.2+commit\n", True), ("7.0.0\n", False)):
                with mock.patch.object(windows_msi.subprocess, "check_output", return_value=version):
                    if accepted:
                        self.assertEqual(windows_msi.find_wix(), "wix")
                    else:
                        with self.assertRaisesRegex(RuntimeError, "expected WiX 6.0.2"):
                            windows_msi.find_wix()

    def test_component_identity_is_stable_and_product_identity_changes(self) -> None:
        self.assertEqual(windows_msi.product_code("2.0.0", "x64"), "2A20BF91-AE77-5C55-B7B5-4EA06FF5C4A0")
        self.assertNotEqual(windows_msi.product_code("2.0.0", "x64"),
                            windows_msi.product_code("2.0.1", "x64"))
        self.assertNotEqual(windows_msi.product_code("2.0.0", "x64"),
                            windows_msi.product_code("2.0.0", "arm64"))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            staging = root / 'payload with spaces & symbols'
            stage_payload(staging, "x86_64-pc-windows-msvc")
            components = []
            ns = {"w": windows_msi.WIX_NAMESPACE}
            for version in ("2.0.0", "2.0.1"):
                source = root / f"{version}.wxs"
                windows_msi.write_source(staging, source, version, "x64")
                xml = ET.parse(source)
                components.append({node.attrib["Id"]: node.attrib["Guid"]
                                   for node in xml.findall(".//w:Component", ns)})
                files = xml.findall(".//w:File", ns)
                self.assertEqual(len(files), sum(path.is_file() for path in staging.rglob("*")))
                self.assertTrue(all(Path(node.attrib["Source"]).is_file() for node in files))
            self.assertEqual(components[0], components[1])


@unittest.skipUnless(sys.platform == "win32" and shutil.which("wix"), "requires Windows and WiX 6.0.2")
class CompiledWindowsMsiTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.temporary = tempfile.TemporaryDirectory(prefix="nyaterm-msi-test-")
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.root = Path(cls.temporary.name)
        cls.packages = {}
        for target in ("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"):
            root = cls.root / target
            staging = root / "payload with spaces & symbols"
            stage_payload(staging, target)
            work, dist = root / "work", root / "dist"
            work.mkdir()
            dist.mkdir()
            info = package_native.target_info(target)
            with (mock.patch.object(package_native, "WORK_DIR", work),
                  mock.patch.object(package_native, "DIST_DIR", dist)):
                package_native.create_windows_msi(staging, info, "2.0.0", "2.0.0")
            path = dist / f"NyaTerm_2.0.0_{info.label}.msi"
            cls.packages[target] = (path, windows_msi.read_database(path), staging)

    def test_compiles_and_extracts_complete_packages_for_both_architectures(self) -> None:
        # Synthetic PEs test cabinet packaging and architecture checks. The real
        # application resource build has its own test_windows_resources coverage.
        for target, (path, _, _) in self.packages.items():
            with (self.subTest(target=target),
                  mock.patch.object(verify_native_package, "verify_windows_application_version")):
                verify_native_package.verify_windows_msi(path, target, "2.0.0")

    def test_compiled_database_rejects_wrong_identity_scope_architecture_and_version(self) -> None:
        for target, (_, baseline, _) in self.packages.items():
            for property_name in ("UpgradeCode", "ProductCode", "ProductVersion", "ALLUSERS"):
                changed = copy.deepcopy(baseline)
                next(row for row in changed["tables"]["Property"]
                     if row["Property"] == property_name)["Value"] = "wrong"
                with self.subTest(target=target, property=property_name), self.assertRaises(RuntimeError):
                    verify_native_package.verify_windows_msi_database(changed, target, "2.0.0")
            changed = copy.deepcopy(baseline)
            changed["template"] = "wrong;1033"
            with self.assertRaisesRegex(RuntimeError, "architecture"):
                verify_native_package.verify_windows_msi_database(changed, target, "2.0.0")

    def test_compiled_database_requires_all_helpers_and_conpty_files(self) -> None:
        for target, (_, baseline, _) in self.packages.items():
            files = verify_native_package.verify_windows_msi_database(baseline, target, "2.0.0")
            for name in (*verify_native_package.helper_filenames(target),
                         *(f"conpty/{p.as_posix()}" for p in package_native.conpty_files(target))):
                changed = copy.deepcopy(baseline)
                changed["tables"]["File"] = [row for row in changed["tables"]["File"] if row["File"] != files[name]]
                with self.subTest(target=target, missing=name), self.assertRaisesRegex(RuntimeError, "file set mismatch"):
                    verify_native_package.verify_windows_msi_database(changed, target, "2.0.0")

    def test_compiled_database_rejects_unsafe_cleanup_and_upgrade_policy(self) -> None:
        target = "x86_64-pc-windows-msvc"
        baseline = self.packages[target][1]
        for table, field, value in (
            ("Component", "ComponentId", "wrong"), ("Directory", "DefaultDir", "../data"),
            ("Upgrade", "Attributes", "0"), ("RemoveFile", "FileName", "*"),
            ("Shortcut", "Target", "wrong.exe"), ("CustomAction", "Target", "--unsafe-command"),
        ):
            changed = copy.deepcopy(baseline)
            changed["tables"][table][0][field] = value
            with self.subTest(table=table), self.assertRaises(RuntimeError):
                verify_native_package.verify_windows_msi_database(changed, target, "2.0.0")
        changed = copy.deepcopy(baseline)
        next(row for row in changed["tables"]["InstallExecuteSequence"]
             if row["Action"] == "RemoveExistingProducts")["Sequence"] = "6700"
        with self.assertRaisesRegex(RuntimeError, "InstallInitialize"):
            verify_native_package.verify_windows_msi_database(changed, target, "2.0.0")
        changed = copy.deepcopy(baseline)
        changed["tables"]["LaunchCondition"] = []
        with self.assertRaisesRegex(RuntimeError, "same-version"):
            verify_native_package.verify_windows_msi_database(changed, target, "2.0.0")

    def test_extracted_payload_rejects_wrong_pe_architecture_and_version(self) -> None:
        target = "x86_64-pc-windows-msvc"
        for relative, content, message in (
            ("NyaTerm.exe", fake_pe(0xAA64), "PE machine"),
            ("nyaterm-vnc-helper.exe", fake_pe(0xAA64), "PE machine"),
            ("conpty/x64/conpty.dll", fake_pe(0xAA64), "ConPTY PE machine"),
            ("VERSION", b"2.0.1\n", "installed version"),
        ):
            with self.subTest(file=relative), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                staging = root / "payload"
                stage_payload(staging, target)
                (staging / relative).write_bytes(content)
                source, output = root / "package.wxs", root / "package.msi"
                windows_msi.write_source(staging, source, "2.0.0", "x64")
                subprocess.run([windows_msi.find_wix(), "build", "-arch", "x64", "-pdbtype", "none",
                                "-o", str(output), str(source)], check=True)
                with (mock.patch.object(verify_native_package, "verify_windows_application_version"),
                      self.assertRaisesRegex(RuntimeError, message)):
                    verify_native_package.verify_windows_msi(output, target, "2.0.0")


if __name__ == "__main__":
    unittest.main()
