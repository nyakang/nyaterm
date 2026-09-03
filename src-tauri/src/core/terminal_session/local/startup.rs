// Local PTY dynamic-title integration. Remote SSH integration remains in
// `core::ssh::osc` so Local title policy cannot change released SSH/history behavior.

use crate::core::ssh::osc::{
    ControlSequenceKind, ShellKind, control_sequence_payload, find_control_sequence_end,
    find_control_sequence_start,
};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

const MAX_STARTUP_OUTPUT_BUFFER: usize = 64 * 1024;
const LOCAL_SHELL_STARTUP_IDLE: Duration = Duration::from_millis(250);
const LOCAL_SHELL_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

/// Outcome of building startup integration for a spawned Local shell.
pub struct LocalStartupScript {
    /// Bash/Zsh source stored in a private temporary file after spawn.
    pub script: Option<String>,
    /// Shell-specific structured init arguments (currently Fish `--init-command`).
    pub shell_init_args: Option<Vec<String>>,
    /// PowerShell spawn arguments appended only for an unmodified default argv.
    pub pwsh_init_args: Option<Vec<String>>,
    /// Process-local cmd.exe PROMPT value; never persisted or written globally.
    pub cmd_prompt: Option<String>,
    /// True when a session-bound ready marker is expected at runtime.
    pub dynamic_title_integration_requested: bool,
}

pub struct LocalStartupScriptFile {
    path: PathBuf,
    removed: AtomicBool,
}

impl LocalStartupScriptFile {
    pub fn remove(&self) {
        if self.removed.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Err(error) = std::fs::remove_file(&self.path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                self.removed.store(false, Ordering::Release);
                tracing::warn!(
                    path = %self.path.display(),
                    error = %error,
                    "Failed to remove temporary Local shell integration file"
                );
            }
        }
    }
}

impl Drop for LocalStartupScriptFile {
    fn drop(&mut self) {
        self.remove();
    }
}

#[cfg(unix)]
pub fn prepare_local_startup_injection(
    source: String,
) -> std::io::Result<(String, Option<Arc<LocalStartupScriptFile>>)> {
    for _ in 0..8 {
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let path = PathBuf::from(format!("/tmp/.nt-{}", &nonce[..16]));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        if let Err(error) = file.write_all(source.as_bytes()).and_then(|_| file.flush()) {
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
        let history_token = format!("nt_{nonce}");
        let command = format!(
            "NYATERM_PRUNE_HISTORY={history_token}; . {}\n",
            path.display()
        );
        return Ok((
            command,
            Some(Arc::new(LocalStartupScriptFile {
                path,
                removed: AtomicBool::new(false),
            })),
        ));
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique Local shell integration file",
    ))
}

#[cfg(not(unix))]
pub fn prepare_local_startup_injection(
    source: String,
) -> std::io::Result<(String, Option<Arc<LocalStartupScriptFile>>)> {
    Ok((source, None))
}

impl LocalStartupScript {
    fn none() -> Self {
        Self {
            script: None,
            shell_init_args: None,
            pwsh_init_args: None,
            cmd_prompt: None,
            dynamic_title_integration_requested: false,
        }
    }
}

/// Build Local-only dynamic-title hooks. `allow_injection` must be false for
/// every non-empty user argv and unresolved wrapper shell.
pub fn build_local_startup_script(
    shell_name: &str,
    ready_marker: &str,
    enabled: bool,
    allow_injection: bool,
) -> LocalStartupScript {
    if !enabled || !allow_injection {
        return LocalStartupScript::none();
    }

    let Some(session_token) = ready_marker_session_token(ready_marker) else {
        return LocalStartupScript::none();
    };
    let basename = shell_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(shell_name)
        .to_ascii_lowercase();
    let basename = basename.strip_suffix(".exe").unwrap_or(&basename);

    if matches!(basename, "powershell" | "pwsh") {
        return LocalStartupScript {
            script: None,
            shell_init_args: None,
            pwsh_init_args: Some(pwsh_prompt_init_args(session_token)),
            cmd_prompt: None,
            dynamic_title_integration_requested: true,
        };
    }

    if cfg!(target_os = "windows") && basename == "cmd" {
        return LocalStartupScript {
            script: None,
            shell_init_args: None,
            pwsh_init_args: None,
            cmd_prompt: Some(cmd_prompt_value(session_token)),
            dynamic_title_integration_requested: true,
        };
    }

    if basename == "fish" {
        let Some(script) = local_unix_injection_script(ShellKind::Fish, ready_marker) else {
            return LocalStartupScript::none();
        };
        return LocalStartupScript {
            script: None,
            shell_init_args: Some(vec!["--init-command".to_string(), script]),
            pwsh_init_args: None,
            cmd_prompt: None,
            dynamic_title_integration_requested: true,
        };
    }

    let shell = match basename {
        "bash" => ShellKind::Bash,
        "zsh" => ShellKind::Zsh,
        _ => return LocalStartupScript::none(),
    };
    let Some(script) = local_unix_injection_script(shell, ready_marker) else {
        return LocalStartupScript::none();
    };

    LocalStartupScript {
        script: Some(script),
        shell_init_args: None,
        pwsh_init_args: None,
        cmd_prompt: None,
        dynamic_title_integration_requested: true,
    }
}

/// Wrapper executables hide or delegate to another shell whose startup
/// language/argv cannot be safely inferred. Keep them passive for this release.
fn should_allow_local_injection(
    configured_shell_path: &str,
    resolved_shell_name: &str,
    shell_args: &str,
    resolved_shell_args_empty: bool,
    resolution_source: ShellResolutionSource,
) -> bool {
    if !shell_args.is_empty() || is_unresolved_shell_wrapper(resolved_shell_name) {
        return false;
    }

    // `wt.exe` is a configuration alias, not the process we spawn. Only a
    // successfully resolved profile is eligible; the compatibility PowerShell
    // fallback must remain passive because it is not the configured profile.
    if is_windows_terminal_alias(configured_shell_path.trim()) {
        return resolution_source == ShellResolutionSource::WindowsTerminalProfile
            && resolved_shell_args_empty;
    }

    should_treat_as_literal_program(configured_shell_path)
}

pub fn is_unresolved_shell_wrapper(shell_path: &str) -> bool {
    let basename = shell_path
        .trim()
        .trim_matches('"')
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(shell_path)
        .to_ascii_lowercase();
    matches!(basename.as_str(), "wsl" | "wsl.exe" | "wt" | "wt.exe")
}

fn ready_marker_session_token(ready_marker: &str) -> Option<&str> {
    ready_marker
        .strip_prefix("\x1b]7777;NyaTermReady:")?
        .strip_suffix('\x07')
        .or_else(|| {
            ready_marker
                .strip_prefix("\x1b]7777;NyaTermReady:")?
                .strip_suffix("\x1b\\")
        })
}

fn local_unix_injection_script(shell: ShellKind, ready_marker: &str) -> Option<String> {
    let ready_osc = ready_marker
        .replace('\x1b', "\\033")
        .replace('\x07', "\\007");
    let ready_payload = ready_marker
        .strip_prefix("\x1b]")?
        .strip_suffix('\x07')?;
    let session_id = ready_payload.strip_prefix("7777;NyaTermReady:")?;
    let ready_failed_payload = format!("7777;NyaTermReadyFailed:{session_id}");

    match shell {
        ShellKind::Bash => Some(format!(
            concat!(
                " NYATERM_PRUNE_HISTORY=\"${{NYATERM_PRUNE_HISTORY:-1}}\";",
                " __nyaterm_local_prune_history(){{",
                " [ -n \"${{NYATERM_PRUNE_HISTORY:-}}\" ] || return 0;",
                " local history_marker=\"$NYATERM_PRUNE_HISTORY\"; unset NYATERM_PRUNE_HISTORY;",
                " local hline history_number; hline=\"$(HISTTIMEFORMAT= history 1 2>/dev/null || true)\";",
                " case \"$hline\" in *\"NYATERM_PRUNE_HISTORY=$history_marker\"*)",
                " history_number=${{hline#\"${{hline%%[![:space:]]*}}\"}}; history_number=${{history_number%%[!0-9]*}};",
                " [ -z \"$history_number\" ] || history -d \"$history_number\" 2>/dev/null || true;; esac;",
                " }};",
                " __nyaterm_local_uri_encode(){{",
                " local LC_ALL=C input=\"$1\" output= ch hex code; local -i i;",
                " for ((i=0; i<${{#input}}; i++)); do ch=${{input:i:1}};",
                " case \"$ch\" in [-A-Za-z0-9._~/]) output+=\"$ch\";; *) printf -v code '%d' \"'$ch\"; printf -v hex '%%%02X' \"$((code & 255))\"; output+=\"$hex\";; esac;",
                " done; printf '%s' \"$output\";",
                " }};",
                " __nyaterm_local_dynamic_prompt(){{",
                " local status=$?; __nyaterm_local_prune_history; local encoded; encoded=\"$(__nyaterm_local_uri_encode \"$PWD\")\";",
                " printf '\\033]2;\\033\\\\';",
                " printf '\\033]7;file://localhost%s\\033\\\\' \"$encoded\";",
                " return \"$status\";",
                " }};",
                " __nyaterm_local_ready_failed(){{ [ -n \"${{__nyaterm_local_failure_reported:-}}\" ] || {{ __nyaterm_local_failure_reported=1; printf '\\033]{}\\007'; }}; }};",
                " __nyaterm_local_restore_status(){{ return \"$1\"; }};",
                " __nyaterm_local_prompt_guard(){{ return $?; }};",
                " __nyaterm_local_prompt_container_valid(){{ local current=\"${{PROMPT_COMMAND[*]-}}\"; case \"$current\" in *__nyaterm_local_dynamic_prompt*__nyaterm_local_ready_prompt*) return 0;; *) return 1;; esac; }};",
                " __nyaterm_local_prompt_liveness(){{ local status=$?; if __nyaterm_local_prompt_container_valid; then printf '\\033]{}\\007'; else __nyaterm_local_ready_failed; fi; return \"$status\"; }};",
                " __nyaterm_local_prompt_state_writable(){{ local name decl; for name in __nyaterm_local_saved_prompt_command __nyaterm_local_extra_prompt_commands __nyaterm_local_exported_prompt_fallback; do decl=\"$(declare -p \"$name\" 2>/dev/null || true)\"; [[ ! \"$decl\" =~ ^declare\\ -[^[:space:]]*r ]] || return 1; done; }};",
                " if __nyaterm_local_prompt_state_writable; then __nyaterm_local_extra_prompt_commands=(); fi;",
                " __nyaterm_local_run_saved_prompt_command(){{",
                " local status=$? command;",
                " if [ -n \"${{__nyaterm_local_saved_prompt_command-}}\" ]; then __nyaterm_local_restore_status \"$status\"; builtin eval -- \"$__nyaterm_local_saved_prompt_command\"; status=$?; fi;",
                " for command in \"${{__nyaterm_local_extra_prompt_commands[@]}}\"; do __nyaterm_local_restore_status \"$status\"; builtin eval -- \"$command\"; status=$?; done;",
                " return \"$status\";",
                " }};",
                " __nyaterm_local_rebuild_exported_prompt_fallback(){{ local command result=\"${{__nyaterm_local_saved_prompt_command-}}\"; for command in \"${{__nyaterm_local_extra_prompt_commands[@]}}\"; do if [ -n \"$result\" ]; then result=\"$result; $command\"; else result=\"$command\"; fi; done; __nyaterm_local_exported_prompt_fallback=\"$result\"; }};",
                " __nyaterm_local_capture_prompt_string(){{",
                " local current=\"$1\" expected=\"$2\" tail; [ \"$current\" = \"$expected\" ] && return 0;",
                " case \"$current\" in \"$expected\"\\;*|\"$expected\"\\&*|\"$expected \"*|\"$expected\"$'\\t'*|\"$expected\"$'\\n'*)",
                " tail=${{current#\"$expected\"}}; while :; do tail=${{tail#\"${{tail%%[![:space:]]*}}\"}}; case \"$tail\" in \\;*|\\&*) tail=${{tail#?}};; *) break;; esac; done;",
                " [ -z \"$tail\" ] || __nyaterm_local_extra_prompt_commands[${{#__nyaterm_local_extra_prompt_commands[@]}}]=\"$tail\";;",
                " *\"$expected\"*) return 1;;",
                " *) __nyaterm_local_saved_prompt_command=\"$current\"; __nyaterm_local_extra_prompt_commands=();; esac; __nyaterm_local_rebuild_exported_prompt_fallback; return 0;",
                " }};",
                " __nyaterm_local_array_prompt_supported(){{ [ \"${{BASH_VERSINFO[0]:-0}}\" -gt 5 ] || {{ [ \"${{BASH_VERSINFO[0]:-0}}\" -eq 5 ] && [ \"${{BASH_VERSINFO[1]:-0}}\" -ge 1 ]; }}; }};",
                " __nyaterm_local_repair_prompt(){{",
                " local decl f current expected canonical exported=0;",
                " canonical='__nyaterm_local_dynamic_prompt; __nyaterm_local_run_saved_prompt_command; __nyaterm_local_ready_prompt; __nyaterm_local_prompt_guard'; expected=\"$canonical\";",
                " decl=\"$(declare -p PROMPT_COMMAND 2>/dev/null || true)\";",
                " __nyaterm_local_prompt_state_writable || return 1;",
                " [[ \"$decl\" =~ ^declare\\ -[^[:space:]]*x ]] && exported=1;",
                " [[ ! \"$decl\" =~ ^declare\\ -[^[:space:]]*r ]] || return 1;",
                " if [ \"$exported\" -eq 1 ]; then expected='if declare -F __nyaterm_local_dynamic_prompt >/dev/null 2>&1; then __nyaterm_local_dynamic_prompt; __nyaterm_local_run_saved_prompt_command; __nyaterm_local_ready_prompt; __nyaterm_local_prompt_guard; else eval -- \"${{__nyaterm_local_exported_prompt_fallback-}}\"; fi'; fi;",
                " if [[ \"$decl\" =~ ^declare\\ -[^[:space:]]*a[^[:space:]]*\\ PROMPT_COMMAND= ]] && __nyaterm_local_array_prompt_supported; then",
                " local -a retained=(); for f in \"${{PROMPT_COMMAND[@]}}\"; do case \"$f\" in __nyaterm_local_dynamic_prompt|__nyaterm_local_ready_prompt) ;; *) retained+=(\"$f\");; esac; done;",
                " PROMPT_COMMAND=(__nyaterm_local_dynamic_prompt \"${{retained[@]}}\" __nyaterm_local_ready_prompt) || return 1;",
                " else current=${{PROMPT_COMMAND-}}; if [[ \"$decl\" =~ ^declare\\ -[^[:space:]]*a[^[:space:]]*\\ PROMPT_COMMAND= ]]; then unset PROMPT_COMMAND; fi;",
                " if ! __nyaterm_local_capture_prompt_string \"$current\" \"$expected\"; then PROMPT_COMMAND=\"$expected\"; return 1; fi; PROMPT_COMMAND=\"$expected\" || return 1; if [ \"$exported\" -eq 1 ]; then export __nyaterm_local_exported_prompt_fallback; else unset __nyaterm_local_exported_prompt_fallback; fi; fi;",
                " return 0;",
                " }};",
                " __nyaterm_local_ready_prompt(){{",
                " local status=$?; local decl marker='\\[$(__nyaterm_local_prompt_liveness)\\]';",
                " if ! __nyaterm_local_repair_prompt; then __nyaterm_local_ready_failed; return \"$status\"; fi;",
                " decl=\"$(declare -p PS1 2>/dev/null || true)\";",
                " if [[ \"$decl\" =~ ^declare\\ -[^[:space:]]*r ]]; then __nyaterm_local_ready_failed; return \"$status\"; fi;",
                " case \"$PS1\" in *\"$marker\"*) ;; *) PS1=\"${{PS1}}${{marker}}\" || __nyaterm_local_ready_failed;; esac;",
                " return \"$status\";",
                " }};",
                " __nyaterm_local_install_prompt(){{ __nyaterm_local_repair_prompt; }};",
                " __nyaterm_local_prompt_markable(){{ local decl; decl=\"$(declare -p PS1 2>/dev/null || true)\"; [[ ! \"$decl\" =~ ^declare\\ -[^[:space:]]*r ]] || return 1; PS1=$PS1; }};",
                // The sourced startup command is already in interactive Bash
                // history now. Prune it before any readonly/failing hook path.
                " __nyaterm_local_prune_history;",
                " if __nyaterm_local_prompt_markable && __nyaterm_local_install_prompt; then :; else __nyaterm_local_ready_failed; fi;\n",
            ),
            ready_failed_payload,
            ready_payload,
        )),
        ShellKind::Zsh => Some(format!(
            concat!(
                " unset NYATERM_PRUNE_HISTORY 2>/dev/null; fc -p /dev/null 2>/dev/null\n",
                " __nyaterm_local_uri_encode(){{",
                " emulate -L zsh; unsetopt multibyte; local input=\"$1\" output='' ch hex; local -i i;",
                " for ((i=1; i<=${{#input}}; i++)); do ch=${{input[i]}};",
                " case \"$ch\" in ([-A-Za-z0-9._~/]) output+=\"$ch\";; (*) printf -v hex '%%%02X' \"'$ch\"; output+=\"$hex\";; esac;",
                " done; print -rn -- \"$output\";",
                " }};",
                " __nyaterm_local_dynamic_emit(){{",
                " local saved_status=$?; local encoded=\"$(__nyaterm_local_uri_encode \"$PWD\")\";",
                " printf '\\033]2;\\033\\\\';",
                " printf '\\033]7;file://localhost%s\\033\\\\' \"$encoded\";",
                " return \"$saved_status\";",
                " }};",
                " __nyaterm_local_ready_failed(){{ [ -n \"${{__nyaterm_local_failure_reported:-}}\" ] || {{ __nyaterm_local_failure_reported=1; printf '\\033]{}\\007'; }}; }};",
                " __nyaterm_local_repair_prompt(){{",
                " [[ ${{parameters[precmd_functions]-}} == *readonly* ]] && return 1;",
                " typeset -ga precmd_functions || return 1;",
                " local -a retained=(); local f; for f in \"${{precmd_functions[@]}}\"; do case \"$f\" in (__nyaterm_local_dynamic_emit|__nyaterm_local_ready_prompt) ;; (*) retained+=(\"$f\");; esac; done;",
                " precmd_functions=(__nyaterm_local_dynamic_emit \"${{retained[@]}}\" __nyaterm_local_ready_prompt) || return 1;",
                " return 0;",
                " }};",
                " __nyaterm_local_ready_prompt(){{",
                " local saved_status=$?; local marker=$'%{{\\e]{}\\a%}}';",
                " if ! __nyaterm_local_repair_prompt; then __nyaterm_local_ready_failed; return \"$saved_status\"; fi;",
                " if [[ ${{parameters[PS1]-}} == *readonly* ]]; then __nyaterm_local_ready_failed; return \"$saved_status\"; fi;",
                " if [[ \"$PS1\" != *\"$marker\"* ]]; then PS1=\"${{PS1}}${{marker}}\" || __nyaterm_local_ready_failed; fi;",
                " return \"$saved_status\";",
                " }};",
                " __nyaterm_local_install_prompt(){{ __nyaterm_local_repair_prompt; }};",
                " __nyaterm_local_prompt_markable(){{ [[ ${{parameters[PS1]-}} != *readonly* ]] || return 1; PS1=$PS1; }};",
                " fc -P 2>/dev/null\n if __nyaterm_local_prompt_markable && __nyaterm_local_install_prompt; then :; else __nyaterm_local_ready_failed; fi;\n",
            ),
            ready_failed_payload,
            ready_payload,
        )),
        ShellKind::Fish => Some(format!(
            concat!(
                " functions -e __nyaterm_local_report_cwd 2>/dev/null;",
                " functions -e __nyaterm_local_dynamic_postexec 2>/dev/null;",
                " function __nyaterm_local_report_cwd;",
                " set -l encoded (string escape --style=url -- $PWD);",
                " printf '\\033]7;file://localhost%s\\033\\\\' $encoded;",
                " end;",
                // fish_title/user prompt titles run after fish_postexec, so
                // reset the previous application title before theme output.
                " function __nyaterm_local_dynamic_postexec --on-event fish_postexec;",
                " set -l saved_status $status;",
                " printf '\\033]2;\\033\\\\';",
                " __nyaterm_local_report_cwd;",
                " return $saved_status;",
                " end;",
                " printf '{}'; __nyaterm_local_report_cwd;\n",
            ),
            ready_osc,
        )),
        ShellKind::PosixSh | ShellKind::Unknown => None,
    }
}

fn build_cmd_prompt(existing: Option<&str>, session_token: &str) -> String {
    let original = existing.filter(|value| !value.is_empty()).unwrap_or("$P$G");
    format!(
        "$E]2;$E\\$E]7777;NyaTermReady:{session_token}$E\\$E]7;nyaterm-cmd://{session_token}/$P$E\\{original}"
    )
}

fn cmd_prompt_value(session_token: &str) -> String {
    let existing = std::env::var("PROMPT").ok();
    build_cmd_prompt(existing.as_deref(), session_token)
}

/// Install a wrapper without touching the user's profile. Metadata is emitted
/// before the original prompt, then the incoming `$?`/`$LASTEXITCODE` state is
/// restored so prompt themes keep both status and deliberate OSC-title behavior.
fn pwsh_prompt_init_args(session_token: &str) -> Vec<String> {
    let init = format!(
        concat!(
            "$__nt_existing_wrapper = $global:__nyaterm_local_prompt_wrapper; $__nt_current_prompt = $function:prompt; if ($__nt_existing_wrapper -and [object]::ReferenceEquals($__nt_current_prompt, $__nt_existing_wrapper)) {{ $__nt_current_prompt = $global:__nyaterm_local_prev_prompt }}; $global:__nyaterm_local_prev_prompt = $__nt_current_prompt; ",
            "$global:__nyaterm_local_failure_reported = $false; ",
            "$global:__nyaterm_local_report = {{ ",
            "$__nt_location = $executionContext.SessionState.Path.CurrentLocation; $__nt_esc = [char]27; $__nt_st = $__nt_esc + '\\'; ",
            "[Console]::Out.Write($__nt_esc + ']2;' + $__nt_st); [Console]::Out.Write($__nt_esc + ']7777;NyaTermReady:{0}' + $__nt_st); ",
            "if ($__nt_location.Provider.Name -eq 'FileSystem') {{ try {{ $__nt_provider_path = $__nt_location.ProviderPath; ",
            "if ([IO.Path]::DirectorySeparatorChar -eq '\\') {{ $__nt_uri = ([Uri]::new($__nt_provider_path)).AbsoluteUri }} else {{ $__nt_builder = [UriBuilder]::new('file','localhost'); $__nt_builder.Path = $__nt_provider_path.Replace('%','%25'); $__nt_uri = $__nt_builder.Uri.AbsoluteUri }}; ",
            "[Console]::Out.Write($__nt_esc + ']7;' + $__nt_uri + $__nt_st) }} catch {{ [Console]::Out.Write($__nt_esc + ']7;nyaterm-clear://{0}' + $__nt_st) }} }} else {{ [Console]::Out.Write($__nt_esc + ']7;nyaterm-clear://{0}' + $__nt_st) }} ",
            "}}; ",
            "$global:__nyaterm_local_ready_failed = {{ if (-not $global:__nyaterm_local_failure_reported) {{ $global:__nyaterm_local_failure_reported = $true; $__nt_esc = [char]27; [Console]::Out.Write($__nt_esc + ']7777;NyaTermReadyFailed:{0}' + $__nt_esc + '\\') }} }}; ",
            "$global:__nyaterm_local_prompt_wrapper = {{ ",
            "$__nt_success = $?; $__nt_had_last_exit = Test-Path variable:global:LASTEXITCODE; $__nt_last_exit = if ($__nt_had_last_exit) {{ $global:LASTEXITCODE }} else {{ $null }}; & $global:__nyaterm_local_report; ",
            "if ($__nt_had_last_exit) {{ $global:LASTEXITCODE = $__nt_last_exit }} else {{ Remove-Variable LASTEXITCODE -Scope Global -ErrorAction Ignore }}; if ($__nt_success) {{ & {{}} }} else {{ Write-Error 'NyaTerm prompt status restore' -ErrorAction Ignore }}; ",
            "$__nt_result = $null; try {{ if ($global:__nyaterm_local_prev_prompt) {{ if ($__nt_success) {{ & {{}} }} else {{ Write-Error 'NyaTerm prompt status restore' -ErrorAction Ignore }}; $__nt_result = & $global:__nyaterm_local_prev_prompt }} else {{ $__nt_result = 'PS ' + $executionContext.SessionState.Path.CurrentLocation.Path + ('>' * ($nestedPromptLevel + 1)) + ' ' }} }} finally {{ ",
            "$__nt_current_prompt = $function:prompt; if (-not [object]::ReferenceEquals($__nt_current_prompt, $global:__nyaterm_local_prompt_wrapper)) {{ $global:__nyaterm_local_prev_prompt = $__nt_current_prompt; try {{ Set-Item Function:\\global:prompt -Value $global:__nyaterm_local_prompt_wrapper -Force -ErrorAction Stop }} catch {{ & $global:__nyaterm_local_ready_failed }} }} }}; $__nt_result ",
            "}}; ",
            "try {{ Set-Item Function:\\global:prompt -Value $global:__nyaterm_local_prompt_wrapper -Force -ErrorAction Stop }} catch {{ & $global:__nyaterm_local_ready_failed }}; ",
            "try {{ if ($global:__nyaterm_local_prompt_guard) {{ $__nt_old_guard_id = $global:__nyaterm_local_prompt_guard.Id; foreach ($__nt_subscriber in @(Microsoft.PowerShell.Utility\\Get-EventSubscriber -SourceIdentifier PowerShell.OnIdle -ErrorAction Ignore)) {{ if ([object]::ReferenceEquals($__nt_subscriber.Action, $global:__nyaterm_local_prompt_guard)) {{ Microsoft.PowerShell.Utility\\Unregister-Event -SubscriptionId $__nt_subscriber.SubscriptionId -Force -ErrorAction Ignore; break }} }}; Microsoft.PowerShell.Core\\Remove-Job -Id $__nt_old_guard_id -Force -ErrorAction Ignore; $global:__nyaterm_local_prompt_guard = $null }}; $global:__nyaterm_local_prompt_guard = Microsoft.PowerShell.Utility\\Register-EngineEvent -SourceIdentifier PowerShell.OnIdle -Action {{ $__nt_current_prompt = $function:prompt; if (-not [object]::ReferenceEquals($__nt_current_prompt, $global:__nyaterm_local_prompt_wrapper)) {{ $global:__nyaterm_local_prev_prompt = $__nt_current_prompt; try {{ Set-Item Function:\\global:prompt -Value $global:__nyaterm_local_prompt_wrapper -Force -ErrorAction Stop; & $global:__nyaterm_local_report }} catch {{ & $global:__nyaterm_local_ready_failed }} }} }} }} catch {{ & $global:__nyaterm_local_ready_failed }}"
        ),
        session_token
    );

    vec![
        "-NoExit".to_string(),
        "-Command".to_string(),
        init,
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupInjectionDecision {
    Wait,
    Inject,
    Timeout,
}

pub struct StartupInjectionSchedule {
    started_at: Instant,
    last_activity: Option<Instant>,
    activity_not_before: Option<Instant>,
    idle: Duration,
    timeout: Duration,
}

impl StartupInjectionSchedule {
    pub fn new(started_at: Instant, idle: Duration, timeout: Duration) -> Self {
        Self {
            started_at,
            last_activity: None,
            activity_not_before: None,
            idle,
            timeout,
        }
    }

    pub fn note_activity(&mut self, at: Instant) {
        if self
            .activity_not_before
            .is_some_and(|not_before| at <= not_before)
        {
            return;
        }
        self.activity_not_before = None;
        self.last_activity = Some(at);
    }

    /// A terminal response has reached the PTY writer. Require subsequent
    /// shell output (normally the completed startup/prompt) before injection;
    /// queue drain alone does not prove the shell consumed the response.
    pub fn require_output_after_terminal_response(&mut self, written_at: Instant) {
        self.last_activity = None;
        self.activity_not_before = Some(written_at);
    }

    pub fn decision(&self, now: Instant) -> StartupInjectionDecision {
        if now.saturating_duration_since(self.started_at) >= self.timeout {
            return StartupInjectionDecision::Timeout;
        }
        if self
            .last_activity
            .is_some_and(|last| now.saturating_duration_since(last) >= self.idle)
        {
            return StartupInjectionDecision::Inject;
        }
        StartupInjectionDecision::Wait
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum StartupGateOutput {
    Buffered(String),
    Ready(String),
    SuppressedOverflow(String),
    Pass(String),
}

const MAX_STARTUP_PASSTHROUGH_SEQUENCE_BYTES: usize = 16 * 1024 + 16;

pub(crate) fn split_startup_passthrough(value: &str) -> (String, String) {
    let mut buffered = String::with_capacity(value.len());
    let mut passthrough = String::new();
    let mut cursor = 0;

    while cursor < value.len() {
        let Some((start, kind, opener_len)) = find_control_sequence_start(value, cursor) else {
            buffered.push_str(&value[cursor..]);
            break;
        };
        buffered.push_str(&value[cursor..start]);

        let Some((content_end, terminator_len)) =
            find_control_sequence_end(value, start, kind, opener_len)
        else {
            buffered.push_str(&value[start..]);
            break;
        };
        let end = content_end + terminator_len;
        let sequence = &value[start..end];
        let is_passthrough = sequence.len() <= MAX_STARTUP_PASSTHROUGH_SEQUENCE_BYTES
            && match kind {
                ControlSequenceKind::Csi => is_csi_passthrough(sequence.as_bytes()),
                ControlSequenceKind::Osc => control_sequence_payload(
                    value,
                    start,
                    opener_len,
                    content_end,
                    terminator_len,
                )
                .is_some_and(|payload| {
                    let payload = payload.as_bytes();
                    is_osc_query(payload) || matches!(osc_selector(payload), Some(0 | 2))
                }),
                ControlSequenceKind::Dcs => control_sequence_payload(
                    value,
                    start,
                    opener_len,
                    content_end,
                    terminator_len,
                )
                .is_some_and(|payload| {
                    let payload = payload.as_bytes();
                    payload.starts_with(b"$q") || payload.starts_with(b"+q")
                }),
            };
        if is_passthrough {
            passthrough.push_str(sequence);
        } else {
            buffered.push_str(sequence);
        }
        cursor = end;
    }

    (buffered, passthrough)
}

fn osc_selector(payload: &[u8]) -> Option<u32> {
    let selector = payload.split(|byte| *byte == b';').next()?;
    if selector.is_empty() || !selector.iter().all(u8::is_ascii_digit) {
        return None;
    }
    selector.iter().try_fold(0_u32, |value, digit| {
        value.checked_mul(10)?.checked_add(u32::from(digit - b'0'))
    })
}

fn is_osc_query(payload: &[u8]) -> bool {
    if !payload.ends_with(b"?") {
        return false;
    }
    matches!(
        osc_selector(payload),
        Some(4 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 52)
    )
}

fn csi_query_number(sequence: &[u8]) -> Option<&[u8]> {
    let body = sequence.get(2..sequence.len().saturating_sub(1))?;
    (sequence.last().copied() == Some(b't'))
        .then(|| body.split(|byte| *byte == b';').next().unwrap_or_default())
}

fn is_csi_passthrough(sequence: &[u8]) -> bool {
    let body = sequence.get(2..sequence.len().saturating_sub(1)).unwrap_or_default();
    match sequence.last().copied() {
        Some(b'c' | b'n') => true,
        Some(b't') => matches!(
            body.split(|byte| *byte == b';').next().unwrap_or_default(),
            b"11" | b"13" | b"14" | b"15" | b"16" | b"18" | b"19" | b"20" | b"21"
        ),
        Some(b'u') => body.starts_with(b"?"),
        Some(b'p') => body.contains(&b'$'),
        Some(b'q') => body.starts_with(b">") || body.contains(&b'$'),
        _ => false,
    }
}

/// CSI queries that xterm.js actually answers with the current secure window
/// options. CSI 20t/21t are still passed through, but title replies are
/// deliberately disabled and therefore must not hold the startup barrier.
fn is_csi_startup_query(sequence: &[u8]) -> bool {
    if csi_query_number(sequence).is_some_and(|number| number == b"20" || number == b"21") {
        return false;
    }
    is_csi_passthrough(sequence)
}

pub struct TerminalQueryDetector {
    pending: String,
}

impl TerminalQueryDetector {
    pub fn new() -> Self {
        Self {
            pending: String::new(),
        }
    }

    pub fn push(&mut self, chunk: &str) -> usize {
        self.pending.push_str(chunk);
        let value = std::mem::take(&mut self.pending);
        let mut cursor = 0;
        let mut count = 0;

        while cursor < value.len() {
            let Some((start, kind, opener_len)) = find_control_sequence_start(&value, cursor)
            else {
                break;
            };
            let Some((content_end, terminator_len)) =
                find_control_sequence_end(&value, start, kind, opener_len)
            else {
                self.pending.push_str(&value[start..]);
                break;
            };
            let is_query = match kind {
                ControlSequenceKind::Csi => {
                    is_csi_startup_query(&value.as_bytes()[start..content_end])
                }
                ControlSequenceKind::Osc => control_sequence_payload(
                    &value,
                    start,
                    opener_len,
                    content_end,
                    terminator_len,
                )
                .is_some_and(|payload| is_osc_query(payload.as_bytes())),
                ControlSequenceKind::Dcs => control_sequence_payload(
                    &value,
                    start,
                    opener_len,
                    content_end,
                    terminator_len,
                )
                .is_some_and(|payload| {
                    let payload = payload.as_bytes();
                    payload.starts_with(b"$q") || payload.starts_with(b"+q")
                }),
            };
            count += usize::from(is_query);
            cursor = content_end + terminator_len;
        }

        if self.pending.len() > 4096 {
            self.pending.clear();
        }
        count
    }

    pub fn has_pending_sequence(&self) -> bool {
        !self.pending.is_empty()
    }
}

#[derive(Debug)]
struct StartupGateState {
    active: bool,
    buffer: String,
}

/// Bounded gate used only while hiding a Unix injected command echo.
pub struct StartupOutputGate {
    state: StdMutex<StartupGateState>,
}

pub fn lock_or_recover<'a, T>(
    mutex: &'a StdMutex<T>,
    label: &'static str,
) -> std::sync::MutexGuard<'a, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!(lock = label, "Recovering poisoned Local terminal mutex");
            poisoned.into_inner()
        }
    }
}

impl StartupOutputGate {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::with_active(true)
    }

    pub fn new_inactive() -> Self {
        Self::with_active(false)
    }

    fn with_active(active: bool) -> Self {
        Self {
            state: StdMutex::new(StartupGateState {
                active,
                buffer: String::new(),
            }),
        }
    }

    pub fn activate(&self) {
        let mut state = lock_or_recover(&self.state, "startup_output_gate");
        state.buffer.clear();
        state.active = true;
    }

    pub fn is_active(&self) -> bool {
        lock_or_recover(&self.state, "startup_output_gate").active
    }

    pub fn consume(
        &self,
        visible: String,
        _visible_after_ready: String,
        ready: bool,
    ) -> StartupGateOutput {
        let mut state = lock_or_recover(&self.state, "startup_output_gate");
        if !state.active {
            return StartupGateOutput::Pass(visible);
        }

        state.buffer.push_str(&visible);
        let (buffered, passthrough) = split_startup_passthrough(&state.buffer);
        state.buffer = buffered;

        if ready {
            state.active = false;
            state.buffer.clear();
            // Startup output is visible while this gate is inactive. Once the
            // short source command is written, suppress every non-query byte
            // through the session-bound marker: Zsh can redraw arbitrary
            // command tails/RPROMPT fragments that are not safely separable
            // from a genuine late banner. The first rendered prompt remains.
            return StartupGateOutput::Ready(passthrough);
        }
        if state.buffer.len() > MAX_STARTUP_OUTPUT_BUFFER {
            // PTY output is ordered: after more than 64 KiB, the short source
            // command echo is already inside the discarded prefix. End the
            // gate immediately so passive shell output cannot remain hidden
            // until the watchdog fires.
            state.active = false;
            state.buffer.clear();
            return StartupGateOutput::SuppressedOverflow(passthrough);
        }
        StartupGateOutput::Buffered(passthrough)
    }

    pub fn discard(&self) -> bool {
        let mut state = lock_or_recover(&self.state, "startup_output_gate");
        if !state.active {
            return false;
        }
        state.active = false;
        state.buffer.clear();
        true
    }

}

#[cfg(unix)]
pub fn local_shell_line_editor_ready(
    master: &dyn portable_pty::MasterPty,
    shell_process_id: Option<u32>,
) -> bool {
    let (Some(fd), Some(shell_process_id)) = (master.as_raw_fd(), shell_process_id) else {
        return false;
    };
    if unsafe { libc::tcgetpgrp(fd) } != shell_process_id as libc::pid_t {
        return false;
    }

    let mut attributes = std::mem::MaybeUninit::<libc::termios>::uninit();
    if unsafe { libc::tcgetattr(fd, attributes.as_mut_ptr()) } != 0 {
        return false;
    }
    let local_flags = unsafe { attributes.assume_init() }.c_lflag;
    local_flags & libc::ICANON == 0 && local_flags & libc::ECHO == 0
}

#[cfg(not(unix))]
pub fn local_shell_line_editor_ready(
    _master: &dyn portable_pty::MasterPty,
    _shell_process_id: Option<u32>,
) -> bool {
    true
}

pub fn write_to_pty(writer: &mut dyn Write, data: &[u8]) -> std::io::Result<()> {
    writer.write_all(data)?;
    writer.flush()
}
