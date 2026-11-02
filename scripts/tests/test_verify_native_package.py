from __future__ import annotations

import io
import plistlib
import struct
import sys
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock


RELEASE_SCRIPTS = Path(__file__).resolve().parents[1] / "release"
sys.path.insert(0, str(RELEASE_SCRIPTS))

import verify_native_package  # noqa: E402
import package_native  # noqa: E402


def fake_pe(
    machine: int, *, subsystem: int = 3, resource_ids: tuple[int, ...] = ()
) -> bytes:
    data = bytearray(1024)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3C, 0x80)
    data[0x80:0x84] = b"PE\0\0"
    struct.pack_into("<H", data, 0x84, machine)
    struct.pack_into("<H", data, 0x86, 1)
    struct.pack_into("<H", data, 0x94, 0xF0)
    optional = 0x98
    struct.pack_into("<H", data, optional, 0x20B)
    struct.pack_into("<H", data, optional + 68, subsystem)
    struct.pack_into("<I", data, optional + 108, 16)
    if resource_ids:
        struct.pack_into("<II", data, optional + 112 + 2 * 8, 0x1000, 0x100)
    section = optional + 0xF0
    data[section : section + 8] = b".rsrc\0\0\0"
    struct.pack_into("<IIII", data, section + 8, 0x100, 0x1000, 0x200, 0x200)
    if resource_ids:
        struct.pack_into("<HH", data, 0x200 + 12, 0, len(resource_ids))
        for index, resource_id in enumerate(resource_ids):
            struct.pack_into("<II", data, 0x210 + index * 8, resource_id, 0)
    return bytes(data)


def fake_macho(cpu_type: int) -> bytes:
    return b"\xcf\xfa\xed\xfe" + cpu_type.to_bytes(4, "little") + bytes(504)


def newc_entry(name: str, content: bytes) -> bytes:
    encoded_name = name.encode("utf-8") + b"\0"
    fields = [0, 0, 0, 0, 1, 0, len(content), 0, 0, 0, 0, len(encoded_name), 0]
    entry = b"070701" + b"".join(f"{value:08x}".encode() for value in fields)
    entry += encoded_name
    entry += bytes((-len(entry)) % 4)
    entry += content
    return entry + bytes((-len(entry)) % 4)


def write_portable(
    path: Path,
    machine: int,
    *,
    helper_machine: int | None = None,
    missing_conpty: str | None = None,
    wrong_conpty_machine: bool = False,
    target: str = "x86_64-pc-windows-msvc",
    application_resources: tuple[int, ...] = (3, 14, 16, 24),
) -> None:
    """Build a portable zip whose layout matches package_native's output."""
    root = "NyaTerm-portable"
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr(
            f"{root}/NyaTerm.exe",
            fake_pe(machine, subsystem=2, resource_ids=application_resources),
        )
        if helper_machine is not None:
            for name in verify_native_package.helper_filenames(
                "x86_64-pc-windows-msvc"
            ):
                archive.writestr(f"{root}/{name}", fake_pe(helper_machine))
        archive.writestr(f"{root}/nyaterm-portable", b"")
        archive.writestr(f"{root}/LICENSE", b"license")
        archive.writestr(f"{root}/VERSION", b"2.0.0\n")
        archive.writestr(f"{root}/data/.keep", b"")
        archive.writestr(f"{root}/conpty/LICENSE.txt", b"license")
        for relative in package_native.conpty_files(target):
            if relative.as_posix() == missing_conpty:
                continue
            expected = 0xAA64 if "arm64" in relative.parts else 0x8664
            archive.writestr(
                f"{root}/conpty/{relative.as_posix()}",
                fake_pe(0xAA64 if wrong_conpty_machine else expected),
            )


def verify_installer_fixture(
    target: str,
    *,
    missing_conpty: str | None = None,
    wrong_conpty_machine: bool = False,
    application_resources: tuple[int, ...] = (3, 14, 16),
) -> None:
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        installer = root / "setup.exe"
        installer.write_bytes(fake_pe(0x8664))
        (root / "nyaterm-installer.nsi").write_text("", encoding="utf-8")

        def extract(command, **kwargs):
            output = Path(command[3][2:])
            entries = {
                "NyaTerm.exe": fake_pe(0x8664, subsystem=2, resource_ids=application_resources),
                "Uninstall.exe": fake_pe(0x8664),
                "LICENSE": b"license",
                "VERSION": b"2.0.0\n",
                "conpty/LICENSE.txt": b"license",
                **{name: fake_pe(0x8664)
                   for name in verify_native_package.helper_filenames(target)},
            }
            for relative in package_native.conpty_files(target):
                if relative.as_posix() == missing_conpty:
                    continue
                expected = 0xAA64 if "arm64" in relative.parts else 0x8664
                machine = expected ^ 1 if wrong_conpty_machine else expected
                entries[f"conpty/{relative.as_posix()}"] = fake_pe(machine)
            for name, data in entries.items():
                path = output / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)

        # Isolate payload validation from NSIS compilation and Windows metadata.
        # Keep the real temporary-directory lifecycle to catch reads after cleanup.
        with (
            mock.patch.object(package_native, "WORK_DIR", root),
            mock.patch.object(verify_native_package.sys, "platform", "linux"),
            mock.patch.object(verify_native_package, "verify_windows_installer_script"),
            mock.patch.object(verify_native_package, "find_7zip", return_value="7z"),
            mock.patch.object(verify_native_package.subprocess, "run", side_effect=extract),
        ):
            verify_native_package.verify_windows_installer(installer, target, "2.0.0")


class VerifyNativePackageTests(unittest.TestCase):
    def setUp(self) -> None:
        # Synthetic PE fixtures only contain directory headers. Test metadata
        # comparisons separately; the Windows resource build test uses real PEs.
        patcher = mock.patch.object(verify_native_package, "windows_version_info")
        self.version_info = patcher.start()
        self.addCleanup(patcher.stop)
        self.version_info.return_value = self.application_metadata("2.0.0")

    @staticmethod
    def application_metadata(version: str) -> dict[str, object]:
        metadata: dict[str, object] = {
            "FileDescription": "NyaTerm", "ProductName": "NyaTerm",
            "FileVersion": package_native.windows_numeric_version(version),
            "ProductVersion": version,
        }
        for prefix in ("File", "Product"):
            for part, value in zip(("Major", "Minor", "Build", "Private"),
                                   package_native.windows_numeric_version(version).split(".")):
                metadata[f"{prefix}{part}Part"] = int(value)
        return metadata

    def test_archive_paths_reject_parent_traversal_and_absolute_paths(self) -> None:
        for path in ("../secret", "dir/../../secret", "/absolute/file"):
            with self.subTest(path=path), self.assertRaises(RuntimeError):
                verify_native_package.require_safe_archive_path(path)
        verify_native_package.require_safe_archive_path("NyaTerm/dir/file")

    def test_windows_portable_has_required_entries(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "portable.zip"
            write_portable(path, 0x8664, helper_machine=0x8664)
            verify_native_package.verify_windows_portable(
                path, "x86_64-pc-windows-msvc", "2.0.0"
            )

    def test_windows_portable_rejects_unknown_duplicate_and_symlink_entries(self) -> None:
        for name in ("unknown.exe", "NYATERM.EXE", "conpty/unknown.dll", "unknown/", "link"):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "portable.zip"
                write_portable(path, 0x8664, helper_machine=0x8664)
                with zipfile.ZipFile(path, "a") as archive:
                    info = zipfile.ZipInfo(f"NyaTerm-portable/{name}")
                    if name == "link":
                        info.create_system = 3
                        info.external_attr = 0o120777 << 16
                    archive.writestr(info, b"unexpected")
                with self.assertRaises(RuntimeError):
                    verify_native_package.verify_windows_portable(path, "x86_64-pc-windows-msvc", "2.0.0")

    def test_windows_portable_rejects_application_without_version_resource(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "portable.zip"
            write_portable(path, 0x8664, helper_machine=0x8664, application_resources=(3, 14))
            with self.assertRaisesRegex(RuntimeError, "version resource"):
                verify_native_package.verify_windows_portable(path, "x86_64-pc-windows-msvc", "2.0.0")

    def test_windows_portable_rejects_application_version_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "portable.zip"
            write_portable(path, 0x8664, helper_machine=0x8664)
            self.version_info.return_value = self.application_metadata("2.0.1")
            with (
                mock.patch.object(verify_native_package.sys, "platform", "win32"),
                self.assertRaisesRegex(RuntimeError, "FileVersion"),
            ):
                verify_native_package.verify_windows_portable(path, "x86_64-pc-windows-msvc", "2.0.0")

    def test_windows_arm64_portable_has_matching_conpty_files(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "portable.zip"
            write_portable(
                path, 0xAA64, helper_machine=0xAA64,
                target="aarch64-pc-windows-msvc",
            )
            verify_native_package.verify_windows_portable(
                path, "aarch64-pc-windows-msvc", "2.0.0"
            )

    def test_windows_portable_rejects_wrong_architecture(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "portable.zip"
            write_portable(path, 0xAA64, helper_machine=0x8664)
            with self.assertRaisesRegex(RuntimeError, "PE machine"):
                verify_native_package.verify_windows_portable(
                    path, "x86_64-pc-windows-msvc", "2.0.0"
                )

    def test_windows_portable_requires_every_helper_binary(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "portable.zip"
            write_portable(path, 0x8664)
            with self.assertRaisesRegex(RuntimeError, "nyaterm-rdp-helper.exe"):
                verify_native_package.verify_windows_portable(
                    path, "x86_64-pc-windows-msvc", "2.0.0"
                )

    def test_windows_portable_requires_conpty_files(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "portable.zip"
            write_portable(
                path, 0x8664, helper_machine=0x8664,
                missing_conpty="x64/arm64/OpenConsole.exe",
            )
            with self.assertRaisesRegex(RuntimeError, "x64/arm64/OpenConsole.exe"):
                verify_native_package.verify_windows_portable(
                    path, "x86_64-pc-windows-msvc", "2.0.0"
                )

    def test_windows_portable_rejects_conpty_architecture_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "portable.zip"
            write_portable(
                path, 0x8664, helper_machine=0x8664, wrong_conpty_machine=True,
            )
            with self.assertRaisesRegex(RuntimeError, "ConPTY PE machine"):
                verify_native_package.verify_windows_portable(
                    path, "x86_64-pc-windows-msvc", "2.0.0"
                )

    def test_windows_installer_validates_conpty_before_temporary_directory_cleanup(self) -> None:
        for target in ("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"):
            with self.subTest(target=target):
                verify_installer_fixture(target)

    def test_windows_installer_requires_conpty_files(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "missing ConPTY files: conpty/x64/conpty.dll"):
            verify_installer_fixture(
                "x86_64-pc-windows-msvc", missing_conpty="x64/conpty.dll",
            )

    def test_windows_installer_rejects_application_without_version_resource(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "version resource"):
            verify_installer_fixture("x86_64-pc-windows-msvc", application_resources=(3, 14))

    def test_windows_installer_rejects_conpty_architecture_mismatch(self) -> None:
        for target in ("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"):
            with self.subTest(target=target), self.assertRaisesRegex(RuntimeError, "ConPTY PE machine"):
                verify_installer_fixture(target, wrong_conpty_machine=True)

    def test_windows_portable_rejects_helper_architecture_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "portable.zip"
            write_portable(path, 0x8664, helper_machine=0xAA64)
            with self.assertRaisesRegex(
                RuntimeError, "PE machine 0xaa64 for nyaterm-rdp-helper.exe"
            ):
                verify_native_package.verify_windows_portable(
                    path, "x86_64-pc-windows-msvc", "2.0.0"
                )

    def test_windows_application_requires_gui_subsystem_and_icon_resources(self) -> None:
        valid = fake_pe(0x8664, subsystem=2, resource_ids=(3, 14, 16, 24))
        verify_native_package.verify_windows_application_binary(valid, "test.exe")

        with self.assertRaisesRegex(RuntimeError, "GUI subsystem"):
            verify_native_package.verify_windows_application_binary(
                fake_pe(0x8664, subsystem=3, resource_ids=(3, 14)), "test.exe"
            )
        for resources in ((14,), (3,), (24,)):
            with self.subTest(resources=resources), self.assertRaisesRegex(
                RuntimeError, "icon resources"
            ):
                verify_native_package.verify_windows_application_binary(
                    fake_pe(0x8664, subsystem=2, resource_ids=resources),
                    "test.exe",
                )

    def test_windows_application_requires_version_resource_on_both_architectures(self) -> None:
        for machine in (0x8664, 0xAA64):
            with self.subTest(machine=machine), self.assertRaisesRegex(RuntimeError, "version resource"):
                verify_native_package.verify_windows_application_binary(
                    fake_pe(machine, subsystem=2, resource_ids=(3, 14)), "test.exe",
                )

    def test_windows_application_rejects_incorrect_version_strings_and_fixed_versions(self) -> None:
        with mock.patch.object(verify_native_package.sys, "platform", "win32"):
            for version in ("2.0.0", "2.0.0-preview.9"):
                expected = self.application_metadata(version)
                self.version_info.return_value = expected
                verify_native_package.verify_windows_application_version(Path("test.exe"), version)
                for key in expected:
                    with self.subTest(version=version, field=key):
                        self.version_info.return_value = {**expected, key: "wrong"}
                        with self.assertRaisesRegex(RuntimeError, key):
                            verify_native_package.verify_windows_application_version(Path("test.exe"), version)

    def test_macos_archive_validates_both_identities_and_rejects_mismatch(self) -> None:
        for version in ("2.0.0", "2.0.0-preview.1"):
            identity = verify_native_package.package_native.release_identity(version)
            bundle = identity.macos_bundle_name
            with self.subTest(version=version), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "NyaTerm.app.tar.gz"
                plist = {
                    "CFBundleIdentifier": identity.macos_identifier,
                    "CFBundleDisplayName": identity.display_name,
                    "CFBundleName": identity.display_name,
                    "CFBundleExecutable": "NyaTerm",
                    "CFBundleShortVersionString": version,
                    "CFBundleVersion": version,
                    "CFBundleURLTypes": [{"CFBundleURLSchemes": list(identity.url_schemes)}],
                }
                entries = {
                    f"{bundle}/Contents/MacOS/NyaTerm": fake_macho(0x0100000C),
                    **{f"{bundle}/Contents/MacOS/{name}": fake_macho(0x0100000C)
                       for name in verify_native_package.helper_filenames("aarch64-apple-darwin")},
                    f"{bundle}/Contents/Info.plist": plistlib.dumps(plist),
                    f"{bundle}/Contents/Resources/VERSION": f"{version}\n".encode(),
                    f"{bundle}/Contents/Resources/LICENSE": b"license",
                    f"{bundle}/Contents/Resources/icon.icns": b"icon",
                }

                def write_archive():
                    with tarfile.open(path, "w:gz") as archive:
                        for name, data in entries.items():
                            item = tarfile.TarInfo(name)
                            item.size = len(data)
                            archive.addfile(item, io.BytesIO(data))

                write_archive()
                verify_native_package.verify_macos_archive(path, "aarch64-apple-darwin", version)
                other = "2.0.0-preview.1" if version == "2.0.0" else "2.0.0"
                with self.assertRaisesRegex(RuntimeError, "is missing"):
                    verify_native_package.verify_macos_archive(path, "aarch64-apple-darwin", other)
                for field, wrong in [("CFBundleIdentifier", "wrong.id"), ("CFBundleDisplayName", "Wrong"),
                                     ("CFBundleName", "Wrong"), ("CFBundleURLTypes", [{"CFBundleURLSchemes": ["ssh"]}])]:
                    original = plist[field]
                    plist[field] = wrong
                    entries[f"{bundle}/Contents/Info.plist"] = plistlib.dumps(plist)
                    write_archive()
                    with self.assertRaises(RuntimeError):
                        verify_native_package.verify_macos_archive(path, "aarch64-apple-darwin", version)
                    plist[field] = original

    def test_macos_scheme_validation_rejects_extra_protocols(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "must register URL schemes"):
            verify_native_package.verify_macos_url_scheme(
                {
                    "CFBundleURLTypes": [
                        {"CFBundleURLSchemes": ["nyaterm", "ssh", "telnet", "unexpected"]}
                    ]
                },
                "Info.plist",
            )

    def test_linux_desktop_validation_requires_nyaterm_scheme_and_percent_u(
        self,
    ) -> None:
        desktop = "\n".join(
            (
                "[Desktop Entry]",
                "Type=Application",
                "Name=NyaTerm",
                "Icon=nyaterm",
                "StartupWMClass=nyaterm",
                "Exec=/opt/nyaterm/nyaterm %U",
                "MimeType=x-scheme-handler/nyaterm;x-scheme-handler/ssh;x-scheme-handler/telnet;",
            )
        )
        verify_native_package.verify_linux_desktop(
            desktop, "/opt/nyaterm/nyaterm", "nyaterm.desktop"
        )
        with self.assertRaisesRegex(RuntimeError, "MimeType"):
            verify_native_package.verify_linux_desktop(
                desktop.replace(
                    "x-scheme-handler/nyaterm;",
                    "x-scheme-handler/unexpected;",
                ),
                "/opt/nyaterm/nyaterm",
                "nyaterm.desktop",
            )
        with self.assertRaisesRegex(RuntimeError, "Exec"):
            verify_native_package.verify_linux_desktop(
                desktop.replace(" %U", ""),
                "/opt/nyaterm/nyaterm",
                "nyaterm.desktop",
            )

    def test_preview_desktop_requires_preview_name_icon_wmclass_and_scheme(self) -> None:
        identity = verify_native_package.package_native.PREVIEW_IDENTITY
        desktop = "\n".join(("[Desktop Entry]", "Type=Application", "Name=NyaTerm Preview",
                             "Icon=nyaterm-preview", "StartupWMClass=nyaterm-preview",
                             "Exec=/opt/nyaterm-preview/nyaterm %U", "MimeType=x-scheme-handler/nyaterm-preview;"))
        verify_native_package.verify_linux_desktop(desktop, "/opt/nyaterm-preview/nyaterm", "preview.desktop", identity)
        for field in ("Name", "Icon", "StartupWMClass", "MimeType"):
            broken = "\n".join(line if not line.startswith(f"{field}=") else f"{field}=stable" for line in desktop.splitlines())
            with self.subTest(field=field), self.assertRaisesRegex(RuntimeError, field):
                verify_native_package.verify_linux_desktop(broken, "/opt/nyaterm-preview/nyaterm", "preview.desktop", identity)

    def test_deb_name_matching_is_exact_not_a_prefix(self) -> None:
        with mock.patch.object(verify_native_package.subprocess, "check_output", return_value=
                               "Package: nyaterm-preview\nVersion: 2.0.0\nArchitecture: amd64\n"):
            with self.assertRaisesRegex(RuntimeError, "wrong Debian package name"):
                verify_native_package.verify_deb(Path("test.deb"), "x86_64-unknown-linux-gnu", "2.0.0")

    def test_rpm_rejects_the_other_flavor_package_metadata(self) -> None:
        with mock.patch.object(verify_native_package.subprocess, "check_output", return_value="nyaterm|2.0.0|0.preview.1|x86_64"):
            with self.assertRaisesRegex(RuntimeError, "RPM metadata"):
                verify_native_package.verify_rpm(Path("test.rpm"), "x86_64-unknown-linux-gnu", "2.0.0-preview.1")

    def test_rpm_member_reader_extracts_desktop_from_newc_payload(self) -> None:
        desktop = b"MimeType=x-scheme-handler/nyaterm;\n"
        payload = newc_entry(
            "./usr/share/applications/nyaterm.desktop", desktop
        ) + newc_entry("TRAILER!!!", b"")
        with (
            mock.patch.object(
                verify_native_package.shutil, "which", return_value="rpm2cpio"
            ),
            mock.patch.object(
                verify_native_package.subprocess,
                "check_output",
                return_value=payload,
            ),
        ):
            actual = verify_native_package.read_rpm_member(
                Path("nyaterm.rpm"), "/usr/share/applications/nyaterm.desktop"
            )
        self.assertEqual(actual, desktop)

    def test_release_verification_fails_before_platform_tools_when_asset_missing(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(RuntimeError, "missing release artifacts"):
                verify_native_package.verify_release(
                    Path(directory), "x86_64-unknown-linux-gnu", "2.0.0"
                )

    def test_stable_windows_verification_requires_msi_before_running_tools(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("NyaTerm_2.0.0_windows_x64-setup.exe", "NyaTerm_2.0.0_windows_x64_portable.zip"):
                (root / name).write_bytes(b"fixture")
            with self.assertRaisesRegex(RuntimeError, "missing release artifacts: NyaTerm_2.0.0_windows_x64.msi"):
                verify_native_package.verify_release(root, "x86_64-pc-windows-msvc", "2.0.0")

    def test_binary_header_helpers_reject_invalid_formats(self) -> None:
        with self.assertRaises(RuntimeError):
            verify_native_package.pe_machine(b"not-pe")
        with self.assertRaises(RuntimeError):
            verify_native_package.elf_machine(b"not-elf")
        with self.assertRaises(RuntimeError):
            verify_native_package.macho_cpu_type(b"not-macho")


if __name__ == "__main__":
    unittest.main()
