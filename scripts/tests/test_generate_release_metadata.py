from __future__ import annotations

import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path


RELEASE_SCRIPTS = Path(__file__).resolve().parents[1] / "release"
sys.path.insert(0, str(RELEASE_SCRIPTS))

import generate_release_metadata  # noqa: E402
import verify_release_metadata  # noqa: E402


class GenerateReleaseMetadataTests(unittest.TestCase):
    def make_release(self, directory: Path, version: str = "2.0.0") -> None:
        for name in generate_release_metadata.expected_artifacts(version):
            (directory / name).write_bytes(f"payload:{name}".encode())
        updater_names = {
            template.format(version=version)
            for template in generate_release_metadata.updater_artifacts(version).values()
        }
        for name in updater_names:
            (directory / f"{name}.sig").write_text(
                f"signature-for-{name}\n", encoding="utf-8"
            )

    def test_generates_download_and_legacy_updater_manifests(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.make_release(directory)
            downloads, updater = generate_release_metadata.generate(
                directory,
                version="2.0.0",
                tag="v2.0.0",
                base_url="https://downloads.nyaterm.app/",
                notes="release notes",
                pub_date="2026-08-31T00:00:00Z",
            )

            self.assertEqual(len(downloads["platforms"]), 10)
            self.assertEqual(len(updater["platforms"]), 16)
            for architecture in ("x86_64", "aarch64"):
                key = f"windows-{architecture}-msi"
                self.assertTrue(downloads["platforms"][key]["url"].endswith(".msi"))
                self.assertEqual(downloads["platforms"][key]["url"], updater["platforms"][key]["url"])
                self.assertTrue(updater["platforms"][key]["signature"].startswith("signature-for-"))
            self.assertIn("windows-x86_64-portable", updater["platforms"])
            self.assertEqual(
                downloads["platforms"]["darwin-aarch64"]["url"],
                "https://downloads.nyaterm.app/releases/v2.0.0/"
                "NyaTerm_2.0.0_macos_arm64.dmg",
            )
            self.assertEqual(
                updater["platforms"]["windows-x86_64"],
                updater["platforms"]["windows-x86_64-nsis"],
            )
            self.assertEqual(
                json.loads((directory / "downloads.json").read_text()), downloads
            )

            checksum_lines = (directory / "SHA256SUMS").read_text().splitlines()
            self.assertEqual(checksum_lines, sorted(checksum_lines, key=lambda line: line[66:]))
            portable = directory / "NyaTerm_2.0.0_windows_x64_portable.zip"
            self.assertIn(
                f"{hashlib.sha256(portable.read_bytes()).hexdigest()}  {portable.name}",
                checksum_lines,
            )

    def test_system_packages_are_signed_and_compatibility_has_its_own_entry(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            version = "2.0.9-preview.1"
            for name in generate_release_metadata.expected_artifacts(version, include_focal=True):
                (directory / name).write_bytes(b"package")
            for template in generate_release_metadata.updater_artifacts(version, include_focal=True).values():
                (directory / (template.format(version=version) + ".sig")).write_text("signature")
            _, manifest = generate_release_metadata.generate(
                directory, version=version, tag=f"v{version}", base_url="https://downloads.nyaterm.app",
                notes="", pub_date="2026-11-28T00:00:00Z", include_focal=True,
            )
            for arch in ("x86_64", "aarch64"):
                for kind in ("deb", "rpm"):
                    item = manifest["platforms"][f"linux-{arch}-{kind}"]
                    self.assertTrue(item["url"].endswith(f".{kind}"))
                    self.assertEqual(item["signature"], "signature")
                self.assertTrue(manifest["platforms"][f"linux-{arch}"]["url"].endswith(".AppImage"))
            self.assertTrue(manifest["platforms"]["linux-x86_64-deb-glibc231"]["url"].endswith("_glibc231.deb"))
            verify_release_metadata.verify(directory, version=version, tag=f"v{version}", base_url="https://downloads.nyaterm.app", check_assets=False)
            (directory / f"NyaTerm_{version}_linux_x64.deb.sig").unlink()
            with self.assertRaisesRegex(RuntimeError, "missing updater signature"):
                generate_release_metadata.generate(directory, version=version, tag=f"v{version}", base_url="https://downloads.nyaterm.app", notes="", pub_date="", include_focal=True)

    def test_stable_and_prerelease_publish_to_separate_channel_manifests(self) -> None:
        self.assertEqual(
            generate_release_metadata.channel_manifest_destinations("2.0.0"),
            {
                "latest.json": "latest.json",
                "downloads.json": "downloads.json",
            },
        )
        preview = generate_release_metadata.channel_manifest_destinations(
            "2.0.0-preview.2"
        )
        self.assertEqual(
            preview,
            {
                "latest.json": "channels/preview/latest.json",
                "downloads.json": "channels/preview/downloads.json",
            },
        )
        self.assertNotIn("latest.json", preview.values())
        self.assertNotIn("downloads.json", preview.values())

    def test_rejects_mismatched_tag_and_missing_signature(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.make_release(directory)
            with self.assertRaises(ValueError):
                generate_release_metadata.generate(
                    directory,
                    version="2.0.0",
                    tag="v2.0.1",
                    base_url="https://downloads.nyaterm.app",
                    notes="",
                    pub_date="2026-08-31T00:00:00Z",
                )
            next(directory.glob("*.sig")).unlink()
            with self.assertRaisesRegex(RuntimeError, "missing updater signature"):
                generate_release_metadata.generate(
                    directory,
                    version="2.0.0",
                    tag="v2.0.0",
                    base_url="https://downloads.nyaterm.app",
                    notes="",
                    pub_date="2026-08-31T00:00:00Z",
                )

    def test_rejects_empty_signature_and_keeps_prerelease_urls_versioned(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            version = "2.1.0-beta.1"
            self.make_release(directory, version)
            signature = next(directory.glob("*.sig"))
            signature.write_text("\n", encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "empty updater signature"):
                generate_release_metadata.generate(
                    directory,
                    version=version,
                    tag=f"v{version}",
                    base_url="https://downloads.nyaterm.app",
                    notes="",
                    pub_date="2026-08-31T00:00:00Z",
                )

            signature.write_text("verified-signature\n", encoding="utf-8")
            downloads, updater = generate_release_metadata.generate(
                directory,
                version=version,
                tag=f"v{version}",
                base_url="https://downloads.nyaterm.app",
                notes="prerelease",
                pub_date="2026-08-31T00:00:00Z",
            )
            for manifest in (downloads, updater):
                self.assertFalse(any(key.endswith("-msi") for key in manifest["platforms"]))
                for platform in manifest["platforms"].values():
                    self.assertIn(f"/releases/v{version}/", platform["url"])
            self.assertEqual(len(downloads["platforms"]), 8)
            self.assertEqual(len(updater["platforms"]), 14)

    def test_stable_requires_both_msi_files_and_their_signatures(self) -> None:
        for suffix, message in ((".msi", "missing release artifacts"),
                                (".msi.sig", "missing updater signature")):
            with tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                self.make_release(directory)
                (directory / f"NyaTerm_2.0.0_windows_arm64{suffix}").unlink()
                with self.subTest(suffix=suffix), self.assertRaisesRegex(RuntimeError, message):
                    generate_release_metadata.generate(
                        directory, version="2.0.0", tag="v2.0.0",
                        base_url="https://downloads.nyaterm.app", notes="", pub_date="2026-10-26T00:00:00Z",
                    )

    def test_build_metadata_hyphens_do_not_select_preview(self) -> None:
        version = "2.0.0+build-with-hyphen"
        self.assertEqual(len(generate_release_metadata.download_artifacts(version)), 10)
        self.assertEqual(len(generate_release_metadata.updater_artifacts(version)), 16)
        self.assertEqual(generate_release_metadata.channel_manifest_destinations(version)["latest.json"], "latest.json")

    def test_rejects_noncanonical_release_base_urls(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.make_release(directory)
            for base_url in (
                "http://downloads.nyaterm.app",
                "https://download.nyaterm.app",
                "https://downloads.nyaterm.app/releases",
                "https://downloads.nyaterm.app?source=release",
            ):
                with self.subTest(base_url=base_url), self.assertRaisesRegex(
                    ValueError, "canonical origin"
                ):
                    generate_release_metadata.generate(
                        directory,
                        version="2.0.0",
                        tag="v2.0.0",
                        base_url=base_url,
                        notes="",
                        pub_date="2026-09-22T00:00:00Z",
                    )

    def test_verifier_requires_exact_urls_complete_platforms_and_integrity(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.make_release(directory)
            generate_release_metadata.generate(
                directory,
                version="2.0.0",
                tag="v2.0.0",
                base_url="https://downloads.nyaterm.app",
                notes="",
                pub_date="2026-09-22T00:00:00Z",
            )
            urls = verify_release_metadata.verify(
                directory,
                version="2.0.0",
                tag="v2.0.0",
                base_url="https://downloads.nyaterm.app",
                check_assets=False,
            )
            expected_urls = {
                template.format(version="2.0.0")
                for template in (
                    list(generate_release_metadata.download_artifacts("2.0.0").values())
                    + list(generate_release_metadata.updater_artifacts("2.0.0").values())
                )
            }
            self.assertEqual(len(urls), len(expected_urls))

            latest_path = directory / "latest.json"
            latest = json.loads(latest_path.read_text(encoding="utf-8"))
            latest["platforms"]["windows-x86_64"]["url"] = (
                "https://downloads.nyaterm.app/releases/releases/v2.0.0/"
                "NyaTerm_2.0.0_windows_x64-setup.exe"
            )
            latest_path.write_text(json.dumps(latest), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "noncanonical"):
                verify_release_metadata.verify(
                    directory,
                    version="2.0.0",
                    tag="v2.0.0",
                    base_url="https://downloads.nyaterm.app",
                    check_assets=False,
                )


if __name__ == "__main__":
    unittest.main()
