# Desktop composition and extraction boundaries

Desktop owns application composition, authoritative feature state, credentials,
persistence, job scheduling, cancellation, approvals, and session/window lifetime.
Presentation crates consume projections, caller-owned input/text entities and typed
callbacks. They do not acquire an application or database dependency.

## Implemented extraction

| Destination | Responsibility moved from desktop | Desktop boundary |
| --- | --- | --- |
| `nyaterm-transport::networktools::runtime` | Network-tools worker, commands, results and lifecycle tests | Instance ownership and shutdown coordination |
| `nyaterm-terminal::selection::words` | Word/cell classification | Selection/session routing |
| `nyaterm-terminal-gpui::{selection,wheel_zoom}` | Selection scrolling helpers and wheel zoom | Terminal interaction and app settings |
| `nyaterm-ui::{overlay,presentation}` | Overlay input layers, dialog primitives and common formatting/input helpers | Native window policy and feature composition |
| `nyaterm-remote-monitor-gpui` | Stats/GPU/NPU/process/Docker panels, immutable projections, list state, clocks and views | SSH leases, host identity validation, data/query caches, jobs, confirmation and details-window coordination |
| `nyaterm-file-browser-gpui` | Breadcrumb/path projection, entry cells, queue rendering, preview bodies/tabs/toolbar and editor surface | Pane/navigation state, drag/selection routing, frozen transfer targets, file I/O, preview caches/jobs and document lifetime |
| `nyaterm-ai-gpui` | Presentation phases, composer geometry/controls, message bubbles, tool-call frames and approval cards | Transcript indexing/text caches, runtime, history, inputs and approval identity validation |
| `nyaterm-notes-gpui` | Tree rows/drag previews/menus, destination projection, Markdown transforms and editor controls | Catalog, move validation, input/document state, dirty/conflict state, save debounce and close handling |
| `nyaterm-connections-gpui` | Display contracts, editor frame/form components, saved-connection rows and safe detail/share cards | Catalog, selection/drag routing, drafts, credentials, discovery, validation and saving |

All five crates are registered in the workspace and selective CI job routing.
`scripts/ci/check_architecture.py` checks their dependency allowlists, including
renamed, development, build and platform-specific dependencies. The monitor's
`test-support` is default-off and provides controlled observations and operations.

## Remaining desktop composition

This extraction does not relocate whole feature controllers into presentation
crates. `AiPanel`, `NotesPanel`, `ConnectionPanel`, `FileManagerWorkspace` and child
window controllers remain in desktop. They still compose some feature-specific
headers, menus, forms and layout around extracted elements. Further extraction
can move those views as their projection and callback boundaries become useful;
it must not move drafts, caches or approvals merely to reduce file size.

Remote-monitor actions validate both the rendered host token and the current
concrete host identity before operating. AI approval callbacks retain the original
run, AI session and request identity. Notes and preview actions return to existing
controllers. No database schema, serialized field, encryption prefix, backup
format, or patched dependency revision is changed by this extraction.

## Validation

Use the package tests while iterating, then run the repository architecture checks,
`cargo check --workspace`, `cargo test --workspace`, `cargo fmt --all -- --check`,
and `cargo clippy --workspace --all-targets`.

On Windows, run `scripts/release/prepare_android.py` and export its reported
`FFMPEG_DIR`, adding its `bin` directory to `PATH`, before workspace helper builds
and tests. The extracted monitor's GPUI debug rendering tests require a larger
thread stack on this platform (`RUST_MIN_STACK=16777216`); the default stack was
insufficient. The same login-user popover test also overflowed in an existing
pre-extraction desktop test binary with the default stack. Automated rendering
and input tests do not replace manual native UI smoke testing.
