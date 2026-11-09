# Layout & Workspace

NyaTerm provides a workspace with multiple sessions and independent tab groups, with common tools docked around the sides of the app.

## Workspace areas

A typical workspace is made up of these areas:

- **Center area** — tabs and terminal panes
- **Left activity bar / panels** — file explorer, network, Security/Auth, Cloud Sync
- **Right activity bar / panels** — saved connections, active sessions, command history, resource monitor
- **Bottom helper area** — quick commands, serial send, recording, lock actions

These areas are not isolated pages. They cooperate around the currently active session.

## Tabs

Each tab holds one session. Each split area has its own tab bar and selected tab.

Common tab actions include:

- Creating a new session
- Closing the current tab
- Switching between tabs
- Renaming a tab
- Setting a tab color
- Duplicating the current session, optionally with a startup command
- Dragging a tab into another dock/split area
- Reconnecting a session
- Viewing session details

Tab names and colors can separate environments, projects, or task phases.

## Command Palette

The **Command Palette** is a searchable overlay opened with `Ctrl / Cmd + Shift + S` (labeled **Open Command Palette** in settings). It searches three kinds of entries:

- **Active sessions** — tabs you already have open
- **Pending sessions** — sessions that were started but have not connected yet, including the failure reason
- **Saved connections** — entries from your connection list; selecting one starts the connection

Use the arrow keys to select and Enter to open. When you have many tabs and connections open, this is the fastest way to navigate.

The palette only searches sessions and connections. It does **not** contain app-level actions such as opening settings, toggling panels, or locking the screen — those live in the top menu and keyboard shortcuts.

## Split panes

Right-click a tab to split the current session into:

- **Split Below** duplicates the current session into a new group below the current area.
- **Split Right** duplicates the current session into a new group to its right.

The new group appears after the connection succeeds. A failed connection stays as a failed tab in the source area, while other groups remain usable. Drop a tab in an area's center to join its group, on an edge to create a split, or in its tab bar to reorder it. Moving the last tab out collapses an empty group. Typical uses:

- Watching logs in one pane and running commands in another
- Comparing two hosts side by side
- Keeping a local terminal next to a remote SSH session
- Watching serial output while running SSH troubleshooting commands

## Sessions and workspace structure

Each group displays its selected session. Clicking a terminal or tab chooses the input target. `Ctrl+Tab`, `Ctrl+Shift+Tab`, and numbered tab shortcuts switch tabs within the current group. Previous/next group commands can be assigned shortcuts in settings.

**Smart Split** arranges existing sessions. Auto mode chooses split directions using the center area's actual dimensions; when space is limited, a group keeps multiple tabs. **Unsplit** merges every group in layout and tab order, preserving connections and the active session.

Drag a divider to resize groups, or double-click it to restore equal sizes. Shrinking the window preserves the layout. Focus mode temporarily displays only the current session; leaving it restores the layout.

## Layout restoration and workspace padding

NyaTerm saves the tab group layout and restores tabs, splits, and active positions when the workspace is reopened. Older splits inside a tab convert to independent tab groups, preserving their directions, proportions, names, colors, and locks.

If you want clearer visual separation between terminal areas, adjust **Workspace Padding** in terminal-related settings. This only changes the workspace spacing; it does not change remote or local shell behavior.

## Left and right panels

### Left side

The left side is mainly for capability entry points:

- File explorer
- Network
- Security/Auth
- Cloud Sync

The **Cloud Sync** panel is a runtime entry point for cross-device config sync. It surfaces current status, recent sync activity, and direct conflict-handling actions when a conflict is detected.

The **File Explorer** can follow the current SSH terminal working directory and can send a directory or file path back to the terminal.

### Right side

The right side is mainly for live state and navigation:

- Saved connections
- Active sessions
- Command history
- Resource Monitor, NVIDIA GPU Monitor, Ascend NPU Monitor, Process Manager, and Docker Manager (SSH sessions)

The five monitoring panels only make sense for SSH sessions, and each is shown or hidden by its own toggle in **Settings → Terminal**; turning a toggle off also hides its activity-bar icon. Resource Monitor, Process Manager, and Docker Manager are on by default; the two accelerator panels are off. See [Remote Host Monitoring](./remote-monitoring).

## AI Assistant panel

The **AI Assistant** is a persistent panel on the right rather than a separate popup, so it always has the active pane as context:

- Generate a terminal command from natural language
- Explain recent terminal output or the current selection
- Analyze an error and suggest a fix
- Produce an approvable command card and run it
- Save a command as a quick command

## Bottom helper area

The bottom area carries features that do not need to occupy a sidebar permanently:

- **Quick Commands** — reusable command snippets with variable prompts
- **Serial Send** — repeatedly send fixed text to a serial device
- **Recording** — start or stop session recording
- **Lock** — lock the app

## Child windows

These flows open dedicated child windows so they do not interrupt the active session in the main workspace:

- Settings
- New session / connection creation
- Quick command editing
- Remote file editing
- Auto-upload prompts

Every child window opens centered on the main window, on whichever display the main window is on, gets its own taskbar entry, and can be minimized, maximized, and restored. Triggering the same action again raises the window that is already open instead of opening a second one.

**Connection creation** and **quick command editing** each own an exclusive draft, so they are modal windows at the operating-system level: the main window is blocked by the system while one is open, and is restored and refocused when it closes. **Settings** is modal too on Windows and Linux; on macOS it is an independent window, because a modal child window becomes a sheet attached under the title bar there, and Apple's convention is that Settings gets a window of its own.

**Remote file editing** is an independent document window and does not block the main window -- editing a remote file usually means going back to the terminal to read output or copy something, and locking the workspace would get in the way. The **auto-upload prompt** does not block it either, but it does stay on top: it appears after you have finished editing a file in an external editor, and a prompt you cannot see is a prompt that never gets answered.

`Ctrl / Cmd + W` closes the current child window; connection creation, quick command editing, and the auto-upload prompt also respond to `Esc`. The settings window does not respond to `Esc`, and closing it with unsaved changes asks for confirmation first.

Child-window position and size are not persisted yet, so each one re-centers when it opens.

## Tray and window state

Enable **Minimize the last window to tray on close** in **Settings → General** to keep the last workspace running in the background when you click ×. With multiple workspace windows, × follows the normal confirmation, save, and cleanup flow, closes that workspace, and removes it from the restore list. The next cold launch restores only retained workspaces. Hidden workspace windows still count toward the total.

Launching the app again, clicking the tray icon, or choosing **Show Most Recent Window** wakes an existing window. Use **New Window** in the menu or tray, or `--new-window`, to create an independent workspace. **Quit NyaTerm** saves and exits the entire app. Active sessions, sync, and backup jobs continue while the last window is hidden; if the tray is unavailable, closing minimizes the window normally.

The session-tab restoration setting only controls session tabs; independent workspace windows still restore from the saved list. Existing extra workspaces are not deleted automatically; close unwanted windows individually.

## Synchronized input groups

A synchronized input group ties several terminal sessions together: keystrokes in one session are mirrored to the other unpaused sessions in the group. Command preview and history are only generated on the session you actually typed in.

Groups are runtime state and are not persisted, so they need to be recreated after an application restart.

### Creating and managing groups

1. Open the **Synchronized Input Groups** panel with `Ctrl / Cmd + Shift + G` or from the right activity bar
2. Click **New group**, then set a name and color
3. Pick sessions to join from the active session list

With many active sessions you can filter by **All**, **Available** (not yet in this group), **Joined**, or **Same host**, and the search box filters by name, type, host, or session ID.

### Broadcast control

- The whole group can be enabled or paused
- Individual sessions in the group can be paused separately
- Sessions that have closed are marked Missing and can be cleared in one action

### Send command panel

The bottom **Send Command** panel offers target selection: the current session, all sessions, or a specific `Group: <name>`. A group target is filtered automatically by session type (serial versus shell) and excludes paused or duplicate sessions.

The quick commands panel also offers **Send to all sessions**, which sends a command to every active session at once.
