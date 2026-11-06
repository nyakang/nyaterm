# Terminal throughput matrix

Run on an otherwise idle Windows machine:

```powershell
cargo build --release -p nyaterm-terminal --example throughput --features benchmark
./target/release/examples/throughput.exe --matrix > results.csv
```

`--quick` limits the matrix to plain/SGR, 2/24 rows and 16 KiB input.
The full matrix includes plain text, SGR, soft wrapping with wide characters,
and cursor rewrites; 2/24/200 rows; scrollback 0/2/24/10000; and per-line,
16 KiB and 64 KiB writes. Correctness tests additionally cover one-row windows.

Each case discards one complete warmup run, then runs five samples with a fresh
screen and 300 prefilled lines. Each sample repeats the workload for at least
250 ms; the CSV includes the actual byte count so batch/scan counts can be
normalized when versions complete different numbers of repetitions.
The CSV reports median throughput (decimal
MB/s), process CPU milliseconds and per-write P50/P95/P99 for the sample with
median throughput, plus cumulative signature rows scanned and synchronization
batches. CPU uses Windows GetProcessTimes, whose timer quantization can be
visible in short cases; non-Windows CPU values are NaN. Timing includes the
public advance path; snapshot materialization happens after the timed interval.
These measurements do not measure GUI input, paint or resize latency.

The `benchmark` Cargo feature enables two counters and their accessor only for
this harness. Normal application builds do not include the counters. For fair
comparisons, instrument both historical revisions at the same stamping sites,
copy this harness unchanged, and use an independent target directory for each
checkout. Do not share release outputs between worktrees with the same package
name/version. Store version hashes with the CSV results.

Acceptance: every plain/SGR 16 KiB case must reach 90% of the main baseline;
every per-line case must reach 90% of the original PR. Compare rows by
(kind, rows, history, chunk), not by CSV position.

```powershell
python crates/nyaterm-terminal/examples/throughput_matrix/compare.py main.csv pr.csv fixed.csv
```

The comparison exits with a failure if a case misses its gate or if the three
CSV matrices differ.

For comparisons, prefer a fresh process for each case and rotate the version
order to reduce allocator-history and clock-frequency drift between versions:

```powershell
python crates/nyaterm-terminal/examples/throughput_matrix/interleave.py main.exe pr.exe fixed.exe results --cpu 2
```

The optional Windows `--cpu` pins only the runner and its child benchmarks to a
logical CPU. Windows child benchmarks run at above-normal priority. This does
not stop other workloads or guarantee an idle core; record interference and
retain failed runs. `--quick` selects the smaller matrix. The executable also
accepts `--matrix --case=plain,24,0,16384` for a single case, or `--matrix --list`
to list case keys without executing samples. Every version must use the same
harness and sampling policy.
