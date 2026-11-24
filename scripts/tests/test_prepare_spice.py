from __future__ import annotations

import hashlib
import io
import json
from pathlib import Path
import sys
import tempfile
import tarfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "release"))
import prepare_spice
import spice_unix
import spice_source_runtime
import native_headers


class SpiceRuntimeTests(unittest.TestCase):
    def test_macos_vpx_configure_uses_environment_for_linker_deployment_floor(self):
        for arch, target in (("arm64", "arm64-darwin20-gcc"),
                             ("x86_64", "x86_64-darwin20-gcc")):
            with self.subTest(arch=arch), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                with mock.patch.object(spice_source_runtime.platform, "machine", return_value=arch), \
                     mock.patch.object(spice_source_runtime, "source", side_effect=lambda name, _: root / name), \
                     mock.patch.object(spice_source_runtime, "output", return_value="/tool"), \
                     mock.patch.object(spice_source_runtime, "run") as run:
                    spice_source_runtime.build(root, root / "prefix", macos=True)
                configure = next(call for call in run.call_args_list
                                 if call.args[0][0] == root / "libvpx/configure")
                self.assertIn("--target=" + target, configure.args[0])
                self.assertFalse(any(str(arg).startswith("--extra-ldflags") for arg in configure.args[0]))
                self.assertIn("-mmacosx-version-min=12.0", configure.kwargs["env"]["LDFLAGS"])
                self.assertEqual(configure.kwargs["env"]["MACOSX_DEPLOYMENT_TARGET"], "12.0")

    def test_source_extraction_rejects_links_and_traversal_before_writing(self):
        for name, kind in (("../outside", tarfile.REGTYPE), ("source/link", tarfile.SYMTYPE),
                           ("source/hardlink", tarfile.LNKTYPE), ("source/device", tarfile.CHRTYPE)):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                archive = io.BytesIO()
                with tarfile.open(fileobj=archive, mode="w") as handle:
                    good = tarfile.TarInfo("source/README")
                    good.size = 2
                    handle.addfile(good, io.BytesIO(b"ok"))
                    bad = tarfile.TarInfo(name)
                    bad.type = kind
                    bad.linkname = "../../outside"
                    handle.addfile(bad)
                archive.seek(0)
                with tarfile.open(fileobj=archive) as handle, self.assertRaises(ValueError):
                    spice_unix.extract_source(handle, Path(directory))
                self.assertEqual(list(Path(directory).iterdir()), [])

    def test_source_extraction_preserves_contents_and_executable_permission(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = io.BytesIO()
            with tarfile.open(fileobj=archive, mode="w") as handle:
                member = tarfile.TarInfo("source/configure")
                member.size, member.mode, member.mtime = 2, 0o755, 1_000_000_000
                handle.addfile(member, io.BytesIO(b"ok"))
            archive.seek(0)
            with tarfile.open(fileobj=archive) as handle:
                spice_unix.extract_source(handle, Path(directory))
            self.assertEqual((Path(directory) / "source/configure").read_bytes(), b"ok")
            self.assertEqual(int((Path(directory) / "source/configure").stat().st_mtime), 1_000_000_000)

    def test_internal_source_aliases_reject_escape_and_cycles_before_writing(self):
        for target in ("../../outside", "alias"):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as directory:
                archive = io.BytesIO()
                with tarfile.open(fileobj=archive, mode="w") as handle:
                    member = tarfile.TarInfo("source/alias")
                    member.type, member.linkname = tarfile.SYMTYPE, target
                    handle.addfile(member)
                archive.seek(0)
                with tarfile.open(fileobj=archive) as handle, self.assertRaises(ValueError):
                    spice_unix.extract_source(handle, Path(directory), internal_links=True)
                self.assertEqual(list(Path(directory).iterdir()), [])

    def test_glib_license_aliases_become_regular_files_without_following_host_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = io.BytesIO()
            with tarfile.open(fileobj=archive, mode="w") as handle:
                license_file = tarfile.TarInfo("glib/LICENSES/LGPL-2.1-or-later.txt")
                license_file.size = 7
                handle.addfile(license_file, io.BytesIO(b"license"))
                for name, target in (("COPYING", "LICENSES/LGPL-2.1-or-later.txt"),
                                     ("gmodule/COPYING", "../LICENSES/LGPL-2.1-or-later.txt")):
                    member = tarfile.TarInfo("glib/" + name)
                    member.type, member.linkname = tarfile.SYMTYPE, target
                    handle.addfile(member)
            archive.seek(0)
            with tarfile.open(fileobj=archive) as handle:
                spice_unix.extract_source(handle, Path(directory), license_links=True)
            for name in ("COPYING", "gmodule/COPYING"):
                path = Path(directory) / "glib" / name
                self.assertFalse(path.is_symlink())
                self.assertEqual(path.read_bytes(), b"license")

    def test_runtime_paths_reject_traversal_windows_streams_and_ambiguous_components(self):
        for value in ("", "/root", "../escape", "a/../b", "a\\b", "C:/x", "a:stream", "a//b", "a/./b"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                prepare_spice.safe_relative(value)
        self.assertEqual(str(prepare_spice.safe_relative("gstreamer-1.0/plugin.dll")), "gstreamer-1.0/plugin.dll")

    def test_both_architectures_lock_video_sasl_ca_and_every_package_digest(self):
        lock = json.loads(prepare_spice.LOCK_PATH.read_text(encoding="utf-8"))
        self.assertEqual(set(lock["targets"]), set(prepare_spice.WINDOWS_TARGETS))
        for target, entry in lock["targets"].items():
            self.assertEqual(entry["machine"], prepare_spice.WINDOWS_TARGETS[target][1])
            for plugin in prepare_spice.VIDEO_PLUGINS:
                self.assertIn(f"gstreamer-1.0/libgst{plugin}.dll", entry["files"])
            for plugin in prepare_spice.SASL_PLUGINS:
                self.assertIn(f"sasl2/lib{plugin}-3.dll", entry["files"])
            self.assertIn("ca-bundle.pem", entry["files"])
            for name, item in entry["files"].items():
                prepare_spice.safe_relative(name)
                self.assertIn(item["package"], entry["packages"])
                self.assertRegex(item["sha256"], r"^[0-9a-f]{64}$")
            for package in entry["packages"].values():
                self.assertRegex(package["sha256"], r"^[0-9a-f]{64}$")
                self.assertTrue(package["url"].startswith(entry["repository"]))

    def test_runtime_validation_rejects_self_consistent_tampering_and_extra_modules(self):
        target = "x86_64-pc-windows-msvc"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runtime = root / "runtime"
            runtime.mkdir()
            payload = b"fixture library"
            digest = hashlib.sha256(payload).hexdigest()
            lock = root / "lock.json"
            lock.write_text(json.dumps({"targets": {target: {"packages": {}, "files": {
                "library.dll": {"sha256": digest}
            }}}}), encoding="utf-8")
            sources = root / "sources.json"
            sources.write_text("{}", encoding="utf-8")
            manifest = {"schema": 1, "target": target, "packages": {},
                        "lock_sha256": prepare_spice.sha256(lock.read_bytes()),
                        "sources_sha256": prepare_spice.sha256(sources.read_bytes()),
                        "files": {"library.dll": digest}}
            (runtime / "library.dll").write_bytes(payload)
            with mock.patch.object(prepare_spice, "LOCK_PATH", lock), mock.patch.object(prepare_spice, "SOURCES_PATH", sources):
                (runtime / "SOURCE.json").write_text(json.dumps(manifest), encoding="utf-8")
                prepare_spice.verify_runtime(runtime, target)
                (runtime / "unreviewed-plugin.dll").write_bytes(b"extra")
                with self.assertRaisesRegex(ValueError, "Unexpected file"):
                    prepare_spice.verify_runtime(runtime, target)
                (runtime / "unreviewed-plugin.dll").unlink()
                (runtime / "library.dll").write_bytes(b"changed")
                manifest["files"]["library.dll"] = prepare_spice.sha256(b"changed")
                (runtime / "SOURCE.json").write_text(json.dumps(manifest), encoding="utf-8")
                with self.assertRaisesRegex(ValueError, "locked package member"):
                    prepare_spice.verify_runtime(runtime, target)

    def test_cached_archive_corruption_is_rejected_without_using_the_network(self):
        with tempfile.TemporaryDirectory() as directory:
            staging = Path(directory)
            (staging / "downloads").mkdir()
            (staging / "downloads/package.tar.zst").write_bytes(b"corrupted")
            with mock.patch.object(prepare_spice, "STAGING", staging), mock.patch.object(prepare_spice.urllib.request, "urlopen") as fetch:
                with self.assertRaisesRegex(ValueError, "cached archive checksum mismatch"):
                    prepare_spice.download("https://example.invalid/package.tar.zst", "package.tar.zst", "0" * 64)
                fetch.assert_not_called()

    def test_unknown_dlls_are_never_classified_from_the_build_host(self):
        self.assertTrue(prepare_spice.system_dll("kernel32.dll"))
        self.assertTrue(prepare_spice.system_dll("api-ms-win-crt-runtime-l1-1-0.dll"))
        self.assertFalse(prepare_spice.system_dll("libcrypto-3-x64.dll"))
        self.assertFalse(prepare_spice.system_dll("unreviewed.dll"))

    def test_unix_runtime_does_not_silently_bundle_the_wrong_architecture(self):
        import struct
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fixture"
            header = bytearray(64)
            header[:6] = b"\x7fELF\x02\x01"
            struct.pack_into("<H", header, 18, 62)
            path.write_bytes(header)
            spice_unix.check_architecture(path, "x86_64-unknown-linux-gnu")
            with self.assertRaisesRegex(ValueError, "ELF architecture"):
                spice_unix.check_architecture(path, "aarch64-unknown-linux-gnu")
            header = bytearray(64)
            header[:4] = b"\xcf\xfa\xed\xfe"
            struct.pack_into("<I", header, 4, 0x0100000C)
            struct.pack_into("<II", header, 16, 1, 24)
            struct.pack_into("<IIIIII", header, 32, 0x32, 24, 1, 12 << 16, 15 << 16, 0)
            path.write_bytes(header)
            spice_unix.check_architecture(path, "aarch64-apple-darwin")
            with self.assertRaisesRegex(ValueError, "Mach-O architecture"):
                spice_unix.check_architecture(path, "x86_64-apple-darwin")

    def test_macos_gate_reads_deployment_floor_and_rejects_missing_or_malformed_commands(self):
        import struct
        data = bytearray(64)
        data[:4] = b"\xcf\xfa\xed\xfe"
        struct.pack_into("<I", data, 4, 0x0100000C)
        struct.pack_into("<II", data, 16, 1, 24)
        struct.pack_into("<IIIIII", data, 32, 0x32, 24, 1, 12 << 16, 15 << 16, 0)
        native_headers.verify_macos_minimum_os(data)
        struct.pack_into("<I", data, 44, 13 << 16)
        with self.assertRaisesRegex(RuntimeError, "requires macOS 13"):
            native_headers.verify_macos_minimum_os(data)
        for platform, length in ((2, 24), (1, 4), (1, 100)):
            struct.pack_into("<II", data, 36, length, platform)
            with self.assertRaises(RuntimeError):
                native_headers.verify_macos_minimum_os(data)
        struct.pack_into("<II", data, 16, 0, 0)
        with self.assertRaisesRegex(RuntimeError, "Missing"):
            native_headers.verify_macos_minimum_os(data)

    def test_prepared_macos_plugins_enforce_deployment_floor_with_self_consistent_hashes(self):
        import struct
        target = "aarch64-apple-darwin"
        for name in ("library.dylib", "sasl2/libplain.so", "sasl2/libplain.so.3"):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                data = bytearray(64)
                data[:4] = b"\xcf\xfa\xed\xfe"
                struct.pack_into("<I", data, 4, 0x0100000C)
                struct.pack_into("<II", data, 16, 1, 24)
                struct.pack_into("<IIIIII", data, 32, 0x32, 24, 1, 13 << 16, 15 << 16, 0)
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
                manifest = {"schema": 1, "target": target,
                            "sources_sha256": prepare_spice.sha256(prepare_spice.SOURCES_PATH.read_bytes()),
                            "files": {name: prepare_spice.sha256(data)}}
                (root / "SOURCE.json").write_text(json.dumps(manifest), encoding="utf-8")
                with self.assertRaisesRegex(RuntimeError, "requires macOS 13"):
                    prepare_spice.verify_runtime(root, target)

    def test_unix_source_and_submodules_have_immutable_revisions(self):
        metadata = json.loads(prepare_spice.SOURCES_PATH.read_text(encoding="utf-8"))["unix_git"]
        for item in [metadata, *metadata["submodules"].values()]:
            self.assertTrue(item["repository"].startswith("https://"))
            self.assertRegex(item["revision"], r"^[0-9a-f]{40}$")
        self.assertEqual(set(metadata["submodules"]), {"subprojects/spice-common", "subprojects/keycodemapdb"})


if __name__ == "__main__":
    unittest.main()
