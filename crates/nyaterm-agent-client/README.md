# External Agent ACP migration

## Rollout status

This change introduces the ACP v1 backend, instance-based Agent configuration and desktop integration. It does **not** satisfy the final removal gate for the old protocol clients.

| Strategy | Adapter | Engine | Execution admission |
| --- | --- | --- | --- |
| Claude | claude-agent-acp 0.110.0 | Claude Code 2.1.330 (SDK 0.3.330) | Windows strict MCP-only policy; exact adapter and engine checks |
| Codex | @agentclientprotocol/codex-acp 2.10.0 | Codex 0.171.0 | Windows/Linux; inherited MCP servers disabled, native tools and startup hooks excluded |
| Unknown/custom | Saved verbatim | Unknown | Disabled |

Codex ACP is enabled on Windows and Linux. Unix children now start in an owned process group; cleanup kills that group before joining pipe readers. Claude on Linux and all adapters on macOS remain gated pending platform acceptance.

Users install adapters themselves. Nothing in the product downloads an engine. The acceptance installation under `target/acp-validation` is a local test fixture, not a runtime dependency. Extra startup arguments are retained in configuration but refused by the admitted strategies. Npm and bundled Claude engine layouts are recognized; unrecognized layouts/versions fail closed instead of using an arbitrary CLI from PATH.

## Architecture and ownership

- `nyaterm-core::ai::external_agents`: profile identity, secret environment values, legacy migration, compatibility aliases and session bindings. No UI or I/O.
- `nyaterm-agent-client::acp`: Rust SDK 5.5.1, discovery, exact-version policy, bounded protocol transport, authentication, session setup, model selection and cancellation. Child processes remain owned by the existing process-tree guard.
- Desktop: profile editing, worker scheduling, generation validation, MCP credentials/approvals, transcript persistence and presentation. The internal NyaTerm agent keeps its own runtime.
- Store: encrypted environment values using the existing credential/master-key formats and portable backup/sync path. No database table or key was renamed. ACP launch profiles are device-local in the current field-level cloud-sync schema (as the old Codex/Claude settings were); portable backup and legacy raw snapshot paths retain their encrypted values.

The old `codex`, `claude_code`, `account`, and `probe` clients are retained until the full acceptance gate passes. Desktop chat dispatch uses ACP for external agents; it never silently falls back to a private protocol when ACP fails. Codex now has a version-pinned execution path; the remaining migration and platform gates below still apply before removing the private-protocol clients.

## Policy details

ACP is a communication protocol, not a sandbox. The pinned Claude strategy removes native tools (`tools: []`), inherited settings sources and extra MCP servers, disables hooks and tool search, refuses bypass mode, and explicitly selects the advertised `default` approval mode before any prompt (including restored sessions). The adapter overwrites SDK `permissionMode`, so setting that SDK option alone is insufficient.

MCP servers and settings are sent through SDK initialization over stdin (`mcpServersDelivery` / `settingsDelivery`: `initialize`), not command-line JSON. The host chooses the verified engine; inherited engine overrides and Node preload settings are removed. No stderr, prompts, tokens, account identity or raw protocol errors are logged.

ACP permission handling accepts only structured NyaTerm MCP identities for the active session and only `allow_once`. Actual capability/terminal approval remains in the NyaTerm MCP host. File and terminal ACP client capabilities are not advertised. Cancel/timeout revoke the request credential before sending cancellation and reaping the child tree. A periodic timer continues cancellation checks under a stream of notifications.

Codex admission pins both adapter 2.10.0 and engine 0.171.0. Before each launch, a bounded app-server `config/read` resolves the effective configuration and its layers, including untrusted project layers that ACP will make trusted. Every inherited MCP server is explicitly disabled through Codex's per-server `enabled` setting. The request's helper and credentials use the reserved `nyaterm_embedded` identity; a pre-existing definition of that identity fails admission rather than merging its transport or environment. Settings, authentication files and existing CLI MCP entries are not rewritten.

A private, temporary launcher applies native-tool, plugin, hook and notification restrictions before app-server initialization, as well as at session setup. It contains only the verified engine path and static restrictions. Read-only mode is explicitly selected on both new and resumed sessions. Configuration is resolved again on each launch; this adapter policy is not an OS sandbox against another process concurrently modifying the user's configuration.

Codex permission requests reference partial tool updates. The client records pending calls only from structured notifications identifying `nyaterm_embedded`, requires the adapter's `is_mcp_tool_approval` marker, and consumes the matching ID once. Only `allow_once` is selected. Native approvals, unknown IDs, completed calls and foreign sessions are rejected. This permits the call to reach the NyaTerm MCP host, whose Confirm/Observer/session-scope policy remains authoritative; it does not grant terminal execution itself. Codex's three resource-list/read tools remain available but can only address the enabled NyaTerm server.

The retained private app-server client also accepts only current-thread NyaTerm MCP tool confirmations with an empty form schema. Resuming refreshes the MCP credential and approval policy, fixing #1318 for backports to the pre-ACP integration.

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

The real-adapter test uses a loopback synthetic model and a harmless MCP executable. It inspects the actual model tool catalog, requests the tool, restores the session in a new adapter process, and checks that the second invocation uses the new request's MCP environment. Both variants seed conflicting project MCP and startup-hook configuration and assert neither starts. The `--native` variant deliberately returns a Bash call from the synthetic model and verifies the engine rejects it without executing the marker command. It does not call a paid model or use user credentials. The Codex-specific acceptance probe below covers the restored execution path.

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

Before deleting legacy clients or making this a complete replacement release, require: old-session behavior; model selection; real authentication flows; desktop integration tests and visual verification; the remaining platform coverage. The local Windows synthetic-model tests are evidence for the paths exercised, not an OS sandbox guarantee.

### Codex approval regression validation

```sh
cargo build -p nyaterm-agent-client --examples
python scripts/tests/codex_acp_acceptance.py /path/to/codex-acp
python scripts/tests/codex_acp_acceptance.py /path/to/codex-acp --native
```

The probe uses an isolated configuration, loopback Responses API and synthetic MCP tool. It verifies a real tool call, fresh credentials on a fresh-process resume, rejection of a forced native command, and that inherited public/project MCP servers and a startup hook do not execute. It checks that source configuration files are unchanged. `CARGO_TARGET_DIR` selects examples built outside the workspace target directory. The validated npm adapter resolves its own Codex 0.171.0 dependency; another global Codex version is not substituted. Windows end-to-end UI verification remains required.
