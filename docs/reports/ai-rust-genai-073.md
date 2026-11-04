# Native AI migration to rust-genai 0.7.3

Implementation branch: `feat/ai-rust-genai-073`, based on `15908b7204ccd05616754fd21a58ea907933a5f5`.

## Dependency boundary

`nyaterm-desktop` uses `genai = "=0.7.3"`, default features disabled, with `rustls-tls` and `system-proxy`. Its separate `reqwest 0.13.5` dependency enables `json`, `query`, `stream`, `socks`, and `system-proxy`. SDK features additionally enable HTTP/2, gzip, charset, and rustls with aws-lc. The existing `zed-reqwest 0.12.15-zed` and other consumers of reqwest remain in place. No existing locked package version was removed or replaced. Bedrock, OTel, and native TLS are not enabled for genai.

SDK types appear only inside `nyaterm-desktop/src/http/ai`. `nyaterm-core::ai::completion` contains SDK-independent, ephemeral generation options, response context, usage, and safe typed errors. There are no new settings or database fields. Generation options and Agent assistant turns use `serde(skip)`; conversation response metadata is held by desktop state only.

## Call chain and lifetime

Previously, native chat and Agent continuations ran synchronous `zed-reqwest` calls and NyaTerm SSE folding inside `BlockingJobScheduler` workers, then forwarded unbounded worker events to GPUI.

`start_ai_ask` / native continuation → `launch_native_chat` → process-owned `AiRuntime` → short `BlockingJobScheduler` preparation → async `stream_chat` → short scheduler result/persistence stage → bounded GPUI drain → existing `AiChatWorkerEvent` state transitions and Agent Harness.

`DesktopController` owns one shared Tokio runtime. The conversation owns a cancellation/abort guard. Native Agent network requests are children of the existing run cancellation token. Dropping the conversation or cancelling the run drops the HTTP future and interrupts channel backpressure. The native stream channel has capacity 64; UTF-8 text, reasoning, and tool-argument fragments are split at 16 KiB. Final captures replace the result projection rather than appending text again. Scope, session, job and native run identity gate delivery.

Each conversation/run holds an immutable provider settings/client snapshot. Editing credentials, endpoints, defaults, or reasoning settings affects subsequent runs. New requests reset the snapshot; Agent continuations reuse it. Removing/disabling the selected credential cancels affected active and inactive conversations when the settings change is applied, including shared settings refresh in other windows. External Codex/Claude Code workers keep their existing protocol.

The snapshot also governs native permission/risk policy, step limits, execution timeout, output limits and history recording. Existing live session execution gates remain authoritative.

## Changed responsibilities

| Location | Responsibility |
| --- | --- |
| `nyaterm-core/src/ai/completion.rs`, AI request/result types, Harness registry | Ephemeral generation options, typed errors, usage and ordered assistant turns; serialization compatibility and redacted diagnostics. |
| `nyaterm-desktop/src/http/ai/{client,async_proxy,request,response}.rs` | Explicit routing, account/client snapshots, authentication, proxy policy and SDK/domain conversion. |
| `nyaterm-desktop/src/http/ai/{adapter,frames,error}.rs` | Async completion/streaming, bounded frame supplements, tool identity/JSON validation, termination and safe errors. |
| `nyaterm-desktop/src/http/ai/{discovery,ollama_compat}.rs` | Dynamic model discovery and the limited native Ollama path. |
| `nyaterm-desktop/src/ai_runtime.rs`, `app_shell/controller.rs` | Shared process runtime and cancellation/abort ownership. |
| `nyaterm-desktop/src/features/ai/{native_jobs,ai_jobs,provider_runtime}.rs`, chat/Harness/state integration | Preparation/network/persistence split, GPUI delivery, per-run snapshots, cancellation, discovery and connection tests. |
| `nyaterm-desktop/src/features/capability_runtime*` | Apply the retained native run policy to tool approval/execution and auditing. |
| `nyaterm-app/src/main.rs` | Global SDK/HTTP log filtering for console and JSONL files, with subprocess tests. |

The old desktop blocking HTTP implementation, SSE reader and proxy/provider modules were removed after differential validation. Pure core wire contracts remain for parity/compatibility tests and the native Ollama response conversion. No architecture allowlist exception was added; no existing AI-specific exception needed removal. `zed-reqwest` remains required by other desktop features.

## Provider routing

| Capability | Backend and retained behavior |
| --- | --- |
| OpenAI / compatible providers | Explicit OpenAI adapter; includes DeepSeek, Groq, xAI, Cohere, MiMo, Z.AI, custom services and Ollama `/v1` routing. Cohere remains on its compatibility protocol. DeepSeek `-none` normalization remains in core. |
| Anthropic | Anthropic adapter; captures ordered native blocks and signed/redacted thinking; replays custom `tool_result` with `is_error`. Thinking-compatible tool choice is `auto`, with the one-decision rule enforced by the Harness. |
| Gemini | Gemini adapter; raw ordered parts preserve function IDs, names, signatures and unknown fields. Synthetic SDK IDs are unique across turns; original provider IDs remain in native continuation parts. |
| Responses | OpenAIResp adapter; `store:false`, encrypted reasoning inclusion, ordered output-item replay and exact call/output association. |
| Ollama native | Ollama adapter for ordinary models. A preselected asynchronous NDJSON compatibility path handles literal `::` model names and extra request parameters that SDK 0.7.3 cannot represent. It never activates after a network failure. |
| Model discovery | Async dynamic discovery; existing protocol URL, pagination, duplicate-cursor guard, deduplication and model-ID contracts. No SDK static catalog replacement. |

`ServiceTarget` and `AuthData::RequestOverride` specify the selected account and exact URL. Existing URL joining preserves paths and query. HeaderMap performs case-insensitive authentication/custom-header merging before SDK handoff. Custom credentials stay on their origin during redirects. System/direct/custom proxy behavior, SOCKS remote DNS, proxy authentication, bypasses, timeouts and user agent are retained. SDK model-name convenience suffix inference is neutralized while restoring the literal wire model; `Auto` does not request reasoning. Historical `None` uses SDK `Zero`. `Ultra` is passed through only where the existing raw protocol represents it, otherwise the request returns an unsupported-capability error.

Streaming uses the SDK decoder and a bounded request-local frame observer, not a second SSE parser. Native terminal evidence is required before accepting synthesized SDK End events. Tool identities, cumulative arguments and final JSON are validated before a completion reaches the Harness. Adjacent unsigned Gemini text fragments with identical metadata coalesce, while signed/tool blocks retain their boundaries. Multiple calls are fully received, then rejected by the existing “exactly one decision” rule; no parallel tool execution is added.

Model POST automatic retries are disabled. JSON tool-protocol fallback requires an explicit unsupported-tools machine code, no generated content and no executed tool, and remains on the same backend. SDK/HTTP diagnostics are blocked at the logging entry point for both console and file sinks, independently of `RUST_LOG`; only safe classifications/status/Retry-After leave the HTTP boundary.

## Validation

Mock fixtures contain no live keys. Before removal, the old Native HTTP baseline passed 17 tests and the SDK/legacy business request/completion comparison passed. The retained comparison now uses the same pure core request/response contracts without the old HTTP backend.

All Cargo validation used `--locked --offline` and the existing shared target directory, after fetching the new registry dependencies. No existing locked package version was removed or replaced; 28 packages were added.

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed. |
| `cargo check -p nyaterm-core`, `cargo check -p nyaterm-desktop` | Passed. |
| `cargo check --workspace` | Passed, including application and both helper crates. |
| `cargo test -p nyaterm-core --lib` | 582 passed, 1 pre-existing ignored. |
| `cargo test -p nyaterm-desktop --lib` (final source) | 2,583 passed, 12 pre-existing ignored. |
| `cargo test -p nyaterm-core --test ai_parity` | 5 passed. |
| AI-related Store tests | 36 passed; full workspace Store library: 233 passed, 1 pre-existing ignored. |
| `cargo clippy -p nyaterm-desktop --all-targets` | Passed; initial new warnings were corrected. |
| `cargo clippy --workspace --all-targets` | Passed with no warnings. |
| `python3 scripts/ci/check_architecture.py`, `git diff --check` | Passed. |
| `cargo test --workspace` with the root-container runner described below | 5,118 passed, 0 failed, 27 pre-existing ignored across unit, integration and doc-test summaries. Includes console/JSONL logging subprocess tests. |
| Latest AI HTTP suite from the workspace-built Desktop test binary (`http::ai`) | 36 passed, including the subsequently added long-Gemini-stream and authenticated SOCKS tests. |
| `cargo test --workspace --no-run` | Passed for the final source, including those two additional tests. |

The first root-run workspace test passed Core, Desktop, app logging, Store and the helper suites, then exposed existing transport test environment assumptions: a read-only file was writable through root's DAC override, and concurrent temporary shell fixtures intermittently reported `ETXTBSY`. Removing DAC privileges for the entire suite is insufficient because another root-specific test deliberately changes file ownership; the system `nobody` account also lacks the home directory required by PTY tests. A root login-shell fixture also exceeded its timeout; selecting `/bin/sh` passed it. The successful workspace run used `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER='python3 /tmp/nyaterm-ai-test-runner.py'` to run transport serially with `SHELL=/bin/sh` in two batches: 687 cases in the ordinary root environment, and `staged_write_preserves_existing_permissions` alone with `setpriv --bounding-set=-dac_override,-dac_read_search` (1 passed). Every test runs; no Cargo feature is disabled. Other workspace test binaries run normally. No transport production/test source was changed for this migration. The unmodified root invocation is not claimed to pass.

Compatibility coverage includes the existing Store tests for legacy embedded AI settings, v6 unknown fields, encrypted/masked provider keys and custom headers, Tauri account/model extensions through reorder/delete/backup, history metadata and unknown fields, and encrypted portable/Tauri snapshots. New ephemeral fields are checked against the old serialized request/Agent contracts.

Mock protocol tests cover all routing matrix entries, account/header/query isolation, same/cross-origin redirects, discovery pagination, explicit reasoning values and thinking budgets, 64-token connectivity requests, typed HTTP errors, incomplete arguments, synthetic End/EOF, UTF-8 splitting, full queues and cancellation, signed/redacted Anthropic turns, stateless Responses items, and Gemini function identity/signature replay. The SOCKS test checks real username/password negotiation and a domain-name CONNECT for an unresolvable `.invalid` origin. Existing Harness/capability tests exercise approval/rejection, risk escalation, step limits, execution and multi-scope cancellation; new state tests cover immutable provider/policy snapshots and provider deletion.

| Platform | Automated evidence and remaining manual checks |
| --- | --- |
| Linux (Ubuntu 26.04, Rust 1.99.0) | Native workspace builds and mock/unit/integration tests below. Still verify actual GPUI windows, closing/switching sessions during live streaming, system proxy settings, and native trust-store integration on a desktop. |
| macOS | No macOS SDK or native build environment available here. Build and run on macOS; check system proxy/Keychain trust, multiple windows, cancellation and application exit. |
| Windows | No Windows MSVC/ATL build environment available here. Build and run with the supported Windows toolchain; check system proxy/bypass, SOCKS authentication, certificate trust, cancellation and application exit. |

Live API account tests remain manual/optional on all platforms; no real provider credential was used. No latency, binary-size or compile-time comparison is claimed.

Reference: [genai 0.7.3 source](https://docs.rs/crate/genai/0.7.3/source/), [Responses function calling](https://developers.openai.com/api/docs/guides/function-calling), [stateless reasoning](https://developers.openai.com/api/docs/guides/reasoning).
