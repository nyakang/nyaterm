# Contributing

Thank you for your interest in contributing to NyaTerm.

## Before you start

1. Read `AGENTS.md` and `CONTRIBUTING.md` at the repository root.
2. Follow [Development Setup](./setup) to configure Rust and platform dependencies.
3. Check [Issues](https://github.com/nyakang/nyaterm/issues) for the expected behavior and current work.

## Choose the owning crate

- Pure models, parsing, compatibility formats, and policies belong in `nyaterm-core`.
- Database execution and compatibility readers belong in `nyaterm-store`.
- PTY, SSH, SFTP, Telnet, Serial, tunnel, and transfer runtimes belong in `nyaterm-transport`.
- Terminal state and snapshots belong in `nyaterm-terminal`; GPUI terminal painting belongs in `nyaterm-terminal-gpui`.
- GPUI state, views, and background coordination belong in `nyaterm-desktop`.
- Shared GPUI controls and theme integration belong in `nyaterm-ui`.
- RDP/VNC session management, input models, and IPC contracts belong in `nyaterm-remote-desktop`; protocol decoders belong only in `nyaterm-rdp-helper` and `nyaterm-vnc-helper`.

Cross-crate changes keep adapters small and explicit, with one authoritative owner for each value.

## Connection editor parity

The connection editor and the asset batch editor must keep the same surface. When adding, removing, or renaming a connection setting:

- add the field to the connection editor section that owns it;
- give the batch editor a row with the same label, the same section, and the same default value used for a new connection;
- keep `BATCH_FIELDS_BY_SECTION` exhaustive; a batch field that no protocol section renders is a parity bug;
- run the `editor_surface_parity` tests in `nyaterm-desktop`, which check field coverage, section reachability, and that `Restore default` writes the value a new connection starts with.

Protocol-specific editor sections compose the shared `editor_surface` primitives. Do not paint a private control shell or copy a control layout into a batch section.

## Contribution workflow

1. Fork the repository and create a branch from `main`.
2. Implement the change in the owning crate and add adjacent tests.
3. Run checks for the affected crate, then the relevant workspace checks.
4. Commit with a Conventional Commit-style subject.
5. Push the branch and open a Pull Request.

```bash
git checkout -b feat/my-feature
cargo check -p <crate-name>
cargo test -p <crate-name>
```

## Commit convention

Use this subject format:

```text
<type>(<scope>): <imperative summary>
```

Examples:

```text
feat(terminal): add search result navigation
fix(transport): handle closed SSH channels
docs: update development setup
```

Common types include `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, and `chore`. Common scopes include `terminal`, `transport`, `desktop`, `storage`, `ui`, `ai`, and `sync`.

## Code and tests

```bash
cargo check --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
```

- Use Rust 2024 idioms, standard rustfmt formatting, and explicit imports.
- Do not add `#[path = "..."]`, `use super::*`, or a shared feature prelude.
- Do not perform database, filesystem, network, or other blocking work in render paths.
- Storage, credential, encryption, backup, and sync changes include new-data round trips and representative legacy-data tests.
- Platform-specific window, PTY, Serial, clipboard, and input behavior is verified on the target operating system.

## Internationalization and documentation

When adding or changing application UI text, update both:

- `crates/nyaterm-i18n/locales/zh-CN.json`
- `crates/nyaterm-i18n/locales/en.json`

For docs-site changes, keep the Chinese source under `docs-site/docs/` and the English pages under `docs-site/i18n/en/docusaurus-plugin-content-docs/current/` in sync, add any new page to `docs-site/sidebars.ts`, then run:

```bash
python3 scripts/ci/check_docs_translations.py
pnpm --dir docs-site build
```

## Changing third-party dependencies

The third-party dependencies NyaTerm patches are not in this repository. Each is a patch series on a fork under [github.com/nyakang](https://github.com/nyakang) on branch `nyaterm`. The workflow is documented in [Development Setup → Changing third-party dependencies](./setup#changing-third-party-dependencies).

In short: commit to the fork branch and push, then bump the pinned revision in the root `Cargo.toml`; keep the patch series split by concern; record the reason and the validation performed on the patch commit and in that branch's `NYATERM.md`. Note the fork branch and revision in your PR description.

**Do not edit `temp/`.** Those are read-only copies that are never compiled — editing them has no effect and produces no error.

## Security and compatibility

Never commit or log passwords, private keys, OTP values, API secrets, or unredacted terminal context. Persistence changes preserve existing tables, keys, field names, encryption prefixes, backup formats, and fallback behavior unless they include a tested migration.

## License

Contributions are licensed under the project's [Apache License 2.0](https://github.com/nyakang/nyaterm/blob/main/LICENSE).
