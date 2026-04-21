---
sidebar_position: 8
---

# Sync & Backup

Dragonfly's **Sync & Backup** capability is not about remote file transfer. It is about **syncing portable application configuration across devices and keeping recoverable backups of that data**.

It helps to think of it as two related but different workflows:

- **Sync** — push the current device's portable configuration snapshot to the cloud, or pull the latest snapshot back from the cloud
- **Backup** — save the current state as a recoverable encrypted backup snapshot, either manually or on a schedule

Dragonfly currently supports two remote storage provider types:

- **WebDAV**
- **S3-compatible** storage

## Prerequisite: set a master password first

Before using **Settings → Sync & Backup**, you must first set a **Master Password** in **Settings → Security**.

In the current implementation this is a prerequisite, not an optional recommendation. Without a master password:

- Sync & Backup cannot be enabled
- Manual actions such as test, push, pull, and backup cannot be run
- Scheduled encrypted backups cannot be used

That requirement exists because Dragonfly uploads **encrypted portable snapshots**, not plain-text config files.

## Where to access it

This feature has two main entry points.

### 1. Settings → Sync & Backup

This is the primary configuration page. Use it to:

- choose a provider
- configure the remote namespace and provider root
- define automatic sync behavior
- define scheduled backup behavior
- run manual actions
- inspect remote backups
- resolve conflicts when they happen

### 2. The Sync & Backup panel in the workspace

The main workspace also includes a **Sync & Backup** panel in the side UI. It is useful for checking:

- current status
- recent sync / backup activity
- last check, sync, and backup times
- conflict state and quick resolution actions

Use the settings page to configure the feature, and the workspace panel to watch its live state.

## Provider configuration

### Common fields

No matter which provider you use, start by checking these fields:

- **Enable Sync & Backup**
- **Provider Configuration**
- **Device Name**
- **Remote Namespace**

These have specific meanings:

- **Device Name** is written into snapshot metadata so you can tell which device uploaded it
- **Remote Namespace** is the top-level path prefix Dragonfly uses for sync and backup snapshots inside the selected provider

### WebDAV

When the provider is **WebDAV**, you will typically configure:

- **WebDAV Endpoint**
- **Provider Root** (optional)
- **Username**
- **Password**

This is a good fit when:

- you already have a NAS, private cloud, or WebDAV gateway
- you want to reuse existing document or object-storage infrastructure

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

This is a good fit when:

- you already use object storage for operational assets
- you want Dragonfly config and backups to live in the same storage ecosystem

### Validation rules

The current implementation performs basic provider validation before actions are allowed:

- WebDAV requires an endpoint
- S3 requires both an endpoint and a bucket
- `Access Key ID` and `Secret Access Key` must be provided together

A good habit is to run **Test Connection** before enabling any automatic strategy.

## Automatic sync strategy

The **Automatic Sync Strategy** section controls two behaviors.

### Check on startup

When enabled, Dragonfly checks the remote provider during startup to see whether a newer sync snapshot already exists.

Typical outcomes include:

- local and remote are already aligned
- the remote side has a newer snapshot available to pull
- local changes are pending upload
- both local and remote changed, so a conflict is detected

This is not real-time remote watching. It is a **startup check of the current state**.

### Auto-push after local changes

When enabled, Dragonfly automatically pushes a snapshot after supported local configuration changes are saved, using a debounce window.

You can control:

- **Sync Debounce Seconds**

That makes the behavior closer to “push shortly after save” than to real-time bidirectional sync.

## Scheduled backup strategy

Sync and backup solve different problems:

- **Sync** helps another device get the latest portable state
- **Backup** preserves a recoverable history of snapshots

In **Scheduled Backup Strategy**, you can configure:

- **Enable Scheduled Backups**
- **Backup Interval (hours)**
- **Retention Count**

The current implementation creates **encrypted backup snapshots** on the configured schedule and deletes older remote backup entries once they exceed the retention limit.

## Manual actions

The **Manual Actions** section is useful during first-time setup, troubleshooting, or cautious rollout.

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

### Run Backup Now

Immediately create a new remote encrypted backup snapshot.

Useful when:

- you want a recovery point before making larger changes
- you are about to migrate devices, reinstall, or do risky troubleshooting

## Remote backups and restore

The **Remote Backups & Conflict** section lists the backup entries currently available in the cloud. For each entry you can inspect details such as:

- backup time
- revision
- device identifier
- payload hash
- app version

### Restore a remote backup

When you restore a remote backup, Dragonfly applies that backup snapshot to the current device.

Useful scenarios include:

- bringing a new device up with familiar configuration quickly
- rolling back after a bad change
- returning to a known baseline during troubleshooting

Important boundaries:

- restore overwrites the **portable local data** included in the snapshot
- it is not a field-by-field merge
- it is better understood as restoring a complete snapshot

## How conflicts happen

In the current implementation, a conflict occurs when **both local state and remote state changed since the last sync baseline**.

A typical example looks like this:

- Device A changed settings but has not pulled Device B's update
- Device B already pushed a newer sync snapshot to the cloud
- Device A then tries to push or performs a startup check and discovers both sides changed

When this happens, Dragonfly can show conflict details such as:

- local snapshot hash
- remote revision
- remote device information
- a human-readable conflict message

## How conflict resolution works

The current implementation offers two resolution actions.

### Download Remote Version

Pull the remote snapshot and apply it to the current device.

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

## What gets synced or backed up?

The current implementation is built on **portable snapshots**. These cover Dragonfly's portable configuration data, such as:

- saved connections and groups
- key, password, and OTP configuration
- proxies and tunnels
- quick commands
- most application settings

Backup snapshots also include additional command-history data so recovery can bring back a fuller working environment.

But you should not think of this as “every piece of UI state roams across devices.” The current implementation deliberately preserves some device-local UI state, such as:

- currently open tabs
- live panel open/close and sizing state

So the feature is best understood as:

- **syncing portable configuration**
- **recovering the core configuration layer of a working environment**

not as a full desktop-session image.

## Recommended first-time setup order

If you are enabling this for the first time, this order works well:

1. Set a master password in **Settings → Security**
2. Open **Settings → Sync & Backup**
3. Choose the provider, then fill in endpoint / bucket / root / credentials
4. Set the device name and remote namespace
5. Run **Test Connection** first
6. Then decide whether to enable:
   - check on startup
   - auto-push
   - scheduled backups
7. Finally run **Push Now** or **Run Backup Now** once to verify the full path works

## Troubleshooting hints

If buttons are disabled or actions are blocked, check these first:

- whether a master password has been configured
- whether current settings have been saved or applied
- whether provider-required fields are complete
- whether the remote namespace, provider root, and endpoint values are correct

If your main question is “what happened recently?”, start with the workspace **Sync & Backup** panel. If your main goal is “change config or restore a backup”, go back to the settings page.

:::tip Practical advice
- When connecting a new provider for the first time, run **Test Connection** before enabling automatic strategies.
- If you are about to make large changes to connections, OTP, or quick commands, create a manual backup first.
- If you switch between devices often, keep a clear device-name convention and a predictable remote-namespace strategy.
:::
