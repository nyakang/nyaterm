# NyaTerm MCP targets

When the Host advertises `connection_targets`, commands and SFTP operations can
use a saved SSH connection without opening a terminal:

```json
{"target":{"type":"connection","connectionId":"saved-id"},"command":"pwd","idempotencyKey":"request-1"}
```

Pass this to `terminal_start`, then use the returned `jobId` with `terminal_poll`
and `terminal_stop`. Saved targets default to `ssh_exec` and reject `inline_pty`.
Each command has its own channel, working directory and environment. Use an
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

Exec and SFTP share cached background transports by saved identity, effective
configuration and authentication revision. Active operations retain a lease;
idle transports are released after 120 seconds. No shell or PTY is created.

The AI / MCP settings page provides the current window's background task list,
retained output, exit evidence and stop controls. Opening a related terminal
starts an independent interactive session. Jobs and bounded output stay in
memory; completed jobs expire after ten minutes. Locking the app, revoking MCP
access or closing the workspace requests bounded shutdown, retaining unknown
remote state when exit cannot be confirmed.
