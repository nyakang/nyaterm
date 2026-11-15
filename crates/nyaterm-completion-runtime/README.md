# nyaterm-completion-runtime

Owns the completion query worker, bounded mutation/search queue, token and
whole-command history caches, defensive user-spec loader and progressive result
publication. Its only workspace
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

`progressive::publish_progressively` awaits caller-supplied ordinary and path
futures and invokes a callback with ordinary results first. Nonempty paths cause
one additional publication carrying the same ordinary result and revision; empty
paths cause none. It starts no tasks. Desktop starts providers, merges candidates
with `nyaterm_core::completion::merge`, and revalidates each publication against
current session, shell context, generations, input, sync state and fill policy.

`cargo test -p nyaterm-completion-runtime` checks queue ordering/pressure,
invalidation, event ordering, closure, defensive spec loading and progressive
publication ordering. Store-backed
observation/deletion/restore and busy-database tests stay in desktop.
