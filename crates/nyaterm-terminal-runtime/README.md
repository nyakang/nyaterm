# nyaterm-terminal-runtime

UI-independent terminal frame processing and transport ingress routing. The only
workspace dependencies, including test dependencies, are `nyaterm-core`,
`nyaterm-terminal`, `nyaterm-transport`, and `nyaterm-recording`.

- `frame::TerminalFramePipeline` owns the frame worker, command queue, coalesced
  events, searches and recording ordering. Use `take_event_wake_receiver()` and
  arm before draining; the queue is authoritative, the wake is only a signal.
- `session_event_bridge::SessionEventBridge` routes transport events into frame
  processing or the caller's UI queue. Protocol handoffs, backpressure and raw
  recording capture preserve ingress order.
- `work_policy` maps caller-supplied visibility to parsing and snapshot work.
  Desktop owns visibility, presentation caches, event application and window/session
  ownership; neither worker stores a GPUI entity.

`TerminalFrameSession` is an opaque ownership-transfer value. Pause ingress and
finish pending output before taking a session, transfer its pending events, then
insert it into the destination pipeline. Insertion returns ownership on failure.
The transferred state includes both decoders, grid history and frame revision.

Call `take_shutdown()` and execute the returned closure on a blocking worker.
The frame pipeline closes its command and event queues before joining, so a
producer waiting on a full event queue can exit. The bridge signals its stop
condition and joins. `Drop` retains the existing final-owner cleanup fallback;
normal desktop shutdown must not join on the UI thread.

The default-off `test-support` feature provides submission/flush and event
fixtures for desktop integration tests. Private queues and worker state remain
private. Queue ordering, fences, wake interests, bounded search, backpressure,
charset handling and raw recording regressions run in this crate:

```sh
cargo test -p nyaterm-terminal-runtime
```
