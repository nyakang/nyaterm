# Executed as a command argument, never sent to the terminal's stdin.
nyaterm_dir=$1
nyaterm_namespace=$2
nyaterm_shell=$3
shift 3
if [ -z "$nyaterm_shell" ]; then
    nyaterm_shell=$(getent passwd "$(id -u)" 2>/dev/null | cut -d: -f7)
    [ -n "$nyaterm_shell" ] || nyaterm_shell=${SHELL:-/bin/sh}
    set -- -l -i
fi
case "${nyaterm_shell##*/}" in
    bash|bash.exe|zsh|zsh.exe|fish|fish.exe) ;;
    *) exec "$nyaterm_shell" "$@" ;;
esac

# Rust owns this directory and removes it after the PTY and processes close.
# WSL must resolve the host path in the selected distribution; no guest /tmp
# directory or EXIT trap is needed, including when startup blocks in an rc file.
case $nyaterm_namespace in
    wsl) nyaterm_dir=$(wslpath -u "$nyaterm_dir") || exec "$nyaterm_shell" "$@" ;;
    native)
        case $nyaterm_dir in
            [a-zA-Z]:/*) nyaterm_dir=$(cygpath -u "$nyaterm_dir") || exec "$nyaterm_shell" "$@" ;;
        esac
        ;;
esac
NYATERM_LOCAL_DIR=$nyaterm_dir
export NYATERM_LOCAL_DIR

cat > "$nyaterm_dir/bash" <<'NYATERM_BASH_EOF' || exec "$nyaterm_shell" "$@"
# Remove only our loader, preserving string and array PROMPT_COMMAND hooks.
if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) =~ ^declare\ -[^[:space:]]*r ]]; then
    unset NYATERM_LOCAL_DIR
    return 0
fi
__nyaterm_loader='if [ -n "${NYATERM_LOCAL_DIR:-}" ]; then . "$NYATERM_LOCAL_DIR/bash"; fi'
for __nyaterm_index in "${!PROMPT_COMMAND[@]}"; do
    PROMPT_COMMAND[$__nyaterm_index]=${PROMPT_COMMAND[$__nyaterm_index]//"$__nyaterm_loader"/:}
done
unset __nyaterm_loader __nyaterm_index
__nyaterm_local_cwd() {
    local ret=$?
    local path=$PWD
    # MSYS mounts such as /tmp need not be drive-letter aliases. Emit the
    # actual Windows URI rather than guessing their host mapping.
    case $OSTYPE in
        msys*|cygwin*) path=$(pwd -W 2>/dev/null) || path=$(cygpath -m "$PWD" 2>/dev/null) || path=$PWD ;;
    esac
    case $path in
        [a-zA-Z]:/*) path=/$path ;;
    esac
    printf '\033]7;file://localhost%s\007' "${path//%/%25}"
    return "$ret"
}
if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == 'declare -a '* ]]; then
    PROMPT_COMMAND+=(__nyaterm_local_cwd)
else
    PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND
}__nyaterm_local_cwd"
fi
export -n PROMPT_COMMAND
unset NYATERM_LOCAL_DIR
__nyaterm_local_cwd
NYATERM_BASH_EOF

cat > "$nyaterm_dir/fish" <<'NYATERM_FISH_EOF' || exec "$nyaterm_shell" "$@"
function __nyaterm_local_cwd --on-event fish_prompt
    printf '\033]7;file://localhost%s\007' (string replace -a -- '%' '%25' $PWD)
end
set -e NYATERM_LOCAL_DIR
NYATERM_FISH_EOF

cat > "$nyaterm_dir/.zshenv" <<'NYATERM_ZSHENV_EOF' || exec "$nyaterm_shell" "$@"
if [[ $NYATERM_LOCAL_USER_ZDOTDIR_SET = x ]]; then
    ZDOTDIR=$NYATERM_LOCAL_USER_ZDOTDIR
else
    unset ZDOTDIR
fi
[[ ! -r "${ZDOTDIR:-$HOME}/.zshenv" ]] || source "${ZDOTDIR:-$HOME}/.zshenv"
NYATERM_LOCAL_USER_ZDOTDIR=${ZDOTDIR:-$HOME}
NYATERM_LOCAL_USER_ZDOTDIR_SET=${ZDOTDIR+x}
if [[ -o rcs ]]; then
    ZDOTDIR=$NYATERM_LOCAL_DIR
else
    unset NYATERM_LOCAL_DIR NYATERM_LOCAL_USER_ZDOTDIR NYATERM_LOCAL_USER_ZDOTDIR_SET
fi
NYATERM_ZSHENV_EOF

cat > "$nyaterm_dir/.zprofile" <<'NYATERM_ZPROFILE_EOF' || exec "$nyaterm_shell" "$@"
if [[ $NYATERM_LOCAL_USER_ZDOTDIR_SET = x ]]; then
    ZDOTDIR=$NYATERM_LOCAL_USER_ZDOTDIR
else
    unset ZDOTDIR
fi
[[ ! -r "${ZDOTDIR:-$HOME}/.zprofile" ]] || source "${ZDOTDIR:-$HOME}/.zprofile"
NYATERM_LOCAL_USER_ZDOTDIR=${ZDOTDIR:-$HOME}
NYATERM_LOCAL_USER_ZDOTDIR_SET=${ZDOTDIR+x}
if [[ -o rcs ]]; then
    ZDOTDIR=$NYATERM_LOCAL_DIR
else
    unset NYATERM_LOCAL_DIR NYATERM_LOCAL_USER_ZDOTDIR NYATERM_LOCAL_USER_ZDOTDIR_SET
fi
NYATERM_ZPROFILE_EOF

cat > "$nyaterm_dir/.zshrc" <<'NYATERM_ZSHRC_EOF' || exec "$nyaterm_shell" "$@"
if [[ $NYATERM_LOCAL_USER_ZDOTDIR_SET = x ]]; then
    ZDOTDIR=$NYATERM_LOCAL_USER_ZDOTDIR
else
    unset ZDOTDIR
fi
if [[ -n $NYATERM_LOCAL_DIR && $HISTFILE == "$NYATERM_LOCAL_DIR/.zsh_history" ]]; then
    HISTFILE=${ZDOTDIR:-$HOME}/.zsh_history
fi
[[ ! -r "${ZDOTDIR:-$HOME}/.zshrc" ]] || source "${ZDOTDIR:-$HOME}/.zshrc"
__nyaterm_local_cwd() {
    printf '\033]7;file://localhost%s\007' "${PWD//%/%25}"
}
autoload -Uz add-zsh-hook
add-zsh-hook precmd __nyaterm_local_cwd
if [[ -o login && -o rcs ]]; then
    NYATERM_LOCAL_USER_ZDOTDIR=${ZDOTDIR:-$HOME}
    NYATERM_LOCAL_USER_ZDOTDIR_SET=${ZDOTDIR+x}
    ZDOTDIR=$NYATERM_LOCAL_DIR
else
    unset NYATERM_LOCAL_DIR NYATERM_LOCAL_USER_ZDOTDIR NYATERM_LOCAL_USER_ZDOTDIR_SET
fi
NYATERM_ZSHRC_EOF

cat > "$nyaterm_dir/.zlogin" <<'NYATERM_ZLOGIN_EOF' || exec "$nyaterm_shell" "$@"
if [[ $NYATERM_LOCAL_USER_ZDOTDIR_SET = x ]]; then
    ZDOTDIR=$NYATERM_LOCAL_USER_ZDOTDIR
else
    unset ZDOTDIR
fi
[[ ! -r "${ZDOTDIR:-$HOME}/.zlogin" ]] || source "${ZDOTDIR:-$HOME}/.zlogin"
unset NYATERM_LOCAL_DIR NYATERM_LOCAL_USER_ZDOTDIR NYATERM_LOCAL_USER_ZDOTDIR_SET
NYATERM_ZLOGIN_EOF

case "${nyaterm_shell##*/}" in
    bash|bash.exe)
        # Profiles retain control: if they replace PROMPT_COMMAND, integration
        # is skipped rather than overriding user startup behavior.
        PROMPT_COMMAND='if [ -n "${NYATERM_LOCAL_DIR:-}" ]; then . "$NYATERM_LOCAL_DIR/bash"; fi'${PROMPT_COMMAND:+"; $PROMPT_COMMAND"}
        export PROMPT_COMMAND
        exec "$nyaterm_shell" "$@"
        ;;
    zsh|zsh.exe)
        NYATERM_LOCAL_USER_ZDOTDIR=${ZDOTDIR:-$HOME}
        NYATERM_LOCAL_USER_ZDOTDIR_SET=${ZDOTDIR+x}
        ZDOTDIR=$nyaterm_dir
        export NYATERM_LOCAL_USER_ZDOTDIR NYATERM_LOCAL_USER_ZDOTDIR_SET ZDOTDIR
        exec "$nyaterm_shell" "$@"
        ;;
    fish|fish.exe)
        exec "$nyaterm_shell" --init-command 'source "$NYATERM_LOCAL_DIR/fish"' "$@"
        ;;
esac
