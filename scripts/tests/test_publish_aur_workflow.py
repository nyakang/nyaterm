"""Regression checks for AUR packaging in its Arch container."""

from pathlib import Path
import re
import unittest

WORKFLOW = Path(__file__).resolve().parents[2] / ".github/workflows/publish-aur.yml"


class AurWorkflowTests(unittest.TestCase):
    def test_package_contents_checked_inside_arch_container(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn('contents="$(bsdtar -tf /pkg/*.pkg.tar.zst)"', workflow)
        self.assertIn("for binary in nyaterm nyaterm-mcp; do", workflow)
        self.assertNotIn("bsdtar -tf aur/*.pkg.tar.zst", workflow)

    def test_required_desktop_libraries_are_declared(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        depends = re.search(r"depends=\(([^\n]+)\)", workflow).group(1)
        for package in ("glib2", "gtk3", "gdk-pixbuf2", "hicolor-icon-theme"):
            with self.subTest(package=package):
                self.assertIn(f"'{package}'", depends)


if __name__ == "__main__":
    unittest.main()
