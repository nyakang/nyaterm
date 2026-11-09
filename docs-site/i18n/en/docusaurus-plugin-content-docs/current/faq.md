# FAQ

## Sessions and connections

### SSH works, so why do Local Terminal, Telnet, Serial, RDP, and VNC behave differently?

Because NyaTerm supports multiple session types, and their capabilities are not identical:

- **SSH** — the most complete workflow, including SFTP, OTP, resource monitoring, proxy, jump host, and tunnels
- **Local Terminal** — local shell workflow only
- **Telnet** — lightweight remote terminal without SSH-specific features
- **Serial** — serial debugging, not an SSH network path
- **RDP** — graphical remote desktop without text-terminal behavior
- **VNC** — traditional RFB graphical desktop; some real-server combinations are still being validated

If you need the file explorer, remote resource monitoring, or OTP, make sure the current tab is an **SSH session**.

### Why is the file explorer missing for some sessions?

The file explorer depends on SFTP, so it is only available for **SSH sessions**.

These session types do not provide the remote file explorer:

- Local Terminal
- Telnet
- Serial
- RDP
- VNC

### Why can’t I see remote resource monitoring?

Check both of these:

1. The current tab is an **SSH session**
2. **Show Remote Resource Stats** is enabled in **Settings → Terminal**

Resource monitoring is on by default; turning this setting off also hides the Resource Monitor icon in the right activity bar.

### What should I do if the serial port list is empty?

Check that:

- The device is physically connected
- The operating system recognizes the serial port
- Another tool is not already holding the port open

When you reopen the port dropdown on the Serial tab, NyaTerm reloads the available ports.

## Terminal experience

### Why can’t I click action links?

Usually one of these is true:

1. **Action Links** is not enabled in **Settings → Terminal**
2. You are not using **Ctrl / Cmd + click**

Action links are disabled by default, and opening them requires a modifier key to avoid accidental activation.

### Why can’t I see keyword highlighting?

Keyword highlighting is disabled by default. Enable it first in **Settings → Terminal**, then confirm the current output actually matches one of the configured rules.

### Why are line numbers and timestamps not visible?

These are also optional enhancements. Enable them separately in **Settings → Terminal**.

### Why are there no models available in the AI Assistant?

Check, in order:

1. Whether AI is enabled in **Settings → AI**
2. Whether at least one provider is configured and enabled
3. Whether at least one model is enabled for that provider

When using a custom provider, confirm it is OpenAI-compatible and that the base URL, API key, and model discovery results are all correct.

### Why doesn't the app quit when I close the window?

Check **Minimize the last window to tray on close** in **Settings → General**.

When enabled, only closing the last workspace window hides it to the tray; active sessions and sync continue. With multiple windows, × closes the current workspace normally. Launching again or clicking the tray wakes an existing window. Choose **Quit NyaTerm** to exit the entire app. The session-tab restoration setting does not control workspace window restoration, and existing extra windows are not deleted automatically.

## File transfer

### Why didn’t the auto-upload prompt appear after I opened a remote file?

The auto-upload prompt only appears in this workflow:

1. You choose **Open** on a remote file from the SSH file explorer
2. NyaTerm downloads it into a local temporary directory and starts watching it
3. You save that watched file in your local editor

If you copied the file elsewhere and edited that copy manually, NyaTerm no longer knows it maps back to the remote file.

### Why didn’t the file explorer follow my `cd` command automatically?

Auto-follow depends on terminal path tracking support for the session. If the current session does not support it, automatic sync is disabled and you need to trigger sync manually.

### Where do uploads and downloads go?

That depends on your transfer settings:

- If **ask every time** is enabled, NyaTerm prompts for a destination on each download
- Otherwise it uses the default download directory

You can also change the default download path and the default editor in settings.

## Security and authentication

### Why can I unlock the screen without entering a password?

Because screen lock is enabled, but **no master password is set yet**.

In the current behavior:

- With a master password: unlocking requires the master password
- Without a master password: unlocking can be done directly

### What if I forget the master password?

There is currently no built-in recovery flow for the master password. If your local data is protected by it and you can no longer provide the correct password, those protected sensitive settings cannot continue to be used in the original way.

Before making manual changes, back up the data directory for the current mode: usually `~/.nyaterm/` for an installed build, or the adjacent `data/` directory for portable mode.

### Where should OTP entries be managed?

Manage them centrally in the **OTP** tab of the **Security/Auth** panel, then bind them to individual SSH connections in the connection form.

## Import and migration

### Which clients can NyaTerm import sessions from?

Xshell, MobaXterm, WindTerm, SecureCRT, FinalShell, Termius, and NyaTerm / Electerm JSON. Each format and its caveats are documented in [Importing sessions from other clients](./guide/ssh-connection#import-sessions-from-other-clients).

After import, review the username, port, authentication method, and whether proxy / jump host / OTP binding still needs to be configured.

### Where are NyaTerm’s config files stored?

In installed mode, the main data file is normally `~/.nyaterm/nyaterm.redb`; portable mode uses configuration data under the adjacent `data/config/` directory. This data includes settings, connections, keys, OTP data, tunnels, proxies, history, and other local state.

Legacy Dragonfly data is handled through compatibility fallbacks for old encryption prefixes and storage documents. NyaTerm does not promise to copy the entire `~/.dragonfly/` directory or create a rollback copy on first launch.
