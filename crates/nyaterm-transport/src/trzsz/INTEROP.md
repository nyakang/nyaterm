# trzsz interoperability checks

The reference implementation is unmodified trzsz-go **v1.2.0**. The opt-in
tests check the executable version and require `TRZSZ_GO_BIN` to name the
directory containing `trz` and `tsz` (with `.exe` on Windows).

Download the official release from
https://github.com/trzsz/trzsz-go/releases/tag/v1.2.0 and verify it against that
release's checksums. The Windows x86_64 ZIP used for local validation has SHA256
`0b46318a0fc7e39beaf97e6f07ab257f5b36fdffc3304118b6f72412dbbb52ca`.
No reference binaries or source patches are required by the normal build.

```powershell
$env:TRZSZ_GO_BIN = 'C:\tools\trzsz_1.2.0_windows_x86_64'
cargo test -p nyaterm-transport trzsz::interop_tests -- --ignored --nocapture --test-threads=1
```

On Linux/macOS set the same environment variable to the native tools directory.
`TRZSZ_PROTOCOL=1` (or 2, 3, 4) restricts the protocol matrix;
`TRZSZ_TRACE=1` prints test-only protocol frames. Do not enable tracing for
real user transfers.

## Coverage

- Bidirectional protocols 1–4, text and actual escaped binary, compression
  yes/no/auto, empty files, small boundary sizes and multiple independent roots.
- Legacy JSON directory names, protocol 4 streaming archives, repeated
  basenames in separate directories, empty directories and files.
- Protocol 3/4 cumulative prefix hashes, matching and mismatching existing
  prefixes beyond 10 MiB, and empty files replacing nonempty targets.
- File-backed 32/128 MiB transfers with a slow streaming download sink and
  delayed acknowledgements, bounded parser/output/read-ahead checks, MD5
  verification, and cancellation of a real sender under backpressure.
- A separate 16 MiB upload with 100 ms delays before reading ACK batches.
- Truncated DATA captured from real protocol 2–4 senders cannot produce an ACK
  or successful completion.

Binary tests use the reference tool's TCP tunnel, since the Windows reference
tool disables binary mode over its terminal pipe. Text tests also cover the
Windows terminal newline convention. Unit tests cover arbitrary stream splits,
truncation, invalid archive paths and lengths, decoder sink failures, and
compressed 64 MiB input without collecting decoded file data.

## Local measurement

Windows x86_64, debug test build: the isolated slow-transfer test completed
32 MiB download/upload in 2.02/1.84 seconds and 128 MiB in 6.73/5.67 seconds.
Cancellation took about 0.60 seconds. Sampling the Rust test process every
100 ms gave a peak working set of 14,561,280 bytes across both file sizes.
The separate 16 MiB upload with 100 ms ACK latency completed in 8.42 seconds.
This measurement excludes the Go child and GPUI application, and simulates
latency by delaying ACK handling rather than an external WAN emulator.
The file payload is never accumulated in the test harness. Runtime
interoperability on Linux/macOS still requires running the command there.
