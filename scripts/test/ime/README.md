# Linux X11 IME regression checks

Related reports: [#1073](https://github.com/nyakang/nyaterm/issues/1073),
[#1017](https://github.com/nyakang/nyaterm/issues/1017), and
[#1156](https://github.com/nyakang/nyaterm/issues/1156).

#1073 and #1156 explicitly concern X11 input-method integration, but do not
establish one identical failure. #1073 includes both permanent connection loss and
candidate placement; #1156 concerns IBus/Rime stopping after several seconds.
#1017 describes focus recovery with Fcitx, but does not identify its display
server; this fix addresses the matching X11 failure path, pending confirmation of
that reporter's environment. Missing protocol replies and context lifecycle
errors can stall an input method independently of candidate positioning. Reports
without protocol traces still need confirmation in their original environments.

Source review confirmed that an XIM error previously discarded the client without
reconnecting, while repeated native focus events created new contexts without a
balanced lifetime. Context creation could run before the handshake completed, and
`PreeditStart` lacked its required reply. These are addressed separately from
position reporting. The old preedit callback did already report `SpotLocation`;
the defect was not a complete absence of that code. The repair makes reporting
reliable before composition, after context recovery, and when the focused field
changes. Ordinary gpui-kit inputs already notify caret movement, so their source
does not need another positioning implementation.

The repair lives in the `nyaterm` branches of these forks:

| Fork | Pinned revision | Responsibility |
| --- | --- | --- |
| [zed](https://github.com/nyakang/zed) | `d5564651cf235261b2e55e82067a7cd6ab6c9c42` | Retry, handshake deadlines, per-window contexts, balanced focus, preedit replies, style negotiation, and caret reporting |
| [xim-rs](https://github.com/nyakang/xim-rs) | `d422786a20d3da13e610ca8faec6b518dc008f5f` | Private X11 communication-window cleanup, server-loss detection, and test transport access |
| [gpui-kit](https://github.com/nyakang/gpui-kit) | `28560dc7ea4a461362d83a46aa5b6fa1665fdca1` | Align GPUI and transitive XIM pins; no component source changes |

NyaTerm additionally reports changes to the terminal's composition/cursor bounds,
viewport, active session, focus, and scale through GPUI's coordinate invalidation
API. The presentation cache suppresses identical notifications. No persistence
format or credential contract changes are needed.

## Automated checks

From this repository:

```sh
RUST_MIN_STACK=16777216 cargo test -p nyaterm-desktop --lib --locked ime
cargo check --workspace --locked
RUST_MIN_STACK=16777216 cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked
```

The larger test-thread stack avoids stack overflow in debug GPUI presentation
tests; it is a test-runner setting, not an application runtime requirement.

From a checkout of the pinned Zed fork:

```sh
cargo test -p gpui_linux --lib xim
cargo build -p gpui_platform --example xim_input --features x11
python3 script/test-xim.py --probe target/debug/examples/xim_input --engine fcitx --late-start --output /tmp/nyaterm-xim-fcitx
python3 script/test-xim.py --probe target/debug/examples/xim_input --engine fcitx --on-the-spot --output /tmp/nyaterm-xim-fcitx-callback
python3 script/test-xim.py --probe target/debug/examples/xim_input --engine ibus --late-start --output /tmp/nyaterm-xim-ibus
```

The native fixture requires Xvfb, Openbox, xdotool, D-Bus, and either
fcitx5 with pinyin/classicui or IBus with Rime and the `luna_pinyin` schema data.
It creates a private display, D-Bus session, and XDG configuration. Each run
checks Chinese commits, 100 focus switches, service restart without application
restart, and 60 seconds of continuous input. Fcitx runs also measure candidate
placement at two caret positions. Output contains synthetic-input counters, not
user text. The probe uses two real GPUI windows, not the full NyaTerm UI.

From the pinned xim-rs checkout, using a disposable X server:

```sh
cargo test --lib --features x11rb-client,x11rb-xcb
XIM_TEST_DISPLAY=:173 cargo test --features x11rb-client,x11rb-xcb --test x11_lifecycle -- --ignored
```

The lifecycle fixture checks root child counts after repeated failed and successful
connections and observes server destruction. `:173` is an example; supply the
display of the disposable server you started.

## Native validation recorded on 2026-11-28

All 67 GPUI Linux library tests and the fork's `script/clippy` checks passed in a
scoped source-copy workspace. The native fixture passed with Fcitx5 5.1.19 in both
default and callback-preedit configurations and with IBus 1.5.34-rc2/Rime 1.6.0.
Default Fcitx5 advertises server-rendered preedit, so negotiating only callback
styles would break that configuration. Both styles are covered.

In NyaTerm, the application check and formatting check passed. The desktop test
filter above passed 959 tests, including the new anchor observation test and the
existing composition cleanup, painted/native caret alignment, and fractional
horizontal-scroll geometry tests. The filter also matches unrelated `runtime`
module names, so this count is broader than IME-only coverage.

The final workspace check and Clippy check passed (Clippy reported warnings outside
the IME changes). A shared full-workspace test run exposed default-stack overflows
in five GPUI test targets and a file-permission test that expects an unprivileged
user. The smaller UI targets passed with the larger stack, and the permission
case passed as `nobody`. The full desktop rerun completed with 3217 passing tests,
22 ignored tests, and five failures. Two failures passed in isolated reruns using
a stable test-binary path, while these three assertions remained reproducible:

- `features::assets::batch_edit::tests::batch_field_table_covers_every_editable_field_once`
- `features::assets::batch_edit::tests::editor_surface_parity`
- `features::pages::settings::workspace::connection_types::tests::connection_types_default_badge_follows_first_visible_and_supports_drag_reorder`

The first two report `SshProfile is not applicable to Mosh`; the third cannot find
the expected local-connection default badge. Their source files are outside this
IME change. The full suite therefore is not reported as green.

These runs establish X11 protocol behavior under Xvfb/Openbox. They do not replace
acceptance on KDE/GNOME, the reporters' exact input-method versions/schemas, or
physical multi-monitor setups. Before release, also exercise the actual NyaTerm
terminal and quick-command search: cold start, switching between fields and apps,
moving/resizing/minimizing the window, and input-method restart. Use an isolated
`NYATERM_DATA_DIR`. Check scale changes and negative monitor coordinates, then
smoke-test native Wayland, Windows, and macOS input paths.
