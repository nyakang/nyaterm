//! Line-role dispatch for terminal output highlighting.
//!
//! Instead of running every generic rule against every line, recognise what
//! kind of command produced the line and parse its fields directly. This keeps
//! `ip addr`, `ls -l` and `ps aux` output from sharing rules with unrelated
//! text, which is where most false positives come from.
//!
//! P1 covers three roles. Anything unrecognised keeps the existing generic
//! rules, so this is a purely additive change.

use std::ops::Range;

/// What kind of output a line belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LineRole {
    /// `ip addr` / `ip -br addr` output.
    IpOutput,
    /// `ls -l` long listing.
    FileListingOutput,
    /// `ps aux` output.
    PsOutput,
    /// `ps -ef` output (different columns from `ps aux`).
    PsFullOutput,
    /// `git diff` output.
    GitDiffOutput,
    /// `git log` output.
    GitLogOutput,
    /// `stat` output.
    FileStatOutput,
    /// `getfacl` output.
    FileAclOutput,
    /// Test runner output (`cargo test`, `pytest`, `go test`).
    TestOutput,
    /// Cargo progress lines (`Compiling`, `Finished`, `Running`).
    RustToolOutput,
    /// C/C++ compiler diagnostics with `file:line:col:` locations.
    CCompilerOutput,
    /// Bare SQL statements.
    SqlOutput,
    /// `git status` output.
    GitStatusOutput,
    /// `docker ps` output.
    ContainerOutput,
    /// `systemctl status` output.
    SystemdOutput,
    /// `ss` / `netstat` socket tables.
    SocketOutput,
    /// `df -h` / `du -sh` output.
    DiskUsageOutput,
    /// `free -h` output.
    MemoryUsageOutput,
    /// Compiler and build-tool diagnostics (`cargo`, `make`, `gcc`).
    BuildOutput,
    /// nginx / journalctl / syslog lines.
    LogOutput,
    /// Network-device CLI configuration (H3C / Huawei / Cisco / Ruijie).
    NetworkConfigOutput,
    Unknown,
}

impl LineRole {
    pub(super) fn is_known(self) -> bool {
        self != LineRole::Unknown
    }
}

/// Detect the role of one line from table headers and shape fingerprints.
///
/// Headers are the strongest signal: `ip addr` prints `1: lo: <LOOPBACK...>`,
/// `ls -l` lines start with a mode string, and `ps aux` has a `USER PID` header.
pub(super) fn detect(line: &str) -> LineRole {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return LineRole::Unknown;
    }

    // `ps aux` / `ps -ef` header and rows: USER PID %CPU ...
    let first = trimmed.split_whitespace().next().unwrap_or("");
    // `ps -ef` has PPID/STIME columns; `ps aux` has %CPU/%MEM.
    if matches!(first, "UID" | "USER") && trimmed.contains("PPID") {
        return LineRole::PsFullOutput;
    }
    if matches!(first, "USER" | "UID")
        && trimmed.contains("PID")
        && (trimmed.contains("COMMAND") || trimmed.contains("CMD"))
    {
        return LineRole::PsOutput;
    }
    // Data rows carry the same column layout as their header. Recognising them
    // by shape is the only option for a per-line classifier, and without it the
    // `ps` field parser never runs on real output.
    if looks_like_ps_aux_row(trimmed) {
        return LineRole::PsOutput;
    }
    if looks_like_ps_full_row(trimmed) {
        return LineRole::PsFullOutput;
    }
    // `git diff`: file headers and hunk headers.
    if trimmed.starts_with("diff --git ")
        || trimmed.starts_with("@@ ")
        || trimmed.starts_with("--- a/")
        || trimmed.starts_with("+++ b/")
        || (trimmed.starts_with("index ") && trimmed.contains(".."))
    {
        return LineRole::GitDiffOutput;
    }
    // `git log`: commit headers.
    if trimmed.starts_with("Author: ")
        || trimmed.starts_with("Date:   ")
        || trimmed.starts_with("Merge: ")
        || (trimmed.starts_with("commit ")
            && trimmed
                .split_whitespace()
                .nth(1)
                .is_some_and(|hash| hash.len() >= 7 && hash.bytes().all(|b| b.is_ascii_hexdigit())))
    {
        return LineRole::GitLogOutput;
    }

    // `ls -l`: 10-char mode string, e.g. `-rw-r--r--  1 root root ...`
    if looks_like_file_listing(trimmed) {
        return LineRole::FileListingOutput;
    }

    // `ip addr`: interface heading `2: eth0: <BROADCAST,...>` or an `inet` line.
    if looks_like_ip_heading(trimmed)
        || trimmed.starts_with("inet ")
        || trimmed.starts_with("inet6 ")
    {
        return LineRole::IpOutput;
    }

    // `git status`: branch line, change sections, and per-file status prefixes.
    if looks_like_git_status(line) {
        return LineRole::GitStatusOutput;
    }

    // `docker ps`: header row, or a row starting with a 12-char container id.
    if looks_like_docker_ps(trimmed) {
        return LineRole::ContainerOutput;
    }

    // `systemctl status`: `Active:`/`Loaded:` lines and the ● summary line.
    if looks_like_systemctl(trimmed) {
        return LineRole::SystemdOutput;
    }

    // `stat`: `  File: ...` / `  Size: ... Blocks: ...`
    if (trimmed.starts_with("File: ") && line.starts_with(' '))
        || (trimmed.starts_with("Size: ") && trimmed.contains("Blocks:"))
        || (trimmed.starts_with("Access: (") && trimmed.contains("Uid:"))
        || (line.starts_with(' ')
            && (trimmed.starts_with("Modify: ")
                || trimmed.starts_with("Change: ")
                || trimmed.starts_with("Birth: ")))
    {
        return LineRole::FileStatOutput;
    }
    // `getfacl`
    if trimmed.starts_with("# file: ")
        || trimmed.starts_with("user::")
        || trimmed.starts_with("group::")
        || trimmed.starts_with("other::")
        || trimmed.starts_with("mask::")
        || trimmed.starts_with("default:")
    {
        return LineRole::FileAclOutput;
    }
    // Test runner lines.
    if looks_like_test_output(trimmed) {
        return LineRole::TestOutput;
    }

    if looks_like_socket_table(trimmed) {
        return LineRole::SocketOutput;
    }

    // `free -h`: `Mem:` / `Swap:` rows and the `total used free shared` header.
    if trimmed.starts_with("Mem:")
        || trimmed.starts_with("Swap:")
        || (trimmed.starts_with("total") && trimmed.contains("free") && trimmed.contains("buff"))
    {
        return LineRole::MemoryUsageOutput;
    }
    if looks_like_disk_usage(trimmed) {
        return LineRole::DiskUsageOutput;
    }

    // `main.c:10:5: error: ...` / `main.c: In function 'main':`
    if looks_like_compiler_location(trimmed) {
        return LineRole::CCompilerOutput;
    }
    // Cargo progress: `   Compiling serde v1.0.0`
    if looks_like_cargo_progress(line, trimmed) {
        return LineRole::RustToolOutput;
    }
    if looks_like_build_diagnostic(trimmed) {
        return LineRole::BuildOutput;
    }
    // Bare SQL statement.
    if looks_like_sql_statement(trimmed) {
        return LineRole::SqlOutput;
    }

    if looks_like_log_line(trimmed) {
        return LineRole::LogOutput;
    }

    if looks_like_network_config(line) {
        return LineRole::NetworkConfigOutput;
    }

    LineRole::Unknown
}

fn looks_like_file_listing(line: &str) -> bool {
    let mut chars = line.chars();
    let Some(kind) = chars.next() else {
        return false;
    };
    if !matches!(kind, '-' | 'd' | 'l' | 'c' | 'b' | 's' | 'p') {
        return false;
    }
    let mode: String = chars.take(9).collect();
    let valid_mode = mode.len() == 9
        && mode
            .chars()
            .all(|c| matches!(c, 'r' | 'w' | 'x' | '-' | 's' | 'S' | 't' | 'T'))
        && mode
            .chars()
            .any(|c| c == 'r' || c == 'w' || c == 'x' || c == 's');
    if !valid_mode {
        return false;
    }
    // A real listing continues with a space and the link count. Without this
    // check any token starting with a mode-like prefix (`drwxr-xr-xsuffix`)
    // would be treated as `ls -l` output.
    let Some(rest) = line.get(10..) else {
        return false;
    };
    // RHEL/CentOS mark an SELinux label with `.` and an ACL with `+` directly
    // after the mode, so `-rw-r--r--. 1 root root ...` is the common shape there.
    let rest = rest.strip_prefix(['.', '+']).unwrap_or(rest);
    let trimmed = rest.trim_start();
    rest.len() > trimmed.len() && trimmed.bytes().take_while(u8::is_ascii_digit).count() > 0
}

/// `ps aux` data row: `USER PID %CPU %MEM VSZ RSS TTY STAT START TIME COMMAND`.
fn looks_like_ps_aux_row(line: &str) -> bool {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 11 {
        return false;
    }
    is_uint(fields[1])
        && is_decimal(fields[2])
        && is_decimal(fields[3])
        && is_uint(fields[4])
        && is_uint(fields[5])
        && is_tty(fields[6])
        && looks_like_ps_state(fields[7])
        && looks_like_clock(fields[9])
}

/// `ps -ef` data row: `UID PID PPID C STIME TTY TIME CMD`.
fn looks_like_ps_full_row(line: &str) -> bool {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 8 {
        return false;
    }
    is_uint(fields[1])
        && is_uint(fields[2])
        && is_uint(fields[3])
        && looks_like_start_time(fields[4])
        && is_tty(fields[5])
        && looks_like_clock(fields[6])
}

fn is_uint(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}

fn is_decimal(value: &str) -> bool {
    value.contains('.') && value.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

fn is_tty(value: &str) -> bool {
    value == "?" || value == "console" || value.starts_with("pts/") || value.starts_with("tty")
}

/// `Ss`, `R+`, `Sl`, `Z` — a state letter plus optional modifiers.
fn looks_like_ps_state(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(state) = chars.next() else {
        return false;
    };
    matches!(
        state,
        'R' | 'S' | 'D' | 'T' | 'Z' | 'I' | 'N' | 'W' | 'X' | 'K' | 'P'
    ) && value.len() <= 4
        && chars.all(|c| matches!(c, '<' | '>' | 'N' | 'L' | 's' | 'l' | '+'))
}

/// `0:01` or `00:00:01`.
fn looks_like_clock(value: &str) -> bool {
    let mut parts = value.split(':');
    let (Some(minutes), Some(seconds)) = (parts.next(), parts.next()) else {
        return false;
    };
    if !is_uint(minutes) || seconds.len() != 2 || !is_uint(seconds) {
        return false;
    }
    match parts.next() {
        None => true,
        Some(hours) => hours.len() == 2 && is_uint(hours) && parts.next().is_none(),
    }
}

/// `Oct30`, `Oct` (already split by whitespace), `10月29`, or a bare year.
fn looks_like_start_time(value: &str) -> bool {
    if value.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return true;
    }
    value.get(..3).is_some_and(is_month)
}

fn looks_like_ip_heading(line: &str) -> bool {
    // `2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 ...`
    let Some((index, rest)) = line.split_once(':') else {
        return false;
    };
    if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let Some((name, after)) = rest.trim_start().split_once(':') else {
        return false;
    };
    !name.is_empty() && after.trim_start().starts_with('<')
}

/// A parsed field: byte range plus the built-in rule id that colours it.
pub(super) struct RoleField {
    pub range: Range<usize>,
    /// Built-in rule id, resolved to a colour by `SemanticHighlighter::add`.
    pub rule: &'static str,
    pub priority: u16,
}

/// Field-level parse for a recognised role.
pub(super) fn fields(role: LineRole, line: &str) -> Vec<RoleField> {
    match role {
        LineRole::IpOutput => ip_fields(line),
        LineRole::FileListingOutput => file_listing_fields(line),
        LineRole::PsOutput => ps_fields(line),
        LineRole::PsFullOutput => ps_full_fields(line),
        LineRole::GitDiffOutput => git_diff_fields(line),
        LineRole::GitLogOutput => git_log_fields(line),
        LineRole::FileStatOutput => file_stat_fields(line),
        LineRole::FileAclOutput => file_acl_fields(line),
        LineRole::TestOutput => test_fields(line),
        LineRole::RustToolOutput => rust_tool_fields(line),
        LineRole::CCompilerOutput => c_compiler_fields(line),
        LineRole::SqlOutput => sql_fields(line),
        LineRole::GitStatusOutput => git_status_fields(line),
        LineRole::ContainerOutput => container_fields(line),
        LineRole::SystemdOutput => systemd_fields(line),
        LineRole::SocketOutput => socket_fields(line),
        LineRole::DiskUsageOutput => disk_usage_fields(line),
        LineRole::MemoryUsageOutput => disk_usage_fields(line),
        LineRole::BuildOutput => build_fields(line),
        LineRole::LogOutput => log_fields(line),
        LineRole::NetworkConfigOutput => network_config_fields(line),
        LineRole::Unknown => Vec::new(),
    }
}

// ── ip addr ──────────────────────────────────────────────────────────────

fn ip_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    let mut push = |start: usize, end: usize, rule: &'static str, priority: u16| {
        if start < end {
            out.push(RoleField {
                range: start..end,
                rule,
                priority,
            });
        }
    };

    let trimmed_start = line.len() - line.trim_start().len();
    let body = &line[trimmed_start..];

    if let Some((index, rest)) = body.split_once(':') {
        let index_end = trimmed_start + index.len();
        if !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()) {
            // interface index
            push(trimmed_start, index_end, "builtin-number", 96);
            let rest_start = trimmed_start + index.len() + 1;
            let name_trim = rest.len() - rest.trim_start().len();
            let name_start = rest_start + name_trim;
            if let Some(colon) = rest.trim_start().find(':') {
                let name_end = name_start + colon;
                // interface name
                push(name_start, name_end, "builtin-address", 96);
                // flag block `<UP,LOWER_UP>`
                let after = &line[name_end..];
                if let Some(open) = after.find('<')
                    && let Some(close) = after.find('>')
                {
                    let flags_start = name_end + open + 1;
                    let flags_end = name_end + close;
                    push(flags_start, flags_end, "builtin-constant", 94);
                }
                // `mtu 1500`, `state UP`
                for (needle, rule) in [
                    ("mtu ", "builtin-number"),
                    ("state ", "builtin-success"),
                    ("qdisc ", "builtin-constant"),
                ] {
                    if let Some(pos) = line[name_end..].find(needle) {
                        let value_start = name_end + pos + needle.len();
                        let value_end = value_start
                            + line[value_start..]
                                .bytes()
                                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                                .count();
                        push(value_start, value_end, rule, 95);
                    }
                }
            }
        }
    }

    // `inet 192.168.1.10/24 brd ...` / `inet6 fe80::1/64 scope link`
    for (needle, rule) in [("inet ", "builtin-info"), ("inet6 ", "builtin-info")] {
        if let Some(pos) = line.find(needle) {
            let start = pos + needle.len();
            let end = start
                + line[start..]
                    .bytes()
                    .take_while(|b| b.is_ascii_hexdigit() || matches!(b, b'.' | b':' | b'/'))
                    .count();
            push(start, end, rule, 96);
        }
    }
    for needle in ["brd ", "scope ", "valid_lft "] {
        if let Some(pos) = line.find(needle) {
            let start = pos + needle.len();
            let end = start
                + line[start..]
                    .bytes()
                    .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b':' | b'_'))
                    .count();
            push(start, end, "builtin-constant", 93);
        }
    }

    out
}

// ── ls -l ────────────────────────────────────────────────────────────────

fn file_listing_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    if line.len() < 10 {
        return out;
    }
    // Mode string: the type byte, then r/w/x/s/t each get their own colour so a
    // listing reads like `ls -l --color` (OxideTerm does the same).
    if line.len() >= 10 {
        out.push(RoleField {
            range: 0..1,
            rule: "builtin-comment",
            priority: 97,
        });
        for (offset, byte) in line.as_bytes()[1..10].iter().enumerate() {
            let rule = match byte {
                b'r' => "builtin-perm-read",
                b'w' => "builtin-perm-write",
                b'x' => "builtin-perm-execute",
                b's' | b'S' | b't' | b'T' => "builtin-perm-special",
                _ => continue,
            };
            let start = 1 + offset;
            out.push(RoleField {
                range: start..start + 1,
                rule,
                priority: 97,
            });
        }
    }
    // numeric columns: links, size, month, day, time
    let mut cursor = 10;
    // Skip the SELinux `.` / ACL `+` marker so the columns stay aligned.
    if matches!(line.as_bytes().get(cursor), Some(b'.' | b'+')) {
        cursor += 1;
    }
    let mut column = 0;
    while cursor < line.len() && column < 5 {
        while cursor < line.len() && line.as_bytes()[cursor] == b' ' {
            cursor += 1;
        }
        let start = cursor;
        while cursor < line.len() && line.as_bytes()[cursor] != b' ' {
            cursor += 1;
        }
        if start == cursor {
            break;
        }
        let token = &line[start..cursor];
        let rule = match column {
            0 => "builtin-number",   // links
            3 => "builtin-number",   // size
            4 => "builtin-datetime", // month/day/time
            _ => "",
        };
        if !rule.is_empty() {
            out.push(RoleField {
                range: start..cursor,
                rule,
                priority: 92,
            });
        }
        // month names get their own rule when present
        if column == 4 && is_month(token) {
            out.push(RoleField {
                range: start..cursor,
                rule: "builtin-month",
                priority: 93,
            });
        }
        column += 1;
    }
    out
}

fn is_month(token: &str) -> bool {
    matches!(
        token,
        "Jan"
            | "Feb"
            | "Mar"
            | "Apr"
            | "May"
            | "Jun"
            | "Jul"
            | "Aug"
            | "Sep"
            | "Oct"
            | "Nov"
            | "Dec"
    )
}

// ── ps aux ───────────────────────────────────────────────────────────────

fn ps_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    let mut cursor = 0;
    let mut column = 0;
    while cursor < line.len() && column < 11 {
        while cursor < line.len() && line.as_bytes()[cursor] == b' ' {
            cursor += 1;
        }
        let start = cursor;
        while cursor < line.len() && line.as_bytes()[cursor] != b' ' {
            cursor += 1;
        }
        if start == cursor {
            break;
        }
        let rule = match column {
            1 => "builtin-number",     // PID
            2 | 3 => "builtin-number", // %CPU %MEM
            // `USER PID %CPU %MEM VSZ RSS TTY STAT START TIME COMMAND`: TIME is
            // the last numeric column, COMMAND follows it and must stay text.
            9 => "builtin-number",
            _ => "",
        };
        if !rule.is_empty() {
            out.push(RoleField {
                range: start..cursor,
                rule,
                priority: 92,
            });
        }
        column += 1;
    }
    out
}

// ── git status ───────────────────────────────────────────────────────────

fn looks_like_git_status(line: &str) -> bool {
    line.starts_with("On branch ")
        || line.starts_with("Your branch is ")
        || line.starts_with("Changes to be committed:")
        || line.starts_with("Changes not staged for commit:")
        || line.starts_with("Untracked files:")
        || line.starts_with("nothing to commit")
        || line.starts_with("no changes added to commit")
        || (line.starts_with('\t')
            && (line.contains("modified:")
                || line.contains("new file:")
                || line.contains("deleted:")
                || line.contains("renamed:")))
}

fn git_status_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    let trimmed_start = line.len() - line.trim_start().len();

    // `On branch main` — branch name is a variable.
    if let Some(rest) = line.trim_start().strip_prefix("On branch ") {
        let start = line.len() - rest.len();
        out.push(RoleField {
            range: start..start + rest.len(),
            rule: "builtin-success",
            priority: 94,
        });
        return out;
    }

    // Section headers.
    for header in [
        "Changes to be committed:",
        "Changes not staged for commit:",
        "Untracked files:",
        "nothing to commit",
        "no changes added to commit",
    ] {
        if line.trim_start().starts_with(header) {
            let start = trimmed_start;
            out.push(RoleField {
                range: start..start + header.len(),
                rule: "builtin-warn",
                priority: 94,
            });
            return out;
        }
    }

    // `	modified:   src/main.rs` / `	new file:   a.txt` / `	deleted: ...`
    let body_start = trimmed_start;
    for (needle, rule) in [
        ("modified:", "builtin-error"),
        ("new file:", "builtin-success"),
        ("deleted:", "builtin-error"),
        ("renamed:", "builtin-warn"),
    ] {
        if let Some(pos) = line[body_start..].find(needle) {
            let start = body_start + pos;
            out.push(RoleField {
                range: start..start + needle.len(),
                rule,
                priority: 95,
            });
            // Path after the label.
            let after = start + needle.len();
            let path_start = line[after..].len() - line[after..].trim_start().len() + after;
            let path_end = line.len();
            if path_start < path_end {
                out.push(RoleField {
                    range: path_start..path_end,
                    rule: "builtin-path",
                    priority: 93,
                });
            }
            break;
        }
    }
    out
}

// ── docker ps ────────────────────────────────────────────────────────────

fn looks_like_docker_ps(line: &str) -> bool {
    if line.starts_with("CONTAINER ID") {
        return true;
    }
    // `docker compose ps` prints `NAME IMAGE COMMAND SERVICE CREATED STATUS PORTS`;
    // without this the header words fall through to the generic rules and
    // `CREATED` picks up the "created" success word.
    if line.starts_with("NAME") && line.contains("IMAGE") && line.contains("COMMAND") {
        return true;
    }
    // Rows begin with a 12-hex container id followed by whitespace.
    let mut chars = line.chars();
    let id: String = chars.by_ref().take(12).collect();
    id.len() == 12
        && id.chars().all(|c| c.is_ascii_hexdigit())
        && chars.next().is_some_and(|c| c == ' ')
}

fn container_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    if line.starts_with("CONTAINER ID")
        || (line.starts_with("NAME") && line.contains("IMAGE") && line.contains("COMMAND"))
    {
        return out;
    }
    // Container id: first token.
    let start = 0;
    let end = line.bytes().take_while(|b| *b != b' ').count();
    if end == 12 {
        out.push(RoleField {
            range: start..end,
            rule: "builtin-uuid",
            priority: 93,
        });
    }
    // Status words.
    for (needle, rule) in [
        ("Up ", "builtin-success"),
        ("Exited", "builtin-error"),
        ("Restarting", "builtin-warn"),
        ("Created", "builtin-constant"),
        ("Paused", "builtin-warn"),
        ("(healthy)", "builtin-success"),
        ("(unhealthy)", "builtin-error"),
    ] {
        if let Some(pos) = line.find(needle) {
            out.push(RoleField {
                range: pos..pos + needle.trim_end().len(),
                rule,
                priority: 95,
            });
        }
    }
    out
}

// ── systemctl status ─────────────────────────────────────────────────────

fn looks_like_systemctl(line: &str) -> bool {
    line.starts_with('●')
        || line.starts_with("Active:")
        || line.starts_with("Loaded:")
        || line.starts_with("Main PID:")
        || line.starts_with("Tasks:")
        || line.starts_with("Memory:")
}

fn systemd_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    for (needle, rule) in [
        ("active (running)", "builtin-success"),
        ("active (exited)", "builtin-success"),
        ("active (waiting)", "builtin-warn"),
        ("inactive (dead)", "builtin-debug"),
        ("failed", "builtin-error"),
        ("enabled", "builtin-success"),
        ("disabled", "builtin-debug"),
        ("loaded", "builtin-constant"),
        ("not-found", "builtin-error"),
    ] {
        if let Some(pos) = line.find(needle) {
            out.push(RoleField {
                range: pos..pos + needle.len(),
                rule,
                priority: 95,
            });
        }
    }
    // `Active: ... since ...` — timestamp after "since".
    if let Some(pos) = line.find(" since ") {
        let start = pos + " since ".len();
        let end = line.len();
        if start < end {
            out.push(RoleField {
                range: start..end,
                rule: "builtin-datetime",
                priority: 92,
            });
        }
    }
    out
}

// ── ss / netstat ─────────────────────────────────────────────────────────

fn looks_like_socket_table(line: &str) -> bool {
    // `ss` prints `Netid State Recv-Q ...`; older builds and `netstat` start
    // with `State`/`Proto`.
    if line.contains("Recv-Q") && (line.contains("State") || line.starts_with("Netid")) {
        return true;
    }
    if line.starts_with("Active Internet connections") || line.starts_with("Proto ") {
        return true;
    }
    let first = line.split_whitespace().next().unwrap_or("");
    const STATES: &[&str] = &[
        "LISTEN",
        "ESTAB",
        "ESTABLISHED",
        "TIME-WAIT",
        "CLOSE-WAIT",
        "SYN-SENT",
        "SYN-RECV",
        "FIN-WAIT-1",
        "FIN-WAIT-2",
        "LAST-ACK",
        "CLOSING",
        "UNCONN",
    ];
    if STATES.contains(&first) && line.split_whitespace().count() >= 4 {
        return true;
    }
    matches!(first, "tcp" | "tcp6" | "udp" | "udp6") && line.split_whitespace().count() >= 5
}

fn socket_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    let mut push = |start: usize, end: usize, rule: &'static str, priority: u16| {
        if start < end {
            out.push(RoleField {
                range: start..end,
                rule,
                priority,
            });
        }
    };

    if line.starts_with("State")
        || line.starts_with("Proto")
        || line.starts_with("Active ")
        || line.starts_with("Netid")
    {
        return out;
    }

    for (state, rule) in [
        ("LISTEN", "builtin-success"),
        ("ESTAB", "builtin-success"),
        ("UNCONN", "builtin-info"),
        ("TIME-WAIT", "builtin-warn"),
        ("CLOSE-WAIT", "builtin-warn"),
        ("SYN-SENT", "builtin-warn"),
        ("SYN-RECV", "builtin-warn"),
        ("FIN-WAIT", "builtin-warn"),
        ("LAST-ACK", "builtin-warn"),
        ("CLOSING", "builtin-warn"),
    ] {
        if let Some(pos) = line.find(state) {
            push(pos, pos + state.len(), rule, 96);
            break;
        }
    }

    // `0.0.0.0:22` / `[::]:22` / `*:22` — colour the port digits.
    let mut search = 0;
    while let Some(rel) = line[search..].find(':') {
        let colon = search + rel;
        let start = colon + 1;
        let end = start + line[start..].bytes().take_while(u8::is_ascii_digit).count();
        push(start, end, "builtin-number", 94);
        search = colon + 1;
    }

    out
}

// ── df / du / free ───────────────────────────────────────────────────────

fn looks_like_disk_usage(line: &str) -> bool {
    if (line.starts_with("Filesystem") || line.starts_with("文件系统"))
        && (line.contains("Size")
            || line.contains("Use%")
            || line.contains("已用")
            || line.contains("大小"))
    {
        return true;
    }
    if line.starts_with("Mem:") || line.starts_with("Swap:") {
        return true;
    }
    let first = line.split_whitespace().next().unwrap_or("");
    if first == "total" && line.contains("free") {
        return true;
    }
    (first.starts_with("/dev/")
        || first == "tmpfs"
        || first == "overlay"
        || first == "none"
        || first.starts_with('/'))
        && line.split_whitespace().any(|token| token.ends_with('%'))
}

fn disk_usage_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    let mut push = |start: usize, end: usize, rule: &'static str, priority: u16| {
        if start < end {
            out.push(RoleField {
                range: start..end,
                rule,
                priority,
            });
        }
    };

    let trimmed_start = line.len() - line.trim_start().len();
    let body = &line[trimmed_start..];
    let token_count = body.split_whitespace().count();
    // Header rows and bare `Mem:` labels carry no values to colour.
    if body.starts_with("Filesystem")
        || body.starts_with("total")
        || ((body.starts_with("Mem:") || body.starts_with("Swap:")) && token_count <= 1)
    {
        return out;
    }
    if body.starts_with("Mem:") || body.starts_with("Swap:") {
        push(trimmed_start, trimmed_start + 4, "builtin-constant", 95);
    }

    let mut offset = trimmed_start;
    let last_token_start = body
        .split_whitespace()
        .last()
        .map(|token| line.rfind(token).unwrap_or(offset))
        .unwrap_or(offset);

    for token in body.split_whitespace() {
        let start = line[offset..]
            .find(token)
            .map(|idx| offset + idx)
            .unwrap_or(offset);
        let end = start + token.len();
        offset = end;

        if token.ends_with('%') {
            let used: u32 = token.trim_end_matches('%').parse().unwrap_or(0);
            let rule = if used >= 90 {
                "builtin-error"
            } else if used >= 80 {
                "builtin-warn"
            } else {
                "builtin-number"
            };
            push(start, end, rule, 96);
        } else if is_size_token(token) {
            push(start, end, "builtin-size", 94);
        } else if start == trimmed_start || start == last_token_start {
            // Device at the left, mount point at the right.
            push(start, end, "builtin-path", 94);
        }
    }

    out
}

fn is_size_token(token: &str) -> bool {
    let split = token
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(token.len());
    if split == 0 || split == token.len() {
        return false;
    }
    matches!(
        &token[split..],
        "B" | "K"
            | "M"
            | "G"
            | "T"
            | "P"
            | "Ki"
            | "Mi"
            | "Gi"
            | "Ti"
            | "Pi"
            | "KB"
            | "MB"
            | "GB"
            | "TB"
    )
}

// ── build diagnostics ────────────────────────────────────────────────────

fn looks_like_build_diagnostic(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("error[")
        || trimmed.starts_with("error:")
        || trimmed.starts_with("warning:")
        || trimmed.starts_with("note:")
        || trimmed.starts_with("help:")
        || trimmed.starts_with("--> ")
        || trimmed.starts_with("make: ***")
        || trimmed.starts_with("make[")
        || trimmed.contains(": error: ")
        || trimmed.contains(": warning: ")
        || trimmed.contains(": note: ")
}

fn build_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    let mut push = |start: usize, end: usize, rule: &'static str, priority: u16| {
        if start < end {
            out.push(RoleField {
                range: start..end,
                rule,
                priority,
            });
        }
    };

    let trimmed_start = line.len() - line.trim_start().len();
    let body = &line[trimmed_start..];

    // `error[E0308]:` / `error:` / `warning:` / `note:` / `help:`
    for (needle, rule) in [
        ("error", "builtin-error"),
        ("warning", "builtin-warn"),
        ("note", "builtin-info"),
        ("help", "builtin-constant"),
    ] {
        if let Some(pos) = body.find(needle) {
            let after = &body[pos + needle.len()..];
            if after.starts_with('[') || after.starts_with(':') {
                push(
                    trimmed_start + pos,
                    trimmed_start + pos + needle.len(),
                    rule,
                    96,
                );
                break;
            }
        }
    }

    // `error[E0308]` — colour the code itself.
    if let Some(pos) = body.find("error[") {
        let code_start = trimmed_start + pos + "error[".len();
        let code_end = line[code_start..]
            .find(']')
            .map(|idx| code_start + idx)
            .unwrap_or(code_start);
        push(code_start, code_end, "builtin-constant", 95);
    }

    // `--> src/main.rs:10:5` — path, then line/column.
    if let Some(pos) = line.find("--> ") {
        let start = pos + 4;
        let end = start + line[start..].trim_end().len();
        let location = &line[start..end];
        // Two trailing `:number` groups belong to the location, not the path.
        let mut path_end = end;
        for _ in 0..2 {
            let Some(colon) = line[start..path_end].rfind(':') else {
                break;
            };
            let digits = &line[start + colon + 1..path_end];
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                path_end = start + colon;
            } else {
                break;
            }
        }
        push(start, path_end, "builtin-path", 95);
        push(path_end, end, "builtin-number", 94);
        let _ = location;
    }

    out
}

// ── nginx / journalctl / syslog ──────────────────────────────────────────

fn looks_like_log_line(line: &str) -> bool {
    // nginx error log: `2026/10/30 21:00:00 [error] 1234#0: *5 ...`
    let bytes = line.as_bytes();
    if bytes.len() > 20
        && bytes[4] == b'/'
        && bytes[7] == b'/'
        && bytes[10] == b' '
        && bytes[13] == b':'
    {
        return true;
    }
    // nginx access log: `1.2.3.4 - - [30/Oct/2026:21:00:00 +0800] "GET / HTTP/1.1" 200`
    if line.contains(" - - [") && line.contains('"') {
        return true;
    }
    // Application logs: `2026-10-30 13:15:13.420 ... DEBUG path:line message`
    if looks_like_app_log(line) {
        return true;
    }
    // Gin access log: `[GIN] 2026/10/30 - 23:14:08 | 200 | 127.0.0.1 | GET "/health"`
    if line.starts_with("[GIN] ") {
        return true;
    }
    // syslog / journalctl: `Oct 30 21:00:00 host proc[1]: msg`
    let mut parts = line.split_whitespace();
    let Some(month) = parts.next() else {
        return false;
    };
    const MONTHS: &[&str] = &[
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    if !MONTHS.contains(&month) {
        return false;
    }
    let Some(day) = parts.next() else {
        return false;
    };
    if day.is_empty() || !day.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    parts.next().is_some_and(|time| time.contains(':'))
}

/// `YYYY-MM-DD HH:MM:SS` prefix plus a level token.
fn looks_like_app_log(line: &str) -> bool {
    let bytes = line.as_bytes();
    if bytes.len() < 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b' '
        || bytes[13] != b':'
    {
        return false;
    }
    const LEVELS: &[&str] = &[
        "DEBUG", "INFO", "WARN", "WARNING", "ERROR", "TRACE", "FATAL", "CRIT",
    ];
    LEVELS.iter().any(|level| line.contains(level))
}

fn log_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    let mut push = |start: usize, end: usize, rule: &'static str, priority: u16| {
        if start < end {
            out.push(RoleField {
                range: start..end,
                rule,
                priority,
            });
        }
    };

    // nginx severity tag.
    for (needle, rule) in [
        ("[emerg]", "builtin-error"),
        ("[alert]", "builtin-error"),
        ("[crit]", "builtin-error"),
        ("[error]", "builtin-error"),
        ("[warn]", "builtin-warn"),
        ("[notice]", "builtin-info"),
        ("[info]", "builtin-info"),
        ("[debug]", "builtin-debug"),
    ] {
        if let Some(pos) = line.find(needle) {
            push(pos, pos + needle.len(), rule, 96);
            break;
        }
    }

    // HTTP status right after the quoted request.
    if let Some(pos) = line.find("] \"")
        && let Some(close) = line[pos + 3..].find('"')
    {
        let after = pos + 3 + close + 1;
        let rest = line[after..].trim_start();
        let code_start = after + (line[after..].len() - rest.len());
        let code_len = rest.bytes().take_while(u8::is_ascii_digit).count();
        if code_len == 3 {
            let code: u32 = line[code_start..code_start + 3].parse().unwrap_or(0);
            let rule = match code {
                200..=299 => "builtin-success",
                300..=399 => "builtin-info",
                400..=499 => "builtin-warn",
                _ => "builtin-error",
            };
            push(code_start, code_start + 3, rule, 96);
        }
    }

    // Leading timestamp, either `YYYY/MM/DD` (nginx) or `YYYY-MM-DD` (apps).
    let trimmed_start = line.len() - line.trim_start().len();
    if line.len() >= trimmed_start + 19 {
        let head = &line[trimmed_start..trimmed_start + 19];
        let bytes = head.as_bytes();
        if (bytes[4] == b'/' && bytes[7] == b'/') || (bytes[4] == b'-' && bytes[7] == b'-') {
            push(trimmed_start, trimmed_start + 19, "builtin-datetime", 95);
        }
    }

    // Application log level: one colour per severity.
    for (level, rule) in [
        ("DEBUG", "builtin-debug"),
        ("TRACE", "builtin-debug"),
        ("INFO", "builtin-info"),
        ("NOTICE", "builtin-info"),
        ("WARNING", "builtin-warn"),
        ("WARN", "builtin-warn"),
        ("ERROR", "builtin-error"),
        ("FATAL", "builtin-error"),
        ("CRIT", "builtin-error"),
    ] {
        if let Some(pos) = find_level_word(line, level) {
            push(pos, pos + level.len(), rule, 96);
            break;
        }
    }

    // `.../checker.go:332` — the source location after the level.
    for extension in [
        ".go:", ".rs:", ".py:", ".js:", ".ts:", ".java:", ".c:", ".cpp:",
    ] {
        if let Some(pos) = line.find(extension) {
            let start = line[..pos]
                .rfind(char::is_whitespace)
                .map(|idx| idx + 1)
                .unwrap_or(0);
            let after = pos + extension.len();
            let digits = line[after..].bytes().take_while(u8::is_ascii_digit).count();
            push(start, after + digits, "builtin-path", 95);
            break;
        }
    }

    // Gin: `[GIN] 2026/10/30 - 23:14:08 | 200 | 127.0.0.1 | GET "/health"`
    if line.starts_with("[GIN] ") {
        push(0, 5, "builtin-constant", 95);
        let mut search = 0;
        while let Some(rel) = line[search..].find("| ") {
            let after = search + rel + 2;
            let digits = line[after..].bytes().take_while(u8::is_ascii_digit).count();
            if digits == 3 {
                let code: u32 = line[after..after + 3].parse().unwrap_or(0);
                let rule = match code {
                    200..=299 => "builtin-success",
                    300..=399 => "builtin-info",
                    400..=499 => "builtin-warn",
                    _ => "builtin-error",
                };
                push(after, after + 3, rule, 96);
            }
            search = after + digits.max(1);
        }
        for method in ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"] {
            if let Some(pos) = find_level_word(line, method) {
                push(pos, pos + method.len(), "builtin-command", 95);
                break;
            }
        }
    }

    out
}

/// Whole-word search so `INFO` does not match inside `INFORMATION`.
fn find_level_word(line: &str, word: &str) -> Option<usize> {
    let mut search = 0;
    while let Some(rel) = line[search..].find(word) {
        let pos = search + rel;
        let before_ok = pos == 0 || !line.as_bytes()[pos - 1].is_ascii_alphanumeric();
        let end = pos + word.len();
        let after_ok = end >= line.len() || !line.as_bytes()[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return Some(pos);
        }
        search = pos + word.len();
    }
    None
}

// ── network device configuration ─────────────────────────────────────────

/// H3C / Huawei / Cisco / Ruijie configuration blocks.
///
/// Only strong shapes are accepted: an `interface` name that carries a digit or
/// a slash, `vlan <number>`, `ip address <a.b.c.d>`, and the fixed keywords a
/// configuration block uses. Plain prose that happens to start with
/// `description` is ignored unless it is indented like a block body.
fn looks_like_network_config(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Block separator.
    if trimmed == "#" {
        return true;
    }
    if let Some(rest) = trimmed.strip_prefix("interface ") {
        let name = rest.trim();
        return !name.is_empty()
            && (name.bytes().any(|b| b.is_ascii_digit()) || name.contains('/'));
    }
    if let Some(rest) = trimmed.strip_prefix("vlan ") {
        let value = rest.trim();
        return !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit());
    }
    if let Some(rest) = trimmed.strip_prefix("rule ") {
        return rest
            .trim()
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_digit());
    }
    if let Some(rest) = trimmed.strip_prefix("acl number ") {
        return rest
            .trim()
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_digit());
    }
    for prefix in [
        "port link-type ",
        "port trunk ",
        "port default vlan ",
        "port access ",
        "port hybrid ",
        "switchport ",
        "ip address ",
        "ip pool ",
        "ip route-static ",
        "dhcp select ",
        "dhcp server ",
        "gateway-list ",
        "dns-list ",
        "sysname ",
        "authentication-mode ",
        "authorization-attribute ",
        "accounting-scheme ",
        "excluded-ip-address ",
        "forbidden-ip ",
        "option ",
        "authorization-mode ",
        "accounting-mode ",
        "password-history ",
        "password-control ",
        "undo ",
        "domain ",
        "radius-server ",
        "local-user ",
        "ntp-service ",
        "info-center ",
        "snmp-agent ",
        "stp ",
        "lldp ",
    ] {
        if trimmed.starts_with(prefix) {
            return true;
        }
    }
    // Bare block openers.
    if matches!(trimmed, "aaa" | "quit" | "return") {
        return true;
    }
    if trimmed.starts_with("network ") && trimmed.contains("mask ") {
        return true;
    }
    // ` description web-gateway` inside an interface block.
    line.starts_with(' ') && trimmed.starts_with("description ")
}

fn network_config_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    let mut push = |start: usize, end: usize, rule: &'static str, priority: u16| {
        if start < end {
            out.push(RoleField {
                range: start..end,
                rule,
                priority,
            });
        }
    };

    let trimmed_start = line.len() - line.trim_start().len();
    let body = &line[trimmed_start..];

    // Block separators recede: they delimit sections, they are not content.
    if body == "#" {
        push(
            trimmed_start,
            trimmed_start + 1,
            "builtin-net-secondary",
            90,
        );
        return out;
    }

    // Leading command word.
    let keyword_len = body
        .bytes()
        .take_while(|byte| byte.is_ascii_alphabetic() || *byte == b'-')
        .count();
    push(
        trimmed_start,
        trimmed_start + keyword_len,
        "builtin-net-keyword",
        93,
    );

    // `interface GigabitEthernet1/0/22` — the port name is the interesting part.
    if let Some(rest) = body.strip_prefix("interface ") {
        let start = trimmed_start + "interface ".len();
        let name = rest.trim_end();
        // OxideTerm keeps the port name in the command colour.
        push(start, start + name.len(), "builtin-net-port", 95);
    }

    // VLAN ids, including the list in `port trunk allow-pass vlan 3320 3330`.
    let mut search = 0;
    while let Some(rel) = body[search..].find("vlan ") {
        let mut cursor = search + rel + "vlan ".len();
        loop {
            let leading = body[cursor..].len() - body[cursor..].trim_start().len();
            cursor += leading;
            let digits = body[cursor..]
                .bytes()
                .take_while(u8::is_ascii_digit)
                .count();
            if digits == 0 {
                break;
            }
            push(
                trimmed_start + cursor,
                trimmed_start + cursor + digits,
                // OxideTerm paints VLAN ids green; they read as "this is the
                // identifier being acted on", the same role UP plays for links.
                "builtin-net-vlan",
                94,
            );
            cursor += digits;
        }
        search = cursor.max(search + rel + "vlan ".len());
    }

    // Addresses; a `255.255.255.0` style mask reads as secondary information.
    let mut offset = trimmed_start;
    for token in body.split_whitespace() {
        let start = line[offset..]
            .find(token)
            .map(|idx| offset + idx)
            .unwrap_or(offset);
        let end = start + token.len();
        offset = end;
        // An address is an address; a `255.255.255.0` mask is secondary detail.
        if is_ipv4(token) {
            let rule = if token.starts_with("255.") {
                "builtin-net-secondary"
            } else {
                "builtin-net-address"
            };
            push(start, end, rule, 94);
        }
    }

    // `port link-type trunk` / `dhcp select global`.
    for needle in ["link-type ", "select "] {
        if let Some(pos) = body.find(needle) {
            let start = pos + needle.len();
            let word = body[start..]
                .bytes()
                .take_while(u8::is_ascii_alphabetic)
                .count();
            push(
                trimmed_start + start,
                trimmed_start + start + word,
                "builtin-net-keyword",
                93,
            );
        }
    }

    // `local-user admin password irreversible-cipher $1c$...` — the hash is
    // noise, so it recedes instead of competing with the command words.
    if body.starts_with("local-user ") {
        let mut offset = trimmed_start;
        for token in body.split_whitespace() {
            let start = line[offset..]
                .find(token)
                .map(|idx| offset + idx)
                .unwrap_or(offset);
            let end = start + token.len();
            offset = end;
            if token.len() > 20 || token.starts_with('$') {
                push(start, end, "builtin-debug", 91);
            }
        }
    }

    // ` description web-gateway` — the text after the keyword.
    if let Some(rest) = body.strip_prefix("description ") {
        let start = trimmed_start + "description ".len();
        let text = rest.trim_end();
        // The description body stays in the command colour, matching OxideTerm.
        push(start, start + text.len(), "builtin-net-secondary", 92);
    }

    out
}

fn is_ipv4(token: &str) -> bool {
    let mut parts = 0;
    for part in token.split('.') {
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        if part.parse::<u16>().map(|value| value > 255).unwrap_or(true) {
            return false;
        }
        parts += 1;
    }
    parts == 4
}

// ── ps -ef ───────────────────────────────────────────────────────────────

fn ps_full_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    let mut push = |start: usize, end: usize, rule: &'static str, priority: u16| {
        if start < end {
            out.push(RoleField {
                range: start..end,
                rule,
                priority,
            });
        }
    };
    // UID PID PPID C STIME TTY TIME CMD
    let mut cursor = 0;
    let mut column = 0;
    while cursor < line.len() && column < 7 {
        while cursor < line.len() && line.as_bytes()[cursor] == b' ' {
            cursor += 1;
        }
        let start = cursor;
        while cursor < line.len() && line.as_bytes()[cursor] != b' ' {
            cursor += 1;
        }
        if start == cursor {
            break;
        }
        let rule = match column {
            1 | 2 => "builtin-number", // PID, PPID
            4 => "builtin-datetime",   // STIME
            6 => "builtin-duration",   // TIME
            _ => "",
        };
        if !rule.is_empty() {
            push(start, cursor, rule, 92);
        }
        column += 1;
    }
    if column >= 7 {
        let rest = &line[cursor..];
        let start = cursor + (rest.len() - rest.trim_start().len());
        push(start, line.len(), "builtin-command", 90);
    }
    out
}

// ── git diff ─────────────────────────────────────────────────────────────

fn git_diff_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    let mut push = |start: usize, end: usize, rule: &'static str, priority: u16| {
        if start < end {
            out.push(RoleField {
                range: start..end,
                rule,
                priority,
            });
        }
    };
    let trimmed_start = line.len() - line.trim_start().len();
    let body = &line[trimmed_start..];

    if body.starts_with("diff --git ") {
        // Both sides: `a/path b/path`.
        let mut search = trimmed_start + "diff --git ".len();
        for _ in 0..2 {
            let rest = &line[search..];
            let start = search + (rest.len() - rest.trim_start().len());
            let len = line[start..].bytes().take_while(|b| *b != b' ').count();
            push(start, start + len, "builtin-path", 95);
            search = start + len;
        }
    } else if body.starts_with("@@ ") {
        // `@@ -1,7 +1,7 @@ context`
        let end = body[2..].find("@@").map(|idx| trimmed_start + 2 + idx + 2);
        push(
            trimmed_start,
            end.unwrap_or(trimmed_start + body.len()),
            "builtin-number",
            93,
        );
    } else if body.starts_with("--- ") || body.starts_with("+++ ") {
        let start = trimmed_start + 4;
        let len = line[start..]
            .bytes()
            .take_while(|b| *b != b' ' && *b != b'\t')
            .count();
        push(start, start + len, "builtin-path", 95);
    } else if body.starts_with("index ") {
        let start = trimmed_start + "index ".len();
        let len = line[start..].bytes().take_while(|b| *b != b' ').count();
        push(start, start + len, "builtin-uuid", 92);
    }
    out
}

// ── git log ──────────────────────────────────────────────────────────────

fn git_log_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    let mut push = |start: usize, end: usize, rule: &'static str, priority: u16| {
        if start < end {
            out.push(RoleField {
                range: start..end,
                rule,
                priority,
            });
        }
    };
    let trimmed_start = line.len() - line.trim_start().len();
    let body = &line[trimmed_start..];

    if let Some(rest) = body.strip_prefix("commit ") {
        let start = trimmed_start + "commit ".len();
        let len = rest.bytes().take_while(u8::is_ascii_hexdigit).count();
        push(start, start + len, "builtin-uuid", 95);
    } else if let Some(rest) = body.strip_prefix("Author: ") {
        let start = trimmed_start + "Author: ".len();
        push(start, start + rest.len(), "builtin-variable", 94);
    } else if let Some(rest) = body.strip_prefix("Date:   ") {
        let start = trimmed_start + "Date:   ".len();
        push(start, start + rest.len(), "builtin-datetime", 94);
    } else if let Some(rest) = body.strip_prefix("Merge: ") {
        let start = trimmed_start + "Merge: ".len();
        push(start, start + rest.len(), "builtin-uuid", 93);
    }
    out
}

// ── stat ─────────────────────────────────────────────────────────────────

fn file_stat_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    let mut push = |start: usize, end: usize, rule: &'static str, priority: u16| {
        if start < end {
            out.push(RoleField {
                range: start..end,
                rule,
                priority,
            });
        }
    };
    let trimmed_start = line.len() - line.trim_start().len();
    let body = &line[trimmed_start..];

    if let Some(rest) = body.strip_prefix("File: ") {
        let start = trimmed_start + "File: ".len();
        push(start, start + rest.trim_end().len(), "builtin-path", 95);
    }
    if let Some(rest) = body.strip_prefix("Size: ") {
        let start = trimmed_start + "Size: ".len();
        let len = rest.bytes().take_while(u8::is_ascii_digit).count();
        push(start, start + len, "builtin-number", 94);
    }
    // `Access: (0644/-rw-r--r--)` — one colour per permission character.
    if let Some(pos) = body.find("/-") {
        let start = trimmed_start + pos + 1;
        let len = body[pos + 1..].bytes().take_while(|b| *b != b')').count();
        // `stat` prints the type byte too (`-rw-r--r--`), so take the last nine.
        if len >= 9 {
            let perm_start = start + (len - 9);
            for (offset, byte) in line.as_bytes()[perm_start..start + len].iter().enumerate() {
                let rule = match byte {
                    b'r' => "builtin-perm-read",
                    b'w' => "builtin-perm-write",
                    b'x' => "builtin-perm-execute",
                    b's' | b'S' | b't' | b'T' => "builtin-perm-special",
                    _ => continue,
                };
                let at = perm_start + offset;
                push(at, at + 1, rule, 95);
            }
        }
    }
    for prefix in ["Modify: ", "Change: ", "Birth: "] {
        if let Some(rest) = body.strip_prefix(prefix) {
            let start = trimmed_start + prefix.len();
            push(start, start + rest.trim_end().len(), "builtin-datetime", 94);
        }
    }
    out
}

// ── getfacl ──────────────────────────────────────────────────────────────

fn file_acl_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    let mut push = |start: usize, end: usize, rule: &'static str, priority: u16| {
        if start < end {
            out.push(RoleField {
                range: start..end,
                rule,
                priority,
            });
        }
    };
    let trimmed_start = line.len() - line.trim_start().len();
    let body = &line[trimmed_start..];

    if let Some(rest) = body.strip_prefix("# file: ") {
        let start = trimmed_start + "# file: ".len();
        push(start, start + rest.trim_end().len(), "builtin-path", 95);
        return out;
    }
    if body.starts_with('#') {
        return out;
    }
    // `user::rw-` / `mask::rwx` — colour the permission characters only.
    if let Some(colon) = body.find("::") {
        let start = trimmed_start + colon + 2;
        let len = body[colon + 2..].bytes().take_while(|b| *b != b' ').count();
        for (offset, byte) in line.as_bytes()[start..start + len].iter().enumerate() {
            let rule = match byte {
                b'r' => "builtin-perm-read",
                b'w' => "builtin-perm-write",
                b'x' => "builtin-perm-execute",
                b's' | b'S' | b't' | b'T' => "builtin-perm-special",
                _ => continue,
            };
            let at = start + offset;
            push(at, at + 1, rule, 95);
        }
    }
    out
}

// ── test runners ─────────────────────────────────────────────────────────

fn looks_like_test_output(line: &str) -> bool {
    if line.starts_with("running ") && line.contains(" test") {
        return true;
    }
    if line.starts_with("test result: ") {
        return true;
    }
    if line.starts_with("test ")
        && (line.ends_with("... ok")
            || line.ends_with("... FAILED")
            || line.ends_with("... ignored"))
    {
        return true;
    }
    line.contains(" passed") && (line.contains(" failed") || line.contains(" passed in"))
}

fn test_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    let mut push = |start: usize, end: usize, rule: &'static str, priority: u16| {
        if start < end {
            out.push(RoleField {
                range: start..end,
                rule,
                priority,
            });
        }
    };

    // Verdict word at the end of a case line.
    for (needle, rule) in [
        ("FAILED", "builtin-error"),
        ("ignored", "builtin-warn"),
        ("ok", "builtin-success"),
    ] {
        if let Some(pos) = line.rfind(needle) {
            let before_ok = pos == 0 || line.as_bytes()[pos - 1] == b' ';
            let end = pos + needle.len();
            let after_ok = end == line.len() || !line.as_bytes()[end].is_ascii_alphanumeric();
            if before_ok && after_ok {
                push(pos, end, rule, 96);
                break;
            }
        }
    }

    // Counts: `running 5 tests`, `3 passed; 0 failed`.
    let mut cursor = 0;
    while cursor < line.len() {
        let skip = line[cursor..]
            .bytes()
            .take_while(|b| !b.is_ascii_digit())
            .count();
        let start = cursor + skip;
        if start >= line.len() {
            break;
        }
        let len = line[start..].bytes().take_while(u8::is_ascii_digit).count();
        push(start, start + len, "builtin-number", 90);
        cursor = start + len;
    }
    out
}

// ── cargo progress ───────────────────────────────────────────────────────

fn looks_like_cargo_progress(line: &str, trimmed: &str) -> bool {
    // Cargo right-aligns these verbs in a 12-column field.
    if !line.starts_with("   ") {
        return false;
    }
    const VERBS: &[&str] = &[
        "Compiling",
        "Finished",
        "Running",
        "Downloading",
        "Downloaded",
        "Updating",
        "Building",
        "Checking",
        "Documenting",
        "Fresh",
        "Blocking",
    ];
    VERBS.iter().any(|verb| trimmed.starts_with(verb)) && trimmed.contains(' ')
}

fn rust_tool_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    let trimmed_start = line.len() - line.trim_start().len();
    let body = &line[trimmed_start..];

    for (verb, rule) in [
        ("Finished", "builtin-success"),
        ("Downloaded", "builtin-success"),
        ("Fresh", "builtin-debug"),
        ("Compiling", "builtin-info"),
        ("Checking", "builtin-info"),
        ("Building", "builtin-info"),
        ("Running", "builtin-info"),
        ("Downloading", "builtin-info"),
        ("Updating", "builtin-info"),
        ("Documenting", "builtin-info"),
        ("Blocking", "builtin-info"),
    ] {
        if body.starts_with(verb) {
            out.push(RoleField {
                range: trimmed_start..trimmed_start + verb.len(),
                rule,
                priority: 95,
            });
            break;
        }
    }
    out
}

// ── C / C++ compiler ─────────────────────────────────────────────────────

/// `main.c:10:5: error: ...` or `main.c: In function 'main':`
fn looks_like_compiler_location(line: &str) -> bool {
    let Some(first_colon) = line.find(':') else {
        return false;
    };
    let path = &line[..first_colon];
    // A compiler location starts with the file name: no spaces, and an
    // extension. This keeps Rust's `--> src/main.rs:10:5` out of this branch.
    if path.is_empty() || !path.contains('.') || path.contains(' ') {
        return false;
    }
    let rest = &line[first_colon + 1..];
    if rest.starts_with(" In function ") {
        return true;
    }
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0 && rest[digits..].starts_with(':')
}

fn c_compiler_fields(line: &str) -> Vec<RoleField> {
    let mut out = Vec::new();
    let mut push = |start: usize, end: usize, rule: &'static str, priority: u16| {
        if start < end {
            out.push(RoleField {
                range: start..end,
                rule,
                priority,
            });
        }
    };
    let trimmed_start = line.len() - line.trim_start().len();
    let body = &line[trimmed_start..];

    // `path:line:col:` — path first, then the location numbers.
    if let Some(colon) = body.find(':') {
        let path_end = trimmed_start + colon;
        push(trimmed_start, path_end, "builtin-path", 95);
        let rest = &body[colon + 1..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            let mut end = trimmed_start + colon + 1 + digits;
            let after = &rest[digits..];
            if let Some(stripped) = after.strip_prefix(':') {
                let more = stripped.bytes().take_while(u8::is_ascii_digit).count();
                if more > 0 {
                    end += 1 + more;
                }
            }
            push(trimmed_start + colon + 1, end, "builtin-number", 94);
        }
    }

    for (needle, rule) in [
        ("error", "builtin-error"),
        ("warning", "builtin-warn"),
        ("note", "builtin-info"),
    ] {
        if let Some(pos) = body.find(&format!("{needle}: ")) {
            let start = trimmed_start + pos;
            push(start, start + needle.len(), rule, 96);
            break;
        }
    }
    out
}

// ── SQL ──────────────────────────────────────────────────────────────────

fn looks_like_sql_statement(line: &str) -> bool {
    let upper = line.to_ascii_uppercase();
    const STARTERS: &[&str] = &[
        "SELECT ", "INSERT ", "UPDATE ", "DELETE ", "CREATE ", "ALTER ", "DROP ", "WITH ",
    ];
    STARTERS.iter().any(|starter| upper.starts_with(starter))
}

fn sql_fields(line: &str) -> Vec<RoleField> {
    const KEYWORDS: &[&str] = &[
        "SELECT",
        "FROM",
        "WHERE",
        "INSERT",
        "INTO",
        "VALUES",
        "UPDATE",
        "SET",
        "DELETE",
        "JOIN",
        "LEFT",
        "RIGHT",
        "INNER",
        "OUTER",
        "ON",
        "GROUP",
        "BY",
        "ORDER",
        "HAVING",
        "LIMIT",
        "OFFSET",
        "AS",
        "AND",
        "OR",
        "NOT",
        "NULL",
        "IS",
        "IN",
        "LIKE",
        "BETWEEN",
        "CREATE",
        "ALTER",
        "DROP",
        "TABLE",
        "INDEX",
        "PRIMARY",
        "KEY",
        "FOREIGN",
        "REFERENCES",
        "DEFAULT",
        "UNIQUE",
        "DISTINCT",
        "CASE",
        "WHEN",
        "THEN",
        "ELSE",
        "END",
        "COUNT",
        "SUM",
        "AVG",
        "MIN",
        "MAX",
    ];
    let mut out = Vec::new();
    let mut offset = 0;
    for token in line.split_whitespace() {
        let start = line[offset..]
            .find(token)
            .map(|idx| offset + idx)
            .unwrap_or(offset);
        let end = start + token.len();
        offset = end;

        let cleaned = token.trim_matches(|c: char| matches!(c, '(' | ')' | ',' | ';' | '*'));
        if cleaned.is_empty() {
            continue;
        }
        if KEYWORDS.contains(&cleaned.to_ascii_uppercase().as_str()) {
            let inner = token.find(cleaned).unwrap_or(0);
            let at = start + inner;
            out.push(RoleField {
                range: at..at + cleaned.len(),
                rule: "builtin-keyword",
                priority: 95,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        LineRole, build_fields, c_compiler_fields, container_fields, detect, disk_usage_fields,
        file_listing_fields, file_stat_fields, git_log_fields, git_status_fields, ip_fields,
        log_fields, network_config_fields, ps_full_fields, socket_fields, sql_fields,
        systemd_fields, test_fields,
    };

    #[test]
    fn detects_ip_addr_lines() {
        assert_eq!(
            detect("2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500"),
            LineRole::IpOutput
        );
        assert_eq!(
            detect("    inet 192.168.1.10/24 brd 192.168.1.255 scope global eth0"),
            LineRole::IpOutput
        );
        assert_eq!(
            detect("    inet6 fe80::1/64 scope link"),
            LineRole::IpOutput
        );
    }

    #[test]
    fn detects_ls_long_listing() {
        assert_eq!(
            detect("-rw-r--r--  1 root root  1234 Oct 30 10:00 file.txt"),
            LineRole::FileListingOutput
        );
        assert_eq!(
            detect("drwxr-xr-x  3 user group  4096 Oct 29 09:00 dir"),
            LineRole::FileListingOutput
        );
        assert_eq!(
            detect("lrwxrwxrwx  1 root root     7 Oct 28 08:00 link -> /usr/bin/x"),
            LineRole::FileListingOutput
        );
    }

    #[test]
    fn detects_ls_long_listing_with_security_markers() {
        // RHEL/CentOS print `.` for an SELinux label and `+` for an ACL right
        // after the mode string. Without accepting those the whole listing falls
        // back to plain output on those systems.
        for line in [
            "-rw-r--r--. 1 root root  1518 Sep  8 10:33 anaconda-ks.cfg",
            "drwxr-xr-x. 2 root root  4096 Oct 30 10:00 downloads",
            "-rw-rw-r--+ 1 root root  1518 Sep  8 10:33 shared.txt",
        ] {
            assert_eq!(detect(line), LineRole::FileListingOutput, "{line}");
        }
    }

    #[test]
    fn ls_columns_stay_aligned_after_the_security_marker() {
        // The marker must not shift the column count: `.` is not the link count.
        let line = "-rw-r--r--. 1 root root  1518 Sep  8 10:33 anaconda-ks.cfg";
        let fields = file_listing_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        assert!(texts.contains(&"1"), "{texts:?}");
        assert!(texts.contains(&"1518"), "{texts:?}");
        assert!(texts.contains(&"Sep"), "{texts:?}");
        assert!(
            !texts.contains(&"."),
            "marker must not become a field: {texts:?}"
        );
    }

    #[test]
    fn detects_ps_aux() {
        assert_eq!(
            detect("USER         PID %CPU %MEM    VSZ   RSS TTY      STAT START   TIME COMMAND"),
            LineRole::PsOutput
        );
    }

    #[test]
    fn detects_ps_data_rows() {
        // Recognising only the header left every real row on the generic rules.
        assert_eq!(
            detect(
                "root      1234  0.0  0.1 123456 7890 ?        Ss   Oct30   0:01 /usr/sbin/sshd"
            ),
            LineRole::PsOutput
        );
        assert_eq!(
            detect("root      1234     1  0 10月29 ?        00:00:01 /usr/lib/systemd/systemd"),
            LineRole::PsFullOutput
        );
        assert_eq!(
            detect("www       9876  1.2  2.3 2345678 98765 pts/0  R+   10:11   1:23 nginx: worker"),
            LineRole::PsOutput
        );
    }

    #[test]
    fn rejects_ps_lookalike_rows() {
        for line in [
            // `vmstat` header: same width, no PID/percentage columns.
            "r  b   swpd   free   buff  cache   si   so    bi    bo   in   cs us sy id wa st",
            // Numeric table with decimals but no tty/state columns.
            "10:00:01 1234 5678 0.12 0.34 0.56 0.78 0.90 1.23 4.56 7.89",
            // `iostat` style row.
            "avg-cpu:  %user   %nice %system %iowait  %steal   %idle",
        ] {
            assert_eq!(detect(line), LineRole::Unknown, "{line}");
        }
    }

    #[test]
    fn detects_git_status() {
        assert_eq!(detect("On branch main"), LineRole::GitStatusOutput);
        assert_eq!(
            detect("Changes not staged for commit:"),
            LineRole::GitStatusOutput
        );
        assert_eq!(
            detect("\tmodified:   src/main.rs"),
            LineRole::GitStatusOutput
        );
        assert_eq!(
            detect("nothing to commit, working tree clean"),
            LineRole::GitStatusOutput
        );
    }

    #[test]
    fn detects_docker_ps() {
        assert_eq!(
            detect("CONTAINER ID   IMAGE     COMMAND   CREATED   STATUS    PORTS   NAMES"),
            LineRole::ContainerOutput
        );
        assert_eq!(
            detect(
                "a1b2c3d4e5f6   nginx:latest   \"nginx -g daemon off;\"   2 hours ago   Up 2 hours   80/tcp   web"
            ),
            LineRole::ContainerOutput
        );
    }

    #[test]
    fn detects_systemctl_status() {
        assert_eq!(
            detect("● nginx.service - A high performance web server"),
            LineRole::SystemdOutput
        );
        assert_eq!(
            detect("     Active: active (running) since Fri 2026-10-30 10:00:00 CST"),
            LineRole::SystemdOutput
        );
        assert_eq!(
            detect("     Loaded: loaded (/lib/systemd/system/nginx.service; enabled)"),
            LineRole::SystemdOutput
        );
    }

    #[test]
    fn git_status_fields_mark_labels_and_paths() {
        let line = "\tmodified:   src/main.rs";
        let fields = git_status_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        assert!(texts.contains(&"modified:"), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("src/main.rs")), "{texts:?}");
    }

    #[test]
    fn docker_fields_mark_status_words() {
        let line = "a1b2c3d4e5f6   nginx   Up 2 hours   80/tcp   web";
        let fields = container_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        assert!(texts.contains(&"a1b2c3d4e5f6"), "{texts:?}");
        assert!(texts.contains(&"Up"), "{texts:?}");
    }

    #[test]
    fn systemd_fields_mark_state() {
        let line = "     Active: active (running) since Fri 2026-10-30 10:00:00 CST";
        let fields = systemd_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        assert!(texts.contains(&"active (running)"), "{texts:?}");
    }

    #[test]
    fn detects_socket_tables() {
        assert_eq!(
            detect("State  Recv-Q Send-Q Local Address:Port Peer Address:Port"),
            LineRole::SocketOutput
        );
        assert_eq!(
            detect("LISTEN 0      128          0.0.0.0:22        0.0.0.0:*"),
            LineRole::SocketOutput
        );
        assert_eq!(
            detect("tcp        0      0 0.0.0.0:22              0.0.0.0:*               LISTEN"),
            LineRole::SocketOutput
        );
    }

    #[test]
    fn socket_fields_mark_state_and_ports() {
        let line = "LISTEN 0      128          0.0.0.0:22        0.0.0.0:*";
        let fields = socket_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        assert!(texts.contains(&"LISTEN"), "{texts:?}");
        assert!(texts.contains(&"22"), "{texts:?}");
    }

    #[test]
    fn detects_disk_usage() {
        assert_eq!(
            detect("Filesystem      Size  Used Avail Use% Mounted on"),
            LineRole::DiskUsageOutput
        );
        assert_eq!(
            detect("/dev/sda1       100G   60G   40G  60% /"),
            LineRole::DiskUsageOutput
        );
        assert_eq!(
            detect(
                "Mem:           31Gi       5.2Gi        20Gi       1.0Gi       6.5Gi        25Gi"
            ),
            LineRole::MemoryUsageOutput
        );
        assert_eq!(
            detect(
                "               total        used        free      shared  buff/cache   available"
            ),
            LineRole::MemoryUsageOutput
        );
    }

    #[test]
    fn disk_fields_flag_high_usage() {
        let line = "/dev/sda1       100G   60G   40G  92% /";
        let fields = disk_usage_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        assert!(texts.contains(&"92%"), "{texts:?}");
        assert!(texts.contains(&"/dev/sda1"), "{texts:?}");
        assert!(texts.contains(&"100G"), "{texts:?}");
        let usage = fields
            .iter()
            .find(|f| &line[f.range.clone()] == "92%")
            .expect("usage field");
        assert_eq!(usage.rule, "builtin-error");
    }

    #[test]
    fn detects_build_diagnostics() {
        assert_eq!(
            detect("error[E0308]: mismatched types"),
            LineRole::BuildOutput
        );
        assert_eq!(
            detect("warning: unused variable: `x`"),
            LineRole::BuildOutput
        );
        assert_eq!(detect("   --> src/main.rs:10:5"), LineRole::BuildOutput);
        assert_eq!(
            detect("make: *** [Makefile:10: all] Error 1"),
            LineRole::BuildOutput
        );
    }

    #[test]
    fn build_fields_mark_level_and_location() {
        let line = "error[E0308]: mismatched types";
        let fields = build_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        assert!(texts.contains(&"error"), "{texts:?}");
        assert!(texts.contains(&"E0308"), "{texts:?}");

        let location = "   --> src/main.rs:10:5";
        let fields = build_fields(location);
        let texts: Vec<&str> = fields.iter().map(|f| &location[f.range.clone()]).collect();
        assert!(texts.contains(&"src/main.rs"), "{texts:?}");
        assert!(texts.contains(&":10:5"), "{texts:?}");
    }

    #[test]
    fn detects_log_lines() {
        assert_eq!(
            detect("2026/10/30 21:00:00 [error] 1234#0: *5 open() failed"),
            LineRole::LogOutput
        );
        assert_eq!(
            detect("192.168.1.10 - - [30/Oct/2026:21:00:00 +0800] \"GET / HTTP/1.1\" 200 1234"),
            LineRole::LogOutput
        );
        assert_eq!(
            detect("Oct 30 21:00:00 web01 systemd[1]: Started nginx.service"),
            LineRole::LogOutput
        );
    }

    #[test]
    fn log_fields_mark_level_and_status() {
        let line = "2026/10/30 21:00:00 [error] 1234#0: *5 open() failed";
        let fields = log_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        assert!(texts.contains(&"[error]"), "{texts:?}");
        assert!(texts.contains(&"2026/10/30 21:00:00"), "{texts:?}");

        let access = "192.168.1.10 - - [30/Oct/2026:21:00:00 +0800] \"GET / HTTP/1.1\" 404 12";
        let fields = log_fields(access);
        let texts: Vec<&str> = fields.iter().map(|f| &access[f.range.clone()]).collect();
        assert!(texts.contains(&"404"), "{texts:?}");
    }

    #[test]
    fn detects_network_device_configuration() {
        for line in [
            "interface GigabitEthernet1/0/22",
            "interface Vlanif1",
            "vlan 4000",
            "ip address 10.2.2.10 255.255.255.0",
            "port link-type trunk",
            "port trunk allow-pass vlan 3320 3330",
            "dhcp select global",
            " description web-gateway",
            "sysname H3C-Core",
            "#",
        ] {
            assert_eq!(detect(line), LineRole::NetworkConfigOutput, "{line}");
        }
        // Prose that merely starts with a configuration word stays untouched.
        assert_eq!(detect("description of the problem"), LineRole::Unknown);
        assert_eq!(detect("interface guidelines"), LineRole::Unknown);
        assert_eq!(detect("rule of thumb"), LineRole::Unknown);
    }

    #[test]
    fn detects_dhcp_pool_block_from_real_device() {
        for line in [
            "dhcp server ip-pool 2000",
            " gateway-list 10.10.1.1",
            " network 10.10.1.0 mask 255.255.255.0",
            " dns-list 10.9.9.9 10.8.8.8",
            " option 43 ip-address 10.1.0.100",
            " forbidden-ip 192.168.117.253",
            "#",
        ] {
            assert_eq!(detect(line), LineRole::NetworkConfigOutput, "{line}");
        }
    }

    #[test]
    fn detects_cargo_compiler_and_sql() {
        assert_eq!(
            detect("   Compiling serde v1.0.0"),
            LineRole::RustToolOutput
        );
        assert_eq!(
            detect("    Finished release [optimized] target(s) in 5.01s"),
            LineRole::RustToolOutput
        );
        assert_eq!(
            detect("main.c:10:5: error: 'x' undeclared"),
            LineRole::CCompilerOutput
        );
        assert_eq!(
            detect("main.c: In function 'main':"),
            LineRole::CCompilerOutput
        );
        assert_eq!(
            detect("SELECT * FROM users WHERE id = 1"),
            LineRole::SqlOutput
        );
        assert_eq!(detect("insert into t values (1)"), LineRole::SqlOutput);
    }

    #[test]
    fn sql_fields_mark_keywords() {
        let line = "SELECT * FROM users WHERE id = 1";
        let fields = sql_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        assert!(texts.contains(&"SELECT"), "{texts:?}");
        assert!(texts.contains(&"FROM"), "{texts:?}");
        assert!(texts.contains(&"WHERE"), "{texts:?}");
    }

    #[test]
    fn compiler_fields_mark_location() {
        let line = "main.c:10:5: error: 'x' undeclared";
        let fields = c_compiler_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        assert!(texts.contains(&"main.c"), "{texts:?}");
        assert!(texts.contains(&"10:5"), "{texts:?}");
        assert!(texts.contains(&"error"), "{texts:?}");
    }

    #[test]
    fn detects_stat_acl_and_test_output() {
        assert_eq!(detect("  File: /etc/passwd"), LineRole::FileStatOutput);
        assert_eq!(
            detect("  Size: 1234      \tBlocks: 8          IO Block: 4096   regular file"),
            LineRole::FileStatOutput
        );
        assert_eq!(
            detect("Access: (0644/-rw-r--r--)  Uid: (    0/    root)   Gid: (    0/    root)"),
            LineRole::FileStatOutput
        );
        assert_eq!(detect("# file: test.txt"), LineRole::FileAclOutput);
        assert_eq!(detect("user::rw-"), LineRole::FileAclOutput);
        assert_eq!(detect("mask::rwx"), LineRole::FileAclOutput);
        assert_eq!(detect("running 5 tests"), LineRole::TestOutput);
        assert_eq!(
            detect("test result: ok. 5 passed; 0 failed; 0 ignored"),
            LineRole::TestOutput
        );
        assert_eq!(detect("test foo::bar ... ok"), LineRole::TestOutput);
        assert_eq!(detect("test foo::bar ... FAILED"), LineRole::TestOutput);
    }

    #[test]
    fn test_fields_mark_verdict() {
        let ok = "test foo::bar ... ok";
        let fields = test_fields(ok);
        let texts: Vec<&str> = fields.iter().map(|f| &ok[f.range.clone()]).collect();
        assert!(texts.contains(&"ok"), "{texts:?}");
        let failed = "test foo::bar ... FAILED";
        let fields = test_fields(failed);
        let texts: Vec<&str> = fields.iter().map(|f| &failed[f.range.clone()]).collect();
        assert!(texts.contains(&"FAILED"), "{texts:?}");
    }

    #[test]
    fn stat_fields_mark_permissions() {
        let line = "Access: (0644/-rw-r--r--)  Uid: (    0/    root)   Gid: (    0/    root)";
        let fields = file_stat_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        assert!(texts.contains(&"r"), "{texts:?}");
        assert!(texts.contains(&"w"), "{texts:?}");
    }

    #[test]
    fn detects_ps_full_git_diff_and_log() {
        assert_eq!(
            detect("UID          PID    PPID  C STIME TTY          TIME CMD"),
            LineRole::PsFullOutput
        );
        // Data rows are matched by column shape: the classifier sees one line at
        // a time, so waiting for the header would leave every real row on the
        // generic number/word rules.
        assert_eq!(
            detect("root           1       0  0 10月29 ?      00:00:01 /usr/lib/systemd/systemd"),
            LineRole::PsFullOutput,
        );
        assert_eq!(
            detect("diff --git a/src/main.rs b/src/main.rs"),
            LineRole::GitDiffOutput
        );
        assert_eq!(detect("@@ -1,7 +1,7 @@ fn main()"), LineRole::GitDiffOutput);
        assert_eq!(detect("--- a/src/main.rs"), LineRole::GitDiffOutput);
        assert_eq!(
            detect("commit 3f2a1b9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f3a"),
            LineRole::GitLogOutput
        );
        assert_eq!(
            detect("Author: Lao Liu <lao@example.com>"),
            LineRole::GitLogOutput
        );
        assert_eq!(
            detect("Date:   Thu Oct 30 23:00:00 2026 +0800"),
            LineRole::GitLogOutput
        );
    }

    #[test]
    fn ps_full_fields_mark_pids_and_command() {
        let line = "root           1       0  0 10月29 ?      00:00:01 /usr/lib/systemd/systemd";
        let fields = ps_full_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        assert!(texts.contains(&"1"), "{texts:?}");
        assert!(texts.contains(&"0"), "{texts:?}");
        assert!(texts.contains(&"00:00:01"), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("systemd")), "{texts:?}");
    }

    #[test]
    fn git_log_fields_mark_hash_and_author() {
        let line = "commit 3f2a1b9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f3a";
        let fields = git_log_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        assert!(
            texts.contains(&"3f2a1b9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f3a"),
            "{texts:?}"
        );
        let author = "Author: Lao Liu <lao@example.com>";
        let fields = git_log_fields(author);
        let texts: Vec<&str> = fields.iter().map(|f| &author[f.range.clone()]).collect();
        assert!(texts.contains(&"Lao Liu <lao@example.com>"), "{texts:?}");
    }

    #[test]
    fn detects_application_log_lines() {
        assert_eq!(
            detect(
                "2026-10-30 13:15:13.420 2026-10-30 23:13:20.422 DEBUG github.com/x/y/checker.go:332 message"
            ),
            LineRole::LogOutput
        );
        assert_eq!(
            detect("[GIN] 2026/10/30 - 23:14:08 | 200 | 127.0.0.1 | GET \"/health\""),
            LineRole::LogOutput
        );
        // Prose that merely contains a level word is not a log line.
        assert_eq!(
            detect("DEBUG the parser before shipping"),
            LineRole::Unknown
        );
    }

    #[test]
    fn app_log_fields_mark_level_and_location() {
        let line = "2026-10-30 13:15:13.420 2026-10-30 23:13:20.422 DEBUG github.com/x/y/checker.go:332 message";
        let fields = log_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        assert!(texts.contains(&"DEBUG"), "{texts:?}");
        assert!(
            texts.contains(&"github.com/x/y/checker.go:332"),
            "{texts:?}"
        );
        assert!(texts.contains(&"2026-10-30 13:15:13"), "{texts:?}");
        let level = fields
            .iter()
            .find(|f| &line[f.range.clone()] == "DEBUG")
            .expect("level field");
        assert_eq!(level.rule, "builtin-debug");
    }

    #[test]
    fn gin_log_marks_status_code() {
        let line = "[GIN] 2026/10/30 - 23:14:08 | 200 | 127.0.0.1 | GET \"/health\"";
        let fields = log_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        assert!(texts.contains(&"200"), "{texts:?}");
        assert!(texts.contains(&"GET"), "{texts:?}");
        let status = fields
            .iter()
            .find(|f| &line[f.range.clone()] == "200")
            .expect("status field");
        assert_eq!(status.rule, "builtin-success");
    }

    #[test]
    fn detects_common_h3c_config_lines() {
        for line in [
            "aaa",
            "domain default",
            "domain default_admin",
            "local-user admin password irreversible-cipher $1c$G2hX",
            "local-user admin service-type telnet terminal ssh http",
            "ntp-service server disable",
            "ntp-service unicast-server 172.30.60.9",
            "excluded-ip-address 10.2.1.2 10.2.1.10",
            "option 43 sub-option 3 ascii 10.2.1.1",
            "undo password alert original",
            "password-control login-attempt 5",
            "authorization-mode none",
            "accounting-mode none",
            "radius-server template default",
        ] {
            assert_eq!(detect(line), LineRole::NetworkConfigOutput, "{line}");
        }
    }

    #[test]
    fn network_config_fields_mark_ports_vlans_and_addresses() {
        let line = "interface GigabitEthernet1/0/22";
        let fields = network_config_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        assert!(texts.contains(&"interface"), "{texts:?}");
        assert!(texts.contains(&"GigabitEthernet1/0/22"), "{texts:?}");

        let vlan = "port trunk allow-pass vlan 3320 3330";
        let fields = network_config_fields(vlan);
        let texts: Vec<&str> = fields.iter().map(|f| &vlan[f.range.clone()]).collect();
        assert!(texts.contains(&"3320"), "{texts:?}");
        assert!(texts.contains(&"3330"), "{texts:?}");

        let address = "ip address 10.2.2.10 255.255.255.0";
        let fields = network_config_fields(address);
        let texts: Vec<&str> = fields.iter().map(|f| &address[f.range.clone()]).collect();
        assert!(texts.contains(&"10.2.2.10"), "{texts:?}");
        assert!(texts.contains(&"255.255.255.0"), "{texts:?}");
        let mask = fields
            .iter()
            .find(|f| &address[f.range.clone()] == "255.255.255.0")
            .expect("mask field");
        assert_eq!(mask.rule, "builtin-net-secondary");
    }

    #[test]
    fn leaves_ordinary_output_unknown() {
        assert_eq!(detect("total 48"), LineRole::Unknown);
        assert_eq!(detect("hello world"), LineRole::Unknown);
        assert_eq!(detect(""), LineRole::Unknown);
    }

    #[test]
    fn ip_fields_cover_interface_and_addresses() {
        let line = "2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc fq_codel state UP";
        let fields = ip_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        assert!(texts.contains(&"2"), "{texts:?}");
        assert!(texts.contains(&"eth0"), "{texts:?}");
        assert!(
            texts.contains(&"BROADCAST,MULTICAST,UP,LOWER_UP"),
            "{texts:?}"
        );
        assert!(texts.contains(&"1500"), "{texts:?}");
        assert!(texts.contains(&"UP"), "{texts:?}");
    }

    #[test]
    fn ip_fields_cover_inet_lines() {
        let line = "    inet 192.168.1.10/24 brd 192.168.1.255 scope global eth0";
        let fields = ip_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        assert!(texts.contains(&"192.168.1.10/24"), "{texts:?}");
        assert!(texts.contains(&"192.168.1.255"), "{texts:?}");
    }

    #[test]
    fn file_listing_fields_mark_mode_and_numbers() {
        let line = "-rw-r--r--  1 root root  1234 Oct 30 10:00 file.txt";
        let fields = file_listing_fields(line);
        let texts: Vec<&str> = fields.iter().map(|f| &line[f.range.clone()]).collect();
        // One field per permission character, plus the type byte.
        assert!(texts.contains(&"-"), "{texts:?}");
        assert!(texts.contains(&"1"), "{texts:?}");
        assert!(texts.contains(&"1234"), "{texts:?}");
        assert!(texts.contains(&"Oct"), "{texts:?}");
        let read = fields
            .iter()
            .find(|f| &line[f.range.clone()] == "r")
            .expect("read field");
        assert_eq!(read.rule, "builtin-perm-read");
        let write = fields
            .iter()
            .find(|f| &line[f.range.clone()] == "w")
            .expect("write field");
        assert_eq!(write.rule, "builtin-perm-write");
        // The month column carries both datetime and month; the month class wins
        // on priority, so assert that one exists rather than picking the first.
        assert!(
            fields
                .iter()
                .any(|f| &line[f.range.clone()] == "Oct" && f.rule == "builtin-month"),
            "{texts:?}"
        );
    }
}
