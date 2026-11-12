"""Keep crate boundaries effective for aliases and platform dependencies."""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from scripts.ci import check_architecture


class PluginDependencyTests(unittest.TestCase):
    def check_dependencies(self, sdk, host, overrides=None):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "Cargo.toml").write_text(
                '[workspace.dependencies]\nhidden-transport = { package = "nyaterm-transport", version = "1" }\n',
                encoding="utf-8",
            )
            crates = (
                "nyaterm-terminal-gpui",
                "nyaterm-files", "nyaterm-recording", "nyaterm-remote-ops",
                "nyaterm-core", "nyaterm-android", "nyaterm-transport", "nyaterm-terminal", "nyaterm-store", "nyaterm-sync",
                "nyaterm-remote-desktop", "nyaterm-desktop", "nyaterm-plugin-api", "nyaterm-plugin-host",
                "nyaterm-mcp-host", "nyaterm-agent-client",
                "nyaterm-ai-client", "nyaterm-cloud-storage", "nyaterm-updater", "nyaterm-preview", "nyaterm-translation-client",
            )
            for crate in crates:
                directory = root / "crates" / crate
                directory.mkdir(parents=True)
                document = sdk if crate == "nyaterm-plugin-api" else host if crate == "nyaterm-plugin-host" else ""
                document = (overrides or {}).get(crate, document)
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

    def test_transport_cannot_depend_on_business_crates_even_through_aliases(self):
        for crate in ["nyaterm-files", "nyaterm-recording", "nyaterm-remote-ops"]:
            with self.subTest(crate=crate):
                errors = self.check_dependencies("", "", {"nyaterm-transport":
                    f'[target.\'cfg(windows)\'.dev-dependencies]\nhidden = {{ package = "{crate}", version = "1" }}\n'})
                self.assertEqual(errors, [f"crate_boundary: nyaterm-transport must not depend on {crate}"])

    def test_business_crates_remain_independent_and_recording_needs_no_transport(self):
        for source, target in [("nyaterm-files", "nyaterm-remote-ops"),
                               ("nyaterm-remote-ops", "nyaterm-recording"),
                               ("nyaterm-recording", "nyaterm-transport")]:
            with self.subTest(source=source, target=target):
                errors = self.check_dependencies("", "", {source:
                    f'[dependencies]\nhidden = {{ package = "{target}", version = "1" }}\n'})
                self.assertEqual(errors, [f"crate_boundary: {source} must not depend on {target}"])

    def test_native_adapters_accept_core_and_network_dependencies(self):
        for crate in ["nyaterm-ai-client", "nyaterm-cloud-storage", "nyaterm-updater", "nyaterm-preview", "nyaterm-translation-client"]:
            with self.subTest(crate=crate):
                self.assertEqual(self.check_dependencies("", "", {crate:
                    '[dependencies]\nnyaterm-core = "1"\nreqwest = "0.13"\ntokio = "1"\n'}), [])

    def test_native_adapters_reject_all_other_project_crates_in_every_dependency_category(self):
        for source in ["nyaterm-ai-client", "nyaterm-cloud-storage", "nyaterm-updater", "nyaterm-preview", "nyaterm-translation-client"]:
            for target in ["nyaterm-desktop", "nyaterm-store", "nyaterm-ui",
                           "nyaterm-transport", "nyaterm-files", "nyaterm-sync",
                           "nyaterm-recording", "nyaterm-remote-ops",
                           *[crate for crate in ["nyaterm-ai-client", "nyaterm-cloud-storage", "nyaterm-updater", "nyaterm-preview", "nyaterm-translation-client"]
                             if crate != source]]:
                for section in ["dependencies", "dev-dependencies", "build-dependencies",
                                "target.'cfg(windows)'.dependencies",
                                "target.'cfg(windows)'.dev-dependencies",
                                "target.'cfg(windows)'.build-dependencies"]:
                    with self.subTest(source=source, target=target, section=section):
                        errors = self.check_dependencies("", "", {source:
                            f'[{section}]\nhidden = {{ package = "{target}", version = "1" }}\n'})
                        self.assertEqual(errors, [f"crate_boundary: {source} must not depend on {target}"])

    def test_workspace_alias_cannot_hide_native_adapter_transport_dependency(self):
        for crate in ["nyaterm-ai-client", "nyaterm-cloud-storage", "nyaterm-updater", "nyaterm-preview", "nyaterm-translation-client"]:
            with self.subTest(crate=crate):
                errors = self.check_dependencies("", "", {crate:
                    '[dev-dependencies]\nhidden-transport.workspace = true\n'})
                self.assertEqual(errors, [f"crate_boundary: {crate} must not depend on nyaterm-transport"])

    def test_native_adapters_reject_aliased_gpui_platform_and_components(self):
        for crate in ["nyaterm-ai-client", "nyaterm-cloud-storage", "nyaterm-updater", "nyaterm-preview", "nyaterm-translation-client"]:
            for package in ["gpui", "gpui_platform", "gpui-kit", "gpui-base"]:
                with self.subTest(crate=crate, package=package):
                    errors = self.check_dependencies("", "", {crate:
                        f'[target.\'cfg(windows)\'.dev-dependencies]\nhidden = {{ package = "{package}", version = "1" }}\n'})
                    normalized = package.replace("_", "-")
                    self.assertEqual(errors, [f"crate_boundary: {crate} must not depend on {normalized}"])

    def test_agent_and_mcp_adapters_enforce_dependencies_in_all_categories(self):
        sections = ["dependencies", "dev-dependencies", "build-dependencies",
                    "target.'cfg(windows)'.dependencies", "target.'cfg(windows)'.dev-dependencies",
                    "target.'cfg(windows)'.build-dependencies"]
        for source in ["nyaterm-agent-client", "nyaterm-mcp-host"]:
            allowed = {"nyaterm-core"}
            if source == "nyaterm-mcp-host":
                allowed.add("nyaterm-mcp-protocol")
            for target in ["nyaterm-core", "nyaterm-mcp-protocol", "nyaterm-agent-client",
                           "nyaterm-mcp-host", "nyaterm-ai-client", "nyaterm-desktop",
                           "nyaterm-store", "nyaterm-transport", "nyaterm-files",
                           "gpui", "gpui_platform", "gpui-kit"]:
                if source == target:
                    continue
                for section in sections:
                    with self.subTest(source=source, target=target, section=section):
                        errors = self.check_dependencies("", "", {source:
                            f'[{section}]\nhidden = {{ package = "{target}", version = "1" }}\n'})
                        expected = [] if target in allowed else [f"crate_boundary: {source} must not depend on {target.replace('_', '-')}" ]
                        self.assertEqual(errors, expected)
            self.assertEqual(self.check_dependencies("", "", {source:
                '[dev-dependencies]\nhidden-transport.workspace = true\n'}),
                [f"crate_boundary: {source} must not depend on nyaterm-transport"])


    def test_terminal_presentation_accepts_ui_but_rejects_reverse_dependencies(self):
        for section in ["dependencies", "dev-dependencies", "build-dependencies",
                        "target.'cfg(windows)'.dependencies", "target.'cfg(windows)'.dev-dependencies",
                        "target.'cfg(windows)'.build-dependencies"]:
            for package in ["nyaterm-desktop", "nyaterm-store", "nyaterm-files",
                            "nyaterm-core", "nyaterm-terminal", "nyaterm-ui", "gpui"]:
                with self.subTest(section=section, package=package):
                    errors = self.check_dependencies("", "", {"nyaterm-terminal-gpui":
                        f'[{section}]\nhidden = {{ package = "{package}", version = "1" }}\n'})
                    expected = [f"crate_boundary: nyaterm-terminal-gpui must not depend on {package}"] if package in {"nyaterm-desktop", "nyaterm-store", "nyaterm-files"} else []
                    self.assertEqual(errors, expected)
        self.assertEqual(self.check_dependencies("", "", {"nyaterm-terminal-gpui":
            '[dev-dependencies]\nhidden-transport.workspace = true\n'}),
            ["crate_boundary: nyaterm-terminal-gpui must not depend on nyaterm-transport"])


if __name__ == "__main__":
    unittest.main()
