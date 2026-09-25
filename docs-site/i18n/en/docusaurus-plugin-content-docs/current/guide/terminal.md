---
sidebar_position: 3
---

# Terminal Features

NyaTerm's terminal experience is designed around high-frequency local and remote work inside one workspace. It is built on xterm.js, but the practical experience goes beyond a terminal canvas: search, command history, suggestions, optional enhancements, recording, and SSH-aware helpers are all part of it.

## Core operations

### Search, copy, and the context menu

The terminal context menu exposes these frequent actions directly:

- Copy / Paste
- Paste selected text
- Find text
- Search selected text online
- Translate selected text with a provider
- Clear screen / Clear all
- Select all

In **Settings → Interaction**, you can also adjust:

- **Copy on select**
- **Right-click paste**
- Word separators
- Default character encoding

### Scrollback, fonts, and zoom

- The default scrollback buffer keeps **10000 lines**
- You can customize font family, font size, normal font weight, bold font weight, cursor style, and cursor blink
- Use the menu or keyboard shortcuts to zoom in, zoom out, or reset zoom; the terminal root coordinates zoom handling so nested workspaces do not process it twice
- Character-art symbols (Braille Patterns U+2800-U+28FF and the Legacy Computing sextant/eighth-block/segmented-digit ranges) are drawn to fill the whole cell: the WebGL renderer rasterizes them itself and the DOM renderer uses the bundled `NyaTerm Symbols` font, so dot art such as chafa output, sparklines or mosaic graphics has no phantom line spacing
- **Hardware acceleration** is optional and is **not enabled by default**; you can toggle it manually in **Settings → Terminal** if you want to compare rendering behavior

### Paste and input compatibility

- Terminal settings can control image path pasting, which is useful when you want to pass screenshot or local image paths to command-line tools
- On macOS, enable the IME compatibility option in **Settings → Interaction** if composition text, candidate windows, or special key behavior feels wrong
- In interactive programs such as vim, less, or top, NyaTerm suppresses command suggestions so history completions do not interfere with the program's own input handling

## Command history and suggestions

NyaTerm provides two related helpers for session workflows.

### Command history

- Commands entered in the terminal are recorded automatically
- Fuzzy search is supported
- You can review history from the **Command History** panel on the right
- The search UI keeps recent queries so repeated log/error searches are easier

### Terminal find

`Ctrl / Cmd + Shift + F` opens terminal find. The find UI supports:

- Previous / next result navigation
- Current-result and total-result feedback
- No-result, searching, and invalid-regex states
- Reusing recent queries without retyping the full keyword

### Input suggestions

While typing, NyaTerm can suggest commands based on history. This is useful for repeated operational commands, build commands, and troubleshooting scripts.

## AI Assistant from the terminal

NyaTerm can invoke AI workflows directly from terminal context instead of forcing you to copy text into a separate tool.

### Common entry points

- **Explain recent output** when you want the latest terminal activity summarized immediately
- **Explain selected text** for a highlighted log fragment or command result
- **Analyze error** when terminal output already contains a failure you want help with
- **Generate command** when you want AI to propose the next step from the current session context

### Inline terminal capture

During Agent execution, NyaTerm can capture AI command execution events and show a compact inline preview of command output close to the terminal workflow.

The amount of inline output is controlled by `Terminal Output Lines` in **Settings → AI → Agent Settings**:

- `Terminal Output Lines` sets how many output lines are shown after each AI-executed command
- Set it to `0` to disable inline terminal previews
- This setting changes the AI workflow feedback, not the underlying shell execution itself

## Optional terminal enhancements

These features are intentionally opt-in rather than enabled all at once.

### Line numbers and timestamp gutter

In **Settings → Terminal**, you can enable:

- **Show line numbers**
- **Show timestamps**

When enabled, a gutter appears on the left side of terminal output. It is especially useful for long logs, command output, and recorded sessions.

### Action links

Action links are off by default. When enabled, NyaTerm can detect and open patterns such as:

- IPv4 addresses like `192.168.1.10`
- `host:port` pairs like `db.internal:5432`
- Archive names like `backup.tar.gz`

Notes:

- First enable **Action Links** in **Settings → Terminal**
- Opening a link requires **Ctrl / Cmd + click** to avoid conflicts with normal text selection
- The three matcher groups can be enabled or disabled separately

### Keyword highlighting

Keyword highlighting is also off by default. After enabling it, NyaTerm applies built-in rules and then overlays your custom rules.

The built-in rules cover more than error keywords. They also include:

- Common state words such as error / warn / success / info / debug
- Dates and times
- Numbers, sizes, and durations
- Structured text such as addresses, URLs, UUIDs, and versions

You can define your own rules with:

- A custom rule name
- Separate colors for dark and light themes
- One matching pattern per line
- An option to continue matching across wrapped lines

#### Import custom highlight rules

In **Settings → Terminal → Custom Rules**, click **Import** to import custom highlight rules from a JSON file.

Imports merge into your existing rules and do not clear the list:

- Rules with the same `id` are updated
- Rules with new `id` values are appended
- Empty `id` values are generated automatically
- Rules with an empty `name` or no valid `patterns` are skipped

The recommended JSON shape is below. A top-level rules array is also supported.

```json
{
  "keyword_highlights": [
    {
      "id": "deploy-errors",
      "name": "Deploy Errors",
      "patterns": ["deploy failed", "rollback required", "fatal"],
      "color_dark": "#ff7b72",
      "color_light": "#cf222e",
      "enabled": true
    }
  ]
}
```

### Large-output protection

When a session produces too much output too quickly, NyaTerm can enter a temporary protection mode so the terminal remains responsive.

During that period, the app temporarily suppresses some expensive decorations and reports how many queued characters were skipped. Once pressure drops, normal rendering resumes. This is mainly intended for log storms or constantly streaming output.

### Large-output drain

Terminal output now passes through a batched drain before it is written to xterm.js. During high-throughput situations such as `tail -f`, build logs, or load-test output, this helps preserve input responsiveness, scrolling, and search behavior.

This is not a setting you need to enable. It is the default terminal output pipeline and works together with large-output protection, line-number / timestamp gutters, and command suggestions.

## SSH-specific helpers

### Keep-Alive

For SSH sessions, you can configure a Keep-Alive interval in **Settings → Terminal**:

- Default is **60 seconds**
- Set it to `0` to disable it
- Useful for reducing idle disconnects on long-lived sessions

### Remote host monitoring panels

NyaTerm provides four right-side monitoring panels for SSH sessions: **Resource Monitor**, **GPU Monitor**, **Process Manager**, and **Docker Manager**. They share some behavior:

- They only make sense for an **SSH session**, and bind only to a genuinely active SSH session
- Each is shown or hidden by its own toggle in **Settings → Terminal**; turning a toggle off also hides its activity-bar icon
- Poll intervals are adjustable, ranging from **3 to 120 seconds**
- A panel stops refreshing after several consecutive polling failures, to avoid repeatedly hitting an unsupported host

#### Resource Monitor

Remote resource monitoring is on by default. To see data, both of these must be true:

1. The current tab is an **SSH session**
2. **Show Remote Resource Stats** is enabled in **Settings → Terminal**

When enabled, the **Resource Monitor** icon appears in the right activity bar and the panel polls the host on the configured interval. The default interval is **3 seconds**, and you can change it manually.

The panel displays:

- Hostname, OS, architecture, uptime
- Load average
- CPU usage
- Memory usage
- Network throughput

#### GPU Monitor

The **GPU Monitor** panel shows NVIDIA GPU status on the remote host. It is off by default; enable **Show GPU Monitor** in **Settings → Terminal** (default poll interval 3 seconds).

The panel displays:

- Driver version and CUDA version
- Summary: GPU count, highest utilization, memory usage, highest temperature
- Per-GPU card: index, model, performance state (pstate), utilization and memory bars; expand for UUID, temperature, power draw, fan speed, and free memory. Utilization above **70% / 90%** is color-coded differently
- A searchable GPU process list (filter by PID, GPU index, user, or process name), sorted by GPU memory used

If the remote host has no NVIDIA GPU or is missing `nvidia-smi`, the panel shows a matching empty state.

#### Process Manager

The **Process Manager** panel shows a live process list from the remote host. It is off by default; enable **Show Process Manager** in **Settings → Terminal** (default poll interval 5 seconds).

Key capabilities:

- Total process count and a search box (filter by PID, user, state, command, or full command line)
- Adaptive layout that adds or drops columns based on panel width; sort by process name, PID, CPU%, MEM%, or user
- Expand a process for PID/PPID, user, state, CPU%, memory%, RSS, elapsed time, and the full command line, plus adjusting the nice value (`-20` to `19`) and clicking **Apply** (renice)
- A row action menu to copy the PID or command, or send `TERM` / `HUP` / `STOP` / `CONT` signals; `KILL` first opens a confirmation dialog showing the `kill` command

If the remote host does not support process queries, the panel shows a distinct message.

#### Docker Manager

The **Docker Manager** panel manages Docker on the remote host. It is off by default; enable **Show Docker Manager** in **Settings → Terminal** (default poll interval 10 seconds).

Key capabilities:

- Overview: running / stopped container counts and image count, with the Docker engine version in the header
- Global search plus tabs for containers, images, volumes, networks, and Compose (when available); extra tabs collapse into a **More** dropdown
- **Containers**: a state-sorted virtualized list; a row menu views logs (runs `docker logs -f` in the terminal), enters the container (opens a shell), starts / stops / restarts / kills (confirm) / removes (confirm); clicking a row opens a live-refreshing details dialog
- **Images / Volumes / Networks**: fetched on demand, each row supports removal (confirm)
- **Compose**: lists projects; expand to lazily load services; supports project-level up / restart / down and service-level logs / enter / up / stop / restart
- The **More** menu offers `docker system prune` (destructive, confirmed)

Logs and enter-container actions run in the real terminal session; remove, kill, Compose down, prune, and other destructive operations route through a confirmation dialog showing the exact command.

## Translation and online search

After selecting text in the terminal, you can use the context menu to:

- Send the selection to an online search engine
- Open a translation dialog with an enabled translation provider

Provider visibility depends on settings:

- **Google** and **Microsoft** work without extra credentials
- **DeepL / Baidu / Alibaba / Youdao** appear after you enter credentials in **Settings → Translation**

## Recording and workflow combinations

NyaTerm supports session recording, which is useful for:

- Preserving troubleshooting steps
- Sharing a reproducible path with teammates
- Capturing terminal examples with visible timing

In **Settings → Transfer**, you can also tune recording behavior:

- **Auto-start recording**: begin recording as soon as a session opens, so you never forget to start it for sessions you always want captured
- **Include timestamps**: write timestamps into the saved transcript, which helps with auditing or correlating output with wall-clock time

You can also configure recording on individual saved connections. This is useful when a policy belongs to the connection itself, such as "always record production bastion sessions" or "do not record this lab device", instead of deciding manually after each session opens.

If you are preparing screenshots or demos, a good combination is:

- Line numbers / timestamp gutter
- Keyword highlighting
- Action links
- Command history
- Resource monitor

That usually gives a more realistic screenshot than showing one toggle in isolation.

:::tip Screenshot suggestion
- Suggested image path: `/img/docs/terminal/gutter-line-numbers-timestamps.png`
- Enable line numbers and timestamps in **Settings → Terminal**, then run `scripts/demo-terminal-gutter.sh`
- Another good image path: `/img/docs/terminal/action-links-and-highlights.png`
- Enable action links and keyword highlighting, then run `scripts/demo-terminal-output.sh` and `scripts/demo-action-links.sh`
:::
