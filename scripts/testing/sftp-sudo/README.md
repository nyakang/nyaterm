# SFTP sudo validation

Run `python3 scripts/testing/sftp-sudo/run.py` from a Linux checkout with Docker.
The runner builds a disposable Ubuntu OpenSSH/sudo image, generates a fresh
password in memory, publishes SSH on a random loopback port, and removes each
container in `finally`. It never changes host accounts or host sudo policy.
Only `/fixture` inside the containers and disposable local directories are used.

The matrix covers password sudo, NOPASSWD, denied sudo, requiretty,
internal-sftp only, a missing external sftp-server, and MaxSessions=1.
Positive cases verify root directory access, downloads, editor conflict detection,
UID/GID and mode preservation, pooled directory transfers, independent source
and target identities, move protection, mode-bit write heuristics, and revocation
of old service clones and export readers. No privileged sftp-server process may
remain after cleanup. Negative cases retain ordinary access where supported.

Automatic fixtures:

- `cargo test -p nyaterm-transport --test ssh_sudo`: fragmented prompts, exactly
  one password write, passwordless streams, preserved stdout, separate stderr,
  cancellation, a 20-second authentication timeout, bounded output, typed errors,
  exit status after EOF, and shared connection survival.
- `cargo test -p nyaterm-files --test sftp_sudo`: separate caches in ordinary and
  compatibility modes, renewed grants, old clones, and no fallback on failure.
- `cargo test -p nyaterm-desktop --lib remote_files` and
  `cargo test -p nyaterm-desktop --lib privilege_runtime`: state transitions,
  automation isolation, transfer ownership, late results, busy tasks, secret
  input cleanup, focus restoration, lock cancellation and pane invalidation.

When running the whole suite as root on Linux, permission tests need ordinary
DAC checks. Use `setpriv --bounding-set=-dac_override,-dac_read_search cargo test
--workspace --no-fail-fast`; this changes only the test process capabilities.

The implementation began with the existing architecture-check failure
`raw_scroll_container` in `overlays_favorites.rs`. No allowlist was changed.
Existing unrelated tmux work was left intact.

Windows and macOS native password-dialog focus and system drag-export behavior
require verification on those operating systems. Linux GPUI interaction is
covered by GPUI's test window; a physical desktop interaction pass remains
separate from those tests.

## Recorded validation: 2026-11-12, Linux

| Check | Result |
| --- | --- |
| Disposable OpenSSH/sudo matrix | All seven policies passed, including privileged-channel cleanup. |
| Final transport, files and desktop package tests | 3,700 passed, zero failed; opt-in and platform-specific tests remain ignored by the ordinary Cargo run. |
| `cargo check --workspace` | Passed, including `nyaterm-app`. |
| `cargo clippy --workspace --all-targets` | Passed without warnings. |
| `cargo fmt --all -- --check` | Passed. |
| `python3 scripts/ci/check_architecture.py` | Only the pre-existing `overlays_favorites.rs` raw-scroll violation remains. |

The full `cargo test --workspace --no-fail-fast` run completed. It found a new
translation-key typo, which was corrected, and two failures in existing tests:
`dirty_tab_close_opens_the_confirmation_from_the_rendered_strip` and
`missing_variable_is_cached_until_refresh` (timeout). Both existing tests passed
when rerun individually. The subsequent complete transport/files/desktop test
run, including the corrected translation check and both existing tests, passed
with the totals above. All other workspace test targets passed in the full run.
The complete workspace suite was not repeated after that package-level rerun.

The final package run used:

```sh
setpriv --bounding-set=-dac_override,-dac_read_search \
  cargo test -p nyaterm-transport -p nyaterm-files -p nyaterm-desktop --no-fail-fast
```

Linux native desktop interaction, Windows/macOS password-dialog focus, and
Windows/macOS system drag export remain **pending real-device verification**.
The automated GPUI tests do not substitute for those checks. No persisted data
formats, dependency fork revisions, or files under `temp/` were changed for this
feature.
