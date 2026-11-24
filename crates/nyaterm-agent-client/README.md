# External Agent ACP migration

## Rollout status

This change introduces the ACP v1 backend, instance-based Agent configuration and desktop integration. It does **not** satisfy the final removal gate for the old protocol clients.

| Strategy | Adapter | Engine | Execution admission |
| --- | --- | --- | --- |
| Claude | claude-agent-acp 0.110.0 | Claude Code 2.1.330 (SDK 0.3.330) | Windows strict MCP-only policy; exact adapter and engine checks |
| Codex | codex-acp 2.10.0 | Not admitted | Inspection only: inherited MCP configuration is merged by the adapter |
| Unknown/custom | Saved verbatim | Unknown | Disabled |

ACP startup on Linux/macOS is currently gated: the existing child guard only reaps a full process tree on Windows. Unix process-group cleanup and platform acceptance must be implemented before enabling it there.

Users install adapters themselves. Nothing in the product downloads an engine. The acceptance installation under `target/acp-validation` is a local test fixture, not a runtime dependency. Extra startup arguments are retained in configuration but refused by the admitted strategies. Npm and bundled Claude engine layouts are recognized; unrecognized layouts/versions fail closed instead of using an arbitrary CLI from PATH.

## Architecture and ownership

- `nyaterm-core::ai::external_agents`: profile identity, secret environment values, legacy migration, compatibility aliases and session bindings. No UI or I/O.
- `nyaterm-agent-client::acp`: Rust SDK 5.5.1, discovery, exact-version policy, bounded protocol transport, authentication, session setup, model selection and cancellation. Child processes remain owned by the existing process-tree guard.
- Desktop: profile editing, worker scheduling, generation validation, MCP credentials/approvals, transcript persistence and presentation. The internal NyaTerm agent keeps its own runtime.
- Store: encrypted environment values using the existing credential/master-key formats and portable backup/sync path. No database table or key was renamed. ACP launch profiles are device-local in the current field-level cloud-sync schema (as the old Codex/Claude settings were); portable backup and legacy raw snapshot paths retain their encrypted values.

The old `codex`, `claude_code`, `account`, and `probe` clients are retained until the full acceptance gate passes. Desktop chat dispatch uses ACP for external agents; it never silently falls back to a private protocol when ACP fails. In particular, existing Codex users must be told that ACP execution is currently blocked. This is a migration candidate, **not a complete replacement release**.

## Policy details

ACP is a communication protocol, not a sandbox. The pinned Claude strategy removes native tools (`tools: []`), inherited settings sources and extra MCP servers, disables hooks and tool search, refuses bypass mode, and explicitly selects the advertised `default` approval mode before any prompt (including restored sessions). The adapter overwrites SDK `permissionMode`, so setting that SDK option alone is insufficient.

MCP servers and settings are sent through SDK initialization over stdin (`mcpServersDelivery` / `settingsDelivery`: `initialize`), not command-line JSON. The host chooses the verified engine; inherited engine overrides and Node preload settings are removed. No stderr, prompts, tokens, account identity or raw protocol errors are logged.

ACP permission handling accepts only structured NyaTerm MCP identities for the active session and only `allow_once`. Actual capability/terminal approval remains in the NyaTerm MCP host. File and terminal ACP client capabilities are not advertised. Cancel/timeout revoke the request credential before sending cancellation and reaping the child tree. A periodic timer continues cancellation checks under a stream of notifications.

Codex cannot be admitted by adapter version alone: session configuration merges local MCP servers, and feature switches do not establish an exclusive NyaTerm tool boundary. `policy::admit_execution` deliberately rejects it before launching a prompt. A reviewed upstream mechanism or adapter change must eliminate that inheritance and pass the tests below before changing this gate. Do not weaken the gate to make a smoke test green.

## Compatibility

Settings schema 8 adds `external_agents`. An absent field migrates legacy Codex and Claude settings exactly once to `builtin-codex` and `builtin-claude`; an empty list means intentionally deleted profiles and is not reseeded. Old executable paths remain legacy engine hints, never ACP adapter commands. Unknown strategy IDs and profile extension fields survive saves. Legacy fields remain serialized for supported-data compatibility.

Environment values remain masked in UI snapshots. Chat and account workers resolve those masks from encrypted storage by profile ID, row ID and variable name, preserving unsaved explicit values. Duplicating a profile clears its environment values and leaves the new instance disabled; credentials must be entered for that instance.

The model input accepts an Agent model ID and validates it against the session’s negotiated model options; changing the model does not invalidate the session binding.

An ACP binding records profile ID, protocol, configuration revision, working directory, configuration directory and external session ID. Launch-affecting edits increment the revision. A binding cannot resume under another instance, revision or directory. Restore uses advertised `session/resume`, falling back to advertised `session/load`; it never retries failed restoration as `session/new`. Replayed messages are not appended again.

Private-protocol sessions have no verified strict launch provenance. Their text and original IDs remain readable, but continuation requires the explicit **New chat from history** action. This opens a reviewable draft in a new local conversation, without sending it or executing command cards. It does not claim to restore the original agent's private state. Disabling history recording also means no external resume binding is persisted.

## Validation and removal gate

Build fixture executables:

```powershell
cargo build -p nyaterm-agent-client --examples
python -X utf8 scripts/tests/acp_adapter_acceptance.py claude PATH_TO_CLAUDE_AGENT_ACP
python -X utf8 scripts/tests/acp_adapter_acceptance.py claude PATH_TO_CLAUDE_AGENT_ACP --native
```

The real-adapter test uses a loopback synthetic model and a harmless MCP executable. It inspects the actual model tool catalog, requests the tool, restores the session in a new adapter process, and checks that the second invocation uses the new request's MCP environment. Both variants seed conflicting project MCP and startup-hook configuration and assert neither starts. The `--native` variant deliberately returns a Bash call from the synthetic model and verifies the engine rejects it without executing the marker command. It does not call a paid model or use user credentials. The Codex invocation intentionally fails admission.

Unit/fixture coverage includes profile migration and deletion, unknown fields, masked secret identity, encrypted environment backup/sync, corruption rejection, instance-bound history, stale account results, replay suppression, foreign-session notifications, native permission denial, cancellation, timeout, EOF, refusals, oversized frames and consumer failure.

On Windows, run the workspace tests with `$env:RUST_MIN_STACK = '8388608'` in the test shell. The default Rust test-thread stack overflowed in the existing remote-monitor test `compact_login_entry_and_popover_fit_narrow_cards_in_both_themes`; increasing the stack allows that test to complete without changing product code.

### Recorded local validation (Windows)

| Check | Result |
| --- | --- |
| `cargo check --workspace` | Passed |
| Workspace unit/integration suites with the 8 MiB test stack | 7,588 passed, 73 ignored |
| Final credential change: core and desktop suites | Core: 1,085 passed including integration tests; desktop: 3,023 passed initially, and the new credential regression passed after correcting its fixture-directory prefix |
| `cargo test -p nyaterm-store` after the credential change | 375 passed, 3 ignored |
| `cargo test --workspace --doc` | 26 passed, 1 ignored |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --workspace --all-targets` | Passed without warnings |
| `python scripts/ci/check_architecture.py` and `git diff --check` | Passed |
| Real Claude adapter, normal MCP + fresh-process resume | Passed |
| Real Claude adapter, forced native Bash + inherited MCP/hooks | Passed; native execution and inherited configuration stayed blocked |

The workspace test invocation completed all unit/integration suites, then its desktop doctest dependency lookup failed while a second Cargo build updated the shared target directory. All workspace doctests subsequently passed when rerun serially. Run Cargo validation commands sequentially. The final credential fix was revalidated in the affected core, desktop and store packages; the fixture-prefix correction changed no production behavior.

Before deleting legacy clients or making this a complete replacement release, require: Codex MCP exclusivity; actual engine compatibility; the equivalent adversarial Codex probes; old-session behavior; model selection; real authentication flows; cancellation with real child descendants; desktop integration tests and visual verification; Linux/macOS coverage. The local Windows synthetic-model tests are evidence for the paths exercised, not an OS sandbox guarantee.
