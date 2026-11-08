# Cloud Sync

## Sync scope and existing field-merge configurations

Cloud Sync uses a fixed scope in both snapshot and field-merge modes: connections and groups, connection icons, proxies and groups, tunnels and groups, quick commands and categories, notes and folders, and custom themes. There are no scope switches. Passwords, private keys, OTP, AI configuration and keys, shortcuts, host trust and appearance preferences stay on each device. New devices need local authentication setup.

The settings page no longer shows the version introduction or mode selector. Snapshot mode remains the default. Existing field-merge configurations retain their mode and support Local Directory, WebDAV, S3, Google Drive, OneDrive and Aliyun Drive. Click Push Now or Pull Now to prepare a preview, review and resolve conflicts in Conflict Handling, then click Confirm, apply and publish to synchronize. Independent fields merge automatically; conflicting fields and concurrent deletion/editing retain candidates for local resolution. Missing local credentials or changes redirecting an existing secret defer the affected fields.

New field publications use schema 5 under `sync/v5/`. New sync snapshots use Sync schema 4; full `.nya` backups retain schema 3 and their complete data contract. All participating devices must upgrade. The first field confirmation creates and verifies an encrypted `cloud-sync-v3-safety-<id>.nya` backup. Old cloud publications and local replicas remain migration inputs; old scope settings remain readable but do not control sync. Only core values enter new publications, while causal references, core conflicts and deletions remain intact. Old pending payloads are retained and never retried in the new space. Later legacy writes require a manual import preview.

Apply and publication retain the existing workflow: business changes commit atomically, then immutable ciphertext is published and read back. Failure retains a pending publication for exact-byte retry. Password changes publish and verify a new `rotation-<id>` subspace before switching the local password and namespace. Other devices must use the new password and namespace. `.nya` backup/restore continues to include credentials, secrets and settings independently of cloud sync.

The sections below describe snapshot mode and provider setup.

NyaTerm's **Cloud Sync** capability is not remote file transfer. It syncs portable application configuration through one encrypted current snapshot in remote storage.

Remote storage uses immutable snapshots and independent device commits:

- `sync/snapshots/<revision>.redb.enc` stores encrypted snapshots used by the current protocol
- `sync/heads/` stores device commits and temporary upload markers, preserving concurrent branches
- `sync/latest.redb` is a compatibility pointer with revision, device, timestamp, and hash metadata
- `sync/current.redb.enc` is a best-effort compatibility/recovery copy, not the sole authority

Simultaneous device uploads preserve both commits. A subsequent check detects concurrent branches and asks you to choose local or cloud explicitly. Providers with delayed visibility may reveal the conflict on a later check. Causal records, rather than device wall clocks, determine which changes supersede others.

Local `.nya` export / import remains the backup and migration path. Maintenance retains referenced device snapshots, unresolved branches, and staged uploads while collecting eligible older snapshots. It does not delete existing `backups/` objects. Retained sync snapshots do not replace independent backups.

NyaTerm currently supports these remote storage provider types:

- **WebDAV**
- **S3-compatible** storage
- **Gitee Snippet**
- **GitHub Gist**
- **Google Drive**
- **OneDrive**
- **AliyunDrive**

## Prerequisite: set a master password first

Before using **Settings → Cloud Sync**, you must first set a **Master Password** in **Settings → Security**.

Without a master password:

- Cloud Sync cannot be enabled
- Manual actions such as test, push, and pull cannot be run

New encrypted snapshots use Argon2id with a random salt and AES-256-GCM. The version header, KDF parameters, salt, and nonce are authenticated along with the payload.

Existing NyaTerm and Dragonfly ciphertext remains readable. The next actual upload or explicit overwrite writes the new envelope; an unchanged-content check does not rewrite old ciphertext. Upgrade every participating device before publishing new-format snapshots: older clients cannot decrypt them or participate in the new concurrency protocol. Keep a local `.nya` backup before upgrading.

An old sync baseline cannot prove that all entities are unchanged. The first check after upgrade can therefore require explicit conflict resolution, protecting VNC trust records and extension data omitted by the historical change hash.

## Where to access it

This feature has two main entry points.

### 1. Settings → Cloud Sync

Use the settings page to:

- choose a provider
- configure the remote namespace and provider root
- define automatic sync behavior
- run test connection, push now, and pull now
- resolve conflicts when they happen

### 2. The Cloud Sync panel in the workspace

The workspace **Cloud Sync** panel shows:

- current status
- recent sync activity
- last check and sync times
- conflict state and quick resolution actions

Use the settings page to configure the feature, and the workspace panel to watch its live state.

## Provider configuration

### Common fields

No matter which provider you use, start by checking these fields:

- **Enable Cloud Sync**
- **Provider Configuration**
- **Device Name**
- **Remote Namespace**

These have specific meanings:

- **Device Name** is written into snapshot metadata so you can tell which device uploaded it
- **Remote Namespace** is the top-level path prefix NyaTerm uses for the cloud sync snapshot inside the selected provider

### WebDAV

When the provider is **WebDAV**, you will typically configure:

- **WebDAV Endpoint**
- **Provider Root** (optional)
- **Username**
- **Password**

WebDAV authentication supports both **Basic** and **Digest** auth. NyaTerm switches to Digest when the server requires it, improving compatibility with NAS / gateways that mandate Digest.

### S3-compatible storage

When the provider is **S3-compatible**, you will typically configure:

- **S3 Endpoint**
- **Bucket**
- **Region**
- **Provider Root** (optional)
- **Access Key ID**
- **Secret Access Key**
- **Session Token** (optional)
- **Virtual Host Style** (enable only if your provider requires it)

### Gitee Snippet

When the provider is **Gitee Snippet**, NyaTerm stores encrypted cloud sync objects in a Gitee code snippet.

Gitee's file-count limit includes snapshots, device commits, and pending markers. Many devices or unresolved branches may require a new snippet or another provider; synchronization never deletes unresolved branches to make room.

You will typically configure:

- **Gitee Personal Access Token**
- **Snippet ID**

### GitHub Gist

When the provider is **GitHub Gist**, NyaTerm authorizes GitHub with device flow and stores encrypted cloud sync objects in a private Gist.

You will typically:

- click **Connect GitHub**
- enter the device code on the GitHub authorization page
- grant the `gist` scope

During the GitHub device-flow authorization, NyaTerm creates and fills in a private Gist when no Gist ID is configured. When a Gist ID already exists, it validates and reuses that Gist.

### Google Drive / OneDrive / AliyunDrive

When the provider is **Google Drive**, **OneDrive**, or **AliyunDrive**, NyaTerm uses native desktop HTTP adapters to access the selected drive service.

The current implementation expects OAuth credentials to be entered manually:

- **Refresh Token**
- **Client ID**
- **Client Secret**
- **Access Token** (optional; refresh token is recommended for long-term sync)
- **Provider Root** (optional)

AliyunDrive also requires **Drive Type**, usually `resource`.

### Validation rules

The current implementation performs basic provider validation before actions are allowed:

- WebDAV requires an endpoint
- S3 requires both an endpoint and a bucket
- `Access Key ID` and `Secret Access Key` must be provided together
- GitHub Gist requires GitHub authorization; the Gist ID can be created and filled in during authorization or supplied for an existing Gist
- Google Drive / OneDrive / AliyunDrive require Refresh Token, Client ID, and Client Secret

A good habit is to run **Test Connection** before enabling any automatic strategy.

## Automatic sync strategy

### Check on startup

When enabled, NyaTerm checks the remote provider during startup to see whether a newer sync snapshot already exists.

Typical outcomes include:

- local and remote are already aligned
- the remote side has a newer snapshot available to pull
- local changes are pending upload
- both local and remote changed, so a conflict is detected

All providers, including GitHub Gist, also check every 15 minutes and when a window regains focus, with a 120-second focus throttle. Disabling startup checks skips only that trigger. Failures use backoff and valid server `Retry-After` hints; authentication failures pause until settings change.

### Auto-pull remote changes

When **Auto Pull Remote Changes** is enabled, if startup check or sync check finds that only the remote side changed, NyaTerm automatically pulls and applies the current remote snapshot.

This only handles cases with no unsynchronized local changes. Active sessions and unsaved settings drafts defer application until it is safe. If both local and remote changed, NyaTerm still enters conflict state instead of choosing one side automatically.

### Auto-push after local changes

When enabled, NyaTerm automatically pushes a snapshot after supported local configuration changes are saved, using a debounce window.

You can control:

- **Sync Debounce Seconds**

That makes the behavior closer to "push shortly after save" than to real-time bidirectional sync.

## Manual actions

### Test Connection

Validate the provider configuration and confirm the remote layout is reachable.

### Push Now

Upload the current device's portable configuration snapshot to the cloud.

Useful when:

- you just finished a meaningful batch of settings changes
- you want explicit control over when cloud state is updated

### Pull Now

Fetch the current latest sync snapshot from the cloud and apply it locally.

Useful when:

- another device has already pushed an update
- the current device needs to align with the latest remote state on demand

## How conflicts happen

A conflict occurs when both local state and remote state changed since the last sync baseline.

A typical example looks like this:

- Device A changed settings but has not pulled Device B's update
- Device B already pushed a newer sync snapshot to the cloud
- Device A then tries to push or performs a startup check and discovers both sides changed

When this happens, NyaTerm can show conflict details such as:

- local snapshot hash
- remote revision
- remote device information
- current remote snapshot time and hash
- a human-readable conflict message

## How conflict resolution works

The current implementation offers two resolution actions.

### Download Remote Version

The branch list shows every concurrent revision, device name (short UUID for old snapshots), time, readability and difference counts. One readable branch is selected automatically; multiple readable branches require an explicit choice. Summaries show record counts, custom theme changes without credentials, OTP values, notes or command text.

Pull the selected immutable revision and apply it to the current device.

Use this when:

- the cloud copy is the version you want to keep
- the current device's local changes should be discarded

### Upload Local Version

Force the current device's local snapshot to become the new remote state.

Use this when:

- the current device has the correct latest version
- you want other devices to follow this state later

The important boundary is:

- this is not a field-level merge
- this is not collaborative conflict resolution
- it is effectively a choice between the local snapshot and the remote snapshot

Device commits and causal records preserve concurrent branches. Explicitly choosing local or cloud publishes a new commit resolving the observed branches; cloud uses the immutable revision explicitly selected in the preview. Later concurrent edits can produce another conflict. Do not manually clear `sync/snapshots/`: it is the current data path.

## What gets synced?

Both modes sync connections and groups, connection icons, proxies and groups, tunnels and groups, quick commands and categories, notes and folders, and custom themes.

Passwords, private keys, OTP, AI configuration and keys, shortcuts, trusted hosts, appearance preferences, history, device paths and window state remain local. Selecting a theme remains local even though custom theme definitions sync. Use a full encrypted `.nya` backup to migrate secrets and application settings.

## Recommended first-time setup order

If you are enabling this for the first time, this order works well:

1. Set a master password in **Settings → Security**
2. Open **Settings → Cloud Sync**
3. Choose the provider, then fill in endpoint / bucket / root / credentials
4. Set the device name and remote namespace
5. Run **Test Connection** first
6. Then decide whether to enable:
   - check on startup
   - auto-pull remote changes
   - auto-push
7. Finally run **Push Now** once to verify the full path works

## Troubleshooting hints

If buttons are disabled or actions are blocked, check these first:

- whether a master password has been configured
- whether current settings have been saved or applied
- whether provider-required fields are complete
- whether the remote namespace, provider root, and endpoint values are correct

If your main question is "what happened recently?", start with the workspace **Cloud Sync** panel. If your main goal is changing config, go back to the settings page.

:::tip Practical advice
- When connecting a new provider for the first time, run **Test Connection** before enabling automatic strategies.
- Before making large changes to connections, OTP, or quick commands, use local `.nya` export if you need a rollback point.
- If you switch between devices often, keep a clear device-name convention and a predictable remote-namespace strategy.
:::


## Credentials, devices and activity history

Google Drive, OneDrive and Aliyun Drive encrypt and save refreshed access tokens
and any returned refresh token before the next authenticated request. An absent
refresh token retains the previous one. Save failure stops the operation; later
sync failure or cancellation does not undo saved credentials. Unedited draft
tokens follow rotation while explicit credential edits retain save-conflict checks.

Device names are inside encrypted snapshots; UUIDs remain the identity. A rename
appears on the next actual publication and does not cause a name-only upload.

Resolution confirms the displayed observed branches and selected revision.
Changed local content, changed branch sets and missing/unreadable selected
snapshots refresh the preview for another confirmation. A concurrent publication
arriving afterwards remains preserved and can cause a later conflict.

Automatic uploads, downloads, conflicts, failures and successful recovery are
recorded once per process across windows. Disabled, busy, debounce and unchanged
checks do not add entries. Identical classification/trigger/revision failures are
deduplicated for 15 minutes. HTTP diagnostics contain controlled classifications,
provider, operation and status, without URLs, signed addresses or response bodies.
History write failures do not change an already successful sync outcome.


## Background checks and traffic

Automatic checks inspect the pointer and every device head. An authenticated,
unchanged snapshot is fully downloaded and validated at most once per 24-hour
receipt lifetime; lightweight checks do not extend that lifetime. When S3
supports ETags, unchanged pointers return 304 and fresh listing validators allow
head metadata reuse. Services without conditional support fall back to GET.

Windows share memory-only caches within one process. Restarting, changing remote
configuration/accounts/credentials/master passwords, eviction and changed
branches may require full reads. Actual push, pull, recovery and conflict
confirmation still validate snapshots. Daily cleanup, upload readback and the
compatibility snapshot also consume traffic.

Local notifications with unchanged synchronized content can skip networking.
Continuous editing does not delay periodic remote checks, while upload debounce
still applies. Successful manual or automatic inspections postpone the next
periodic check and suppress duplicate focus checks for 120 seconds. Connection
tests do not count as inspections. Disabling startup checks does not disable the
15-minute periodic checks.

Other providers still transfer their pointer and directory API responses.
Gist/Snippet container responses may also include file contents. The daily limit
applies to explicit snapshot reads and validation; it does not guarantee that
every provider sends only metadata during a lightweight check.

Controlled logs summarize request counts, upload/download body bytes, 304s,
cache hits and full validations. All providers count at the HTTP boundary,
including authentication retries and consumed error bodies. Upload bytes measure
the buffered body submitted for each attempt; downloads include partial reads
before failure. These exclude headers and TLS and are not billing figures.
Summaries contain no addresses, object
paths, ETags, credentials or synchronized content. Actual traffic depends on
uptime, device count, snapshot size, edits, retries and storage capabilities.
