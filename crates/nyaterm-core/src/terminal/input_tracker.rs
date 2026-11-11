//! Local terminal line input tracker (Tauri `terminalInputTracker.ts` parity).

use regex::Regex;
use std::sync::OnceLock;
use zeroize::Zeroize;

#[derive(Clone, PartialEq, Eq, Default)]
pub struct TerminalInputState {
    pub value: String,
    /// Byte cursor into `value`.
    pub cursor: usize,
    pub desynced: bool,
    pub desync_reason: Option<&'static str>,
    pub line_rewrite_required: bool,
    pub multiline: bool,
    pub paste_mode: bool,
}

impl std::fmt::Debug for TerminalInputState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalInputState")
            .field("value", &"[REDACTED]")
            .field("cursor", &self.cursor)
            .field("desynced", &self.desynced)
            .field("desync_reason", &self.desync_reason)
            .field("line_rewrite_required", &self.line_rewrite_required)
            .field("multiline", &self.multiline)
            .field("paste_mode", &self.paste_mode)
            .finish()
    }
}

impl TerminalInputState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(multiline: bool) -> Self {
        Self {
            multiline,
            ..Self::default()
        }
    }
}

fn leading_env_prefix() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\(([^()\r\n]+)\)\s*").expect("env prefix"))
}

fn prompt_matchers() -> &'static [Regex; 9] {
    static MATCHERS: OnceLock<[Regex; 9]> = OnceLock::new();
    MATCHERS.get_or_init(|| {
        // Consume one separator after the terminator, preserving command spacing.
        // Specific structures precede bare theme arrows (e.g. `➜ ~/src cmd`).
        [
            r"^\[[^\]\r\n]+\] *[#$%](?: |$)",
            r"^[^\s@]+@[^:\s]+:[^#$%\r\n]*[#$%](?: |$)",
            // Windows prompts historically support `C:\path>command` without a separator.
            r"^PS +[^>\r\n]+> ?",
            r"^[A-Za-z]:(?:[\\/][^>\r\n]*)?> ?",
            r"^[A-Za-z0-9_.-]+:(?:~(?:/[^\s#$%❯➜→›]*)?|/[^\s#$%❯➜→›]*) *[#$%](?: |$)",
            r"^(?:~(?:/[^\s#$%❯➜→›]*)?|/[^\s#$%❯➜→›]*) +[#$%❯➜→›](?: |$)",
            r"^[❯➜→›] +(?:~(?:/[^\s#$%❯➜→›]*)?|/[^\s#$%❯➜→›]*)(?: |$)",
            r"^[❯➜→›](?: |$)",
            r"^[#$%](?: |$)",
        ]
        .map(|pattern| Regex::new(pattern).expect("terminal prompt"))
    })
}

/// Compile prompt matchers before the first interactive submission.
///
/// These matchers are used from the UI input path. Keeping their lazy
/// initialization there makes the first Enter key pay the regex compilation
/// cost, which is visible as an input hitch.
pub fn warm_terminal_input_tracker() {
    let _ = leading_env_prefix();
    let _ = prompt_matchers();
}

fn strip_leading_env_prefixes(input: &str) -> String {
    let mut remaining = input.to_string();
    loop {
        if let Some(m) = leading_env_prefix().find(&remaining) {
            remaining = remaining[m.end()..].to_string();
        } else {
            return remaining;
        }
    }
}

/// The exact recognized prompt prefix; navigation may use it as position evidence.
/// This does not grant permission to edit an unknown foreground program's input.
pub fn known_terminal_prompt_prefix(input: &str) -> Option<&str> {
    for matcher in prompt_matchers() {
        if let Some(found) = matcher.find(input) {
            return Some(&input[..found.end()]);
        }
    }
    None
}

fn strip_known_prompt_prefix(input: &str) -> String {
    let start = known_terminal_prompt_prefix(input).map_or(0, str::len);
    input[start..].to_string()
}

/// Remove known shell prompt prefixes while preserving command spacing.
pub fn strip_terminal_command_prompt(input: &str) -> String {
    let without_leading = input.trim_start();
    if without_leading.trim().is_empty() {
        return String::new();
    }
    strip_known_prompt_prefix(&strip_leading_env_prefixes(without_leading))
}

/// Remove known shell prompt prefixes so command parsing stays stable across shells.
pub fn sanitize_terminal_command(input: &str) -> String {
    strip_terminal_command_prompt(input).trim().to_string()
}

fn clamp_cursor(value: &str, cursor: usize) -> usize {
    cursor.min(value.len())
}

/// Upper bound for the locally tracked input line.
///
/// Every keystroke re-derives the tracked command (prompt stripping, suggestion
/// pattern length, history registration), so an unbounded value turns typing
/// inside a full-screen program into quadratic work on the UI thread. Past the
/// bound the tracker stops accumulating and reports itself desynced, which the
/// suggestion, history and smart-input paths already treat as "not a command
/// line".
pub const MAX_TRACKED_INPUT_BYTES: usize = 4096;

fn insert_text(state: &mut TerminalInputState, text: &str) {
    if text.is_empty() {
        return;
    }
    let cursor = clamp_cursor(&state.value, state.cursor);
    if state.value.len().saturating_add(text.len()) > MAX_TRACKED_INPUT_BYTES {
        mark_desynced(state, "too_long");
        return;
    }
    state.value.insert_str(cursor, text);
    state.cursor = cursor + text.len();
    state.multiline |= text.contains(['\n', '\r']);
}

fn refresh_multiline_after_delete(state: &mut TerminalInputState) {
    if state.multiline {
        state.multiline = state.value.contains(['\n', '\r']);
    }
}

fn delete_left(state: &mut TerminalInputState) {
    let cursor = clamp_cursor(&state.value, state.cursor);
    if cursor == 0 {
        return;
    }
    let prev = state.value[..cursor]
        .char_indices()
        .last()
        .map(|(i, _)| i)
        .unwrap_or(0);
    state.value.replace_range(prev..cursor, "");
    state.cursor = prev;
    refresh_multiline_after_delete(state);
}

fn delete_right(state: &mut TerminalInputState) {
    let cursor = clamp_cursor(&state.value, state.cursor);
    if cursor >= state.value.len() {
        return;
    }
    let next = state.value[cursor..]
        .char_indices()
        .nth(1)
        .map(|(i, _)| cursor + i)
        .unwrap_or(state.value.len());
    state.value.replace_range(cursor..next, "");
    state.cursor = cursor;
    refresh_multiline_after_delete(state);
}

fn delete_previous_word(state: &mut TerminalInputState) {
    let cursor = clamp_cursor(&state.value, state.cursor);
    if cursor == 0 {
        return;
    }
    let mut start = cursor;
    for (index, ch) in state.value[..cursor].char_indices().rev() {
        if ch.is_whitespace() {
            start = index;
        } else {
            break;
        }
    }
    for (index, ch) in state.value[..start].char_indices().rev() {
        if ch.is_whitespace() {
            break;
        }
        start = index;
    }
    state.value.replace_range(start..cursor, "");
    state.cursor = start;
    refresh_multiline_after_delete(state);
}

fn mark_desynced(state: &mut TerminalInputState, reason: &'static str) {
    state.desynced = true;
    state.desync_reason = Some(reason);
    state.line_rewrite_required |= reason == "tab";
}

fn apply_pasted_input_data(state: &mut TerminalInputState, data: &str) {
    let mut text = data.to_string();
    if text.contains("\u{1b}[200~") {
        state.paste_mode = true;
        text = text.replace("\u{1b}[200~", "");
    }
    if text.contains("\u{1b}[201~") {
        state.paste_mode = false;
        text = text.replace("\u{1b}[201~", "");
    }
    text = text.replace("\r\n", "\n").replace('\r', "\n");
    if text.is_empty() {
        return;
    }
    if state.desynced && state.desync_reason == Some("tab") {
        state.desynced = false;
        state.desync_reason = None;
        state.line_rewrite_required = true;
    }
    insert_text(state, &text);
}

/// Apply a raw terminal input chunk to the local tracker.
pub fn apply_terminal_input_data(state: &TerminalInputState, data: &str) -> TerminalInputState {
    let mut next = state.clone();
    apply_terminal_input_data_in_place(&mut next, data);
    next
}

/// Apply a raw terminal input chunk without cloning the tracked command buffer.
pub fn apply_terminal_input_data_in_place(state: &mut TerminalInputState, data: &str) {
    if data.is_empty() {
        return;
    }

    match data {
        "\r" | "\u{0003}" => {
            state.value.zeroize();
            *state = TerminalInputState::reset(false);
            return;
        }
        "\u{0001}" => {
            state.cursor = 0;
            return;
        }
        "\u{0005}" => {
            state.cursor = state.value.len();
            return;
        }
        "\u{0015}" => {
            let cursor = clamp_cursor(&state.value, state.cursor);
            state.value.replace_range(..cursor, "");
            state.cursor = 0;
            refresh_multiline_after_delete(state);
            return;
        }
        "\u{0017}" => {
            delete_previous_word(state);
            return;
        }
        "\u{000b}" => {
            let cursor = clamp_cursor(&state.value, state.cursor);
            state.value.truncate(cursor);
            state.cursor = cursor;
            refresh_multiline_after_delete(state);
            return;
        }
        "\u{000c}" => return,
        "\u{007f}" | "\u{0008}" => {
            delete_left(state);
            return;
        }
        "\u{1b}[D" | "\u{1b}OD" => {
            let cursor = clamp_cursor(&state.value, state.cursor);
            state.cursor = if cursor == 0 {
                0
            } else {
                state.value[..cursor]
                    .char_indices()
                    .last()
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            };
            return;
        }
        "\u{1b}[C" | "\u{1b}OC" => {
            let cursor = clamp_cursor(&state.value, state.cursor);
            state.cursor = if cursor >= state.value.len() {
                state.value.len()
            } else {
                state.value[cursor..]
                    .char_indices()
                    .nth(1)
                    .map(|(i, _)| cursor + i)
                    .unwrap_or(state.value.len())
            };
            return;
        }
        "\u{1b}[H" | "\u{1b}OH" => {
            state.cursor = 0;
            return;
        }
        "\u{1b}[F" | "\u{1b}OF" => {
            state.cursor = state.value.len();
            return;
        }
        "\u{1b}[3~" => {
            delete_right(state);
            return;
        }
        "\t" => {
            mark_desynced(state, "tab");
            return;
        }
        _ => {}
    }

    if state.paste_mode || data.contains("\u{1b}[200~") || data.contains("\u{1b}[201~") {
        apply_pasted_input_data(state, data);
        return;
    }

    if (data.contains('\n') || data.contains('\r')) && data != "\r" {
        apply_pasted_input_data(state, data);
        return;
    }

    if data.starts_with('\u{1b}') {
        mark_desynced(state, "terminal");
        return;
    }

    if data.chars().any(|ch| ch.is_control()) {
        mark_desynced(state, "terminal");
        return;
    }

    if state.desynced && state.desync_reason == Some("tab") {
        state.desynced = false;
        state.desync_reason = None;
        state.line_rewrite_required = true;
    }

    insert_text(state, data);
}

pub fn get_tracked_command(state: &TerminalInputState) -> String {
    if state.desynced || state.multiline {
        return String::new();
    }
    sanitize_terminal_command(&state.value)
}

pub fn can_suggest_from_tracker(state: &TerminalInputState) -> bool {
    can_suggest_from_tracked_command(state, &get_tracked_command(state))
}

pub fn can_suggest_from_tracked_command(state: &TerminalInputState, command: &str) -> bool {
    !state.desynced && !state.multiline && state.cursor == state.value.len() && !command.is_empty()
}

pub fn terminal_input_tracker_below_min_chars(
    state: &TerminalInputState,
    min_chars: usize,
) -> bool {
    state.value.chars().count() < min_chars.max(1)
}

pub fn can_register_command_from_tracker(state: &TerminalInputState) -> bool {
    !state.desynced && !state.multiline && !state.line_rewrite_required
}

pub fn get_tracked_submission_command(state: &TerminalInputState) -> String {
    if !can_register_command_from_tracker(state) {
        return String::new();
    }
    sanitize_terminal_command(&state.value)
}

fn normalize_line_content(value: &str) -> String {
    value.replace("\r\n", "").replace(['\n', '\r'], "")
}

fn choose_terminal_line_command(previous_value: &str, line_content: &str) -> Option<String> {
    let previous_command = sanitize_terminal_command(previous_value);
    let sanitized_line = sanitize_terminal_command(line_content);
    let mut candidates: Vec<(String, u32)> = Vec::new();

    let mut push_candidate = |raw: &str| {
        let normalized = strip_terminal_command_prompt(&normalize_line_content(raw));
        let command = sanitize_terminal_command(&normalized);
        if command.is_empty() {
            return;
        }
        let score = if previous_command.is_empty() || command.starts_with(&previous_command) {
            command.len() as u32
        } else {
            0
        };
        if !previous_command.is_empty() && score == 0 {
            return;
        }
        candidates.push((command, score));
    };

    push_candidate(&sanitized_line);
    push_candidate(line_content);

    // Suffix candidates: line after previous prefix
    for prefix in [
        &previous_value.to_string(),
        &previous_command,
        &sanitized_line,
    ] {
        let source = normalize_line_content(line_content);
        let source_cmd = sanitize_terminal_command(&source);
        let prefix_cmd = sanitize_terminal_command(prefix);
        if !prefix_cmd.is_empty()
            && let Some(pos) = source_cmd.find(&prefix_cmd)
        {
            let suffix = &source_cmd[pos..];
            push_candidate(suffix);
        }
    }

    candidates.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.len().cmp(&a.0.len())));
    candidates.into_iter().map(|(cmd, _)| cmd).next()
}

/// Inclusive-exclusive byte range within `TerminalInputState::value`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputSelectionRange {
    pub start: usize,
    pub end: usize,
}

impl InputSelectionRange {
    pub fn new(start: usize, end: usize) -> Option<Self> {
        if end > start {
            Some(Self { start, end })
        } else {
            None
        }
    }

    pub fn len_chars(&self, value: &str) -> usize {
        let end = self.end.min(value.len());
        let start = self.start.min(end);
        value[start..end].chars().count()
    }
}

/// Delete a byte range from the tracked input line (Tauri `deleteTerminalInputRange`).
pub fn delete_terminal_input_range(
    state: &TerminalInputState,
    start: usize,
    end: usize,
) -> TerminalInputState {
    let length = state.value.len();
    let from = start.min(length);
    let to = end.min(length).max(from);
    if to <= from {
        return state.clone();
    }
    let mut next = state.clone();
    next.value.replace_range(from..to, "");
    next.cursor = from;
    refresh_multiline_after_delete(&mut next);
    next
}

/// Build CSI left/right moves between two byte cursors (character steps).
pub fn build_move_input_cursor_data(
    value: &str,
    current_cursor: usize,
    target_cursor: usize,
) -> String {
    let current = current_cursor.min(value.len());
    let target = target_cursor.min(value.len());
    let current_chars = value[..current].chars().count();
    let target_chars = value[..target].chars().count();
    if target_chars > current_chars {
        "\u{1b}[C".repeat(target_chars - current_chars)
    } else if current_chars > target_chars {
        "\u{1b}[D".repeat(current_chars - target_chars)
    } else {
        String::new()
    }
}

/// Map a character index into `value` to a byte offset.
pub fn char_index_to_byte(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map(|(i, _)| i)
        .unwrap_or(value.len())
}

/// Map a byte offset into `value` to a character index.
pub fn byte_index_to_char(value: &str, byte_index: usize) -> usize {
    value[..byte_index.min(value.len())].chars().count()
}

/// Recover tracker value from the terminal buffer line after tab completion desync.
pub fn resync_from_terminal_line(
    current: &TerminalInputState,
    line_content: &str,
) -> Option<TerminalInputState> {
    let value = choose_terminal_line_command(&current.value, line_content)?;
    Some(TerminalInputState {
        value: value.clone(),
        cursor: value.len(),
        desynced: false,
        desync_reason: None,
        line_rewrite_required: false,
        multiline: false,
        paste_mode: false,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_TRACKED_INPUT_BYTES, TerminalInputState, apply_terminal_input_data,
        apply_terminal_input_data_in_place, build_move_input_cursor_data,
        can_suggest_from_tracked_command, can_suggest_from_tracker, delete_terminal_input_range,
        get_tracked_command, get_tracked_submission_command, known_terminal_prompt_prefix,
        resync_from_terminal_line, sanitize_terminal_command, strip_terminal_command_prompt,
        terminal_input_tracker_below_min_chars,
    };

    #[test]
    fn tracks_insert_backspace_and_submit() {
        let mut state = TerminalInputState::new();
        state = apply_terminal_input_data(&state, "gi");
        state = apply_terminal_input_data(&state, "t");
        assert_eq!(get_tracked_command(&state), "git");
        state = apply_terminal_input_data(&state, "\u{007f}");
        assert_eq!(get_tracked_command(&state), "gi");
        state = apply_terminal_input_data(&state, "\r");
        assert_eq!(get_tracked_command(&state), "");
        assert!(!state.desynced);
    }

    #[test]
    fn tab_desync_hides_suggestions() {
        let mut state = TerminalInputState::new();
        state = apply_terminal_input_data(&state, "doc");
        state = apply_terminal_input_data(&state, "\t");
        assert!(state.desynced);
        assert!(!can_suggest_from_tracker(&state));
    }

    #[test]
    fn strips_shell_prompt_prefixes() {
        assert_eq!(sanitize_terminal_command("user@host:~$ ls -la"), "ls -la");
        assert_eq!(sanitize_terminal_command("PS C:\\Users> dir"), "dir");
    }

    #[test]
    fn common_and_themed_prompts_share_exact_boundaries_and_preserve_command_spacing() {
        for prompt in [
            "fuxi:/sdcard # ",
            "host:~$ ",
            "/tmp $ ",
            "~ % ",
            "user@host:~% ",
            "[user@host ~]% ",
            "% ",
            "❯ ",
            "➜ ",
            "→ ",
            "› ",
            "~/src ❯ ",
            "/tmp ➜ ",
            "➜ ~/src ",
            "➜ ~ ",
            "❯ /tmp ",
            "# ",
            "$ ",
            "PS C:\\Users> ",
        ] {
            let line = format!("{prompt} echo  hi ");
            assert_eq!(
                known_terminal_prompt_prefix(&line),
                Some(prompt),
                "{prompt}"
            );
            assert_eq!(
                strip_terminal_command_prompt(&line),
                " echo  hi ",
                "{prompt}"
            );
            assert_eq!(sanitize_terminal_command(&line), "echo  hi", "{prompt}");
        }
        for prompt in ["#", "$", "%", "❯", "host:~$", "fuxi:/sdcard #"] {
            assert_eq!(known_terminal_prompt_prefix(prompt), Some(prompt));
        }
        assert_eq!(
            known_terminal_prompt_prefix("C:\\Users>dir"),
            Some("C:\\Users>")
        );
        assert_eq!(strip_terminal_command_prompt("C:\\Users>dir"), "dir");
    }

    #[test]
    fn prompt_expansion_does_not_strip_commands_challenges_or_arbitrary_output() {
        for text in [
            "echo $HOME",
            "echo $(pwd)",
            "$HOME",
            "${HOME}",
            "echo > file",
            "cat < file",
            "#comment",
            "%value",
            "❯command",
            "Password: ",
            "Token: ",
            "mysql> select",
            "postgres=# select",
            "ordinary output # command",
            "result → command",
            "host:label # cmd",
            "host:~relative$ cmd",
            "src ❯ command",
            "~relative ➜ command",
        ] {
            assert!(known_terminal_prompt_prefix(text).is_none(), "{text}");
            assert_eq!(strip_terminal_command_prompt(text), text, "{text}");
        }
    }

    #[test]
    fn resyncs_after_tab_desync_from_terminal_line() {
        let mut state = TerminalInputState::new();
        state = apply_terminal_input_data(&state, "doc");
        state = apply_terminal_input_data(&state, "\t");
        assert!(state.desynced);
        let recovered =
            resync_from_terminal_line(&state, "user@host:~$ docker compose ps").expect("recover");
        assert_eq!(get_tracked_command(&recovered), "docker compose ps");
        assert!(!recovered.desynced);
    }

    #[test]
    fn resyncs_tab_completion_with_common_shell_prompts() {
        let mut state = TerminalInputState::new();
        state = apply_terminal_input_data(&state, "vi ins\t");
        for line in [
            "[root@rocky9 ~]# vi install-node-exporter.sh",
            "$ vi install-node-exporter.sh",
            "PS C:\\Users> vi install-node-exporter.sh",
            "fuxi:/sdcard # vi install-node-exporter.sh",
            "➜ ~/src vi install-node-exporter.sh",
            "~ % vi install-node-exporter.sh",
        ] {
            let recovered = resync_from_terminal_line(&state, line).expect("recover");
            assert_eq!(recovered.value, "vi install-node-exporter.sh");
        }
    }

    #[test]
    fn submission_requires_synced_state() {
        let mut state = TerminalInputState::new();
        state = apply_terminal_input_data(&state, "ls");
        assert_eq!(get_tracked_submission_command(&state), "ls");
        state = apply_terminal_input_data(&state, "\t");
        assert!(get_tracked_submission_command(&state).is_empty());
    }

    #[test]
    fn tracker_short_input_stays_below_suggestion_threshold() {
        let mut state = TerminalInputState::new();
        state = apply_terminal_input_data(&state, "g");
        assert!(terminal_input_tracker_below_min_chars(&state, 2));
        assert!(!terminal_input_tracker_below_min_chars(&state, 1));
    }

    #[test]
    fn oversized_tracked_input_stops_tracking_instead_of_growing() {
        let mut state = TerminalInputState::new();
        let mut typed = 0usize;
        while typed <= MAX_TRACKED_INPUT_BYTES {
            apply_terminal_input_data_in_place(&mut state, "1");
            typed += 1;
        }

        assert_eq!(state.value.len(), MAX_TRACKED_INPUT_BYTES);
        assert!(state.desynced);
        assert_eq!(state.desync_reason, Some("too_long"));
        assert!(!can_suggest_from_tracker(&state));
        assert!(get_tracked_submission_command(&state).is_empty());
        // A fresh line after Enter must track normally again.
        apply_terminal_input_data_in_place(&mut state, "\r");
        apply_terminal_input_data_in_place(&mut state, "ls");
        assert_eq!(get_tracked_command(&state), "ls");
    }

    #[test]
    fn suggestible_command_can_reuse_precomputed_text() {
        let mut state = TerminalInputState::new();
        state = apply_terminal_input_data(&state, "git");
        let command = get_tracked_command(&state);
        assert!(can_suggest_from_tracked_command(&state, &command));
    }

    #[test]
    fn deletes_input_range() {
        let mut state = TerminalInputState::new();
        state = apply_terminal_input_data(&state, "abcdef");
        state.cursor = 4;
        let next = delete_terminal_input_range(&state, 1, 4);
        assert_eq!(next.value, "aef");
        assert_eq!(next.cursor, 1);
    }

    #[test]
    fn builds_move_cursor_sequences_by_char() {
        let value = "abc";
        assert_eq!(
            build_move_input_cursor_data(value, 0, 3),
            "\u{1b}[C\u{1b}[C\u{1b}[C"
        );
        assert_eq!(
            build_move_input_cursor_data(value, 3, 1),
            "\u{1b}[D\u{1b}[D"
        );
        assert_eq!(build_move_input_cursor_data(value, 2, 2), "");
    }

    #[test]
    fn in_place_input_matches_immutable_wrapper_for_editing_sequences() {
        let chunks = [
            "git status",
            "\u{1b}[D",
            "\u{1b}[D",
            "!",
            "\u{007f}",
            "\u{0017}",
            "echo ",
            "世界",
            "\u{0001}",
            "> ",
            "\u{0005}",
            "\u{1b}[3~",
        ];
        let mut immutable = TerminalInputState::new();
        let mut in_place = TerminalInputState::new();

        for chunk in chunks {
            immutable = apply_terminal_input_data(&immutable, chunk);
            apply_terminal_input_data_in_place(&mut in_place, chunk);
            assert_eq!(in_place, immutable, "state differs after {chunk:?}");
        }
    }

    #[test]
    fn in_place_input_handles_unicode_cursor_and_deletion_boundaries() {
        let mut state = TerminalInputState::new();
        apply_terminal_input_data_in_place(&mut state, "a你🙂z");
        apply_terminal_input_data_in_place(&mut state, "\u{1b}[D");
        apply_terminal_input_data_in_place(&mut state, "\u{007f}");
        assert_eq!(state.value, "a你z");
        assert_eq!(state.cursor, "a你".len());

        apply_terminal_input_data_in_place(&mut state, "\u{1b}[D");
        apply_terminal_input_data_in_place(&mut state, "\u{1b}[3~");
        assert_eq!(state.value, "az");
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn in_place_input_preserves_control_paste_and_reset_behavior() {
        let mut state = TerminalInputState::new();
        apply_terminal_input_data_in_place(&mut state, "one two");
        apply_terminal_input_data_in_place(&mut state, "\u{0017}");
        assert_eq!(state.value, "one ");
        apply_terminal_input_data_in_place(&mut state, "\u{0015}");
        assert!(state.value.is_empty());

        apply_terminal_input_data_in_place(&mut state, "\u{1b}[200~a\r\nb\u{1b}[201~");
        assert_eq!(state.value, "a\nb");
        assert!(state.multiline);
        assert!(!state.paste_mode);

        apply_terminal_input_data_in_place(&mut state, "\t");
        assert_eq!(state.desync_reason, Some("tab"));
        apply_terminal_input_data_in_place(&mut state, "x");
        assert!(!state.desynced);
        assert!(state.line_rewrite_required);

        apply_terminal_input_data_in_place(&mut state, "\u{0003}");
        assert_eq!(state, TerminalInputState::new());
    }

    #[test]
    #[ignore = "manual allocation and complexity regression benchmark"]
    fn sustained_in_place_input_and_deletion() {
        let mut state = TerminalInputState::new();
        for _ in 0..100_000 {
            apply_terminal_input_data_in_place(&mut state, "x");
        }
        assert_eq!(state.value.len(), 100_000);
        for _ in 0..100_000 {
            apply_terminal_input_data_in_place(&mut state, "\u{007f}");
        }
        assert!(state.value.is_empty());
    }
}
