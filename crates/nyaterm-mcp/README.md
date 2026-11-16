# NyaTerm MCP targets

When the Host advertises `connection_targets`, commands and SFTP operations can
use a saved SSH connection without opening a terminal:

```json
{"target":{"type":"connection","connectionId":"saved-id"},"command":"pwd","idempotencyKey":"request-1"}
```

Pass this to `terminal_start`. By default the forwarder waits up to 15 seconds
when the Host returns an actual `ssh_exec` job, so short commands can return
their status, exit evidence and output in one call. Actual `inline_pty` jobs
return immediately by default, including when the Host overrides a requested
backend through its public MCP execution setting. Saved targets default to
`ssh_exec` in automatic mode; inline mode can reuse or open a terminal instead.

`waitMs: 0` opts out. A positive `waitMs` requests a bounded wait for either
backend, capped at 30 seconds; an omitted or `null` value uses the defaults.
The forwarder consumes this option locally and never sends it to the Host,
so older Hosts and idempotency keys retain their existing argument contract.
The budget starts after the start response, excluding approval and job
registration, and includes backoff, bridge contention, reconnects and poll I/O.
On timeout, cancellation or a poll error, the last valid start or poll response
is returned. Waiting does not replay or stop the command; canceling an in-flight
poll retains the ordinary bridge request-cancellation behavior.

Use the returned `jobId` with `terminal_poll` and `terminal_stop`. Waited output
is still one bounded page, not the concatenation of internal polls. Continue
reading with `offset: nextOffset` when more output remains, and preserve
`sourceTruncated` and `droppedBeforeOffset` when interpreting retained output.
`eof` only describes the current output tail and does not prove process exit.
Each `ssh_exec` command has its own channel, working directory and environment. Use an
explicit `cd` or environment assignment when needed. A stop response acknowledges
the request; only `processState: "exit_confirmed"` confirms remote exit.

The same `target` works with `sftp_home`, `sftp_list`, `sftp_stat`,
`sftp_read_text`, `sftp_write_text`, `sftp_mkdir`, `sftp_rename`, `sftp_delete`
and `sftp_chmod`. Existing text limits and write conflict checks still apply.
Failed writes are never automatically replayed.

Session operations accept `{"type":"session","sessionId":"session-id"}`
or the legacy `sessionId` argument. Do not pass both. Session calls retain their
existing result shape and default to `inline_pty`. Older Hosts advertise only
legacy arguments; the sidecar rejects target arguments when unsupported.

Normal external MCP credentials use the existing permission and risk approval
rules. Embedded Agent credentials remain limited to their assigned sessions.
SSH authentication, host-key verification, proxy and jump settings come from the
saved connection. Hosts that disable AI execution or independent Exec cannot run
background commands.

Submitted commands survive client disconnects. Retain the existing recovery
credentials to reconnect within ten minutes; retries with the same idempotency
key and normalized arguments return the original job. Different arguments with
the same key are rejected. RPC or SSH failures do not trigger command replay.

Discovery-based sidecars re-read the discovery document for each new bridge
connection. If the document is missing, unreadable or invalid, they use the last
endpoint that completed authentication, owner attachment and identity replay;
before the first successful connection they use the startup endpoint. A valid
document whose endpoint cannot connect reports the failure without trying an
older endpoint in the same call. Explicit environment endpoints stay fixed.

An expired owner recovery credential triggers one fresh attachment only when
the Host explicitly returns `resume_failed`. Timeouts, disconnects, malformed
responses and other rejections propagate without retrying attachment. A lost
connection fails the current call; the next call can reconnect, without
automatically replaying a submitted command. Cancellation uses the executing
connection's endpoint and owner credential even if discovery has since rotated.
Cancellation never creates a replacement owner when recovery fails.

Exec and SFTP share cached background transports by saved identity, effective
configuration and authentication revision. Active operations retain a lease;
idle transports are released after 120 seconds. No shell or PTY is created.

The AI / MCP settings page provides the current window's background task list,
retained output, exit evidence and stop controls. Opening a related terminal
starts an independent interactive session. Jobs and bounded output stay in
memory; completed jobs expire after ten minutes. Locking the app, revoking MCP
access or closing the workspace requests bounded shutdown, retaining unknown
remote state when exit cannot be confirmed.
