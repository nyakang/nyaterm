"""Exercise publication failures with in-memory storage, never live releases."""

from __future__ import annotations

import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch, Mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "release"))
import artifact_catalog
import generate_release_metadata
import release_channel
import release_io
import release_provenance
import release_receipt
import snapshot_release
import verify_candidate


class MemoryStore:
    def __init__(self):
        self.objects = {}
        self.writes = []
        self.fail_on = None

    def fetch(self, key, path):
        if key not in self.objects:
            return False
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(self.objects[key])
        return True

    def put(self, key, path, *, immutable=False):
        if key == self.fail_on:
            raise RuntimeError("injected upload failure")
        value = path.read_bytes()
        if immutable and key in self.objects and self.objects[key] != value:
            raise ValueError("immutable object differs")
        self.objects[key] = value
        self.writes.append(key)

    def delete(self, key):
        self.objects.pop(key, None)


class DeliveryTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.dist = self.root / "dist"
        self.dist.mkdir()
        self.recovery = self.root / "recovery"
        self.version = "2.1.0"
        self.sha = "a" * 40
        self.store = MemoryStore()
        for name in artifact_catalog.expected(self.version, include_focal=True):
            (self.dist / name).write_bytes(name.encode())
        for template in generate_release_metadata.updater_artifacts(self.version).values():
            (self.dist / (template.format(version=self.version) + ".sig")).write_text("signature", encoding="utf-8")
        generate_release_metadata.generate(self.dist, version=self.version, tag="v" + self.version,
                                           base_url="https://downloads.nyaterm.app", notes="notes",
                                           pub_date="2026-11-07T00:00:00Z", include_focal=True)
        for path in self.dist.glob("*.sig"):
            path.unlink()
        self.info = {"tagName": "v" + self.version, "isDraft": True, "isPrerelease": False, "body": "notes"}

    def commit(self, **overrides):
        def publish(*args):
            self.info["isDraft"] = False
        values = dict(publish=publish, view=lambda tag: self.info, public=lambda *args: None)
        values.update(overrides)
        return release_channel.commit(self.store, self.dist, self.recovery, self.version, **values)

    def test_semver_channel_order_handles_numeric_prereleases_and_build_metadata(self):
        ordered = ["2.0.0-alpha", "2.0.0-alpha.1", "2.0.0-alpha.beta", "2.0.0-beta.2", "2.0.0-beta.11", "2.0.0-rc.1", "2.0.0", "2.0.1"]
        for left, right in zip(ordered, ordered[1:]):
            self.assertLess(release_channel.compare_versions(left, right), 0)
            self.assertGreater(release_channel.compare_versions(right, left), 0)
        self.assertEqual(release_channel.compare_versions("2.0.0+build.1", "2.0.0+build.2"), 0)

    def test_complete_focal_inventory_enters_checksums_without_changing_manifest_keys(self):
        release_channel.validate_candidate(self.dist, self.version)
        hashes = release_channel.checksums(self.dist)
        self.assertTrue(artifact_catalog.names(artifact_catalog.FOCAL_TARGET, "glibc231", self.version) <= hashes.keys())
        manifest = json.loads((self.dist / "downloads.json").read_text())
        self.assertEqual(set(manifest["platforms"]), set(generate_release_metadata.download_artifacts(self.version)))
        next(self.dist.glob("*glibc231.deb")).write_bytes(b"damaged")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            release_channel.validate_candidate(self.dist, self.version)

    def test_stage_is_idempotent_and_rejects_replacing_immutable_bytes(self):
        release_channel.stage(self.store, self.dist, self.version)
        release_channel.stage(self.store, self.dist, self.version)
        key = "releases/v2.1.0/SHA256SUMS"
        self.store.objects[key] = b"different"
        with self.assertRaises(ValueError):
            release_channel.stage(self.store, self.dist, self.version)

    def test_github_staging_reads_back_new_assets_and_rejects_different_existing_bytes(self):
        assets = {}
        def gh(*args):
            if args[1] == "upload":
                path = Path(args[3])
                assets[path.name] = path.read_bytes()
            elif args[1] == "download":
                name = args[args.index("--pattern") + 1]
                out = Path(args[args.index("--dir") + 1])
                (out / name).write_bytes(assets[name])
            else:
                raise AssertionError(args)
        def view(tag):
            return {**self.info, "assets": [{"name": name} for name in assets]}
        with patch.object(release_channel, "gh", side_effect=gh), patch.object(release_channel, "release", side_effect=view):
            release_channel.stage_github(self.dist, self.version)
            self.assertEqual(assets, {p.name: p.read_bytes() for p in self.dist.iterdir()})
            self.info["isDraft"] = False
            release_channel.stage_github(self.dist, self.version)
            assets["SHA256SUMS"] = b"different"
            with self.assertRaisesRegex(ValueError, "readback differs"):
                release_channel.stage_github(self.dist, self.version)

    @unittest.skipUnless(shutil.which("minisign"), "minisign is required for candidate signature integration tests")
    def test_reused_candidate_checks_signatures_for_every_manifest_alias(self):
        public, private = self.root / "test.pub", self.root / "test.key"
        subprocess.run(["minisign", "-G", "-W", "-p", str(public), "-s", str(private)], check=True)
        manifest_path = self.dist / "latest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        signatures = {}
        aliases = {}
        for platform, template in generate_release_metadata.updater_artifacts(self.version).items():
            name = template.format(version=self.version)
            if name not in signatures:
                output = self.root / "signature"
                subprocess.run(["minisign", "-Sm", str(self.dist / name), "-s", str(private), "-x", str(output)], check=True)
                signatures[name] = base64.b64encode(output.read_bytes()).decode()
            aliases.setdefault(name, []).append(platform)
            manifest["platforms"][platform]["signature"] = signatures[name]
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        records = self.root / "records"
        records.mkdir()
        for target, variant in artifact_catalog.packages(self.version, include_focal=True):
            value = release_provenance.record_v2(self.dist, target, self.sha, self.version, "123",
                                                 variant=variant, compiler="rustc 1.99.1 (fixture)")
            (records / f"{target}-{variant}.json").write_text(json.dumps(value), encoding="utf-8")
        with patch.object(verify_candidate, "PUBLIC_KEY_PATH", public):
            verify_candidate.verify(self.dist, records, version=self.version, source_sha=self.sha, run_id="123")
            alias = next(values[-1] for values in aliases.values() if len(values) > 1)
            manifest["platforms"][alias]["signature"] = base64.b64encode(b"invalid signature").decode()
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaises(subprocess.CalledProcessError):
                verify_candidate.verify(self.dist, records, version=self.version, source_sha=self.sha, run_id="123")

    def test_partial_channel_write_can_resume_and_restore_recorded_absence(self):
        release_channel.backup(self.store, self.dist, self.recovery, self.version)
        self.store.fail_on = "latest.json"
        with self.assertRaises(RuntimeError):
            self.commit()
        self.assertTrue(self.info["isDraft"])
        self.store.fail_on = None
        self.commit()
        self.assertFalse(self.info["isDraft"])
        self.commit()  # Receipt upload/retry must not republish or change bytes.
        release_channel.restore(self.store, self.recovery, public=lambda *args: None)
        self.assertEqual(self.store.objects, {})

    def test_public_verification_failure_keeps_github_draft_and_backup(self):
        old = b'{"version":"2.0.0"}'
        self.store.objects = {"latest.json": old, "downloads.json": old}
        release_channel.backup(self.store, self.dist, self.recovery, self.version)
        with self.assertRaises(RuntimeError):
            self.commit(public=Mock(side_effect=RuntimeError("public CDN unavailable")))
        self.assertTrue(self.info["isDraft"])
        release_channel.restore(self.store, self.recovery, public=lambda *args: None)
        self.assertEqual(set(self.store.objects.values()), {old})

    def test_older_candidate_and_same_version_different_bytes_are_rejected(self):
        for current in ("2.2.0", self.version):
            with self.subTest(current=current):
                self.store.objects["downloads.json"] = json.dumps({"version": current}).encode()
                with self.assertRaises(ValueError):
                    release_channel.backup(self.store, self.dist, self.recovery, self.version)
                self.assertEqual(self.store.writes, [])

    def test_changed_channel_after_backup_and_newer_release_restore_are_rejected(self):
        release_channel.backup(self.store, self.dist, self.recovery, self.version)
        self.store.objects["latest.json"] = b'{"version":"2.2.0"}'
        with self.assertRaises(ValueError):
            self.commit()
        with self.assertRaises(ValueError):
            release_channel.restore(self.store, self.recovery, public=lambda *args: None)
        self.assertEqual(self.store.writes, [])

    def test_v2_rejects_changed_build_identity_and_requires_every_variant(self):
        records = self.root / "records"
        records.mkdir()
        for target, variant in artifact_catalog.packages(self.version, include_focal=True):
            value = release_provenance.record_v2(self.dist, target, self.sha, self.version, "123",
                                                 variant=variant, compiler="rustc 1.99.1 (fixture)")
            (records / f"{target}-{variant}.json").write_text(json.dumps(value), encoding="utf-8")
        release_provenance.verify_v2(self.dist, records, self.sha, self.version, "123")
        path = records / f"{artifact_catalog.FOCAL_TARGET}-glibc231.json"
        original = path.read_text()
        for field in ("variant", "artifact_version", "compiler", "lockfile_sha256", "build_config_sha256", "source_sha", "run_id"):
            value = json.loads(original)
            value[field] = "wrong"
            path.write_text(json.dumps(value))
            with self.subTest(field=field), self.assertRaises(ValueError):
                release_provenance.verify_v2(self.dist, records, self.sha, self.version, "123")
        path.unlink()
        with self.assertRaisesRegex(ValueError, "incomplete v2"):
            release_provenance.verify_v2(self.dist, records, self.sha, self.version, "123")

    def test_contract_hash_is_independent_of_checkout_line_endings(self):
        path = self.root / "contract"
        path.write_bytes(b"first\r\nsecond\r\n")
        crlf = artifact_catalog.text_sha256(path)
        path.write_bytes(b"first\nsecond\n")
        self.assertEqual(crlf, artifact_catalog.text_sha256(path))


class R2AdapterTests(unittest.TestCase):
    def test_read_errors_are_not_misclassified_as_absence(self):
        with patch.dict(os.environ, {"R2_ENDPOINT": "https://account.r2.cloudflarestorage.com"}):
            store = release_io.R2()
        with tempfile.TemporaryDirectory() as folder:
            for error, absent in (("An error occurred (NoSuchKey)", True), ("An error occurred (AccessDenied)", False)):
                with patch.object(store, "call", return_value=subprocess.CompletedProcess([], 1, "", error)):
                    if absent:
                        self.assertFalse(store.fetch("key", Path(folder) / "object"))
                    else:
                        with self.assertRaises(RuntimeError):
                            store.fetch("key", Path(folder) / "object")

    def test_immutable_create_uses_atomic_condition_and_accepts_identical_winner(self):
        with patch.dict(os.environ, {"R2_ENDPOINT": "https://account.r2.cloudflarestorage.com"}):
            store = release_io.R2()
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "asset"
            path.write_bytes(b"bytes")
            calls = []
            def fetch(key, out):
                calls.append(key)
                if len(calls) == 1:
                    return False
                out.write_bytes(path.read_bytes())
                return True
            with patch.object(store, "fetch", side_effect=fetch), patch.object(store, "call", return_value=subprocess.CompletedProcess([], 1)) as put:
                store.put("releases/v2/asset", path, immutable=True)
                self.assertIn("--if-none-match", put.call_args.args)
                self.assertIn("*", put.call_args.args)


class ReceiptTests(unittest.TestCase):
    def test_structured_and_legacy_receipts_support_independent_idempotent_distribution(self):
        for structured in (True, False):
            with self.subTest(structured=structured):
                receipt = {"schema_version": 1, "tag": "v2.1.0", "version": "2.1.0", "source_sha": "a" * 40,
                           "channel": "stable", "run_id": "123", "sha256sums_sha256": hashlib.sha256(b"checksums").hexdigest()}
                def api(path, *args):
                    if path == "actions/runs/123":
                        return '{"workflow_id":5,"status":"completed","conclusion":"success"}'
                    if path == "actions/workflows/release.yml":
                        return '{"id":5}'
                    return "published-release\n" if structured else "published-release-tag\n"
                def gh(*args):
                    out = Path(args[args.index("--dir") + 1])
                    if args[0] == "run":
                        if structured:
                            (out / "release-receipt.json").write_text(json.dumps(receipt))
                        else:
                            (out / "published-release-tag.txt").write_text("v2.1.0\n")
                    else:
                        (out / "SHA256SUMS").write_bytes(b"checksums")
                with patch.object(release_receipt, "api", side_effect=api), patch.object(release_receipt, "gh", side_effect=gh), \
                        patch.object(release_receipt, "tag_sha", return_value="a" * 40), \
                        patch.object(release_receipt, "release", return_value={"tagName": "v2.1.0", "isDraft": False, "isPrerelease": False}):
                    for channel in ("homebrew", "aur", "gitee"):
                        for _ in range(2):
                            self.assertEqual(release_receipt.resolve(channel, run_id="123")["tag"], "v2.1.0")
                    if structured:
                        receipt["source_sha"] = "b" * 40
                        with self.assertRaises(ValueError):
                            release_receipt.resolve("aur", run_id="123")

    def test_mismatched_receipt_identity_and_checksum_are_rejected(self):
        value = {"schema_version": 1, "tag": "v2.1.0", "version": "2.1.0", "source_sha": "a" * 40,
                 "channel": "stable", "run_id": "123", "sha256sums_sha256": "b" * 64}
        expected = dict(run_id="123", tag="v2.1.0", source_sha="a" * 40, checksum="b" * 64)
        release_receipt.validate_receipt(value, **expected)
        for key in value:
            wrong = {**value, key: "wrong"}
            with self.subTest(key=key), self.assertRaises(ValueError):
                release_receipt.validate_receipt(wrong, **expected)

    def test_rehearsal_has_no_downstream_release_and_wrong_workflow_is_rejected(self):
        run = {"workflow_id": 5, "status": "completed", "conclusion": "success"}
        with patch.object(release_receipt, "api", side_effect=(json.dumps(run), '{"id":5}', "release-candidate\n")):
            self.assertEqual(release_receipt.resolve("aur", run_id="123")["publish"], "false")
        run["workflow_id"] = 6
        with patch.object(release_receipt, "api", side_effect=(json.dumps(run), '{"id":5}')):
            with self.assertRaises(ValueError):
                release_receipt.resolve("aur", run_id="123")

    def test_manual_prerelease_only_reaches_gitee_and_draft_is_rejected(self):
        info = {"tagName": "v2.1.0-beta.1", "isDraft": False, "isPrerelease": True}
        with patch.object(release_receipt, "release", return_value=info), patch.object(release_receipt, "gh"):
            self.assertEqual(release_receipt.resolve("aur", version="2.1.0-beta.1")["publish"], "false")
            self.assertEqual(release_receipt.resolve("gitee", version="2.1.0-beta.1")["publish"], "true")
            info["isDraft"] = True
            with self.assertRaises(ValueError):
                release_receipt.resolve("gitee", version="2.1.0-beta.1")


class SnapshotTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.dist = self.root / "dist"
        self.dist.mkdir()
        self.recovery = self.root / "recovery"
        (self.dist / "first.zip").write_bytes(b"new-first")
        (self.dist / "second.zip").write_bytes(b"new-second")
        self.assets = {"first.zip": b"old-first", "second.zip": b"old-second", "obsolete.zip": b"old-extra"}
        self.old_assets = dict(self.assets)
        self.ref = {"object": {"sha": "a" * 40}}
        self.info = {"name": "Old snapshot", "body": "old notes", "isDraft": False, "isPrerelease": True}
        self.events = []
        self.fail_asset = None
        for name, value in (("api", self.api), ("gh", self.gh), ("release", self.view)):
            mock = patch.object(snapshot_release, name, side_effect=value)
            mock.start()
            self.addCleanup(mock.stop)

    def api(self, path, *args, **kwargs):
        if path == "releases/tags/main-snapshot":
            return self.info
        if path == "git/ref/tags/main-snapshot":
            return self.ref
        method = args[args.index("--method") + 1]
        if method in ("PATCH", "POST"):
            sha = next(arg[4:] for arg in args if arg.startswith("sha="))
            self.ref = {"object": {"sha": sha}}
            self.events.append("tag")
            return self.ref
        if method == "DELETE":
            self.ref = None
            return None
        raise AssertionError((path, args))

    def view(self, tag):
        return {**self.info, "assets": [{"name": name} for name in self.assets]}

    def gh(self, *args):
        operation = args[1]
        if operation == "download":
            out = Path(args[args.index("--dir") + 1])
            names = [args[args.index("--pattern") + 1]] if "--pattern" in args else list(self.assets)
            for name in names:
                (out / name).write_bytes(self.assets[name])
        elif operation == "upload":
            path = Path(args[3])
            if path.name == self.fail_asset:
                raise RuntimeError("injected snapshot upload failure")
            self.assets[path.name] = path.read_bytes()
            self.events.append("upload:" + path.name)
        elif operation == "delete-asset":
            self.assets.pop(args[3])
            self.events.append("delete:" + args[3])
        elif operation == "edit":
            self.info["body"] = Path(args[args.index("--notes-file") + 1]).read_text()
            if "--draft=false" in args:
                self.info["isDraft"] = False
            self.events.append("notes")
        elif operation == "create":
            self.info = {"name": "NyaTerm Main Snapshot", "body": "Uploading", "isDraft": True, "isPrerelease": True}
        elif operation == "delete":
            self.info = None
            self.assets = {}
        else:
            raise AssertionError(args)

    def test_partial_snapshot_upload_preserves_old_tag_and_can_restore_all_assets(self):
        snapshot_release.backup(self.dist, self.recovery, "b" * 40)
        self.fail_asset = "second.zip"
        with self.assertRaises(RuntimeError):
            snapshot_release.publish(self.dist, self.recovery)
        self.assertEqual(self.ref["object"]["sha"], "a" * 40)
        self.assertIn("obsolete.zip", self.assets)
        self.assertNotIn("tag", self.events)
        self.fail_asset = None
        snapshot_release.restore(self.recovery)
        self.assertEqual(self.assets, self.old_assets)
        self.assertEqual(self.info["body"], "old notes")

    def test_snapshot_moves_tag_after_all_uploads_and_cleans_obsolete_assets_last(self):
        snapshot_release.backup(self.dist, self.recovery, "b" * 40)
        snapshot_release.publish(self.dist, self.recovery)
        self.assertEqual(self.assets, {"first.zip": b"new-first", "second.zip": b"new-second"})
        self.assertLess(self.events.index("upload:second.zip"), self.events.index("tag"))
        self.assertLess(self.events.index("notes"), self.events.index("delete:obsolete.zip"))

    def test_restore_refuses_newer_snapshot_and_corrupt_backup_before_any_write(self):
        snapshot_release.backup(self.dist, self.recovery, "b" * 40)
        (self.recovery / "assets/second.zip").write_bytes(b"corrupt")
        with self.assertRaises(ValueError):
            snapshot_release.restore(self.recovery)
        self.assertEqual(self.events, [])

        self.ref = {"object": {"sha": "c" * 40}}
        with self.assertRaises(ValueError):
            snapshot_release.restore(self.recovery)
        self.assertEqual(self.events, [])

    def test_first_snapshot_is_recoverable_to_original_absence(self):
        self.info, self.ref, self.assets = None, None, {}
        snapshot_release.backup(self.dist, self.recovery, "b" * 40)
        snapshot_release.publish(self.dist, self.recovery)
        self.assertFalse(self.info["isDraft"])
        snapshot_release.restore(self.recovery)
        self.assertIsNone(self.info)
        self.assertIsNone(self.ref)
        self.assertEqual(self.assets, {})

    def test_snapshot_with_no_previous_assets_restores_without_download_error(self):
        self.assets = {}
        snapshot_release.backup(self.dist, self.recovery, "b" * 40)
        snapshot_release.publish(self.dist, self.recovery)
        snapshot_release.restore(self.recovery)
        self.assertEqual(self.assets, {})


if __name__ == "__main__":
    unittest.main()
