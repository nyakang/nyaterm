from __future__ import annotations

import json
import plistlib
import re
import shutil
import subprocess
import struct
import tarfile
from contextlib import contextmanager
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


RELEASE_SCRIPTS = Path(__file__).resolve().parents[1] / "release"
sys.path.insert(0, str(RELEASE_SCRIPTS))

import package_native  # noqa: E402
import verify_native_package  # noqa: E402


@contextmanager
def staged_package(target: str):
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        binary = root / "build" / "nyaterm"
        binary.parent.mkdir()
        binary.write_bytes(b"application")
        suffix = ".exe" if "windows" in target else ""
        helpers = [binary.parent / f"{name}{suffix}" for name in package_native.HELPER_BINS]
        for helper in helpers:
            helper.write_bytes(b"helper")

        def run(command, **kwargs):
            if command[0] == "rpmbuild":
                output = root / "work" / "rpm" / "rpmbuild" / "RPMS" / package_native.linux_rpm_arch(target)
                output.mkdir(parents=True)
                (output / "test.rpm").write_bytes(b"rpm")

        with (
            mock.patch.object(package_native, "WORK_DIR", root / "work"),
            mock.patch.object(package_native, "DIST_DIR", root / "dist"),
            mock.patch.object(package_native, "helper_binary_paths", return_value=helpers),
            mock.patch.object(package_native, "require_tool", side_effect=lambda name: name),
            mock.patch.object(package_native, "find_makensis", return_value="makensis"),
            mock.patch.object(package_native, "run", side_effect=run),
            mock.patch.object(package_native, "linux_deb_dependencies", return_value="libc6"),
        ):
            package_native.WORK_DIR.mkdir()
            package_native.DIST_DIR.mkdir()
            yield root, binary, package_native.target_info(target)


class PackageNativeTests(unittest.TestCase):
    def test_semver_resolves_application_identity(self) -> None:
        for version in ("2.0.0", "2.0.1", "2.1.0", "2.0.0+build-with-hyphen"):
            self.assertEqual(package_native.release_identity(version), package_native.STABLE_IDENTITY)
        for version in ("2.0.0-preview.1", "2.0.0-preview.2", "2.0.0-beta.1", "2.0.0-rc.1"):
            self.assertEqual(package_native.release_identity(version), package_native.PREVIEW_IDENTITY)
        for version in ("02.0.0", "2.0.0-preview.01", "2.0.0-", "2.0.0+", "2.0.0-a..b"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                package_native.release_identity(version)

    def test_windows_identity_isolation_in_generated_installer(self) -> None:
        for version in ("2.0.0", "2.0.0-preview.1"):
            with self.subTest(version=version), staged_package("x86_64-pc-windows-msvc") as (root, binary, info):
                identity = package_native.release_identity(version)
                package_native.create_windows_packages(binary, info, version, version)
                script = (root / "work" / "nyaterm-installer.nsi").read_text()
                verify_native_package.verify_windows_installer_script(script, version)
                numeric_version = package_native.windows_numeric_version(version)
                self.assertIn(f'VIProductVersion "{numeric_version}"', script)
                self.assertIn(f'VIFileVersion "{numeric_version}"', script)
                self.assertIn(f'VIAddVersionKey "ProductVersion" "{version}"', script)
                self.assertIn(f'VIAddVersionKey "FileVersion" "{version}"', script)
                self.assertIn(f'InstallDir "$LOCALAPPDATA\\Programs\\{identity.display_name}"', script)
                self.assertIn(f'DeleteRegKey HKCU "{identity.windows_registry_key}"', script)
                other_version = "2.0.0-preview.1" if version == "2.0.0" else "2.0.0"
                with self.assertRaises(RuntimeError):
                    verify_native_package.verify_windows_installer_script(script, other_version)
                other = package_native.release_identity(other_version)
                with self.assertRaises(RuntimeError):
                    verify_native_package.verify_windows_installer_script(
                        script + f'\nDeleteRegKey HKCU "{other.windows_registry_key}"', version
                    )

    def test_macos_bundle_and_updater_archive_use_the_same_identity(self) -> None:
        for version in ("2.0.0", "2.0.0-preview.1"):
            with self.subTest(version=version), staged_package("aarch64-apple-darwin") as (root, binary, info):
                identity = package_native.release_identity(version)
                package_native.create_macos_packages(binary, info, version, version)
                bundle = root / "work" / identity.macos_bundle_name
                plist = plistlib.loads((bundle / "Contents" / "Info.plist").read_bytes())
                self.assertEqual(plist["CFBundleDisplayName"], identity.display_name)
                self.assertEqual(plist["CFBundleName"], identity.display_name)
                self.assertEqual(plist["CFBundleIdentifier"], identity.macos_identifier)
                self.assertEqual(plist["CFBundleURLTypes"][0]["CFBundleURLSchemes"], [identity.desktop_id])
                self.assertTrue((root / "work" / "dmg" / identity.macos_bundle_name).is_dir())
                self.assertEqual((root / "work" / "dmg" / "Applications").readlink().as_posix(), "/Applications")
                archive = next((root / "dist").glob("*.tar.gz"))
                with tarfile.open(archive) as handle:
                    self.assertIn(f"{identity.macos_bundle_name}/Contents/MacOS/NyaTerm", handle.getnames())

    def test_run_with_retry_retries_transient_command_failures(self) -> None:
        failure = subprocess.CalledProcessError(1, ["hdiutil", "create"])
        with (
            mock.patch.object(package_native, "run", side_effect=[failure, None]) as run,
            mock.patch.object(package_native.time, "sleep") as sleep,
        ):
            package_native.run_with_retry(
                ["hdiutil", "create"], attempts=3, delay_seconds=0.5
            )
        self.assertEqual(run.call_count, 2)
        sleep.assert_called_once_with(0.5)

    def test_linux_formats_use_isolated_package_desktop_icons_and_install_paths(self) -> None:
        for version in ("2.0.0", "2.0.0-preview.1"):
            with self.subTest(version=version), staged_package("x86_64-unknown-linux-gnu") as (root, binary, info):
                identity = package_native.release_identity(version)
                package_native.create_linux_packages(binary, info, version, version)
                control = (root / "work" / "deb" / "DEBIAN" / "control").read_text()
                self.assertIn(f"Package: {identity.desktop_id}\n", control)
                spec = (root / "work" / "rpm" / "rpmbuild" / "SPECS" / "nyaterm.spec").read_text()
                self.assertIn(f"Name: {identity.desktop_id}\n", spec)
                self.assertIn(f"/opt/{identity.desktop_id}\n", spec)
                for payload in (root / "work" / "deb", root / "work" / "rpm" / "payload", root / "work" / "NyaTerm.AppDir"):
                    desktop = payload / "usr" / "share" / "applications" / identity.linux_desktop_file
                    executable = "nyaterm" if payload.name == "NyaTerm.AppDir" else f"/opt/{identity.desktop_id}/nyaterm"
                    verify_native_package.verify_linux_desktop(desktop.read_text(), executable, str(desktop), identity)
                    self.assertTrue((payload / "usr" / "share" / "icons" / "hicolor" / "128x128" / "apps" / f"{identity.desktop_id}.png").is_file())
                    app_root = payload / "usr" / "bin" if payload.name == "NyaTerm.AppDir" else payload / "opt" / identity.desktop_id
                    for name in ("nyaterm", *package_native.HELPER_BINS):
                        self.assertTrue((app_root / name).is_file())

    def test_nix_package_uses_isolated_package_desktop_and_icons(self) -> None:
        repo_root = Path(__file__).resolve().parents[2]
        package_nix_path = repo_root / "nix" / "package.nix"
        self.assertTrue(package_nix_path.is_file())
        package_nix = package_nix_path.read_text()

        self.assertIn('isPrerelease = lib.hasInfix "-" version;', package_nix)
        self.assertIn('displayName = "NyaTerm Preview";', package_nix)
        self.assertIn('desktopId = "nyaterm-preview";', package_nix)
        self.assertIn('displayName = "NyaTerm";', package_nix)
        self.assertIn('desktopId = "nyaterm";', package_nix)
        self.assertIn('pname = identity.desktopId;', package_nix)
        self.assertIn('applications/${identity.desktopId}.desktop', package_nix)
        self.assertIn('apps/${identity.desktopId}.png', package_nix)

        match = re.search(r"<<EOF\n(\[Desktop Entry\].*?)\nEOF", package_nix, re.DOTALL)
        self.assertIsNotNone(match, "desktop template not found in nix/package.nix")
        desktop_template = match.group(1)

        for version in ("2.0.0", "2.0.0-preview.1"):
            with self.subTest(version=version):
                identity = package_native.release_identity(version)
                rendered = (
                    desktop_template
                    .replace("${identity.displayName}", identity.display_name)
                    .replace("${identity.desktopId}", identity.desktop_id)
                )
                verify_native_package.verify_linux_desktop(
                    rendered, "nyaterm", f"nix/package.nix ({version})", identity
                )

        if shutil.which("nix"):
            res = subprocess.run(
                [
                    "nix", "eval", "--impure", "--json", "--expr",
                    """
                    let
                      pkgs = (builtins.getFlake (toString ./.)).inputs.nixpkgs.legacyPackages.x86_64-linux;
                      evalPkg = v: pkgs.callPackage ./nix/package.nix { version = v; };
                    in {
                      stable = let p = evalPkg "2.0.0"; in { inherit (p) pname postInstall postFixup; inherit (p.identity) displayName desktopId; };
                      preview = let p = evalPkg "2.0.0-preview.1"; in { inherit (p) pname postInstall postFixup; inherit (p.identity) displayName desktopId; };
                    }
                    """
                ],
                cwd=str(repo_root),
                capture_output=True,
                text=True,
            )
            if res.returncode == 0:
                data = json.loads(res.stdout)
                for version, key in (("2.0.0", "stable"), ("2.0.0-preview.1", "preview")):
                    with self.subTest(nix_eval=version):
                        identity = package_native.release_identity(version)
                        pkg_data = data[key]
                        self.assertEqual(pkg_data["pname"], identity.desktop_id)
                        self.assertEqual(pkg_data["desktopId"], identity.desktop_id)
                        self.assertEqual(pkg_data["displayName"], identity.display_name)
                        self.assertIn(
                            f"applications/{identity.desktop_id}.desktop",
                            pkg_data["postInstall"],
                        )
                        self.assertIn(
                            f"apps/{identity.desktop_id}.png",
                            pkg_data["postInstall"],
                        )
                        m = re.search(
                            r"<<EOF\n(\[Desktop Entry\].*?)\nEOF",
                            pkg_data["postInstall"],
                            re.DOTALL,
                        )
                        self.assertIsNotNone(m)
                        verify_native_package.verify_linux_desktop(
                            m.group(1), "nyaterm", f"nix evaluated ({version})", identity
                        )

    def test_release_tag_is_normalized(self) -> None:
        self.assertEqual(package_native.validate_version("v2.0.0"), "2.0.0")
        self.assertEqual(
            package_native.validate_version("2.0.0-preview.1"),
            "2.0.0-preview.1",
        )

    def test_invalid_or_mismatched_version_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            package_native.validate_version("release-2")
        with self.assertRaisesRegex(ValueError, "does not match"):
            package_native.validate_version("v2.0.1", "2.0.0")

    def test_snapshot_is_only_allowed_as_an_artifact_label(self) -> None:
        self.assertEqual(
            package_native.validate_artifact_version("main-snapshot"),
            "main-snapshot",
        )
        with self.assertRaises(ValueError):
            package_native.validate_version("main-snapshot")
        with self.assertRaises(ValueError):
            package_native.validate_artifact_version("nightly")
        self.assertEqual(
            package_native.artifact_names(
                "x86_64-pc-windows-msvc", "main-snapshot"
            ),
            {
                "NyaTerm_main-snapshot_windows_x64_portable.zip",
                "NyaTerm_main-snapshot_windows_x64-setup.exe",
            },
        )

    def test_all_release_targets_have_expected_artifact_names(self) -> None:
        expected = {
            "aarch64-apple-darwin": {
                "NyaTerm_2.0.0_macos_arm64.dmg",
                "NyaTerm_2.0.0_macos_arm64.app.tar.gz",
            },
            "x86_64-apple-darwin": {
                "NyaTerm_2.0.0_macos_x64.dmg",
                "NyaTerm_2.0.0_macos_x64.app.tar.gz",
            },
            "aarch64-unknown-linux-gnu": {
                "NyaTerm_2.0.0_linux_arm64.AppImage",
                "NyaTerm_2.0.0_linux_arm64.deb",
                "NyaTerm_2.0.0_linux_arm64.rpm",
            },
            "x86_64-unknown-linux-gnu": {
                "NyaTerm_2.0.0_linux_x64.AppImage",
                "NyaTerm_2.0.0_linux_x64.deb",
                "NyaTerm_2.0.0_linux_x64.rpm",
            },
            "aarch64-pc-windows-msvc": {
                "NyaTerm_2.0.0_windows_arm64_portable.zip",
                "NyaTerm_2.0.0_windows_arm64-setup.exe",
            },
            "x86_64-pc-windows-msvc": {
                "NyaTerm_2.0.0_windows_x64_portable.zip",
                "NyaTerm_2.0.0_windows_x64-setup.exe",
            },
        }
        for target, names in expected.items():
            with self.subTest(target=target):
                self.assertEqual(package_native.artifact_names(target, "v2.0.0"), names)

    def test_release_binary_always_uses_explicit_target_directory(self) -> None:
        with mock.patch.dict("os.environ", {}, clear=True):
            linux = package_native.release_binary_path("x86_64-unknown-linux-gnu")
            windows = package_native.release_binary_path("aarch64-pc-windows-msvc")
        self.assertEqual(
            linux.relative_to(package_native.ROOT_DIR).as_posix(),
            "target/x86_64-unknown-linux-gnu/release/nyaterm",
        )
        self.assertEqual(
            windows.relative_to(package_native.ROOT_DIR).as_posix(),
            "target/aarch64-pc-windows-msvc/release/nyaterm.exe",
        )

    def test_helper_binaries_resolve_beside_the_application(self) -> None:
        self.assertIn("nyaterm-rdp-helper", package_native.HELPER_BINS)
        self.assertIn("nyaterm-mcp", package_native.HELPER_BINS)
        with mock.patch.dict("os.environ", {}, clear=True):
            linux = package_native.helper_binary_paths("x86_64-unknown-linux-gnu")
            windows = package_native.helper_binary_paths("aarch64-pc-windows-msvc")
        self.assertEqual(
            [path.name for path in linux], list(package_native.HELPER_BINS)
        )
        self.assertEqual(
            [path.name for path in windows],
            [f"{name}.exe" for name in package_native.HELPER_BINS],
        )
        application = package_native.release_binary_path("x86_64-unknown-linux-gnu")
        for path in linux:
            self.assertEqual(path.parent, application.parent)

    def test_copy_helpers_stages_every_helper_beside_the_application(self) -> None:
        target = "x86_64-unknown-linux-gnu"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sources = root / "build"
            destination = root / "package"
            destination.mkdir()
            staged = []
            for path in package_native.helper_binary_paths(target):
                fake = sources / path.name
                fake.parent.mkdir(parents=True, exist_ok=True)
                fake.write_bytes(b"helper")
                staged.append(fake)
            with mock.patch.object(
                package_native, "helper_binary_paths", return_value=staged
            ):
                copied = package_native.copy_helpers(destination, target)
            self.assertEqual(
                sorted(path.name for path in copied),
                sorted(package_native.HELPER_BINS),
            )
            for path in copied:
                self.assertTrue(path.is_file())
                self.assertEqual(path.parent, destination)

    def test_windows_installer_script_installs_and_removes_every_helper(self) -> None:
        target = "x86_64-pc-windows-msvc"
        info = package_native.target_info(target)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with (
                mock.patch.object(package_native, "WORK_DIR", root / "work"),
                mock.patch.object(package_native, "DIST_DIR", root / "dist"),
                mock.patch.object(package_native, "run"),
                mock.patch.object(package_native, "helper_binary_paths", return_value=[root / "build" / f"{name}.exe" for name in package_native.HELPER_BINS]),
                mock.patch.object(
                    package_native, "find_makensis", return_value="makensis"
                ),
            ):
                package_native.WORK_DIR.mkdir(parents=True)
                package_native.DIST_DIR.mkdir(parents=True)
                application = root / "nyaterm.exe"
                application.write_bytes(b"MZ")
                for path in package_native.helper_binary_paths(target):
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(b"MZ")
                package_native.create_windows_packages(
                    application, info, "2.0.0", "2.0.0"
                )
                script = (
                    package_native.WORK_DIR / "nyaterm-installer.nsi"
                ).read_text(encoding="utf-8")
        for name in package_native.HELPER_BINS:
            filename = f"{name}.exe"
            with self.subTest(helper=filename):
                self.assertRegex(script, rf'File ".*{filename}"')
                self.assertIn(f'Delete "$INSTDIR\\{filename}"', script)
        self.assertIn(
            r'WriteRegStr HKCU "Software\Classes\nyaterm" "URL Protocol" ""',
            script,
        )
        self.assertIn(
            r'WriteRegStr HKCU "Software\Classes\nyaterm\shell\open\command" "" "$\"$INSTDIR\NyaTerm.exe$\" $\"%1$\""',
            script,
        )
        self.assertIn(
            r'DeleteRegKey HKCU "Software\Classes\nyaterm"',
            script,
        )
        self.assertNotIn(r"Software\Classes\ssh", script)
        self.assertNotIn(r"Software\Classes\telnet", script)

    def test_linux_desktop_registers_only_nyaterm_url_scheme(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "nyaterm.desktop"
            package_native.write_desktop_file(path, "/opt/nyaterm/nyaterm")
            desktop = path.read_text(encoding="utf-8")
        self.assertIn("Exec=/opt/nyaterm/nyaterm %U\n", desktop)
        self.assertIn("MimeType=x-scheme-handler/nyaterm;\n", desktop)
        self.assertNotIn("x-scheme-handler/ssh", desktop)
        self.assertNotIn("x-scheme-handler/telnet", desktop)

    def test_deb_dependencies_cover_helper_binaries(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binaries = [root / "nyaterm", root / "nyaterm-rdp-helper"]
            with (
                mock.patch.object(package_native, "WORK_DIR", root / "work"),
                mock.patch.object(
                    package_native, "require_tool", return_value="dpkg-shlibdeps"
                ),
                mock.patch.object(
                    package_native.subprocess,
                    "check_output",
                    return_value="shlibs:Depends=libc6, libx11-6\n",
                ) as check_output,
            ):
                dependencies = package_native.linux_deb_dependencies(binaries)
        self.assertEqual(dependencies, "libc6, libx11-6")
        command = check_output.call_args.args[0]
        for binary in binaries:
            self.assertIn(str(binary), command)
        self.assertEqual(command.count("-e"), len(binaries))

    def test_release_binary_respects_absolute_cargo_target_dir(self) -> None:
        # Build the absolute path for the running platform: Path("/cache/cargo") has
        # no drive letter, so is_absolute() is False on Windows and the assertion
        # would compare against a path joined onto the repository root instead.
        target_dir = Path(tempfile.gettempdir(), "nyaterm-cargo-target").resolve()
        with mock.patch.dict("os.environ", {"CARGO_TARGET_DIR": str(target_dir)}):
            path = package_native.release_binary_path("x86_64-unknown-linux-gnu")
        self.assertEqual(
            path, target_dir / "x86_64-unknown-linux-gnu" / "release" / "nyaterm"
        )

    def test_platform_package_versions_are_normalized(self) -> None:
        self.assertEqual(package_native.windows_numeric_version("2.4.6-beta.1"), "2.4.6.0")
        self.assertEqual(package_native.linux_rpm_version("2.4.6"), ("2.4.6", "1"))
        self.assertEqual(
            package_native.linux_rpm_version("2.4.6-beta.1"),
            ("2.4.6", "0.beta.1"),
        )

    def test_dpkg_dependency_output_is_parsed(self) -> None:
        output = "ignored=value\nshlibs:Depends=libc6 (>= 2.34), libx11-6\n"
        self.assertEqual(
            package_native.parse_dpkg_dependencies(output),
            "libc6 (>= 2.34), libx11-6",
        )
        with self.assertRaises(RuntimeError):
            package_native.parse_dpkg_dependencies("shlibs:Depends=\n")

    def test_native_icon_resources_have_expected_formats_and_sizes(self) -> None:
        expected_png_sizes = {
            "32x32.png": (32, 32),
            "64x64.png": (64, 64),
            "128x128.png": (128, 128),
            "256x256.png": (256, 256),
            "512x512.png": (512, 512),
        }
        for name, expected_size in expected_png_sizes.items():
            with self.subTest(name=name):
                data = (package_native.ICON_DIR / name).read_bytes()
                self.assertEqual(data[:8], b"\x89PNG\r\n\x1a\n")
                self.assertEqual(struct.unpack(">II", data[16:24]), expected_size)

        self.assertEqual(
            (package_native.ICON_DIR / "icon.icns").read_bytes()[:4], b"icns"
        )
        self.assertEqual(
            (package_native.ICON_DIR / "icon.ico").read_bytes()[:4], b"\0\0\1\0"
        )


if __name__ == "__main__":
    unittest.main()
