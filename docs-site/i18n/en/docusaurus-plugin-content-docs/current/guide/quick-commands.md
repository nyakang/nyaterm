# Quick Commands

Quick Commands let you save common commands as reusable actions, then send them to the current terminal from inside the workspace.

## Good use cases

- Frequently repeated operational commands
- Deployment or troubleshooting scripts with parameters
- Organizing commands by product, environment, or team
- Placing risky commands into the prompt first so you can review them before execution

## Create a quick command

1. Open the **Quick Commands** area in the bottom helper section or side panel
2. Click **Add**
3. Fill in the command details in the dedicated child window

Available fields include:

| Field | Description |
|------|------|
| Label | Display name for the command |
| Category | Command grouping |
| Description | Optional note |
| Color Tag | Custom display color |
| Icon | Custom icon |
| Pin to Top | Whether it stays near the top of the list |
| Execution Mode | Execute immediately or append to the input line |
| Command Script | The command text to send |

After saving, the command appears in the list and can still be edited or deleted later.

## Record a terminal macro

1. Start a connected SSH, local, Telnet, or Serial terminal session.
2. In the **Recording** panel, click **Start Macro** for that session (independent of Transcript and Raw logs).
3. Type, use IME, Enter, Tab, or paste the text you want to replay. The recorder captures input only after a successful send, including successful synchronized targets.
4. Click **Stop Macro and Review**. The Quick Commands editor shows the recorded script. Review it, choose a label, and explicitly save it. Closing the editor discards the draft; stopping an empty macro saves nothing.

Recorded macros default to **Append to prompt**, so playback submits only the Enter keys you actually pressed during recording. You can edit or delete them later in Quick Commands. Input at detected password prompts, sensitive input APIs, automated commands, and terminal protocol replies are excluded. Check the draft before saving, since password detection depends on the terminal prompt.

The script stores a literal backslash as `\\` and control bytes as macro-only `\xHH` escapes; literal `{{...}}` text is replayed rather than treated as a variable placeholder. Pauses of one second or longer between input chunks are represented using `\p` (rounded down to whole seconds and capped at 60 seconds per gap). Faster timing and remote screen reactions are not reproduced. Disconnecting, reconnecting, closing a session, or exceeding the configured recording memory limit discards the unfinished macro.

## Execution modes

### Execute immediately

Clicking the command sends it to the current terminal and runs it at once. Good for:

- Well-understood routine commands
- Daily inspection tasks
- Fixed-format read-only queries

### Append to prompt

Clicking the command only inserts it into the current input line without pressing Enter. Good for:

- Commands whose parameters still need checking
- Script fragments that usually need a small edit
- Higher-risk operations that should be reviewed manually first

## Variable prompts

Command scripts support `{{variableName}}` placeholders for dynamic parameters, for example:

```bash
docker exec -it {{container_name}} bash
```

When you run the command, NyaTerm opens a variable input dialog so the template can be completed before sending it.

## Escape sequences and pauses

These sequences in a command script are interpreted when it is sent:

| Script | Sent |
|--------|------|
| `\r` | carriage return |
| `\n` | line feed |
| `\t` | tab |
| `\p` | one second pause before the text that follows |
| `\\` | a literal backslash |

Any other backslash sequence is left alone, so `\d` in a regular expression and `C:\Users` in a Windows path arrive as typed. Write `\\` when you want one literal backslash.

Device logins, serial menus, and older systems often need "send a chunk, wait a second, send the next one". Put `\p` between the chunks:

```text
admin\r\psecret\r\pdisplay version
```

This script sends `admin` and Enter, waits one second, sends `secret` and Enter, then waits another second before sending `display version`. `\p` only pauses; use `\r` to submit each login response. Stacking `\p\p` waits two seconds.

Targets are fixed when the script starts sending (after submitting any variables). Switching tabs during a pause keeps the same targets. New quick commands, typing, or pasting into a target cancel the remaining steps for that session; other targets continue. Disconnecting or closing a session also cancels its remaining steps.

Cancellation only stops content that has not been sent. A prefix already sent to the remote terminal remains there, and new input may be appended to it. For example, replacing `old\pobsolete` with `new command` during the pause may send `oldnew command` to the terminal. Check the remote prompt and existing input before continuing, especially during login interactions. NyaTerm does not automatically send Ctrl+C or clear input because control characters have different meanings across devices and interactive programs.

With synchronized input, or with **Send to all sessions**, every session receives the same staged writes.

`\n` and `\t` only affect the script itself: the execution mode still decides whether the last chunk gets a carriage return, and a script that already ends in `\r` or `\n` does not get another one.

Values you fill into the variable prompt are sent literally: their backslashes are never interpreted as escapes or pauses. Only a `\p` written in the template pauses.

## Categories, search, sorting, and view modes

The Quick Commands panel supports these management patterns:

- Search by label, command content, or description
- Filter by category from the dropdown
- Create, rename, move, and sort categories
- Keep pinned commands at the top
- Sort by modes such as name, usage frequency, or recently used
- Switch between compact and regular views to fit different panel widths
- Reuse existing categories when creating new commands; imports also try to preserve the category structure from the external file

Common ways to categorize:

- Kubernetes
- Docker
- Database
- Release scripts
- Environment inspection

## Export quick commands

Click **Export Commands** in the upper-right corner of the Quick Commands panel to export the current commands and categories as a NyaTerm JSON file. The exported file can be used to:

- Merging into another device with **Import Quick Commands**
- Sharing a team template
- Keeping a rollback copy before reorganizing many categories

The export does not include terminal history or other application settings. It only covers quick commands and their categories.

## Import quick commands

Click **Import Quick Commands** in the upper-right corner of the Quick Commands panel to import commands from an external file. Imports merge by command ID and update matching commands without clearing your existing list.

### Import from WindTerm

WindTerm quick commands are usually stored at:

```text
~/.wind/profiles/default.v10/terminal/quickbar.config
```

On Windows, look under your user directory:

```text
C:\Users\<username>\.wind\profiles\default.v10\terminal\quickbar.config
```

Choose **WindTerm Quickbar**, then select this `quickbar.config` file. NyaTerm reads fields such as `quick.label`, `quick.text`, `quick.group`, and `quick.uuid`. WindTerm commands with `quick.type` set to `Send Text` are imported as **Append to prompt** so imported scripts are not executed immediately when clicked.

### Import from a JSON file

Choose **NyaTerm JSON** to import a file like this:

```json
{
  "categories": [
    {
      "id": "general",
      "name": "General"
    },
    {
      "id": "k8s",
      "name": "Kubernetes"
    }
  ],
  "commands": [
    {
      "id": "cmd-list-files",
      "label": "List files",
      "command": "ls -la",
      "category_id": "general",
      "description": "List files with details",
      "color_tag": "blue",
      "icon_tag": "terminal",
      "pinned": true,
      "execution_mode": "execute",
      "source": "manual",
      "risk_level": "low"
    },
    {
      "label": "Kubernetes pods",
      "command": "kubectl get pods -A",
      "category": "Kubernetes",
      "execution_mode": "append",
      "risk_level": "low"
    }
  ]
}
```

Notes:

- `categories` is optional; when a command uses a `category` name, missing categories are created automatically
- `id` is optional and is generated when omitted
- `execution_mode` supports `execute` or `append`
- `source` supports `manual` or `ai`
- `risk_level` supports `low`, `medium`, `high`, or `critical`

## How it fits the workspace

Quick Commands are not tied to one specific session type. As long as the current terminal can accept input, you can send commands to:

- SSH sessions
- Local Terminal sessions
- Some serial workflows that need repeated fixed input

Common combinations include:

- Watching logs on one side while triggering diagnostics from Quick Commands on the other
- Running deploy commands remotely while building or using Git locally
- Turning variable-based commands into team-friendly templates
- Saving approved commands generated by AI Assistant as quick commands for later reuse
