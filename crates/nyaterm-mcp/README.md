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

## Streamable HTTP

In **Settings → AI → Agents → MCP**, select **Streamable HTTP**, set a port (default
8002), and enable external MCP access. NyaTerm starts and supervises the helper.
The endpoint is `http://127.0.0.1:8002/mcp`; the bind address and path are fixed.
Use **Copy config** for Codex (TOML), Claude Code, Cursor, or generic HTTP settings.
Each service start generates a fresh bearer token. After restarting, unlocking,
changing HTTP/permission settings, or switching the owning window, copy the
configuration again. Tokens are not stored in application settings or sync data.

There is one HTTP service per application process. **Use this window / Retry**
selects its owning window and restarts it. Focusing another window does not move
it. Closing the owner stops the service until a window is explicitly selected.
The existing current-window/all-sessions scope and approval policy still apply.
Port conflicts are reported; the service never silently chooses another port.

Browser clients must add their exact origin (scheme, hostname, port) to the
allowlist, one per line, then apply the HTTP settings. The empty list rejects all
browser origins; native clients without Origin still authenticate normally.
Wildcards, opaque `null` origins and URL paths are rejected. Browsers must use a
fetch-based MCP client supporting Authorization headers, and may ask permission
for local-network access. CORS exposes the MCP session header. There is no public
web UI, token endpoint, cookie authentication, or unauthenticated MCP access.
Remote clients need a separately configured tunnel to this loopback endpoint.

The HTTP service negotiates 2025-03-26, 2025-06-18 and 2025-11-25. It implements
POST, GET/SSE and DELETE on `/mcp`; OPTIONS only handles authorized-origin
preflight. 2025-03-26 batches share a response stream without a replay cursor.
There are limits of 64 HTTP sessions, 64 batch members and 4 MiB per request.
Sessions expire after ten minutes of inactivity. Disconnecting an SSE stream
neither cancels nor replays submitted commands. Service restarts invalidate all
HTTP sessions and credentials.
Stopping the service cancels its clients' background jobs and pending starts;
embedded Agent jobs keep their independent credentials and lifetime.

No-argument execution of `nyaterm-mcp` remains stdio, including embedded Agents.
The `--transport streamable-http --managed` mode is a private, versioned desktop
control-pipe interface; it does not read tokens from command-line arguments.

For browser regression testing, install Playwright and its Chromium/Firefox
browsers outside the repository, build `nyaterm-mcp`, and run:

```sh
NYATERM_PLAYWRIGHT_MODULE=/path/to/playwright node scripts/tests/mcp_http/browser.cjs /path/to/nyaterm-mcp
```

The test runs an isolated mock bridge and temporary local browser origin; it does
not access a user's NyaTerm instance or print credentials.

## Client setup and activity

The MCP settings card provides a client selector, a configuration preview and
**Copy client configuration**. Select the transport first; stdio and HTTP each
remember their own client selection after settings are saved. Choosing a client
changes only the exported configuration, without restarting the service or
interrupting jobs. Both transports still require NyaTerm to be running.

| Client | Format and destination |
| --- | --- |
| Codex | TOML in `~/.codex/config.toml`, or `config.toml` under `CODEX_HOME`; both stdio and HTTP use TOML. |
| Claude Code | Merge the `mcpServers.nyaterm` entry into a private project `.mcp.json`. |
| Cursor | Merge into `~/.cursor/mcp.json`. |
| VS Code | Merge the `servers` entry into the user MCP configuration. |
| Windsurf | Open MCP configuration from Cascade and merge into `mcp_config.json`. |
| OpenCode | Merge the `mcp` entry into the user `opencode.json`. |
| Cherry Studio | Import the JSON in Settings → MCP Servers, then enable `nyaterm`. |
| Generic HTTP | Copy the URL and Authorization header to an HTTP MCP client. |

Preserve other servers when merging. HTTP previews show a masked token; only an
explicit copy action obtains the current credential. Keep exported HTTP configs
private. Service restarts still rotate the token, so copy again afterwards.
Unknown saved client names are preserved but cannot be exported until a supported
client is selected. Missing helpers and unavailable HTTP services disable copying
and show the reason.

**Recent activity** displays the device's retained MCP audit records, including
client, connection/session, tool, event, result and duration. Filter by client,
connection/session or result, with 50 entries per page. Opening the section or
pressing Refresh loads records in the background; while the section is visible,
it refreshes every five seconds. Closing it or leaving the page stops refreshes.
A read failure keeps the last displayed records and permits retrying.

Activity does not expose commands, arguments, terminal output, file contents or
raw errors, including those present in older audit records. It uses the existing
audit retention limit, not a new archive. Failures and explicit denials have
separate labels; registering a job or sending an interrupt is an event, not proof
of completion. Remote exit confirmation is also shown as a lifecycle event.
Native AI records are excluded; older MCP records without a source tag remain
visible when their action starts with `mcp.`.

The permission description stays visible, including a Full Access warning while
MCP is disabled. Disabling the service does not reset its saved permission mode.
