"""Select Rust CI jobs without installing Rust or resolving Cargo dependencies."""

from __future__ import annotations

from dataclasses import dataclass, field
import html
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
from typing import Iterable, Mapping


ROOT = Path(__file__).resolve().parents[2]
OUTPUTS = ("run_rust", "run_tmux", "run_plugin_sdk", "run_python", "run_workflows", "run_docs")

# Explicit registration makes a new crate fail closed until its CI impact is reviewed.
KNOWN_CRATES = frozenset({
    "nyaterm-ai-client", "nyaterm-cloud-storage", "nyaterm-updater", "nyaterm-preview", "nyaterm-translation-client",
    "nyaterm-app", "nyaterm-core", "nyaterm-desktop", "nyaterm-mcp",
    "nyaterm-files", "nyaterm-recording", "nyaterm-remote-ops",
    "nyaterm-mcp-protocol", "nyaterm-otp", "nyaterm-plugin-api",
    "nyaterm-plugin-host", "nyaterm-rdp-helper", "nyaterm-windows-rdp",
    "nyaterm-remote-desktop", "nyaterm-store", "nyaterm-sync", "nyaterm-terminal",
    "nyaterm-terminal-gpui", "nyaterm-transport", "nyaterm-ui", "nyaterm-vnc-helper",
})

# Include transitive local dependencies, not reverse dependencies. The real tmux
# tests exercise transport; the SDK job also builds the host and tests the store.
# Manifest-closure tests guard both sets against dependency drift.
TMUX_CRATES = frozenset({
    "nyaterm-transport", "nyaterm-terminal", "nyaterm-core",
    "nyaterm-plugin-api", "nyaterm-mcp-protocol",
})
PLUGIN_SDK_CRATES = frozenset({
    "nyaterm-plugin-api", "nyaterm-plugin-host", "nyaterm-core",
    "nyaterm-store", "nyaterm-sync", "nyaterm-mcp-protocol",
})
GLOBAL_CONFIG_NAMES = frozenset({
    "Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml",
    "rustfmt.toml", ".rustfmt.toml", "clippy.toml", ".clippy.toml",
})


@dataclass
class Selection:
    enabled: set[str] = field(default_factory=set)
    reasons: list[str] = field(default_factory=list)

    def enable(self, jobs: Iterable[str], reason: str) -> None:
        self.enabled.update(jobs)
        self.reasons.append(reason)

    def outputs(self) -> dict[str, str]:
        return {name: str(name in self.enabled).lower() for name in OUTPUTS}

    def summary(self) -> str:
        rows = "\n".join(f"| `{name}` | {value} |" for name, value in self.outputs().items())
        # Paths are untrusted Git data; escape markup and control characters rather
        # than letting a filename create headings, links, or workflow commands.
        reasons = "\n".join(f"- <code>{html.escape(reason)}</code>" for reason in self.reasons)
        return f"## Rust CI selection\n\n| Output | Enabled |\n| --- | --- |\n{rows}\n\n{reasons}\n"


def full_selection(reason: str) -> Selection:
    selection = Selection()
    selection.enable(OUTPUTS, reason)
    return selection


def select_paths(paths: Iterable[str]) -> Selection:
    selection = Selection()
    for path in sorted(set(paths)):
        parts = PurePosixPath(path).parts
        name = PurePosixPath(path).name
        description = json.dumps(path, ensure_ascii=True)
        if (
            name in GLOBAL_CONFIG_NAMES
            or "/.cargo/" in f"/{path}"
            or path in {".github/workflows/rust.yml", ".github/workflows/native-package.yml"}
            or path.startswith((".github/actions/", "scripts/ci/"))
        ):
            selection.enable(OUTPUTS, f"Shared build/CI configuration: {description}")
        elif path == ".github/workflows/docs.yml":
            selection.enable({"run_docs", "run_python", "run_workflows"}, f"Documentation workflow: {description}")
        elif len(parts) >= 3 and parts[0] == "crates" and parts[1] in KNOWN_CRATES:
            jobs = {"run_rust"}
            if parts[1] in TMUX_CRATES:
                jobs.add("run_tmux")
            if parts[1] in PLUGIN_SDK_CRATES:
                jobs.add("run_plugin_sdk")
            if path == "crates/nyaterm-app/build.rs" or path.startswith("crates/nyaterm-app/resources/"):
                jobs.add("run_python")
            selection.enable(jobs, f"Workspace crate: {description}")
        elif path.startswith("examples/plugins/"):
            selection.enable({"run_plugin_sdk"}, f"Plugin example: {description}")
        elif path.startswith("scripts/"):
            jobs = {"run_python"}
            if path.startswith("scripts/plugins/"):
                jobs.add("run_plugin_sdk")
            if path in {"scripts/tests/test_windows_resources.py", "scripts/tests/test_windows_msi.py", "scripts/tests/test_package_diagnostic.py"}:
                jobs.add("run_rust")
            selection.enable(jobs, f"Script or script test: {description}")
        elif path.startswith(".github/workflows/"):
            selection.enable({"run_python", "run_workflows"}, f"Workflow: {description}")
        elif path in {".github/actionlint.yaml", ".github/actionlint.yml"}:
            selection.enable({"run_workflows"}, f"Actionlint configuration: {description}")
        elif path.startswith("docs-site/") or (len(parts) == 1 and name.lower().endswith(".md")):
            selection.enable({"run_docs"}, f"Documentation: {description}")
        elif path.startswith(".github/ISSUE_TEMPLATE/") or path == "LICENSE":
            selection.reasons.append(f"Documentation or repository text: {description}")
        else:
            selection.enable(OUTPUTS, f"Unclassified path; run all jobs: {description}")
    if not selection.reasons:
        selection.reasons.append("No changed paths")
    return selection


class ComparisonUnavailable(Exception):
    """An expected Git comparison failure, which must enable the full suite."""


def changed_paths(root: Path, base_sha: str, tested_sha: str) -> list[str]:
    if not base_sha or not tested_sha:
        raise ComparisonUnavailable("Missing base or tested revision")
    try:
        result = subprocess.run(
            ["git", "diff", "--no-renames", "--name-only", "-z", base_sha, tested_sha, "--"],
            cwd=root, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        # Do not copy arbitrary git stderr into Actions logs or the summary.
        raise ComparisonUnavailable("Git comparison unavailable") from error
    return [os.fsdecode(path) for path in result.stdout.split(b"\0") if path]


def select_event(event_name: str, event: dict, tested_sha: str, root: Path = ROOT) -> Selection:
    if event_name not in {"pull_request", "push", "merge_group"}:
        return full_selection(f"Full suite for event: {json.dumps(event_name)}")
    pull_request = event.get("pull_request", {})
    if event_name == "pull_request":
        base, head = pull_request.get("base", {}).get("sha", ""), tested_sha
    elif event_name == "push":
        base, head = event.get("before", ""), event.get("after", "")
    else:
        group = event.get("merge_group", {})
        base, head = group.get("base_sha", ""), group.get("head_sha", "")
    # A selector must describe the exact checkout tested by subsequent jobs.
    if head != tested_sha:
        return full_selection("Event revision does not match tested checkout")
    try:
        paths = changed_paths(root, base, head)
    except ComparisonUnavailable as error:
        selection = full_selection(str(error))
    else:
        selection = select_paths(paths)
    if any(label["name"] == "run-tmux" for label in pull_request.get("labels", [])):
        selection.enable({"run_tmux"}, "PR has run-tmux label")
    return selection


def main(environment: Mapping[str, str] | None = None, root: Path = ROOT) -> int:
    environment = os.environ if environment is None else environment
    event = json.loads(Path(environment["GITHUB_EVENT_PATH"]).read_text(encoding="utf-8"))
    selection = select_event(environment["GITHUB_EVENT_NAME"], event, environment["GITHUB_SHA"], root)
    with Path(environment["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as output:
        output.writelines(f"{name}={value}\n" for name, value in selection.outputs().items())
    with Path(environment["GITHUB_STEP_SUMMARY"]).open("a", encoding="utf-8") as summary:
        summary.write(selection.summary())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
