"""Exercise CI selection against real Git trees, manifests, and workflow gates."""

from __future__ import annotations

import itertools
import json
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib
import unittest
from unittest.mock import patch

from scripts.ci import select_rust_jobs as selector
from scripts.tests.test_release_workflow import gate


ROOT = Path(__file__).resolve().parents[2]
ALL = set(selector.OUTPUTS)


def workflow_jobs(filename: str) -> dict[str, str]:
    text = (ROOT / ".github/workflows" / filename).read_text(encoding="utf-8")
    parts = re.split(r"^  ([a-z][a-z0-9-]*):\s*$", text.split("\njobs:\n", 1)[1], flags=re.M)
    return dict(zip(parts[1::2], parts[2::2]))


class PathSelectionTests(unittest.TestCase):
    def test_paths_enable_only_their_required_jobs(self):
        cases = (
            ("docs-site/docs/guide/tmux.md", {"run_docs"}),
            ("docs-site/i18n/en/code.json", {"run_docs"}),
            ("README.zh-CN.md", {"run_docs"}),
            ("AGENTS.md", {"run_docs"}),
            ("LICENSE", set()),
            (".github/ISSUE_TEMPLATE/bug_report.yml", set()),
            ("crates/nyaterm-desktop/locales/ja.json", {"run_rust"}),
            ("crates/nyaterm-desktop/src/features/settings.rs", {"run_rust"}),
            ("crates/nyaterm-desktop/src/features/tmux.rs", {"run_rust"}),
            ("crates/nyaterm-files/src/sftp/mod.rs", {"run_rust"}),
            ("crates/nyaterm-recording/src/recording.rs", {"run_rust"}),
            ("crates/nyaterm-updater/src/download.rs", {"run_rust"}),
            ("crates/nyaterm-remote-ops/src/docker.rs", {"run_rust"}),
            ("crates/nyaterm-transport/src/tmux/control.rs", {"run_rust", "run_tmux"}),
            ("crates/nyaterm-transport/src/ssh/worker.rs", {"run_rust", "run_tmux"}),
            ("crates/nyaterm-terminal/src/lib.rs", {"run_rust", "run_tmux"}),
            ("crates/nyaterm-terminal-runtime/src/lib.rs", {"run_rust"}),
            ("crates/nyaterm-core/src/lib.rs", {"run_rust", "run_tmux", "run_plugin_sdk"}),
            ("crates/nyaterm-plugin-api/wit/plugin.wit", {"run_rust", "run_tmux", "run_plugin_sdk"}),
            ("crates/nyaterm-mcp-protocol/src/lib.rs", {"run_rust", "run_tmux", "run_plugin_sdk"}),
            ("crates/nyaterm-store/src/lib.rs", {"run_rust", "run_plugin_sdk"}),
            ("crates/nyaterm-sync/src/lib.rs", {"run_rust", "run_plugin_sdk"}),
            ("crates/nyaterm-plugin-host/tests/fixtures/text-tools.wasm", {"run_rust", "run_plugin_sdk"}),
            ("crates/nyaterm-core/tests/fixtures/README.md", {"run_rust", "run_tmux", "run_plugin_sdk"}),
            ("crates/nyaterm-app/assets/icons/terminal.svg", {"run_rust"}),
            ("crates/nyaterm-app/build.rs", {"run_rust", "run_python"}),
            ("crates/nyaterm-app/resources/icons/icon.ico", {"run_rust", "run_python"}),
            ("scripts/release/package_native.py", {"run_python"}),
            ("scripts/tests/test_release_workflow.py", {"run_python"}),
            ("scripts/tests/test_windows_resources.py", {"run_rust", "run_python"}),
            ("scripts/tests/test_prepare_android.py", {"run_rust", "run_python"}),
            ("scripts/release/prepare_android.py", {"run_rust", "run_python"}),
            ("scripts/plugins/build.py", {"run_python", "run_plugin_sdk"}),
            ("examples/plugins/guests/text-tools/src/lib.rs", {"run_plugin_sdk"}),
            ("examples/plugins/templates/templates/inspect.txt", {"run_plugin_sdk"}),
            (".github/workflows/release.yml", {"run_python", "run_workflows"}),
            (".github/workflows/docs.yml", {"run_python", "run_workflows", "run_docs"}),
            (".github/actionlint.yaml", {"run_workflows"}),
        )
        for path, expected in cases:
            with self.subTest(path=path):
                self.assertEqual(selector.select_paths([path]).enabled, expected)

    def test_shared_configuration_enables_every_job(self):
        paths = (
            "Cargo.toml", "Cargo.lock", "crates/nyaterm-desktop/Cargo.toml",
            "examples/plugins/guests/Cargo.lock", ".cargo/config.toml",
            "examples/plugins/guests/.cargo/config.toml", "rust-toolchain",
            "rust-toolchain.toml", "rustfmt.toml", ".rustfmt.toml",
            "clippy.toml", ".clippy.toml", ".github/workflows/rust.yml",
            ".github/workflows/native-package.yml",
            ".github/actions/setup-rust/action.yml", "scripts/ci/select_rust_jobs.py",
            "scripts/ci/install-linux-deps.sh", "scripts/ci/architecture_allowlist.json",
        )
        for path in paths:
            with self.subTest(path=path):
                self.assertEqual(selector.select_paths([path]).enabled, ALL)

    def test_unknown_paths_and_new_crates_enable_every_job(self):
        for path in ("crates/nyaterm-new/src/lib.rs", "fixtures/example.md", ".gitattributes", "new-script.sh"):
            with self.subTest(path=path):
                self.assertEqual(selector.select_paths([path]).enabled, ALL)

    def test_mixed_changes_union_all_jobs_and_preserve_reasons(self):
        selection = selector.select_paths([
            "README.md", "crates/nyaterm-transport/src/lib.rs",
            "scripts/plugins/verify.py", ".github/actionlint.yaml",
        ])
        self.assertEqual(selection.enabled, ALL)
        self.assertEqual(len(selection.reasons), 4)

    def test_empty_diff_skips_optional_jobs(self):
        selection = selector.select_paths([])
        self.assertEqual(selection.enabled, set())
        self.assertIn("No changed paths", selection.summary())

    def test_output_values_are_literal_lowercase_boolean_strings(self):
        self.assertEqual(selector.select_paths(["README.md"]).outputs(), {name: str(name == "run_docs").lower() for name in selector.OUTPUTS})
        self.assertEqual(selector.full_selection("test").outputs(), dict.fromkeys(selector.OUTPUTS, "true"))

    def test_untrusted_filenames_cannot_inject_summary_markup_or_commands(self):
        summary = selector.select_paths(['docs-site/<script>\n::error:: "`x`".md']).summary()
        self.assertNotIn("<script>", summary)
        self.assertNotIn("\n::error::", summary)
        self.assertIn("&lt;script&gt;", summary)
        self.assertIn("\\n::error::", summary)


class GitSelectionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.git("init", "--quiet")
        self.git("config", "user.name", "CI selection tests")
        self.git("config", "user.email", "ci@example.invalid")

    def git(self, *args: str, data: bytes | None = None) -> bytes:
        return subprocess.run(
            ["git", *args], cwd=self.root, input=data, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, check=True,
        ).stdout

    def commit(self, files: dict[str, bytes], parent: str | None = None) -> str:
        # Write Git trees directly, so newline/tab filenames can also be tested on
        # Windows without asking NTFS to materialize Unix-only filenames.
        tree = {}
        for path, content in files.items():
            node = tree
            *directories, name = path.split("/")
            for directory in directories:
                node = node.setdefault(directory, {})
            node[name] = content

        def write_tree(node: dict) -> bytes:
            entries = []
            for name, content in sorted(node.items()):
                if isinstance(content, dict):
                    entry = b"040000 tree " + write_tree(content)
                else:
                    entry = b"100644 blob " + self.git("hash-object", "-w", "--stdin", data=content).strip()
                entries.append(entry + b"\t" + name.encode("utf-8") + b"\0")
            return self.git("mktree", "-z", data=b"".join(entries)).strip()

        args = ["commit-tree", write_tree(tree).decode(), "-m", "test commit"]
        if parent:
            args.extend(["-p", parent])
        return self.git(*args).decode().strip()

    def event(self, base: str, *, action: str = "synchronize", labels: tuple[str, ...] = ()) -> dict:
        return {
            "action": action,
            "pull_request": {"base": {"sha": base}, "labels": [{"name": name} for name in labels]},
        }

    def test_deleted_transport_file_still_runs_tmux(self):
        base = self.commit({"crates/nyaterm-transport/src/ssh.rs": b"before"})
        head = self.commit({}, base)
        self.assertEqual(selector.changed_paths(self.root, base, head), ["crates/nyaterm-transport/src/ssh.rs"])
        selection = selector.select_event("pull_request", self.event(base), head, self.root)
        self.assertEqual(selection.enabled, {"run_rust", "run_tmux"})

    def test_cross_rule_rename_checks_both_old_and_new_paths(self):
        old = "crates/nyaterm-transport/src/ssh.rs"
        new = "docs-site/docs/ssh.md"
        base = self.commit({old: b"identical content"})
        head = self.commit({new: b"identical content"}, base)
        self.assertCountEqual(selector.changed_paths(self.root, base, head), [old, new])
        self.assertIn("run_tmux", selector.select_event("pull_request", self.event(base), head, self.root).enabled)

    def test_nul_delimited_git_paths_preserve_spaces_tabs_newlines_and_unicode(self):
        paths = ["docs-site/docs/space name.md", "docs-site/docs/tab\tname.md", "docs-site/docs/line\nbreak.md", "docs-site/docs/中文.md"]
        base = self.commit({})
        head = self.commit(dict.fromkeys(paths, b"documentation"), base)
        self.assertCountEqual(selector.changed_paths(self.root, base, head), paths)
        self.assertEqual(selector.select_event("pull_request", self.event(base), head, self.root).enabled, {"run_docs"})

    def test_empty_git_diff_skips_optional_jobs(self):
        base = self.commit({"README.md": b"unchanged"})
        selection = selector.select_event("pull_request", self.event(base), base, self.root)
        self.assertEqual(selection.enabled, set())

    def test_diff_compares_event_base_with_tested_merge_revision(self):
        base = self.commit({"README.md": b"base"})
        head = self.commit({"README.md": b"base", "crates/nyaterm-terminal/src/lib.rs": b"change"}, base)
        event = self.event(base)
        event["pull_request"]["head"] = {"sha": base}
        self.assertIn("run_tmux", selector.select_event("pull_request", event, head, self.root).enabled)

    def test_missing_or_unavailable_revisions_run_every_job(self):
        head = self.commit({"README.md": b"docs"})
        for base, tested in (("", head), ("f" * 40, head), (head, ""), (head, "e" * 40)):
            with self.subTest(base=base, tested=tested):
                selection = selector.select_event("pull_request", self.event(base), tested, self.root)
                self.assertEqual(selection.enabled, ALL)

    def test_failed_diff_runs_every_job(self):
        error = subprocess.CalledProcessError(128, "git", stderr=b"private diagnostics")
        with patch.object(selector.subprocess, "run", side_effect=error):
            selection = selector.select_event("pull_request", self.event("base"), "tested", self.root)
        self.assertEqual(selection.enabled, ALL)
        self.assertNotIn("private diagnostics", selection.summary())

    def test_missing_git_runs_every_job(self):
        with patch.object(selector.subprocess, "run", side_effect=FileNotFoundError):
            selection = selector.select_event("pull_request", self.event("base"), "tested", self.root)
        self.assertEqual(selection.enabled, ALL)

    def test_run_tmux_label_applies_on_label_event_and_later_updates(self):
        base = self.commit({})
        head = self.commit({"crates/nyaterm-desktop/locales/fr.json": b"{}"}, base)
        for action in ("labeled", "synchronize"):
            with self.subTest(action=action):
                event = self.event(base, action=action, labels=("run-tmux",))
                event["label"] = {"name": "run-tmux"}
                self.assertEqual(
                    selector.select_event("pull_request", event, head, self.root).enabled,
                    {"run_rust", "run_tmux"},
                )

    def test_unrelated_label_keeps_all_path_selected_jobs(self):
        base = self.commit({})
        head = self.commit({"crates/nyaterm-core/src/lib.rs": b"change"}, base)
        event = self.event(base, action="labeled", labels=("bug",))
        self.assertEqual(
            selector.select_event("pull_request", event, head, self.root).enabled,
            {"run_rust", "run_tmux", "run_plugin_sdk"},
        )

    def test_non_pr_events_run_full_suite_without_a_diff(self):
        for name in ("workflow_dispatch", "schedule"):
            with self.subTest(name=name), patch.object(selector, "changed_paths", side_effect=AssertionError("unexpected diff")):
                self.assertEqual(selector.select_event(name, {}, "tested", self.root).enabled, ALL)

    def test_push_and_merge_queue_compare_actual_tested_revisions(self):
        base = self.commit({"README.md": b"base"})
        head = self.commit({"README.md": b"base", "crates/nyaterm-ai-client/src/lib.rs": b"change"}, base)
        for name, event in (("push", {"before": base, "after": head}),
                            ("merge_group", {"merge_group": {"base_sha": base, "head_sha": head}})):
            with self.subTest(event=name):
                self.assertEqual(selector.select_event(name, event, head, self.root).enabled, {"run_rust"})
                self.assertEqual(selector.select_event(name, event, base, self.root).enabled, ALL)
                self.assertEqual(selector.select_event(name, {}, head, self.root).enabled, ALL)
        self.assertEqual(selector.select_event("push", {"before": "0" * 40, "after": head}, head, self.root).enabled, ALL)

    def test_main_writes_outputs_and_explains_selection(self):
        base = self.commit({})
        head = self.commit({"README.md": b"documentation"}, base)
        event_path = self.root / "event.json"
        event_path.write_text(json.dumps(self.event(base, labels=("run-tmux",))), encoding="utf-8")
        output = self.root / "outputs"
        summary = self.root / "summary"
        environment = {
            "GITHUB_EVENT_PATH": str(event_path), "GITHUB_EVENT_NAME": "pull_request",
            "GITHUB_SHA": head, "GITHUB_OUTPUT": str(output), "GITHUB_STEP_SUMMARY": str(summary),
        }
        self.assertEqual(selector.main(environment, self.root), 0)
        self.assertEqual(dict(line.split("=") for line in output.read_text(encoding="utf-8").splitlines()), {
            "run_rust": "false", "run_tmux": "true", "run_plugin_sdk": "false",
            "run_python": "false", "run_workflows": "false", "run_docs": "true",
        })
        self.assertIn("run-tmux", summary.read_text(encoding="utf-8"))
        self.assertIn("README.md", summary.read_text(encoding="utf-8"))

    def test_unexpected_script_error_is_not_converted_to_success(self):
        with patch.object(selector, "changed_paths", side_effect=RuntimeError("classifier bug")):
            with self.assertRaisesRegex(RuntimeError, "classifier bug"):
                selector.select_event("pull_request", self.event("base"), "tested", self.root)


def manifest_graph(root: Path) -> dict[str, set[str]]:
    """Read local dependency edges only; never resolve/download third-party crates."""
    workspace = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]
    manifests = {
        (root / member / "Cargo.toml").resolve(): tomllib.loads((root / member / "Cargo.toml").read_text(encoding="utf-8"))
        for member in workspace["members"]
    }
    by_name = {document["package"]["name"]: path.parent.name for path, document in manifests.items()}
    graph = {}
    for path, document in manifests.items():
        edges = set()
        sections = [document, *document.get("target", {}).values()]
        for section in sections:
            for kind in ("dependencies", "dev-dependencies", "build-dependencies"):
                for alias, dependency in section.get(kind, {}).items():
                    dependency = dependency if isinstance(dependency, dict) else {}
                    inherited = dependency.get("workspace", False)
                    if inherited:
                        specification = workspace["dependencies"][alias]
                        dependency = specification if isinstance(specification, dict) else {}
                    if "path" in dependency:
                        anchor = root if inherited else path.parent
                        target = (anchor / dependency["path"] / "Cargo.toml").resolve()
                        if target in manifests:
                            edges.add(target.parent.name)
                    else:
                        package = dependency.get("package", alias)
                        if package in by_name:
                            edges.add(by_name[package])
        graph[path.parent.name] = edges
    return graph


def dependency_closure(graph: dict[str, set[str]], roots: set[str]) -> set[str]:
    closure = set(roots)
    pending = list(roots)
    while pending:
        for dependency in graph[pending.pop()]:
            if dependency not in closure:
                closure.add(dependency)
                pending.append(dependency)
    return closure


class ManifestCoverageTests(unittest.TestCase):
    def test_transport_split_preserves_independent_dependency_closures(self):
        graph = manifest_graph(ROOT)
        business = {"nyaterm-files", "nyaterm-recording", "nyaterm-remote-ops"}
        self.assertTrue(business.isdisjoint(dependency_closure(graph, {"nyaterm-transport"})))
        for crate in business:
            with self.subTest(crate=crate):
                closure = dependency_closure(graph, {crate})
                self.assertEqual(closure & business, {crate})
                self.assertTrue({"nyaterm-desktop", "nyaterm-ui", "nyaterm-terminal-gpui"}.isdisjoint(closure))
        self.assertNotIn("nyaterm-transport", dependency_closure(graph, {"nyaterm-recording"}))

    def test_every_workspace_crate_is_registered(self):
        self.assertSetEqual(set(manifest_graph(ROOT)), set(selector.KNOWN_CRATES), "Register workspace crates in select_rust_jobs.py")

    def test_heavy_job_sets_cover_transitive_workspace_dependencies(self):
        graph = manifest_graph(ROOT)
        cases = (
            ({"nyaterm-transport"}, selector.TMUX_CRATES),
            ({"nyaterm-plugin-api", "nyaterm-plugin-host", "nyaterm-core", "nyaterm-store"}, selector.PLUGIN_SDK_CRATES),
        )
        for roots, registered in cases:
            with self.subTest(roots=roots):
                self.assertLessEqual(dependency_closure(graph, roots), registered, "Update CI dependency sets in select_rust_jobs.py")
                self.assertLessEqual(registered, selector.KNOWN_CRATES)

    def test_manifest_guard_follows_aliases_paths_optional_target_build_and_dev_dependencies(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "Cargo.toml").write_text(
                '[workspace]\nmembers = ["crates/a", "crates/b", "crates/c", "crates/d", "crates/e"]\n'
                '[workspace.dependencies]\ninherited = { package = "renamed-b", path = "crates/b" }\n', encoding="utf-8",
            )
            specifications = {
                "a": '[dependencies]\ninherited.workspace = true\n'
                     '[target.\'cfg(windows)\'.dependencies]\noptional-c = { package = "c", version = "1", optional = true }\n'
                     '[build-dependencies]\nanother-name = { path = "../d" }\n'
                     '[target.\'cfg(unix)\'.dev-dependencies]\ntest-only = { package = "e", version = "1" }\n',
                "b": '[dev-dependencies]\ncycle = { package = "a", version = "1" }\n',
                "c": "", "d": "", "e": "",
            }
            for directory, dependencies in specifications.items():
                path = root / "crates" / directory
                path.mkdir(parents=True)
                name = "renamed-b" if directory == "b" else directory
                (path / "Cargo.toml").write_text(f'[package]\nname = "{name}"\n{dependencies}', encoding="utf-8")
            self.assertEqual(manifest_graph(root)["a"], {"b", "c", "d", "e"})
            self.assertEqual(dependency_closure(manifest_graph(root), {"a"}), {"a", "b", "c", "d", "e"})


class WorkflowWiringTests(unittest.TestCase):
    def test_each_optional_job_uses_the_corresponding_output(self):
        jobs = workflow_jobs("rust.yml")
        mapping = {
            "lint": "run_rust", "test": "run_rust", "tmux-compatibility": "run_tmux",
            "plugin-sdk": "run_plugin_sdk", "python": "run_python", "workflows": "run_workflows",
        }
        for job, output in mapping.items():
            with self.subTest(job=job):
                self.assertIn("    needs: [orchestrate]", jobs[job])
                for enabled in ("true", "false", ""):
                    self.assertEqual(gate(jobs[job], {f"needs.orchestrate.outputs.{output}": enabled}), enabled == "true")
        for output in set(selector.OUTPUTS) - {"run_docs"}:
            self.assertIn(f"      {output}: ${{{{ steps.select.outputs.{output} }}}}", jobs["orchestrate"])

    def test_orchestrator_always_tests_rules_before_selection_with_full_history(self):
        orchestrate = workflow_jobs("rust.yml")["orchestrate"]
        self.assertNotRegex(orchestrate, r"(?m)^    if:", msg="orchestrate must run for every event")
        self.assertIn("fetch-depth: 0", orchestrate)
        self.assertIn("persist-credentials: false", orchestrate)
        self.assertLess(
            orchestrate.index("python -m unittest scripts.tests.test_select_rust_jobs"),
            orchestrate.index("python scripts/ci/select_rust_jobs.py"),
        )

    def test_gate_accepts_success_and_skips_and_rejects_failure_or_cancellation(self):
        for filename, name in (("rust.yml", "Rust gate"), ("docs.yml", "Docs gate")):
            jobs = workflow_jobs(filename)
            job = jobs["gate"]
            self.assertIn(f"    name: {name}", job)
            self.assertIn("    if: always()", job)
            self.assertIn("name: Summarize dependency results", job)
            self.assertIn("NEEDS_JSON: ${{ toJSON(needs) }}", job)
            dependencies = re.search(r"^    needs: \[(.+)\]$", job, flags=re.M).group(1).split(", ")
            self.assertCountEqual(dependencies, set(jobs) - {"gate"})
            step = job.split("      - name: Check dependency results\n", 1)[1].replace("        if:", "    if:")
            condition = re.search(r"^    if: .+$", step, flags=re.M).group(0)

            def rejects(results):
                return gate(condition, {
                    "contains(needs.*.result, 'failure')": "failure" in results,
                    "contains(needs.*.result, 'cancelled')": "cancelled" in results,
                })

            for results in itertools.product(("success", "skipped"), repeat=len(dependencies)):
                self.assertFalse(rejects(results))
            for index, dependency in enumerate(dependencies):
                for result in ("failure", "cancelled"):
                    with self.subTest(workflow=filename, dependency=dependency, result=result):
                        results = ["skipped"] * len(dependencies)
                        results[index] = result
                        self.assertTrue(rejects(results))
            self.assertIn("        run: exit 1", step)

    def test_event_filters_keep_branches_and_report_both_gates_for_merge_queue(self):
        for filename in ("rust.yml", "docs.yml"):
            text = (ROOT / ".github/workflows" / filename).read_text(encoding="utf-8")
            events = text.split("\non:\n", 1)[1].split("\npermissions:", 1)[0]
            self.assertIn("  workflow_dispatch:", events)
            self.assertIn("  merge_group:\n    types: [checks_requested]", events)
            self.assertIn("  push:\n    branches: [main, migration/gpui]", events)
            self.assertIn("  pull_request:\n    branches: [main, migration/gpui]", events)
            self.assertNotRegex(events, r"(?m)^\s+paths(?:-ignore)?:")
            if filename == "rust.yml":
                self.assertIn("    types: [opened, synchronize, reopened, labeled]", events)

    def test_existing_test_commands_and_tmux_matrix_are_preserved(self):
        jobs = workflow_jobs("rust.yml")
        self.assertIn("cargo clippy --workspace --all-targets --locked -- -D warnings", jobs["lint"])
        self.assertIn("cargo test --workspace --locked --no-fail-fast", jobs["test"])
        self.assertIn("cargo test -p nyaterm-transport --lib real_tmux --locked -- --ignored", jobs["tmux-compatibility"])
        self.assertEqual(re.findall(r"^            tmux: (.+)$", jobs["tmux-compatibility"], flags=re.M), ["3.2a", "'3.4'", "'3.8'", "system"])


if __name__ == "__main__":
    unittest.main()
