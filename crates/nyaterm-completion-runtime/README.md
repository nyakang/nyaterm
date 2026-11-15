# nyaterm-completion-runtime

Owns the completion query worker, bounded mutation/search queue, token and
whole-command history caches and defensive user-spec loader. Its only workspace
dependency is `nyaterm-core`, including development dependencies.

Construct `runtime::CompletionRuntime` with a blocking
`history::CompletionHistoryRepository`, an optional spec directory and a typed
`CompletionEvent` callback. These callbacks run on the worker; never mutate GPUI
state directly from them. Desktop supplies its `StoreBlockingClient` adapter and
maps events onto its existing persistence-result channel.

A repository snapshot captures its generation in the same serialized operation
as its rows. Mutations publish the shared generation; append results may carry an
incremental update only for a single-generation transaction. External deletion,
restore or another worker's writes invalidate the cache. Failed snapshots may be
retried, without serving stale history.

Searches coalesce without reordering accepted mutations. Saturation reports
`Busy`; a closed worker reports `Unavailable`. Dropping the last runtime client
drains accepted requests and releases the worker and event sink. The spec loader
retains its independent blocking I/O and existing shutdown semantics. Desktop
owns settings, task lifetimes, terminal fill authorization and store persistence.

`cargo test -p nyaterm-completion-runtime` checks queue ordering/pressure,
invalidation, event ordering, closure and defensive spec loading. Store-backed
observation/deletion/restore and busy-database tests stay in desktop.
