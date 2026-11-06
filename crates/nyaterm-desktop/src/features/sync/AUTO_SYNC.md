# Automatic synchronization write audit

Audited `store_mutation`, `request_mutation_fn`, `store_request`, `request_fn`,
typed `StoreRequest` implementations, and settings broadcasts on 2026-11-06.
The request marker means **candidate local configuration change**, not proof
that the active protocol/scope has something to publish.

| Operation | Sync impact | Handling |
| --- | --- | --- |
| Saved connection usage, including reconnect and system RDP launch | Runtime activity | Ordinary request; persist usage and refresh the catalog |
| Monitoring asset flush on session close | Runtime activity | Ordinary request; preserve operator asset fields |
| Automatic connection icon | Local observation | Ordinary request; transaction updates only the icon, rechecks automatic mode and preserves current configuration/password |
| Manual connection/icon/group edits, moves, imports, copies and deletes | Portable configuration | Candidate mutation |
| Quick command execution count, command/completion history | Runtime activity | Ordinary request |
| Quick command/category edits, reorder, imports and AI card saves | Portable configuration | Candidate mutation |
| Proxy/tunnel/group edits and auto-open configuration | Portable configuration | Candidate mutation; runtime connection/open state is excluded by V4 projection |
| Note/folder create, edit, move and delete | Portable configuration | Candidate mutation |
| Notes selection, expansion and editor/window state | Local UI | Ordinary request |
| Open tabs, pane/window/workspace manifests and shutdown restoration | Local UI | Ordinary request, including typed manifest requests |
| Immediate UI layout saves | Local UI | Ordinary request; broadcasting settings must not restart synchronization |
| Other settings domains, theme mutations, keyword rules and translation settings | Protocol-dependent; mixed local/portable values | Candidate mutation; protocol preflight filters excluded/no-op changes |
| AI configuration | Optional V4 AI/secret scope | Candidate mutation; runtime discovery and permissions stay governed by the existing schema |
| AI chats, audit and agent messages | Local history | Ordinary request |
| Credential/key/password/OTP CRUD and authenticated credential saves | Optional V4 secret scope | Candidate mutation; ordinary decrypt/list operations remain reads |
| SSH/RDP remembered trust | Optional V4 trusted-host scope | Candidate mutation |
| VNC authenticated remembered trust | Optional V4 trusted-host scope | Candidate mutation (previously missed); retain cancellation/current-key checks |
| Portable backup import and legacy sync application | Bulk replacement | Keep existing candidate notifications and replacement guards |
| V4 Apply, publication receipts, sync history/state, OAuth token refresh and password rotation bookkeeping | Synchronization maintenance | Existing ordinary requests; no automatic feedback loop |
| Final shutdown settings flush | Durability/recovery | Keep ordinary shutdown request; next startup checks persisted changes |
| Cross-window ordinary/theme settings broadcast | State propagation | Does not reset retry state or enqueue a Settings sync |
| Committed cloud configuration or master-password change | Explicit sync reconfiguration | Enqueue Settings sync, retaining authentication recovery and scope/destination changes |

## Existing protocol boundaries

| Data | V3 | V4 |
| --- | --- | --- |
| Connections, groups, icons, commands/categories, notes/folders, proxies/tunnels and their groups | Existing portable entities | Core resource/field allowlists; device bindings and runtime observations excluded |
| Ordinary settings | Existing `project_sync_settings` projection; cloud configuration, keybindings and listed device settings excluded | Only existing appearance preference fields when `preferences` is enabled; themes remain core resources |
| Keyboard shortcuts | Excluded | Existing supported bindings when `keybindings` is enabled |
| AI configuration | Existing settings snapshot | Existing AI resource allowlists when `ai` is enabled; local permissions remain local |
| Vault credentials, keys, OTP and bound passwords/API secrets | Existing encrypted snapshot contract | Existing secret value/reference projection when `secrets` is enabled, independently of AI configuration |
| SSH and VNC remembered trust | Existing known-host entities | Existing trust projection when `trusted_hosts` is enabled |
| Translation configuration/secrets | Existing settings snapshot | No resource in the current V4 schema; candidate is skipped if nothing else changed |
| History, layouts and monitoring | Keep released snapshot field selection, including runtime fields retained inside catalog records | No projected runtime changes |

Audit entry points are the connection CRUD/import/icon modules, command runtime
and editors, notes runtime/editor, tunnel runtime, session start/auth/dialog
runtime, remote-desktop trust/events, settings draft/immediate/theme/security
jobs, AI/translation writers, shell persistence, controller broadcasts, and the
Store runtime's typed window/restore requests and legacy cloud application.
Mixed closures remain candidates rather than dropping a credential/configuration
change merely because the same operation also updates local state.

## Preflight boundary

Change candidates are coalesced by the existing debounce. V3 retains its
released snapshot content hash, field selection and wire format. Runtime
fields still present in a V3 snapshot may accompany a later real sync; this
optimization does not redefine that compatibility contract.

V4 loads the current local view before constructing any provider adapter.
It checks the current scope, baseline, field edit journal, local capture and
publication receipts. No-op or excluded writes return `Skipped` without network
access or a sync status/history entry. The local view is reused if remote work
is needed, and Apply still verifies the record and database generation.

An absent baseline, migration, scope change, unpublished/pending publication,
deferred application or known remote work prevents the shortcut. Reverted
edits retain their causal journal semantics. Startup, focus, periodic, Settings,
retry and explicit synchronization continue to inspect the remote normally.

The durable V4 generation also protects local bindings against concurrent
Apply; it is deliberately not an upload-dirty flag. A skipped job only drains
its captured candidate generation, preserves newer candidates and does not
report network recovery or postpone periodic checks. No table, serialized
field, encryption contract or backup format changes are required.

## Validation (Windows x86_64, 2026-11-06)

| Check | Result |
| --- | --- |
| `cargo test -p nyaterm-store` | 239 passed, 1 existing opt-in test ignored |
| `cargo test -p nyaterm-desktop change_tests -- --nocapture` | All 5 V4 preflight tests passed |
| Filtered desktop coordinator, command runtime, persistence debounce and MSTSC tests | 32 passed on Windows |
| `cargo check --workspace` | Passed |
| `cargo test --workspace --no-fail-fast -j 1` | 5509 passed, 0 failed, 44 ignored across 60 test/doc-test targets |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --workspace --all-targets` | Passed without warnings |
| Final raw unknown-field icon test and `cargo clippy -p nyaterm-store --all-targets -j 1` | Passed after strengthening the compatibility fixture |
| `git diff --check` | Passed |

The first unrestricted workspace test build exhausted Windows commit/page-file
capacity (`os error 1455`, allocation failures). Limiting build parallelism to
one completed the full suite; no remaining assertion or compiler failures were
observed. Existing ignored tests, including external cloud, interactive ConPTY
and manual scale workloads, remain opt-in. Windows GPUI state/lifecycle tests
ran here; interactive native-window smoke testing was not performed.

An existing fixture limitation was observed: the raw default appearance blob
uses `font_size: 16.0`, while V4 preference validation accepts integral font
sizes. The new fixtures use the ordinary typed appearance saver, matching the
current settings form. This change does not alter that existing validation or
legacy data policy.
