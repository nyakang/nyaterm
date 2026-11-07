"""Exercise release job gates without dispatching any publishing operation."""

from __future__ import annotations

import re
import unittest
from pathlib import Path


WORKFLOW = Path(__file__).resolve().parents[2] / ".github/workflows/release.yml"


def jobs() -> dict[str, str]:
    parts = re.split(r"^  ([a-z_][a-z_0-9]*):\s*$", WORKFLOW.read_text(encoding="utf-8"), flags=re.M)
    return dict(zip(parts[1::2], parts[2::2]))


def gate(job: str, values: dict[str, object]) -> bool:
    condition = re.search(r"^    if: (.+(?:\n      [^\n]+)*)", job, flags=re.M).group(1)
    condition = condition.removeprefix(">-").strip().replace("\n", " ")
    condition = condition.replace("always()", "True")
    for name, value in sorted(values.items(), key=lambda item: -len(item[0])):
        condition = condition.replace(name, repr(value))
    condition = re.sub(r"\btrue\b(?=(?:[^']*'[^']*')*[^']*$)", "True", condition)
    condition = condition.replace("&&", " and ").replace("||", " or ")
    # Only literals/boolean operators remain; never evaluate shell or workflow code.
    if re.search(r"[^\w\s'().=!-]", condition):
        raise ValueError(f"unexpected gate expression: {condition}")
    return bool(eval(condition, {"__builtins__": {}}, {}))


class ReleaseWorkflowTests(unittest.TestCase):
    def test_windows_packaging_preserves_msvc_path_in_release_and_snapshot(self) -> None:
        for workflow in (WORKFLOW.with_name("native-package.yml"),):
            with self.subTest(workflow=workflow.name):
                content = workflow.read_text(encoding="utf-8")
                linux_macos = re.search(
                    r"      - name: Package native application\n(.*?)(?=      - name:)",
                    content, re.S,
                ).group(1)
                windows = re.search(
                    r"      - name: Package native application \(Windows MSVC\)\n(.*?)(?=      - name:)",
                    content, re.S,
                ).group(1)
                self.assertIn("if: runner.os != 'Windows'", linux_macos)
                self.assertIn("shell: bash", linux_macos)
                self.assertIn("if: runner.os == 'Windows'", windows)
                self.assertIn("shell: pwsh", windows)
                self.assertIn("Get-Command link.exe -CommandType Application", windows)
                self.assertIn("Select-Object -First 1).Source", windows)
                self.assertNotIn("Application -ErrorAction Stop).Source", windows)
                self.assertIn(r"\\VC\\Tools\\MSVC", windows)
                linker_pattern = re.search(
                    r"if \(\$linker -notmatch '([^']+)'\)", windows
                ).group(1)
                for arch in ("x64", "arm64"):
                    with self.subTest(arch=arch):
                        msvc = rf"C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Tools\MSVC\14.51.36231\bin\HostX64\{arch}\link.exe"
                        self.assertRegex(msvc, re.compile(linker_pattern, re.I))
                self.assertIsNone(re.search(linker_pattern, r"C:\Program Files\Git\usr\bin\link.exe", re.I))
                self.assertIn('python scripts/release/package_native.py "$env:TARGET"', windows)
                self.assertIn("if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }", windows)

    def test_stable_windows_packages_install_pinned_wix_and_sign_msi(self) -> None:
        package = WORKFLOW.with_name("native-package.yml").read_text(encoding="utf-8")
        install = re.search(r"      - name: Install WiX\n(.*?)(?=      - name:)", package, re.S).group(1)
        self.assertIn("runner.os == 'Windows' && inputs.release_type == 'release' && !contains(inputs.version, '-')", install)
        self.assertIn("./scripts/ci/install-wix.ps1", install)
        preparation = jobs()["prepare_release"]
        self.assertIn('if [[ "${PRERELEASE}" == \'false\' ]]; then', preparation)
        for architecture in ("x64", "arm64"):
            self.assertIn(f'"dist-release/NyaTerm_${{VERSION}}_windows_{architecture}.msi"', preparation)
        self.assertIn('".msi": "application/x-msi"', (WORKFLOW.parents[2] / "scripts/release/release_io.py").read_text(encoding="utf-8"))
        installer = WORKFLOW.parents[2] / "scripts/ci/install-wix.ps1"
        self.assertIn("$version = '6.0.2'", installer.read_text(encoding="utf-8"))
        snapshot = WORKFLOW.with_name("main-snapshot.yml").read_text(encoding="utf-8")
        self.assertNotIn("install-wix", snapshot)

    def test_rehearsal_never_enables_publication(self) -> None:
        blocks = jobs()
        for publish, verify, package, reuse, expected in (
            (False, False, "success", "", False),
            (False, True, "success", "", True),
            (False, True, "skipped", "123", True),
            (False, True, "failure", "", False),
            (True, False, "success", "", True),
        ):
            with self.subTest(publish=publish, verify=verify, package=package):
                values = {
                    "needs.preflight.result": "success",
                    "needs.preflight.outputs.publish": str(publish).lower(),
                    "inputs.verify_release": verify,
                    "needs.package.result": package,
                    "needs.focal.result": package,
                    "inputs.source_run_id": reuse,
                    "needs.release.result": "success",
                    "needs.prepare_release.result": "success",
                }
                self.assertEqual(gate(blocks["prepare_release"], values), expected)
                self.assertEqual(gate(blocks["release"], values), publish)
                self.assertEqual(gate(blocks["r2"], values), publish)

    def test_signing_job_has_no_publication_commands(self) -> None:
        preparation = jobs()["prepare_release"]
        for forbidden in ("gh release create", "gh release upload", "gh release edit", "aws s3", "git push", "gh workflow run"):
            self.assertNotIn(forbidden, preparation)
        self.assertIn("needs: [preflight, prepare_release]", jobs()["release"])

    def test_decoded_signing_key_is_not_persisted_in_step_environment(self) -> None:
        preparation = jobs()["prepare_release"]
        self.assertNotIn('>> "${GITHUB_ENV}"', preparation)
        self.assertIn("unset TAURI_SIGNING_PRIVATE_KEY", preparation)

    def test_later_checkouts_cannot_follow_a_moving_branch(self) -> None:
        blocks = jobs()
        for name in ("prepare_release", "release", "r2", "publish"):
            self.assertIn("ref: ${{ needs.preflight.outputs.source_sha }}", blocks[name])
        self.assertIn("source_sha: ${{ needs.preflight.outputs.source_sha }}", blocks["package"])
        self.assertIn("source_ref: ${{ needs.preflight.outputs.source_sha }}", blocks["focal"])
        self.assertIn("uses: ./.github/workflows/native-package.yml", blocks["package"])
        snapshot = WORKFLOW.with_name("main-snapshot.yml").read_text(encoding="utf-8")
        self.assertIn("uses: ./.github/workflows/native-package.yml", snapshot)
        self.assertIn("cancel-in-progress: false", snapshot)

    def test_downstream_channels_use_the_shared_published_release_resolver(self) -> None:
        publication = jobs()["publish"]
        self.assertLess(publication.index("release_channel.py commit"), publication.index("release_receipt.py create"))
        self.assertLess(publication.index("release_receipt.py create"), publication.index("name: published-release"))
        self.assertIn("name: published-release-tag", publication)
        for filename, channel in (("homebrew", "homebrew"), ("aur", "aur"), ("gitee-release", "gitee")):
            workflow = WORKFLOW.with_name(f"publish-{filename}.yml").read_text(encoding="utf-8")
            self.assertIn(f"release_receipt.py resolve --channel {channel}", workflow)
            self.assertIn("ref: ${{ github.workflow_sha }}", workflow)
            self.assertNotIn("Publish Cloudflare R2 mirror", workflow)
        gitee = WORKFLOW.with_name("publish-gitee-release.yml").read_text(encoding="utf-8")
        self.assertIn("github.event_name != 'workflow_dispatch' || inputs.upload_small_assets", gitee)

    def test_publication_lock_covers_channel_commit_and_receipt(self) -> None:
        publication = jobs()["publish"]
        self.assertIn("group: release-channel-", publication)
        self.assertIn("cancel-in-progress: false", publication)
        self.assertLess(publication.index("Retain preimage before"), publication.index("release_channel.py commit"))
        self.assertIn("if: always()", publication)
        # Parallel builds, including manual runs from main, must not cancel one another.
        self.assertNotRegex(WORKFLOW.read_text(encoding="utf-8"), r"(?m)^concurrency:")
        self.assertNotIn("cancel-in-progress: true", WORKFLOW.with_name("linux-focal.yml").read_text(encoding="utf-8"))

    def test_recovery_can_verify_a_candidate_from_an_interrupted_publication(self) -> None:
        preflight = jobs()["preflight"]
        self.assertIn('[[ "${source_status}" == \'completed\' ]]', preflight)
        self.assertNotIn("source_conclusion", preflight)
        preparation = jobs()["prepare_release"]
        self.assertIn("name: release-candidate", preparation)
        self.assertIn("scripts/release/verify_candidate.py", preparation)

    def test_draft_notes_are_preserved_until_publish_gate(self) -> None:
        blocks = jobs()
        # A GITHUB_TOKEN with contents:read cannot see unpublished draft releases.
        for name in ("preflight", "prepare_release", "release", "publish"):
            self.assertIn("contents: write", blocks[name])
        self.assertIn("Wait for populated draft release", blocks["preflight"])
        self.assertIn("gh release view", blocks["prepare_release"])
        self.assertIn("--json body --jq '.body'", blocks["prepare_release"])
        self.assertNotIn("gh release create", blocks["release"])
        self.assertNotIn("body=@release-notes.md", blocks["release"])
        self.assertNotIn("--draft=false", blocks["release"])
        self.assertIn("release_channel.py commit", blocks["publish"])
        values = {
            "needs.preflight.result": "success",
            "needs.preflight.outputs.publish": "true",
            "needs.release.result": "success",
            "needs.r2.result": "success",
        }
        self.assertTrue(gate(blocks["publish"], values))
        values["needs.r2.result"] = "failure"
        self.assertFalse(gate(blocks["publish"], values))

    def test_recovery_lints_triggering_workflow_not_frozen_checkout(self) -> None:
        preflight = jobs()["preflight"]
        self.assertIn('for _ in {1..36}; do', preflight)
        self.assertIn('git show "${GITHUB_SHA}:.github/workflows/release.yml"', preflight)
        self.assertIn("docker run --rm", preflight)

    def test_auto_discovered_historical_compatibility_fixtures_have_git_history(self) -> None:
        for filename, job_name in (("rust.yml", "python"), ("release.yml", "preflight"), ("main-snapshot.yml", "prepare")):
            content = WORKFLOW.with_name(filename).read_text(encoding="utf-8")
            checkout = content.split(f"\n  {job_name}:\n", 1)[1].split("      - name: Install Python", 1)[0]
            with self.subTest(workflow=filename):
                self.assertIn("fetch-depth: 0", checkout)

if __name__ == "__main__":
    unittest.main()
