# Keyboard Shortcuts

The easiest way to understand NyaTerm shortcuts is to split them into two groups:

1. **App-level shortcuts** — toggle panels, create sessions, copy terminal content; handled by NyaTerm
2. **Shell-level keys** — keys sent to the remote or local shell, such as `Ctrl+C`

Copying terminal text needs an app-level shortcut; `Ctrl+C` in the shell will not become a copy action.

## Conventions

- **Ctrl / Cmd** means `Ctrl` on Windows/Linux and `Cmd` on macOS
- The groups below match the categories in **Settings → Keybindings**, so the two are easy to cross-reference
- Every shortcut is rebindable — see [Customizing shortcuts](#customizing-shortcuts)

## Terminal operations

| Shortcut | Action |
|----------|--------|
| `Ctrl / Cmd + Shift + C` | Copy (copies visible terminal text when there is no selection) |
| `Ctrl / Cmd + Shift + V` or `Shift + Insert` | Paste |
| `Ctrl / Cmd + Shift + X` | Paste selected text |
| `Ctrl / Cmd + Shift + F` | Find |
| `Ctrl / Cmd + L` | Clear screen |
| `Ctrl / Cmd + Shift + A` | Select all visible terminal content |
| `Ctrl / Cmd + Shift + G` | Manage synchronized input groups |
| `Alt + R` | Show command suggestions (recent history and pinned quick commands when the input line is empty) |
| `Ctrl / Cmd + Shift + R` | Start or stop recording the current session |

The terminal itself also accepts `Ctrl / Cmd + V` and context-menu paste.

## Tabs and sessions

| Shortcut | Action |
|----------|--------|
| `Ctrl / Cmd + Shift + N` | New SSH session (opens the saved connections page) |
| `Ctrl / Cmd + Alt + N` | Open the temporary SSH link dialog |
| `Ctrl / Cmd + Shift + S` | Open Command Palette |
| ``Ctrl / Cmd + ` `` | New local terminal |
| `Ctrl / Cmd + Shift + W` | Close current tab |
| `Ctrl + Tab` | Next tab |
| `Ctrl + Shift + Tab` | Previous tab |
| `Ctrl / Cmd + 1-8` | Jump to tabs 1 through 8 |
| `Ctrl / Cmd + 9` | Jump to the last tab |
| `Ctrl / Cmd + Shift + D` | Duplicate current session |
| `Ctrl / Cmd + Shift + M` | Multiplex the current SSH connection |
| `Ctrl / Cmd + Alt + D` | Duplicate current session with a startup command |
| `Ctrl / Cmd + Alt + M` | Multiplex SSH with a startup command |

## View and layout

| Shortcut | Action |
|----------|--------|
| `Ctrl / Cmd + Shift + E` | Toggle the left sidebar |
| `Ctrl / Cmd + Shift + B` | Toggle the right sidebar |
| `Ctrl / Cmd + =` | Zoom in |
| `Ctrl / Cmd + -` | Zoom out |
| `Ctrl / Cmd + 0` | Reset zoom |
| `Ctrl / Cmd + ,` | Open settings |
| `Ctrl / Cmd + Alt + I` | Open AI assistant |
| `Ctrl / Cmd + Shift + P` | Show/hide the bottom quick commands panel |

`Ctrl / Cmd + Shift + P` is labeled **Show/Hide Quick Commands Panel** in settings. Opening it focuses the search field; pressing the shortcut again hides the panel and returns focus to the workspace. Reopening preserves filters and panel height. The welcome screen’s **Open Quick Commands** action only opens the panel.

`Ctrl / Cmd + Shift + S` opens the Command Palette overlay to search workspace tabs, connecting sessions, and saved connections.

## File explorer

| Shortcut | Action |
|----------|--------|
| `F2` | Rename the selected file or directory |

This shortcut applies only while the file explorer has focus.

## Saved connections

| Shortcut | Action |
|----------|--------|
| `Ctrl / Cmd + Alt + C` | Copy the selected saved connections |

## Special

| Shortcut | Action |
|----------|--------|
| `Ctrl / Cmd + Shift + L` | Lock the screen |

## Customizing shortcuts

**Settings → Keybindings** lists all 32 shortcuts grouped by the six categories above, and all of them can be changed:

- Click a shortcut to start recording, press the new combination, then press Enter or **Save**
- If the combination is already taken, the conflicting entry is named and nothing is overwritten
- Changed entries are marked **Custom**; you can **Reset** one or **Reset all**
- The search box filters by name

**Switch to tab 1-9** is a template shortcut: record it by pressing your modifiers plus the digit `1`, and it expands to `1` through `9` at runtime.

## Practical advice

- Copying logs from the terminal often: remember `Ctrl / Cmd + Shift + C`
- Managing remote and local sessions together: `Ctrl / Cmd + Shift + N` and ``Ctrl / Cmd + ` ``
- Using the lock screen: remember `Ctrl / Cmd + Shift + L`
