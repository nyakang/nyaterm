# nyaterm shell integration v1
#
# Optional SSH persistent Bash integration. This file is definitions-only: the
# activation command calls __nyaterm_install_prompt after exporting the
# session-bound command marker. It never edits a user's profile by itself.

__nyaterm_host(){ hostname 2>/dev/null || printf localhost; }

__nyaterm_ready_failed(){
  [ -n "${__nyaterm_failure_reported:-}" ] || {
    __nyaterm_failure_reported=1
    printf '%s' "${NYATERM_READY_FAILED_MARKER-}"
  }
}

__nyaterm_restore_status(){ return "$1"; }
__nyaterm_prompt_guard(){ return $?; }

__nyaterm_prune_history(){
  [ -n "${NYATERM_PRUNE_HISTORY:-}" ] || return 0
  unset NYATERM_PRUNE_HISTORY
  local hline history_number
  hline="$(HISTTIMEFORMAT= history 1 2>/dev/null || true)"
  case "$hline" in
    *NYATERM_PRUNE_HISTORY*|*NYATERM_INJ*|*__nyaterm_install_prompt*|*NyaTermReady*)
      history_number=${hline#"${hline%%[![:space:]]*}"}
      history_number=${history_number%%[!0-9]*}
      [ -z "$history_number" ] || history -d "$history_number" 2>/dev/null || true
      ;;
  esac
  NYATERM_LAST_HISTCMD="${HISTCMD-}"
}

__nyaterm_emit_command(){
  local histcmd="${HISTCMD-}"
  if [ -n "$histcmd" ] && [ "${NYATERM_LAST_HISTCMD-}" != "$histcmd" ]; then
    NYATERM_LAST_HISTCMD="$histcmd"
    local cmd
    cmd="$(fc -ln -1 2>/dev/null)"
    if [ -n "$cmd" ] && command -v base64 >/dev/null 2>&1; then
      local b64
      b64="$(printf '%s' "$cmd" | base64 | tr -d '\r\n')"
      printf '\033]7777;NyaTermCommand:%s\007' "$b64"
    fi
  fi
}

__nyaterm_prompt(){
  local status=$?
  __nyaterm_prune_history
  __nyaterm_emit_command
  if [ -n "${NYATERM_READY_PENDING:-}" ]; then
    unset NYATERM_READY_PENDING
    printf '%s' "${NYATERM_READY_MARKER-}"
  fi
  local cwd="${PWD//%/%25}"
  printf '\033]7;file://%s%s\007' "$(__nyaterm_host)" "$cwd"
  return "$status"
}

__nyaterm_array_prompt_supported(){
  [ "${BASH_VERSINFO[0]:-0}" -gt 5 ] || {
    [ "${BASH_VERSINFO[0]:-0}" -eq 5 ] && [ "${BASH_VERSINFO[1]:-0}" -ge 1 ]
  }
}

__nyaterm_prompt_state_writable(){
  local name decl
  for name in \
    __nyaterm_saved_prompt_command \
    __nyaterm_saved_prompt_command_mirror \
    __nyaterm_extra_prompt_commands \
    __nyaterm_extra_prompt_commands_mirror \
    __nyaterm_exported_prompt_fallback; do
    decl="$(declare -p "$name" 2>/dev/null || true)"
    [[ ! "$decl" =~ ^declare\ -[^[:space:]]*r ]] || return 1
  done
}

__nyaterm_state_token(){
  local marker token
  marker="${NYATERM_COMMAND_MARKER-}"
  case "$marker" in
    7777\;NyaTermCommand:*:)
      token="${marker#7777;NyaTermCommand:}"
      token="${token%:}"
      [ -n "$token" ] || return 1
      printf '%s' "$token"
      ;;
    *) return 1 ;;
  esac
}

__nyaterm_prompt_command_is_managed(){
  local current="${PROMPT_COMMAND-}"
  local canonical='__nyaterm_prompt; __nyaterm_run_saved_prompt_command; __nyaterm_repair_prompt; __nyaterm_prompt_guard'
  local exported='if [ "${__nyaterm_integration_active-}" = 1 ] && declare -F __nyaterm_prompt >/dev/null 2>&1; then __nyaterm_prompt; __nyaterm_run_saved_prompt_command; __nyaterm_repair_prompt; __nyaterm_prompt_guard; else eval -- "${__nyaterm_exported_prompt_fallback-}"; fi'
  local direct_exported='if declare -F __nyaterm_prompt >/dev/null 2>&1; then __nyaterm_prompt; __nyaterm_run_saved_prompt_command; __nyaterm_repair_prompt; __nyaterm_prompt_guard; else eval -- "${__nyaterm_exported_prompt_fallback-}"; fi'
  [ "$current" = "$canonical" ] || [ "$current" = "$exported" ] || [ "$current" = "$direct_exported" ]
}

__nyaterm_initialize_prompt_state(){
  local token expected nonce_decl owner_decl name decl
  token="$( __nyaterm_state_token )" || return 1
  expected="nyaterm-shell-integration-v1:$token"
  nonce_decl="$(declare -p __nyaterm_prompt_state_nonce 2>/dev/null || true)"
  owner_decl="$(declare -p __nyaterm_prompt_state_owner 2>/dev/null || true)"

  if [ -n "$nonce_decl" ] || [ -n "$owner_decl" ]; then
    [[ "$nonce_decl" =~ ^declare\ -[^[:space:]]*r ]] || return 1
    [[ "$owner_decl" =~ ^declare\ -[^[:space:]]*r ]] || return 1
    [ "${__nyaterm_prompt_state_nonce-}" = "$token" ] || return 1
    [ "${__nyaterm_prompt_state_owner-}" = "$expected" ] || return 1
    return 0
  fi

  for name in __nyaterm_saved_prompt_command __nyaterm_saved_prompt_command_mirror __nyaterm_extra_prompt_commands __nyaterm_extra_prompt_commands_mirror; do
    decl="$(declare -p "$name" 2>/dev/null || true)"
    [ -z "$decl" ] || return 1
  done

  __nyaterm_prompt_state_nonce="$token" || return 1
  readonly __nyaterm_prompt_state_nonce || return 1
  __nyaterm_prompt_state_owner="$expected" || return 1
  readonly __nyaterm_prompt_state_owner || return 1
  __nyaterm_extra_prompt_commands=() || return 1
}

__nyaterm_prompt_state_mirror_valid(){
  local nonce_decl owner_decl expected
  nonce_decl="$(declare -p __nyaterm_prompt_state_nonce 2>/dev/null || true)"
  owner_decl="$(declare -p __nyaterm_prompt_state_owner 2>/dev/null || true)"
  [[ "$nonce_decl" =~ ^declare\ -[^[:space:]]*r ]] || return 1
  [[ "$owner_decl" =~ ^declare\ -[^[:space:]]*r ]] || return 1
  expected="nyaterm-shell-integration-v1:${__nyaterm_prompt_state_nonce}"
  [ "${__nyaterm_prompt_state_owner-}" = "$expected" ] || return 1
  [[ $(declare -p __nyaterm_extra_prompt_commands_mirror 2>/dev/null) == declare\ -a* ]]
}

__nyaterm_sync_prompt_state(){
  __nyaterm_prompt_state_writable || return 1
  __nyaterm_saved_prompt_command_mirror="${__nyaterm_saved_prompt_command-}"
  __nyaterm_extra_prompt_commands_mirror=("${__nyaterm_extra_prompt_commands[@]}")
}

__nyaterm_restore_prompt_state(){
  __nyaterm_prompt_state_mirror_valid || return 1
  __nyaterm_prompt_state_writable || return 1
  __nyaterm_saved_prompt_command="${__nyaterm_saved_prompt_command_mirror-}"
  __nyaterm_extra_prompt_commands=("${__nyaterm_extra_prompt_commands_mirror[@]}")
}

__nyaterm_prompt_state_valid(){
  __nyaterm_prompt_state_mirror_valid || return 1
  [[ $(declare -p __nyaterm_extra_prompt_commands 2>/dev/null) == declare\ -a* ]] || return 1
  [ "${__nyaterm_saved_prompt_command-}" = "${__nyaterm_saved_prompt_command_mirror-}" ] || return 1
  [ ${#__nyaterm_extra_prompt_commands[@]} -eq ${#__nyaterm_extra_prompt_commands_mirror[@]} ] || return 1
  local i
  for ((i=0; i<${#__nyaterm_extra_prompt_commands[@]}; i++)); do
    [ "${__nyaterm_extra_prompt_commands[i]}" = "${__nyaterm_extra_prompt_commands_mirror[i]}" ] || return 1
  done
}

__nyaterm_rebuild_exported_prompt_fallback(){
  local command result="${__nyaterm_saved_prompt_command-}"
  for command in "${__nyaterm_extra_prompt_commands[@]}"; do
    if [ -n "$result" ]; then result="$result; $command"; else result="$command"; fi
  done
  __nyaterm_exported_prompt_fallback="$result"
}

__nyaterm_run_saved_prompt_command(){
  local status=$? command
  [ "${__nyaterm_integration_active-}" = 1 ] || return "$status"
  __nyaterm_prompt_state_writable || return "$status"
  if ! __nyaterm_prompt_state_valid; then
    __nyaterm_ready_failed
    return "$status"
  fi
  if [ -n "${__nyaterm_saved_prompt_command-}" ]; then
    __nyaterm_restore_status "$status"
    builtin eval -- "$__nyaterm_saved_prompt_command"
    status=$?
  fi
  for command in "${__nyaterm_extra_prompt_commands[@]}"; do
    __nyaterm_restore_status "$status"
    builtin eval -- "$command"
    status=$?
  done
  return "$status"
}

__nyaterm_capture_prompt_string(){
  local current="$1" expected="$2" tail
  if [ "$current" = "$expected" ]; then
    return 0
  fi
  case "$current" in
    "$expected"*)
      tail=${current#"$expected"}
      while :; do
        tail=${tail#"${tail%%[![:space:]]*}"}
        case "$tail" in \;*|\&*) tail=${tail#?};; *) break;; esac
      done
      [ -z "$tail" ] || __nyaterm_extra_prompt_commands[${#__nyaterm_extra_prompt_commands[@]}]="$tail"
      ;;
    *"$expected"*) return 1;;
    *) __nyaterm_saved_prompt_command="$current"; __nyaterm_extra_prompt_commands=();;
  esac
  __nyaterm_rebuild_exported_prompt_fallback
  __nyaterm_sync_prompt_state
}

__nyaterm_repair_prompt_container(){
  local decl f current canonical expected exported=0
  canonical='__nyaterm_prompt; __nyaterm_run_saved_prompt_command; __nyaterm_repair_prompt; __nyaterm_prompt_guard'
  expected="$canonical"
  [ "${__nyaterm_integration_active-}" = 1 ] || return 1
  __nyaterm_initialize_prompt_state || return 1
  __nyaterm_prompt_state_writable || return 1
  decl="$(declare -p PROMPT_COMMAND 2>/dev/null || true)"
  [[ "$decl" =~ ^declare\ -[^[:space:]]*x ]] && exported=1
  [[ ! "$decl" =~ ^declare\ -[^[:space:]]*r ]] || return 1
  if [ "$exported" -eq 1 ]; then
    expected='if [ "${__nyaterm_integration_active-}" = 1 ] && declare -F __nyaterm_prompt >/dev/null 2>&1; then __nyaterm_prompt; __nyaterm_run_saved_prompt_command; __nyaterm_repair_prompt; __nyaterm_prompt_guard; else eval -- "${__nyaterm_exported_prompt_fallback-}"; fi'
  fi
  if [[ "$decl" =~ ^declare\ -[^[:space:]]*a[^[:space:]]*\ PROMPT_COMMAND= ]] && __nyaterm_array_prompt_supported; then
    local -a retained=()
    for f in "${PROMPT_COMMAND[@]}"; do
      case "$f" in __nyaterm_prompt|__nyaterm_repair_prompt) ;; *) retained+=("$f");; esac
    done
    PROMPT_COMMAND=(__nyaterm_prompt "${retained[@]}" __nyaterm_repair_prompt) || return 1
  else
    current=${PROMPT_COMMAND-}
    if [[ "$decl" =~ ^declare\ -[^[:space:]]*a[^[:space:]]*\ PROMPT_COMMAND= ]]; then unset PROMPT_COMMAND; fi
    if ! __nyaterm_capture_prompt_string "$current" "$expected"; then
      PROMPT_COMMAND="$expected"
      return 1
    fi
    PROMPT_COMMAND="$expected" || return 1
    if [ "$exported" -eq 1 ]; then
      __nyaterm_rebuild_exported_prompt_fallback
      export __nyaterm_exported_prompt_fallback
    else
      unset __nyaterm_exported_prompt_fallback
    fi
  fi
  __nyaterm_sync_prompt_state || return 1
}

__nyaterm_repair_prompt(){
  local status=$?
  __nyaterm_repair_prompt_container || __nyaterm_ready_failed
  return "$status"
}

__nyaterm_install_prompt(){
  local owner_decl current
  owner_decl="$(declare -p __nyaterm_prompt_state_owner 2>/dev/null || true)"
  __nyaterm_initialize_prompt_state || return 1
  if [ -z "$owner_decl" ]; then
    __nyaterm_prompt_state_writable || return 1
    current="${PROMPT_COMMAND-}"
    if __nyaterm_prompt_command_is_managed; then
      current="${__nyaterm_exported_prompt_fallback-}"
      [ -n "$current" ] || return 1
    fi
    __nyaterm_saved_prompt_command="$current"
    __nyaterm_extra_prompt_commands=()
    __nyaterm_rebuild_exported_prompt_fallback
    __nyaterm_sync_prompt_state || return 1
  else
    __nyaterm_restore_prompt_state || return 1
  fi
  __nyaterm_integration_active=1 || return 1
  NYATERM_LAST_HISTCMD="${HISTCMD-}"
  __nyaterm_repair_prompt_container
}

# A child Bash can inherit a direct-session wrapper while loading this
# definitions-only resource from .bashrc. Until activation explicitly calls
# install_prompt, restore the exported user hook so the inherited wrapper
# cannot emit metadata or suppress user prompt behavior in the child.
if [ "${__nyaterm_integration_active-}" != 1 ] && __nyaterm_prompt_command_is_managed; then
  PROMPT_COMMAND="${__nyaterm_exported_prompt_fallback-}"
fi
