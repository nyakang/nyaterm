# AI Assistant

NyaTerm includes an AI Assistant panel that can start from terminal context, selected text, file operations, or a manual prompt. It supports two working modes: **Ask** and **Agent**.

## Working modes

### Ask mode

Ask is the default mode and fits one-off help:

- Generate commands
- Explain terminal output
- Analyze errors
- Derive a fix command from selected text

Commands returned by AI are shown as structured cards with risk labels.

### Agent mode

Agent mode lets AI perform multi-step work. It uses a ReAct-style loop: observe terminal output → decide the next command → execute → observe again until the task is complete or a step limit is reached.

Agent mode characteristics:

- Requires an active terminal session
- Shows each command's state: running, completed, failed, timed out, needs approval, or blocked by policy
- Configurable maximum steps and per-step timeout in **Settings → AI → Agent Settings**
- Uses `Terminal Output Lines` to control how much AI-executed command output is shown inline near the terminal
- High-risk commands still require manual approval before execution

## Agent tools, plans and questions

Native Agent and external MCP share capability contracts, scope, risk checks, approvals, output paging and redacted audits. Native does not need the MCP service or helper. Claude/Codex continue to access capabilities exclusively through MCP.

Native can query the environment and sessions, execute terminal commands, read recent output, list/stat/read text through SFTP, and read subsequent output pages. SFTP writes are outside this stage. Available targets are fixed when the run starts; calls with multiple targets must select one explicitly.

Multi-step tasks can maintain a plan with stable task IDs and pending, in-progress, completed or blocked states. At most one task is in progress. Simple tasks may finish directly. Agent can ask 1–3 questions with optional choices and free-text answers. Answers require submission; the same run then resumes. Cancel ends the run, while switching conversations preserves the waiting state.

Each tool decision, including planning and questions, consumes one step. At the limit only the final summary is allowed. Waiting for approval or answers does not start a command timeout. Final answers include verified, unverified or blocked status and verification notes. An unfinished plan cannot be marked verified.

Foreground execution keeps terminal capture; background execution keeps the existing separate backend. Unknown exit codes from quiet observation, timeouts and lost output are explicit and cannot mean a successful exit. Large output is paged per run or MCP connection and cleared when the owner ends or credentials expire.

Runs, plans and tool transcripts live only in memory. Chat history stores display text and does not resume execution. Settings, history, audit and MCP wire formats stay compatible. Capability audits contain no commands, output, file content or user answers.

## Conversation management

AI Assistant supports multiple conversations:

- **New conversation** creates an independent context
- **History** groups conversations by time, such as today, yesterday, last 7 days, and older
- **Search history** fuzzy-searches conversation content
- **Delete conversation** removes history you no longer need

## Session mentions

Type `@` in the input box to mention other terminal sessions and bring their context into the current AI conversation, for cross-session analysis or comparison.

## Command cards and risk control

AI commands are displayed as structured cards with:

- Command text
- Risk level: low / medium / high / critical
- Execute or approval actions
- Save-as-quick-command option

### How the risk level is decided

The risk level is not the model's call alone. NyaTerm takes the **higher** of the model's self-reported level and the **local rule verdict** as the effective level, so a model that underestimates risk still gets stopped by local rules. The command card shows the effective risk.

Local rules use four tiers, and they classify by command *shape* rather than command name:

| Level | Examples |
|-------|----------|
| Critical | `rm -rf /`, `mkfs`, `wipefs`, `dd` to a block device, fork bombs, `shutdown` / `reboot` / `poweroff`, stopping sshd |
| High | anything starting with `sudo`, `rm -r` / `rm -f`, `chmod -R` / `chown -R`, package installs and removals, `docker rm` / `system prune`, `kubectl delete` / `drain` / `apply`, `git reset --hard` |
| Medium | bare `chmod` / `chown`, redirects `>` / `>>`, `cp` / `mv` / `mkdir` / `touch`, `git pull` / `merge`, `npm run`; anything matching no rule also lands here |
| Low | read-only diagnostics such as `ls`, `cat`, `ps`, `df`, `docker ps`, `kubectl get`, `git status` |

Note that tiers look at command shape: bare `chmod` / `chown` is medium and only the recursive `-R` flag raises it to high; `rm -rf /` is critical while `rm -rf ./build` is high.

Commands you type manually go through the same rules — see [Terminal Features → Command risk assessment](./terminal#command-risk-assessment).

### Command execution policy

The effective level decides whether a command may run, together with **Command execution policy** in **Settings → AI → Agent Settings**:

| Policy | Behavior |
|--------|----------|
| Confirm before execution | Every command needs your confirmation |
| Fully automatic | Ordinary and sensitive reads are allowed; only explicitly classified commands below High run automatically |
| Smart approval | Further tightens automatic execution using the **Smart mode auto-execute ceiling** |

Every Native mode requires approval for unknown, High and Critical commands. Confirm mode offers only per-call command approval. Other Native session grants cover only the current run, target, tool and identical arguments. Critical and destructive operations always require per-call approval. Approval is followed by another scope, live-session and execution-disabled check.

**Settings → AI** also configures:

- **Smart mode auto-execute ceiling**
- Whether command risk checking is enabled
- Whether generated commands may be saved as quick commands

### Safer alternatives

When a command is judged high risk, AI may also provide a safer alternative command.

## Recent output and inline terminal output

AI Assistant can work around "what just happened" instead of only selected text.

- Use **Explain recent output** from the terminal context menu to send recent output from the current session to AI
- In Agent mode, AI command execution events are captured and summarized near the terminal workflow
- `Terminal Output Lines` controls the maximum number of inline feedback lines; set it to `0` to disable them

## Reasoning content

If the selected model exposes a reasoning channel, such as DeepSeek R1 or QwQ-style models, AI Assistant can show reasoning content in the response. Reasoning content is collapsed by default and can be expanded when needed.

## Provider and model configuration

Manage providers and models in **Settings → AI**.

### Provider configuration

- Built-in providers such as OpenAI, Anthropic, Google, and DeepSeek
- Custom **OpenAI Compatible** providers
- Each provider needs an API key and optional Base URL

### Model management

- Fetch available models from providers
- Manually add models that a provider does not return automatically
- Group models by provider and credential group
- Enable / disable individual models
- Choose a default model

### Other settings

- **Context lines**: maximum terminal output lines sent to AI
- **Request timeout**: timeout for one AI request
- **Record history**: whether to save AI conversation history
- **Sensitive redaction**: redact sensitive content before sending

## Invoke from terminal and file explorer

AI Assistant can be invoked from context menus, not only from the panel input.

### Terminal context menu

- **Explain recent output** sends recent output from the current session
- **Explain selected text** explains the selected log, error, or fragment
- **Analyze error** asks AI to generate repair suggestions from an error context
- **Fix selected text** derives the next command from selected error text
- **Generate command** asks AI to produce a command for your goal

### File explorer context menu

For files in the SFTP file explorer, you can send file content to AI for analysis from the context menu. File size limits apply and can be adjusted in settings.

### Error auto-detection

When terminal output matches error patterns, AI Assistant can suggest analyzing the error automatically.

## MCP execution lifecycle

`session_open` creates a new session by default. Set `mode: "reuse_or_create"` to reuse a connected, executable terminal from the same saved connection within the current scope. Names and addresses do not identify a saved connection. An explicit scope is never widened automatically.

Use `idempotencyKey` on session opens and terminal executions to recover the original operation after a lost response. Keys bind the client, tool and normalized parameters. Changed parameters conflict; confirmed failures require a new key. Expired keys cannot execute again, and a full ledger rejects new records.

`terminal_execute` retains its synchronous timeout and result contract; `returnJob: true` adds `jobId`. For long work, call `terminal_start` with `sessionId`, `command`, optional `backend`, `timeoutMs` and `idempotencyKey`. Poll the returned `jobId` with `terminal_poll`, passing the previous `nextOffset`. Pages default to 16 KiB and use UTF-8 byte boundaries. `baseOffset`, `droppedBeforeOffset` and `sourceTruncated` describe lost output separately from pagination. Job status, remote exit evidence and terminal availability are separate fields.

The integrated client's legacy local background mode remains available, but rejects `returnJob: true`. Use `terminal_start` with `inline_pty` for a pollable local job.

The default `inline_pty` backend preserves the current shell directory and variables. Only one automatic command can occupy a shell. Timeout, uncertain writes, cancellation or output loss require recovery before further automatic execution. Manually interrupt and inspect the shell, then use **Confirm shell is ready** in the terminal tab menu. A matching late END marker also confirms recovery. Ordinary output, time passing and Ctrl+C alone do not prove recovery. Commands are never replayed automatically.

SSH sessions may explicitly select `ssh_exec`, an auxiliary channel that does not inherit interactive shell state. `terminal_stop` sends at most one safe Ctrl+C or channel INT and never escalates to KILL. It distinguishes accepted, sent, confirmed, unconfirmed, manual recovery and invalid targets. EOF does not prove process exit. Stop remains unconfirmed after two seconds without exit evidence; reaching the execution deadline closes only the auxiliary channel.

Long jobs default to five minutes and accept one second through twenty-four hours. Rolling output limits are 4 MiB per job, 16 MiB per client and 64 MiB globally. Active limits are eight jobs per client and thirty-two globally. Completed results remain for ten minutes. Every poll, stop and result recovery checks owner and current scope. Startup approval grants access to that job's output. Revoking access does not kill a process or close the user's SSH session.

The Host issues an in-memory recovery credential for reconnecting the same sidecar process. A sidecar restart cannot inherit previous jobs. Submitted jobs continue after an RPC disconnect, and the bridge never replays a side-effect request. Old Hosts omit unsupported job tools and parameters; old sidecars retain the original result shape on new Hosts.
