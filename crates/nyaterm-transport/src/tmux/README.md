# Tmux control-mode discovery

NyaTerm's SSH terminal integration recommends `tmux -CC` (or
`tmux -CC attach`). The detector continuously recognizes its
`ESC P 1000 p` output marker. Discovery does not depend on seeing the command
in keyboard input, so aliases, scripts, history recall, editing, bracketed
paste and automatic attach can all enter integration mode.

## Single-C compatibility

`tmux -C` has no DCS entry marker. Its bare control lines are considered only
for 30 seconds after a complete, unambiguous command is submitted through
terminal input or a configured post-login command. Existing command-position,
wrapper and option-argument checks still apply. Repeating `-C` selects the
double-C mode, just like `-CC`.

History recall, edited input, bracketed paste, suffix guesses and indirect
commands do not enable this compatibility window. Without explicit evidence,
single-C output remains terminal output and NyaTerm sends no verification
probe. Use `-CC` for these launch methods. A submitted `-CC` command does not
enable bare single-C discovery.

This changes automatic discovery only; there is no new preference or saved
configuration field.

## Discovery and handoff

Input launch hints, the single-C window and output candidates have separate
lifetimes. A new submission or cancellation clears the old hint/window and
releases a pending single-C candidate. It does not cancel a double-C candidate
or reset output framing.

The output scanner tracks terminal escape/string context across chunks,
including while buffering candidates. Similar text inside OSC, another DCS,
SOS, PM or APC cannot establish an entry point. Rejected or expired candidates
are released verbatim without resetting that context. Ordinary output is
forwarded as it arrives; only possible entry prefixes and startup candidates
are buffered.

Both modes must supply a matching startup response block and a valid session
notification. Only then does the existing SSH handoff block competing writes
and transfers and send a random-token query. A matching response is required
before creating the integrated control session. Failed active verification
closes that terminal channel; only an explicit protocol exit permits shell
handback. The detector sends no recovery or detach command on verification
failure.

Bounds remain: 4096 bytes of tracked input, 512 KiB per confirmation buffer,
5 seconds per candidate and per active handshake, and a 10-second silence
advisory. Candidate deadlines do not extend with incoming fragments. A
single-C candidate that started within its window uses its own deadline.

## Diagnostics and regression coverage

Shell output or stderr suppresses a pending advisory; it does not prove the
launch failed or stop protocol discovery. The first suppression emits one
DEBUG event per launch. Normal output and input editing produce no tmux INFO
events. Lifecycle events remain INFO, active verification failures remain
WARN, and rejected complete startup candidates use DEBUG. No terminal text,
command content or random tokens are added to logs.

Detector tests cover every startup split point, bytewise input, terminal
string framing, bounded recovery, independent deadlines, input cancellation
and advisory/log deduplication. Loopback SSH tests cover discovery followed by
nonce verification and bare-output passthrough without probes. Opt-in real
tmux tests cover explicit single-C, double-C without input evidence, recalled
double-C, detach/reattach and unrequested single-C passthrough.

Run the focused groups with:

```sh
cargo test -p nyaterm-transport --lib tmux --locked
cargo test -p nyaterm-transport --lib real_tmux --locked -- --ignored
```

The existing compatibility CI runs real tmux 3.2a, 3.4 and 3.8 on Linux and
the system tmux on macOS.
