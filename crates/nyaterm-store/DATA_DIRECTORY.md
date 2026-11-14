# Data-directory switching and recovery

Installed builds keep bootstrap configuration, logs, crash reports, MCP discovery
and cloud-sync staging in the fixed local installation directory. The selected
data root contains the database, plugins and recordings. Portable layouts remain
unchanged and do not expose data-directory switching.

Directory selection is applied after an orderly process restart. All workspaces
must complete their save/discard gates and persistence barriers before the pending
selection is written. Independent installations share data-root leases; migration
also holds the source database's redb byte-range lock and legacy Unix file lock. The successor waits for
the previous installation owner to release its lock. The store closes its database
before acknowledging shutdown. A launch failure restores the previous selection,
removes the handoff marker and returns to the application.

An existing selection must contain a usable NyaTerm database. A missing selection
is never recreated as an empty database: startup uses a validated existing default
with a warning, or opens the recovery screen. Adoption validates a copy before
queueing and validates the actual database again before changing the pointer.
Unknown location fields are retained and unsupported metadata versions are rejected.

Migration stages and verifies the database, plugins and recordings together. Each
regular file is hashed with SHA-256 and synced. Unsupported links or special files
cause a failure without publishing a database. A journal identifies incomplete
publication; plugins and recordings are published before the database. Staging
directories belong to the pending migration ID, so an interrupted copy can be
restarted without deleting unrelated files. The source
is retained and pointer failures keep the original selection. Recovery refuses a
changed source or target instead of silently adopting incomplete data.

On Unix, metadata replacements and publication sync parent directories. Windows
uses atomic, write-through metadata replacement; directory-fsync guarantees differ
by filesystem and must not be described as identical to Unix power-loss behavior.
Network roots are detected using Linux mount information, macOS native mount flags,
and Windows UNC or mapped-drive classification. Third-party sync-client markers
are searched on the selected directory and its three parents.

Default session settings are local application settings. They are included in
existing backup/restore data; they are not published through the fixed V4 cloud
sync contract. Each protocol keeps its own draft, unknown fields survive edits,
and SSH Agent forwarding with the All policy requires consent in each new draft.

Native S3 GET, LIST and DELETE make up to three attempts for eligible transient
transport or HTTP failures. HTTP outcomes are classified within each attempt.
Responses carrying Retry-After are returned to the scheduler. Conditional 304 and
missing-object 404 retain their existing semantics. PUT remains single-attempt
because the publication journal handles ambiguous write outcomes. Native S3 keeps
its 30-second request timeout; OpenDAL keeps its 90-second timeout.
