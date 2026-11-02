"""Keep plugin dependency rules effective for aliases and platform dependencies."""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from scripts.ci import check_architecture


class PluginDependencyTests(unittest.TestCase):
    def check_dependencies(self, sdk, host):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "Cargo.toml").write_text(
                '[workspace.dependencies]\nhidden-transport = { package = "nyaterm-transport", version = "1" }\n',
                encoding="utf-8",
            )
            crates = (
                "nyaterm-core", "nyaterm-transport", "nyaterm-terminal", "nyaterm-store", "nyaterm-sync",
                "nyaterm-remote-desktop", "nyaterm-desktop", "nyaterm-plugin-api", "nyaterm-plugin-host",
            )
            for crate in crates:
                directory = root / "crates" / crate
                directory.mkdir(parents=True)
                document = sdk if crate == "nyaterm-plugin-api" else host if crate == "nyaterm-plugin-host" else ""
                (directory / "Cargo.toml").write_text(document, encoding="utf-8")
            with patch.object(check_architecture, "ROOT", root):
                return check_architecture.dependency_errors()

    def test_guest_and_host_keep_their_allowed_dependencies(self):
        self.assertEqual(self.check_dependencies(
            '[dependencies]\nwit-bindgen = "0.49"\n',
            '[dependencies]\nnyaterm-core = "1"\nnyaterm-store = "1"\nwasmtime = "39"\n',
        ), [])

    def test_workspace_alias_cannot_hide_sdk_transport_dependency(self):
        errors = self.check_dependencies('[dependencies]\nhidden-transport.workspace = true\n', "")
        self.assertEqual(errors, ["crate_boundary: nyaterm-plugin-api must not depend on nyaterm-transport"])

    def test_target_and_build_aliases_cannot_hide_presentation_dependencies(self):
        errors = self.check_dependencies(
            '[target.\'cfg(windows)\'.dev-dependencies]\npresentation = { package = "gpui-base", version = "1" }\n',
            '[build-dependencies]\nshell = { package = "nyaterm-desktop", version = "1" }\n',
        )
        self.assertCountEqual(errors, [
            "crate_boundary: nyaterm-plugin-api must not depend on gpui-base",
            "crate_boundary: nyaterm-plugin-host must not depend on nyaterm-desktop",
        ])


if __name__ == "__main__":
    unittest.main()
