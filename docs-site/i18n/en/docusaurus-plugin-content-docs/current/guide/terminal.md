# Terminal Features

This page covers terminal basics, command history and suggestions, the optional enhancements, SSH-specific helpers, and session recording.

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

- The default scrollback buffer keeps **5000 lines**, adjustable from **100 to 100000**
- You can customize font family, font size, normal font weight, bold font weight, cursor style, and cursor blink
- Use the menu or keyboard shortcuts to zoom in, zoom out, or reset zoom; the terminal root coordinates zoom handling so nested workspaces do not process it twice
- **Hardware acceleration** is enabled by default; turn it off in **Settings → Terminal** when diagnosing rendering or driver issues

### Paste and input compatibility

- **Multi-line paste prompt** is on by default. Pasting multi-line text first opens a review window where you can paste directly, send line by line, or cancel
- Terminal settings can control image path pasting, so you can pass screenshot or local image paths to command-line tools
- **Allow OSC 52 clipboard writes** controls whether remote applications can write to your local clipboard through terminal output. It lets remote tmux or vim copy straight to your machine, at the cost of giving remote processes the ability to write your local clipboard
- On macOS, enable the IME compatibility option in **Settings → Interaction** if composition text, candidate windows, or special key behavior feels wrong
- On macOS, enable **Treat Option as Meta/Alt** in **Settings → Interaction** if you want `Option+f` to send `ESC` plus the key instead of typing `ƒ`
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

While typing, NyaTerm suggests commands based on history. In **Settings → Interaction**, you can enable suggestions and set the minimum and maximum history command lengths. Length filters select completion candidates while preserving history records.

## Invoking AI from the terminal

The terminal context menu offers these AI entry points, each carrying the current session context automatically:

- **Generate command** — describe the goal and let AI produce a command card
- **Explain recent output** — send the session's latest terminal output to AI without copying logs by hand
- **Explain selected text** — explain a highlighted log fragment, error, or snippet
- **Analyze error** — hand the error context to AI for a fix suggestion
- **Fix selected text** — derive the next command from a selected error

During Agent execution, an output summary appears inline near the terminal. The line count is controlled by `Terminal Output Lines` in **Settings → AI → Agent Settings**; set it to `0` to disable it.

Modes, command cards, risk control, and provider/model configuration are documented in [AI Assistant](./ai-assistant).

## Command risk assessment

Commands you type manually go through the same risk assessment. When a rule matches, a prompt shows the risk level, why it matched, and a safer alternative; you can proceed anyway or switch to the alternative.

This requires **Command risk checking** in **Settings → AI**. The four tiers and how they are decided are documented in [AI Assistant → How the risk level is decided](./ai-assistant#how-the-risk-level-is-decided).

## Optional terminal enhancements

All of the following are off by default; enable what you need.

### Line numbers and timestamp gutter

In **Settings → Terminal**, you can enable:

- **Show line numbers**
- **Show timestamps**

When enabled, a gutter appears on the left side of terminal output, which helps when reading long logs, command output, or a recorded session.

### Action links

Action links are off by default. When enabled, NyaTerm can detect and open patterns such as:

- IPv4 addresses like `192.168.1.10`
- `host:port` pairs like `db.internal:5432`
- Archive names like `backup.tar.gz`

Notes:

- First enable **Action Links** in **Settings → Terminal**
- Opening a link requires **Ctrl / Cmd + click** to avoid conflicts with normal text selection
- The three matcher groups can be enabled or disabled separately

### Terminal row zebra stripes

**Terminal row zebra stripes** is on by default. Enabling it highlights the shell input region and the row you click, which helps locate the current input position in dense output.

When enabled, the **Alternate by row** sub-switch (off by default) replaces shell input shading with alternating backgrounds across input and output display rows. Soft-wrapped rows alternate individually; with wrapping disabled, each complete line alternates. The clicked row still takes priority. Turning off the main switch preserves the sub-switch preference.

### Low latency mode

**Low latency mode** is off by default. Enabling it temporarily skips command suggestions, keyword highlighting, and action-link enrichment to keep terminal input and scrolling on the shortest path. Useful during high-throughput output or on high-latency links, at the cost of disabling the other enhancements on this page while it is on.

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

### Large-output handling

Terminal output is processed in batches before the screen is updated. This helps preserve input responsiveness, scrolling, and search during `tail -f`, build logs, and other high-throughput output; it is enabled by default and requires no setting.

## SSH-specific helpers

### Keep-Alive

For SSH sessions, you can configure a Keep-Alive interval in **Settings → Terminal**:

- Default is **30 seconds**, adjustable from **0 to 600 seconds**
- Set it to `0` to disable it
- Useful for reducing idle disconnects on long-lived sessions

The same group also has a **Keep-alive mode** with **Disabled**, **Compatible**, and **Strict**. The current SSH runtime gives **Strict** the same transport behavior as **Compatible**; the option only records configuration intent.

### Remote host monitoring

SSH sessions can open five monitoring panels on the right: Resource Monitor, NVIDIA GPU Monitor, Ascend NPU Monitor, Process Manager, and Docker Manager. Their toggles, defaults, poll intervals, and capabilities are documented in [Remote Host Monitoring](./remote-monitoring).

## Translation and online search

After selecting text in the terminal, you can use the context menu to:

- Send the selection to an online search engine
- Open a translation dialog with an enabled translation provider

### Search engines

In **Settings → Search**, you can maintain a custom search engine list, with per-engine:

- Name
- URL template (using `%s` as the placeholder)
- Icon
- Whether it appears in the search menu

### Translation providers

**Google** and **Microsoft** work out of the box; **DeepL**, **Baidu**, **Alibaba**, and **Youdao** only appear in the menu after you enter credentials in **Settings → Translation**. See [Translation](./translation) for the full description.

## Session recording

`Ctrl / Cmd + Shift + R` starts or stops recording the active session, writing terminal output to a log file.

Recordings go to the system download directory by default. **Settings → Transfer** has a separate **Recording path** so archived transcripts stay apart from ordinary downloads.

The same group has two more recording options:

- **Auto-start recording** — record as soon as a session opens, with no manual trigger
- **Include timestamps** — write timestamps into the transcript so output can be correlated with wall-clock time

Recording options can also be set on an individual saved connection, which binds a policy like "always record production bastion sessions" to the connection itself instead of deciding after each session opens.
