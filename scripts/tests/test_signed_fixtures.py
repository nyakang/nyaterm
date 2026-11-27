"""Signed fixtures must survive Git's Windows checkout conversions unchanged."""

from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
FIXTURES = Path("crates/nyaterm-plugin-host/tests/fixtures/native")


class SignedFixtureTests(unittest.TestCase):
    @unittest.skipUnless(shutil.which("git"), "Git is required")
    def test_native_signatures_and_payloads_survive_crlf_checkout(self):
        with tempfile.TemporaryDirectory() as directory:
            repository = Path(directory) / "repository"
            checkout = Path(directory) / "checkout"
            repository.mkdir()
            subprocess.run(["git", "init", "-q", str(repository)], check=True)
            shutil.copy2(ROOT / ".gitattributes", repository / ".gitattributes")
            shutil.copytree(ROOT / FIXTURES, repository / FIXTURES)
            git = ["git", "-C", str(repository), "-c", "core.autocrlf=true",
                   "-c", "core.safecrlf=false"]
            subprocess.run([*git, "add", "."], check=True)
            subprocess.run([*git, "checkout-index", "--all", f"--prefix={checkout.as_posix()}/"],
                           check=True)
            for fixture in (ROOT / FIXTURES).iterdir():
                with self.subTest(fixture=fixture.name):
                    self.assertEqual((checkout / FIXTURES / fixture.name).read_bytes(),
                                     fixture.read_bytes())
