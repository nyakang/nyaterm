# SFTP File Transfer

SSH sessions support **remote file browsing, SFTP transfers, and local editing with upload back to the server**.

## File explorer

After connecting an SSH session, the file explorer panel lets you browse remote directories directly.

Core capabilities include:

- Automatically entering the remote user's home directory
- Entering folders, going up, and jumping by typing a path
- Refreshing the current directory
- Syncing with the terminal's working directory
- Disabling auto-sync when the session does not support path tracking

## Common file operations

From the file list or the context menu, you can perform:

| Operation | Description |
|------|------|
| Open | Download to a local temp directory, then open with the default editor |
| Upload File | Upload a local file to the current remote directory |
| Upload Folder | Upload a full local directory tree |
| Download | Download a file or an entire directory |
| Rename | Change the remote name |
| Move | Move a file or directory to another path |
| Delete | Remove a file or directory |
| Properties | View size, timestamps, UID/GID, permissions, and more |
| New File / Folder / Symlink | Create entries directly in the current directory; symlinks use OpenSSH-compatible handling |

Use **Open** to edit a file locally and upload changes back to the server.

## Selection and bulk operations

The file explorer supports the selection model you would expect from a file manager:

- Click to select a single entry
- `Ctrl / Cmd` for multi-select
- `Shift` for range select
- `Ctrl / Cmd + A` to select every entry in the current directory
- `Delete` to delete the current selection

With multiple entries selected, the toolbar's **Download** and **Delete** act on the whole selection. Opening the context menu on an already-selected entry while multi-selecting also applies the bulk actions to the entire selection.

## Uploads and downloads

### Upload

Use the toolbar, context menu, or drag and drop to upload local content into the current remote directory.

- Multiple files are queued one by one
- Folder uploads preserve directory structure
- Good for syncing scripts, config files, or release packages

### External drag-and-drop upload

The file explorer supports dragging **files or folders from your system file manager directly into the NyaTerm file browser** for upload.

Typical flow:

1. Open an SSH session and switch to the file explorer
2. Drag a local file or folder into the file list area
3. Release when the drag overlay appears
4. NyaTerm adds the dropped items to the upload flow automatically

Notes:

- Upload is only triggered when you drop onto the file browser list area
- Some drag sources do not expose a real local file path. In that case, NyaTerm cannot resolve the dropped item directly and will prompt you to use **Upload File** or **Upload Folder** instead

### Download

Downloads usually follow one of two workflows:

- Save directly into a default download directory
- Ask for a destination every time for ad hoc troubleshooting or task-based organization

Both file downloads and directory downloads are supported.

## Transfer panel and transfer settings

NyaTerm puts uploads and downloads into a shared transfer queue so you can inspect:

- Current progress
- Real-time transfer speed
- Success, paused, canceled, and failed states
- Concurrent transfers
- The current download target

Each transfer item supports:

- **Pause**
- **Resume**
- **Cancel**
- **Retry after failure**
- **Remove after completion**

The panel also provides bulk actions:

- **Pause All**
- **Resume All**
- **Cancel All**
- **Clear Completed**

In **Settings → Transfer**, you can adjust:

- Upload / download thread count
- Conflict handling strategy
- Maximum retry count
- Transfer buffer size
- Whether to preserve timestamps
- Whether to continue resumable transfers
- Default file permissions
- Default download path
- Whether to ask for the save location every time
- The local editor used when opening remote files

### Duplicate target handling

When an upload or download reaches an existing file, the duplicate target dialog lets you choose:

- **Overwrite** — replace the target file
- **Overwrite and do not ask again for this task** — keep overwriting for the current batch
- **Skip** — keep the target file and skip the current item

If you often sync the same directories in batches, tune the default conflict strategy in **Settings → Transfer** to reduce repeated prompts.

### SFTP reliability and performance

The newer SFTP backend improves directory handling, symlinks, known-size downloads, and large uploads. You usually do not need to think about the implementation details; the visible result is clearer speed feedback, more specific errors, and directory/symlink behavior that is closer to OpenSSH.

NyaTerm also limits how many SFTP channels run concurrently on a single SSH connection, and automatically retries with backoff when a channel open hits a transient failure. This keeps multiple file operations on the same host more stable, so a momentary shortage of channel resources is less likely to surface as an error. These are backend behaviors with no settings to configure.

## Zmodem transfers (rz / sz)

Besides SFTP, NyaTerm supports the Zmodem transfers common in the terminal — the ones triggered when you run `rz` (upload to the remote) or `sz` (download from the remote) on the host.

- When `rz` is detected, NyaTerm prompts you to pick the local files to upload; before uploading it probes the remote directory for name conflicts and resolves them
- When `sz` is detected, NyaTerm prompts you for a local save directory
- Zmodem transfers appear as items in the transfer queue with live progress

Note that the controls available for Zmodem transfers differ from regular SFTP transfers:

- Zmodem items **do not** support pause, resume, retry, or cancel
- Once a transfer finishes, you can only remove it from the list
- The panel's **Pause all / Resume all / Retry all** actions skip Zmodem items automatically

This is because the Zmodem protocol is driven directly by both ends of the terminal; NyaTerm only handles progress display and file selection, and does not intervene in the protocol's own pause/resume behavior.

## trzsz transfer (`trz` / `tsz`)

NyaTerm also supports in-terminal trzsz transfers. Run `tsz` on the remote side to download files, or `trz` to upload files; directory uploads open a local directory picker. These transfers appear in the queue with progress information.

trzsz, like Zmodem, is driven by the terminal protocol and is not a replacement for SFTP file operations. Use it when SFTP is unavailable but the remote host provides the trzsz commands. Pause, resume, and retry behavior follows the active protocol session and should not be treated like an SFTP queue job.

## Sync with terminal paths

The file explorer can work together with the current SSH terminal path:

- **Manual Sync** — jump the explorer to the terminal's current directory
- **Auto Sync** — automatically follow when the terminal changes directories

After you `cd` into a deploy or log directory in the terminal, the file explorer follows to the same path.

## Edit locally and upload back automatically

This is one of NyaTerm's most practical workflows for real operations work.

### How it works

1. In the SSH file explorer, choose **Open** on a remote file
2. NyaTerm downloads it into a local temp directory
3. A file watcher is started
4. After you save in your local editor, NyaTerm opens an upload prompt

NyaTerm fingerprints the watched file by content (it hashes the content for smaller files and falls back to size and modification time for larger ones). The upload prompt only fires when the file's **content** actually changes; saves that only touch the timestamp or that write identical content will not trigger a spurious upload. This also handles atomic saves (write a temp file, then rename) correctly.

### Upload prompt window

After the file changes, you can choose:

- **Upload once**
- **Always upload**
- **Cancel**

If you choose **Always upload** for a file, later saves in the **current session** are sent back automatically without prompting again.

### Only uploads when content actually changed

The watcher decides by content fingerprint, not just save events or modification times:

- When content is unchanged (an editor only bumping the mtime, for example), no upload is triggered
- Only a real content change opens the upload prompt or performs the automatic upload
- Atomic saves that write a temporary file and rename it are recognized correctly

This avoids duplicate uploads caused by editor save behavior.

### Typical uses

- Editing remote config files
- Tweaking deploy scripts
- Pulling a file locally for inspection, then sending changes back
- Preparing screenshots that demonstrate the round-trip editing flow

## File properties and permissions

The **Properties** view shows:

- File size
- Modified time and access time
- Owner and group
- UID / GID
- Octal permission values

If your workflow requires checking permissions before replacing a file, this is often clearer than relying only on `ls -l`.

## Temporary sudo root access

Choose **Enable sudo** in a remote file path bar and enter the sudo password, or leave it empty for passwordless sudo. After authentication, SFTP initialization and a directory check succeed, **sudo · root** remains visible. Click it again to disable. The SSH session's tmux panes, sidebar and file-manager windows share the grant; separate SSH sessions authorize separately.

Manual browsing, previews, transfers, file operations and editor saves use this identity. AI/MCP and terminals retain the login identity. The password stays only in connection memory; disconnecting or closing the session revokes access. Nothing is saved in configuration, backups or cloud sync. Locking cancels pending authorization without interrupting already authorized transfers.

Finish or cancel running, queued and paused transfers, deletions and saves before switching. Switching refreshes listings and invalidates old drag gestures. Editors preserve unsaved changes and revisions; saving still checks conflicts and preserves UID/GID and permissions. External edits made during a switch remain pending for upload.

The server must allow SSH exec, sudo root and OpenSSH `sftp-server`. `su`, `doas`, policies requiring a PTY, SFTP-only sessions and disabled file services are unsupported. Startup failures never fall back to ordinary access or a new SSH connection. Expired grants require disabling and authorizing again. The current directory stays unchanged; return home if it becomes inaccessible after disabling sudo.
