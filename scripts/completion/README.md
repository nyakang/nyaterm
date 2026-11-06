# Offline command catalog

The bundled catalog is generated from the pinned MIT-licensed Carapace source
archive, not from an executable or a live shell. Install the tool dependency with
`python -m pip install -r scripts/completion/requirements.txt`, download
`https://codeload.github.com/carapace-sh/carapace-bin/tar.gz/6bc2b8d69a2474b6eeb626a85ef83254c2ab7d82`, and run:

```
python scripts/completion/generate_catalog.py --archive /path/carapace.tar.gz
python scripts/completion/generate_catalog.py --archive /path/carapace.tar.gz --check
```

The generator verifies the archive hash and emits deterministic UTF-8 JSON,
source metadata including the catalog hash, and the original upstream license.
Unsupported YAML and dynamic values are counted rather than evaluated. Docker's
public root command shortcuts reuse the corresponding `container` declarations.
Updating the catalog requires changing the pinned revision and digest together,
regenerating all three artifacts, and reviewing the statistics and diff.

The runtime parses the artifact on its completion worker and only enumerates the
active command's tree. It does not query paths, branches or containers. Ordinary
terminal completion inserts only an exact raw suffix through the existing input
confirmation transaction. Whole-command history is explicitly opened with Alt+R.

Scoped history is device-local and excluded from portable backup and cloud-sync
documents. Restoring/replacing history clears scoped contributions and registers
restored rows as legacy global history. The learning filter intentionally drops
entire suspicious commands; it cannot recognize every possible secret.

Matching uses nucleo-matcher 0.3.1 (MPL-2.0), unchanged, from crates.io:
https://crates.io/crates/nucleo-matcher/0.3.1
https://github.com/helix-editor/nucleo
The Carapace data license is bundled beside the generated asset. Release packages
must retain that notice and the nucleo source/license reference in this document.

## Reproducible performance baseline

Run the synthetic fixture benchmark in an optimized build:

```
cargo run --release -p nyaterm-core --example completion_bench > /tmp/completion-bench.csv
```

It measures the previous matcher, the reusable Nucleo matcher and the full search
adapter at 64, 1,000, 10,000 and 50,000 candidates. It also measures warm token
completion against the embedded catalog and 5,000 distinct learned Docker
commands. Each query runs once to warm buffers, then 25 timed samples; p95 is
sample 24 after sorting. The counting allocator reports average allocations per
query and the maximum increase in live heap bytes, excluding preloaded data.
These synthetic strings contain no user history. Numbers depend on hardware,
concurrent load and the instrumentation; they are not production guarantees.

The Linux baseline and raw CSV are in [benchmark-baseline.csv](benchmark-baseline.csv).
Warm contextual p95 ranges from 0.05 to 4.53 ms in that run, below the 8 ms target.
Raw Unicode scans are slower than the old matcher for several queries; Nucleo
does eliminate warm per-item scoring allocations. Full history scans still scale
with history size and run on the worker. The contextual engine enumerates only
the active node and the relevant learned relation bucket.

## Validation and platform boundaries

Pure Rust tests cover all five dialects, byte ranges, Unicode highlights, option
values, scope isolation, source merging and deterministic ordering. Store tests
cover reopening the database, old global history, unknown fields, unsupported
versions, retention, deletion and restore. Desktop tests cover worker ordering,
cross-window invalidation and the existing input transaction guards. The local
integration suite exercises actual interactive Bash, Zsh and Fish processes and
verifies that each first submission produces exactly one accepted-command event.
Optional live-shell fixtures skip when their executable is absent.

The implementation environment is Linux; Bash, Zsh 5.9 and Fish 4.2.1 were
validated locally. Windows PowerShell/Cmd, macOS, live SSH and WSL require
platform validation before claiming complete platform acceptance. On each:

1. Verify `dock`, `docker r`, `docker run --na`, an empty argument and learned
   internal commands. Tab/Enter must fill a suffix without a space or execution.
2. Open Alt+R, verify current-scope history and the legacy badge, then confirm
   execution only after the input echo. Verify that `!` reaches the shell.
3. Submit on two hosts/users/distributions, restart, delete history from another
   window and restore a backup; unrelated scopes and stale popups must stay clear.
4. Exercise delayed echo, passwords, alternate screen, broadcast, disconnect,
   lock screen, middle-of-line cursors and complex graphemes; unsafe fills must
   be rejected.

The first version deliberately excludes middle-of-line replacement, editing
inside quoted/escaped tokens, Ghost Text and dynamic environment queries.

When driving the workspace tests as Linux root, run the file crate's tests as an
unprivileged user. The existing file permission test expects opening an
owner-read-only file for writing to fail; unrestricted root can write that file.
Other tests branch on the actual UID, so removing root capabilities alone is not
equivalent to an ordinary user. The file crate passed all 204 active unit tests
using this runner configuration:

```
cargo test -p nyaterm-files --lib --config 'target.x86_64-unknown-linux-gnu.runner=["runuser","-u","nobody","--"]'
```

For a root-driven workspace run, use a native Cargo runner that selects
`runuser -u nobody --` for `nyaterm_files-*` test executables and directly runs
other executables. Existing desktop SSH-start fixtures depend on the launch
user's environment and fail when the entire suite is forced into the `nobody`
account. These are validation-environment constraints; the completion changes do
not modify those test behaviors.
