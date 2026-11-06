# History eviction regression and CPU benchmark

Issue: <https://github.com/nyakang/nyaterm/issues/1043>.

The ignored `full_history_surface_update_benchmark` uses 80 columns, 24 viewport
rows, 48 prefetched rows on each side, and the existing surface cache limits.
Each scenario starts with full scrollback, processes 2000 output frames, and
runs five times at capacities 5000 and 100000. Historical viewing starts 256
rows above the bottom and follows the original content as output scrolls.

Run without concurrent compilation or other benchmark jobs:

```sh
cargo test -p nyaterm-desktop full_history_surface_update_benchmark --lib -- --ignored --nocapture > candidate.log 2>&1
```

For a baseline, create a detached worktree at `833d6d357`, copy only
`crates/nyaterm-desktop/src/features/terminal/terminal_surface_entity/history_rotation_bench.rs`
into its corresponding directory, and add `#[cfg(test)] mod history_rotation_bench;`
to that directory's `mod.rs`. Build and run the same command in the worktree,
with a separate target directory. Keep the same compiler, Cargo profile,
fixture, viewport, prefetch sizes, and cache limits. Save the output as
`baseline.log`. The fixture also works with the original surface implementation.

```sh
python3 scripts/bench/history_rotation_compare.py baseline.log candidate.log
```

The comparison requires all five runs of all four scenarios. It compares the
median output throughput at the bottom (maximum 5% decline) and the median of
each historical run's update p95 (maximum 10% increase). `recoveries` counts
viewport snapshot fetches when retained surface data cannot serve the target.
Throughput includes parsing and surface updates; update p95 excludes parsing.
This is a debug CPU state benchmark, not native window/GPU or transport throughput.

Deterministic tests separately check shared row `Arc`s, zero row-cache refresh
during retained-window translation, stable cache keys, clipping and reversed
selection, copy generations, request coalescing, no-wrap frames and indexes,
and cache row/byte bounds. The same retained-window test uses 128, 5000, and
100000 history rows with identical viewport and cache sizes.

## Recorded validation

Linux 7.0.0-38-generic, rustc 1.99.0, debug test profile, shared host. Neither
builds nor tests ran concurrently with the recorded A/B benchmarks. Baseline
is `833d6d357` with only the identical benchmark module added; candidate is
the Issue #1043 patch. All five raw runs are in
[`history_rotation_results.csv`](history_rotation_results.csv).

| History rows | Bottom throughput median, bytes/s | Change | Historical update p95 median | Recoveries per 2000 updates |
| --- | --- | --- | --- | --- |
| 5000 | 55660.5 → 56810.4 | +2.07% | 13.286 ms → 0.02161 ms | 2000 → 0 |
| 100000 | 51677.2 → 53894.8 | +4.29% | 13.346 ms → 0.020543 ms | 2000 → 0 |

Both throughput and historical update thresholds pass. The retained-row tests
also pass at both capacities, sharing the original row-data `Arc` throughout
repeated rotation without refreshing row entries or growing cache limits.

Validation commands and results:

- `cargo check --workspace`: passed.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets`: passed without warnings.
- `cargo test --workspace -- --test-threads=8`: desktop 2763 passed; the run
  then encountered an existing files permission test that assumes an
  unprivileged process. Root can write its mode-0400 fixture.
- `download_path::tests::staged_write_preserves_existing_permissions` run
  from the same test binary with `runuser -u nobody`: passed.
- `cargo test --workspace --exclude nyaterm-app --exclude nyaterm-core
  --exclude nyaterm-desktop --no-fail-fast -- --test-threads=8
  --skip download_path::tests::staged_write_preserves_existing_permissions`:
  passed, completing the remaining crate tests; terminal 314 passed.

Windows 11 GUI validation has not been executed. The CPU measurements and
state-transition tests do not establish native rendering or clipboard behavior.

## Windows 11 GUI validation

This validation cannot be performed by the Linux CPU fixture. With Scrollback
set to 5000, run both of the following in the local PowerShell terminal:

```powershell
$i = 0; while ($true) { "helloworld $i"; $i++ }
```

```powershell
$i = 0; while ($true) { "helloworld $i"; $i++; Start-Sleep -Milliseconds 10 }
```

After capacity is reached, check the bottom, a retained history window, and
the oldest history. Drag and reverse-drag selections while output continues;
copy them and compare the numbered text against the highlight. Repeat at
100000 rows. Retained text must stay anchored; the oldest window must advance
when its actual rows are evicted, without recurring blank frames. A partially
evicted selection must start at the first retained row, column zero, preserving
its direction; a fully evicted selection must disappear. Stop with Ctrl+C.

Also check soft-wrapped wide characters and a long line in no-wrap mode, where
the previous complete frame and its hit geometry remain visible until the new
worker frame arrives. Reset or screen switch must invalidate old selections.
