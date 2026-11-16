//! Pure path-completion logic: which token is a path, where to list, and how
//! listed entries become candidates. Listing itself (SFTP or local FS) is I/O
//! and lives in the desktop crate so this stays UI- and transport-independent.
use crate::completion::matching::TextMatcher;
use crate::completion::parser::parse_path_context;
use crate::completion::types::{
    CandidateKind, CandidateSource, CompletionCandidate, CompletionContext, CompletionScope,
    ShellDialect,
};
use crate::terminal::editing::supports_scalar_editing;
use crate::terminal::input_tracker::MAX_TRACKED_INPUT_BYTES;
use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::ops::Range;

/// Commands whose positional arguments are file-system paths.
const PATH_COMMANDS: &[&str] = &[
    "cd",
    "pushd",
    "ls",
    "ll",
    "la",
    "cat",
    "bat",
    "less",
    "more",
    "head",
    "tail",
    "vi",
    "vim",
    "nvim",
    "nano",
    "emacs",
    "code",
    "cp",
    "mv",
    "rm",
    "mkdir",
    "rmdir",
    "touch",
    "ln",
    "chmod",
    "chown",
    "chgrp",
    "stat",
    "file",
    "du",
    "df",
    "tree",
    "find",
    "source",
    ".",
    "tar",
    "unzip",
    "zip",
    "gzip",
    "gunzip",
    "rsync",
    "scp",
    "diff",
    "wc",
    "sort",
    "grep",
    "sed",
    "awk",
    "xdg-open",
    "open",
    "realpath",
    "readlink",
    "basename",
    "dirname",
    "sha256sum",
    "md5sum",
    "tee",
];
/// Commands that can only take a directory, so files are not offered.
const DIRECTORY_COMMANDS: &[&str] = &["cd", "pushd", "rmdir", "tree"];
/// Bounds the work for pathological directories (e.g. maildirs).
pub const MAX_LISTED_ENTRIES: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathQuery {
    /// Raw token bytes the chosen candidate replaces.
    pub range: Range<usize>,
    /// Unescaped text up to and including the last `/`, as the user typed it.
    pub dir_text: String,
    /// Unescaped partial entry name after the last `/`.
    pub base: String,
    pub directories_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: Option<u64>,
}

/// None means this token is not a path the engine should complete.
pub fn path_query(context: &CompletionContext) -> Option<PathQuery> {
    path_query_with_prefix(context, &context.prefix)
}

/// Parses `input` and returns the path query for its last token.
///
/// Only the current token may use tilde expansion. Lexical ranges distinguish
/// escaped separators and literal tildes from shell expansions.
pub fn path_query_for_input(
    input: &str,
    dialect: ShellDialect,
    scope: CompletionScope,
) -> Option<PathQuery> {
    let context = parse_path_context(input, dialect, scope)?;
    // An escaped tilde is a literal directory name, not a home expansion.
    if context.prefix.starts_with('~') && !input[context.token_range.clone()].starts_with('~') {
        return path_query_with_prefix(&context, &format!("./{}", context.prefix));
    }
    path_query(&context)
}

fn path_query_with_prefix(context: &CompletionContext, prefix: &str) -> Option<PathQuery> {
    if matches!(
        context.dialect,
        ShellDialect::Cmd | ShellDialect::PowerShell
    ) || context.option_value.is_some()
    {
        return None;
    }
    let command = context.preceding.first()?;
    let explicit = prefix.starts_with(['/', '~'])
        || prefix.starts_with("./")
        || prefix == "."
        || prefix == "..";
    let command_name = command.rsplit('/').next().unwrap_or(command);
    let known = PATH_COMMANDS.contains(&command_name);
    let directories_only = DIRECTORY_COMMANDS.contains(&command_name);
    if prefix.starts_with('-')
        || context
            .input
            .get(context.token_range.clone())
            .is_none_or(has_unescaped_expansion)
    {
        return None;
    }
    // `cd ` + nothing lists the working directory; other commands wait for
    // a character so a plain `ls ` does not flood the popup.
    let wanted = if prefix.is_empty() {
        directories_only
    } else {
        explicit || known || prefix.contains('/')
    };
    if !wanted {
        return None;
    }
    // `~user/` needs passwd lookup, which a remote listing cannot provide.
    if prefix.starts_with('~') && !(prefix == "~" || prefix.starts_with("~/")) {
        return None;
    }
    let (dir_text, base) = match prefix.rfind('/') {
        Some(index) => (
            prefix[..=index].to_string(),
            prefix[index + 1..].to_string(),
        ),
        None if prefix == "~" || prefix == "." || prefix == ".." => {
            (format!("{prefix}/"), String::new())
        }
        None => (String::new(), prefix.to_string()),
    };
    Some(PathQuery {
        range: context.token_range.clone(),
        dir_text,
        base,
        directories_only,
    })
}

fn has_unescaped_expansion(raw: &str) -> bool {
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if chars.next().is_none() {
                return true;
            }
        } else if matches!(
            ch,
            '*' | '?' | '[' | ']' | '$' | '`' | '(' | ')' | '{' | '}' | '<' | '>'
        ) {
            return true;
        }
    }
    false
}

/// Absolute directory to list, or None when the typed prefix cannot be resolved
/// (relative path without a known working directory, or an unknown home).
pub fn resolve_directory(
    query: &PathQuery,
    cwd: Option<&str>,
    home: Option<&str>,
) -> Option<String> {
    let dir = query.dir_text.as_str();
    let joined = if dir.starts_with('/') {
        dir.to_string()
    } else if let Some(rest) = dir.strip_prefix("~/") {
        format!("{}/{rest}", home?.trim_end_matches('/'))
    } else {
        let cwd = cwd.filter(|cwd| cwd.starts_with('/'))?;
        format!("{}/{dir}", cwd.trim_end_matches('/'))
    };
    Some(joined)
}

fn needs_escape(ch: char) -> bool {
    !(ch.is_alphanumeric() || "_./~@%+=:,-".contains(ch))
}

/// Backslash-escapes shell metacharacters; a leading `~` stays live so tilde
/// expansion still happens.
pub fn escape_shell_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 4);
    for (index, ch) in path.chars().enumerate() {
        if needs_escape(ch) || (index == 0 && ch == '#') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Unicode format and line/paragraph separators (categories Cf, Zl, Zp) are
/// invisible or reorder the text around them, so a remote file name could
/// make the popup show something other than what would be inserted.
fn is_invisible_format(ch: char) -> bool {
    matches!(
        ch,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
    )
}

fn entry_is_safe(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name
            .chars()
            .any(|ch| ch.is_control() || is_invisible_format(ch))
}

fn human_size(size: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn path_candidates(
    query: &PathQuery,
    entries: &[PathEntry],
    input: &str,
    matcher: &mut TextMatcher,
    limit: usize,
) -> Vec<CompletionCandidate> {
    path_candidates_with_policy(query, entries, input, matcher, limit, None)
}

pub fn path_candidates_with_policy(
    query: &PathQuery,
    entries: &[PathEntry],
    input: &str,
    matcher: &mut TextMatcher,
    limit: usize,
    policy: Option<crate::completion::types::CompletionFillPolicy>,
) -> Vec<CompletionCandidate> {
    if limit == 0 {
        return Vec::new();
    }
    let show_hidden = query.base.starts_with('.');
    matcher.set_query(&query.base);
    let mut best = BinaryHeap::<RankedPath>::new();
    for entry in entries.iter().take(MAX_LISTED_ENTRIES) {
        if !entry_is_safe(&entry.name)
            || (query.directories_only && !entry.is_dir)
            || (!show_hidden && entry.name.starts_with('.'))
        {
            continue;
        }
        let Some(score) = matcher.score(&entry.name) else {
            continue;
        };
        let prefix = entry.name.starts_with(&query.base);
        // Most rows need only borrowed scoring. Allocate fill text when a row
        // can enter the bounded result set, and validate it before using a slot.
        if best.len() == limit
            && best.peek().is_some_and(|worst| {
                path_rank(prefix, score, entry.is_dir, &entry.name).cmp(&path_rank(
                    worst.prefix,
                    worst.candidate.score,
                    worst.is_dir,
                    &worst.name,
                )) != Ordering::Less
            })
        {
            continue;
        }
        let display = if entry.is_dir {
            format!("{}/", entry.name)
        } else {
            entry.name.clone()
        };
        let anchor = if query.dir_text.is_empty() && display.starts_with(['~', '=', '-']) {
            "./"
        } else {
            ""
        };
        let insert_text = escape_shell_path(&format!("{anchor}{}{display}", query.dir_text));
        let mut candidate = CompletionCandidate {
            indices: Vec::new(),
            display,
            insert_text,
            description: if entry.is_dir {
                "directory".into()
            } else {
                entry.size.map(human_size).unwrap_or_default()
            },
            range: query.range.clone(),
            kind: CandidateKind::Argument,
            sources: vec![CandidateSource::Catalog],
            use_count: 0,
            last_used_at_ms: 0,
            score,
            fillable: false,
            context_priority: 2,
        };
        let Some(target) = candidate.target_line(input) else {
            continue;
        };
        candidate.fillable = !target.is_empty()
            && target.len() <= MAX_TRACKED_INPUT_BYTES
            && supports_scalar_editing(&target);
        if !candidate.fillable || policy.is_some_and(|policy| !policy.allows(input, &target)) {
            continue;
        }
        if best.len() == limit {
            best.pop();
        }
        best.push(RankedPath {
            name: entry.name.clone(),
            prefix,
            is_dir: entry.is_dir,
            candidate,
        });
    }
    best.into_sorted_vec()
        .into_iter()
        .map(|mut item| {
            item.candidate.indices = matcher.indices(&item.name);
            item.candidate
        })
        .collect()
}

// Prefix, fuzzy score and directory priority lead; name is the stable final tie.
fn path_rank(
    prefix: bool,
    score: u32,
    is_dir: bool,
    name: &str,
) -> (Reverse<bool>, Reverse<u32>, Reverse<bool>, &str) {
    (Reverse(prefix), Reverse(score), Reverse(is_dir), name)
}
struct RankedPath {
    name: String,
    prefix: bool,
    is_dir: bool,
    candidate: CompletionCandidate,
}
impl Ord for RankedPath {
    fn cmp(&self, other: &Self) -> Ordering {
        path_rank(self.prefix, self.candidate.score, self.is_dir, &self.name).cmp(&path_rank(
            other.prefix,
            other.candidate.score,
            other.is_dir,
            &other.name,
        ))
    }
}
impl PartialOrd for RankedPath {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for RankedPath {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for RankedPath {}

#[cfg(test)]
mod tests {
    use super::{
        PathEntry, PathQuery, escape_shell_path, path_candidates, path_query, path_query_for_input,
        resolve_directory,
    };
    use crate::completion::matching::TextMatcher;
    use crate::completion::parser::parse_context;
    use crate::completion::types::{CompletionScope, ShellDialect};

    #[test]
    fn escaped_directories_continue_completion_without_changing_byte_ranges() {
        for dialect in [ShellDialect::Bash, ShellDialect::Zsh, ShellDialect::Fish] {
            for (input, directory, base) in [
                (r"cd my\ dir/sub", "my dir/", "sub"),
                (r"cd my\ dir/", "my dir/", ""),
                (r"cd ~/my\ dir/sub", "~/my dir/", "sub"),
                (r"cd \~literal/sub", "./~literal/", "sub"),
                ("cd 中文/子", "中文/", "子"),
            ] {
                let query =
                    path_query_for_input(input, dialect, CompletionScope::legacy()).unwrap();
                assert_eq!(query.dir_text, directory);
                assert_eq!(query.base, base);
                assert_eq!(query.range, 3..input.len());
            }
            for input in ["cd 'my dir/", "cd my\\", "echo ~/earlier | cd pr"] {
                assert!(path_query_for_input(input, dialect, CompletionScope::legacy()).is_none());
            }
        }
    }

    #[test]
    fn unfillable_names_do_not_displace_valid_paths() {
        let input = "cd a";
        let query =
            path_query_for_input(input, ShellDialect::Bash, CompletionScope::legacy()).unwrap();
        let mut entries: Vec<_> = (0..12)
            .map(|i| PathEntry {
                name: format!("a{i:02}e\u{301}"),
                is_dir: true,
                is_symlink: false,
                size: None,
            })
            .collect();
        entries.push(PathEntry {
            name: "az-valid".into(),
            is_dir: true,
            is_symlink: false,
            size: None,
        });
        let candidates = path_candidates(&query, &entries, input, &mut TextMatcher::default(), 12);
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].target_line(input).as_deref(),
            Some("cd az-valid/")
        );
    }

    fn query(input: &str) -> Option<PathQuery> {
        path_query_for_input(input, ShellDialect::Bash, CompletionScope::legacy())
    }

    fn entry(name: &str, is_dir: bool) -> PathEntry {
        PathEntry {
            name: name.into(),
            is_dir,
            is_symlink: false,
            size: (!is_dir).then_some(2048),
        }
    }

    #[test]
    fn splits_the_typed_token_at_the_last_slash() {
        let q = query("cd /va").unwrap();
        assert_eq!((q.dir_text.as_str(), q.base.as_str()), ("/", "va"));
        assert!(q.directories_only);
        let q = query("ls ./src/ma").unwrap();
        assert_eq!((q.dir_text.as_str(), q.base.as_str()), ("./src/", "ma"));
        assert_eq!(q.range, 3..11);
    }

    #[test]
    fn ignores_flags_wildcards_and_unknown_commands() {
        assert!(query("ls -").is_none());
        assert!(query("ls *.rs").is_none());
        assert!(query("mytool abc").is_none());
        assert!(query("mytool /etc/ho").is_some());
        assert!(query("ls ").is_none());
        assert!(query("cd ").is_some());
        assert!(query("cd ~user/x").is_none());
    }

    #[test]
    fn skips_windows_shells_and_option_values() {
        let context =
            parse_context("cd C:/Us", 8, ShellDialect::Cmd, CompletionScope::legacy()).unwrap();
        assert!(path_query(&context).is_none());
    }

    #[test]
    fn resolves_absolute_relative_and_home_directories() {
        let q = query("cd /var/l").unwrap();
        assert_eq!(resolve_directory(&q, None, None).unwrap(), "/var/");
        let q = query("cd src/fe").unwrap();
        assert_eq!(
            resolve_directory(&q, Some("/srv/app/"), None).unwrap(),
            "/srv/app/src/"
        );
        assert!(resolve_directory(&q, None, None).is_none());
        let q = query("cd ~/pr").unwrap();
        assert_eq!(
            resolve_directory(&q, None, Some("/home/u/")).unwrap(),
            "/home/u/"
        );
        let q = query("cd ~").unwrap();
        assert_eq!(q.dir_text, "~/");
    }

    #[test]
    fn only_the_token_being_completed_may_start_with_a_tilde() {
        let q = query("cd ~/pr").unwrap();
        assert_eq!((q.dir_text.as_str(), q.base.as_str()), ("~/", "pr"));
        assert_eq!(q.range, 3..7);
        assert!(query("cp ~/a ~/b").is_none());
        assert!(query("cp ~/a /tmp/b").is_none());
        assert!(query("cd H/pr").unwrap().dir_text == "H/");
        assert!(query("cd ~").unwrap().dir_text == "~/");
    }

    #[test]
    fn escapes_spaces_and_metacharacters_but_keeps_tilde_expansion() {
        assert_eq!(escape_shell_path("~/my dir/a&b"), "~/my\\ dir/a\\&b");
        assert_eq!(escape_shell_path("/plain-name_1.txt"), "/plain-name_1.txt");
    }

    #[test]
    fn ranks_prefix_matches_first_and_hides_dotfiles_unless_asked() {
        let q = query("ls /etc/ho").unwrap();
        let entries = [
            entry("hosts", false),
            entry("host.conf", false),
            entry("shadow", false),
            entry(".hidden", false),
            entry("hdparm", true),
        ];
        let got = path_candidates(&q, &entries, "ls /etc/ho", &mut TextMatcher::default(), 8);
        let shown: Vec<_> = got.iter().map(|c| c.display.as_str()).collect();
        let mut leading = shown[..2].to_vec();
        leading.sort_unstable();
        assert_eq!(leading, ["host.conf", "hosts"]);
        assert!(!shown.contains(&"hdparm/"));
        assert!(!shown.contains(&".hidden"));
        let q = query("ls /etc/.h").unwrap();
        let got = path_candidates(&q, &entries, "ls /etc/.h", &mut TextMatcher::default(), 8);
        assert_eq!(got[0].display, ".hidden");
    }

    #[test]
    fn directory_only_commands_skip_files_and_directories_get_a_slash() {
        let q = query("cd /v").unwrap();
        let entries = [entry("var", true), entry("vmlinuz", false)];
        let got = path_candidates(&q, &entries, "cd /v", &mut TextMatcher::default(), 8);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].insert_text, "/var/");
        assert_eq!(got[0].target_line("cd /v").unwrap(), "cd /var/");
        assert!(got[0].fillable);
    }

    #[test]
    fn an_empty_name_part_lists_every_visible_entry() {
        let q = query("cd /var/").unwrap();
        assert_eq!((q.dir_text.as_str(), q.base.as_str()), ("/var/", ""));
        let entries = [
            entry("log", true),
            entry("lib", true),
            entry(".cache", true),
        ];
        let got = path_candidates(&q, &entries, "cd /var/", &mut TextMatcher::default(), 8);
        let shown: Vec<_> = got.iter().map(|c| c.insert_text.as_str()).collect();
        assert_eq!(shown, ["/var/lib/", "/var/log/"]);
    }

    #[test]
    fn hides_names_with_invisible_or_reordering_characters() {
        let q = query("cat /tmp/a").unwrap();
        let entries = [
            entry("a\u{202E}gpj.sh", false),
            entry("a\u{200B}b", false),
            entry("a\u{2028}b", false),
            entry("ascii", false),
        ];
        let got = path_candidates(&q, &entries, "cat /tmp/a", &mut TextMatcher::default(), 8);
        let shown: Vec<_> = got.iter().map(|c| c.display.as_str()).collect();
        assert_eq!(shown, ["ascii"]);
    }

    #[test]
    fn bare_names_that_look_like_options_or_expansions_are_anchored() {
        let q = path_query_for_input("cd ", ShellDialect::Bash, CompletionScope::legacy()).unwrap();
        let entries = [
            entry("~tmp", true),
            entry("-odd", true),
            entry("=eq", true),
            entry("plain", true),
        ];
        let got = path_candidates(&q, &entries, "cd ", &mut TextMatcher::default(), 8);
        let mut inserted: Vec<_> = got.iter().map(|c| c.insert_text.clone()).collect();
        inserted.sort();
        assert_eq!(inserted, ["./-odd/", "./=eq/", "./~tmp/", "plain/"]);
    }

    #[test]
    fn rejects_names_that_could_inject_input() {
        let q = query("cat /tmp/a").unwrap();
        let entries = [
            entry("a\nrm -rf", false),
            entry("a\u{1b}[31m", false),
            entry("ok", false),
        ];
        assert!(
            path_candidates(&q, &entries, "cat /tmp/a", &mut TextMatcher::default(), 8).is_empty()
        );
    }
    #[test]
    fn generated_escapes_continue_through_literal_shell_metacharacters() {
        for dialect in [ShellDialect::Bash, ShellDialect::Zsh, ShellDialect::Fish] {
            for name in [
                "my$dir", "my*dir", "my?dir", "my[dir]", "my`dir", "my(dir)", "my{dir}",
            ] {
                let input = format!("cd {}/ch", escape_shell_path(name));
                let query =
                    path_query_for_input(&input, dialect, CompletionScope::legacy()).unwrap();
                assert_eq!(query.dir_text, format!("{name}/"));
                assert_eq!(query.base, "ch");
                assert_eq!(query.range, 3..input.len());
                assert!(
                    path_query_for_input(
                        &format!("cd {name}/ch"),
                        dialect,
                        CompletionScope::legacy()
                    )
                    .is_none()
                );
            }
        }
    }
}
