from __future__ import annotations

import base64
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "release"))
import verify_updater_signatures


class UpdaterSignatureSelectionTests(unittest.TestCase):
    def test_stable_signatures_include_msi_and_preview_signatures_do_not(self) -> None:
        for version, count in (("2.0.0", 14), ("2.0.0-preview.1", 12)):
            with (mock.patch.object(sys, "argv", ["verify", "--dist", "dist", "--version", version,
                                                   "--public-key", "test.pub"]),
                  mock.patch.object(verify_updater_signatures, "verify_file") as verify):
                verify_updater_signatures.main()
            names = {call.args[0].name for call in verify.call_args_list}
            self.assertEqual(len(names), count)
            self.assertEqual(sum(name.endswith(".msi") for name in names), 2 if count == 14 else 0)

    def test_missing_or_invalid_base64_msi_signature_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "package.msi"
            artifact.write_bytes(b"test MSI")
            with self.assertRaises(FileNotFoundError):
                verify_updater_signatures.verify_file(artifact, Path("test.pub"))
            artifact.with_suffix(".msi.sig").write_text("invalid!", encoding="utf-8")
            with self.assertRaises(ValueError):
                verify_updater_signatures.verify_file(artifact, Path("test.pub"))


@unittest.skipUnless(shutil.which("minisign"), "minisign is required for signature integration tests")
class UpdaterSignatureTests(unittest.TestCase):
    def test_accepts_valid_signature_and_rejects_tampering(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            private = root / "test.key"
            public = root / "test.pub"
            artifact = root / "package.msi"
            artifact.write_bytes(b"test package")
            subprocess.run(["minisign", "-G", "-W", "-p", str(public), "-s", str(private)], check=True)
            subprocess.run(["minisign", "-Sm", str(artifact), "-s", str(private)], check=True)
            signature = root / "package.msi.sig"
            signature.write_bytes(base64.b64encode((root / "package.msi.minisig").read_bytes()))
            verify_updater_signatures.verify_file(artifact, public)
            artifact.write_bytes(b"tampered package")
            with self.assertRaises(subprocess.CalledProcessError):
                verify_updater_signatures.verify_file(artifact, public)
            signature.write_bytes(base64.b64encode(b"invalid signature"))
            with self.assertRaises(subprocess.CalledProcessError):
                verify_updater_signatures.verify_file(artifact, public)


if __name__ == "__main__":
    unittest.main()
