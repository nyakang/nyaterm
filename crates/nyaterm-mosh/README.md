# NyaTerm Mosh client

Imported from the NyaMosh working tree (base `554b9d3`), with the original GPL-3.0-or-later license retained. This library owns the Mosh wire protocol, remote state reconstruction and prediction. NyaTerm transport owns SSH bootstrap, worker scheduling and session lifecycle. No standalone client executable is shipped.

## Connections

Select Mosh in the connection editor. The remote machine needs `mosh-server`
and a UTF-8 locale. SSH defaults to port 22; UDP defaults to ports 60000–61000.
The editor supports a custom server executable, UDP hostname and port range,
and adaptive/always/never prediction. `mosh://user@host:22` uses the SSH port;
inline passwords are rejected.

SSH reuses saved credentials, Agent authentication, host-key verification,
proxies and jump hosts. UDP connects directly and is not tunneled through SSH.
The bootstrap SSH connection also serves SFTP, monitoring and independent AI
commands. Losing auxiliary SSH does not close the UDP terminal; desktop retries
auxiliary SSH independently. Restoring or reconnecting a tab creates a new
Mosh server session.

Prediction affects painting only. Copy, search, recording and AI context use
confirmed terminal state. Mosh synchronizes screen states, so scrollback and
recordings cannot contain every intermediate server output byte. Mosh uses
UTF-8; raw terminal file-transfer protocols, X11/Agent forwarding, shell CWD
markers and tmux control mode are unavailable. Use SFTP for files. Agent
authentication and ordinary remote tmux remain supported.

## Validation

Run `cargo test -p nyaterm-mosh` for protocol tests. Integration tests in core,
store, desktop, terminal, terminal-runtime and transport cover configuration,
sync/backup compatibility, prediction isolation and session lifecycle.

`python3 scripts/test/mosh/run.py` uses Linux Docker bridge networking to create
a disposable SSH/Mosh server and ephemeral SSH key. It verifies bootstrap,
confirmed input/output, Unicode, resize, auxiliary SSH disconnection and
reconnection, recovery after three seconds of packet loss, and bounded shutdown.
The fixture cleans up its container and key. Windows/macOS native UI and URL
registration still require validation on those platforms.
