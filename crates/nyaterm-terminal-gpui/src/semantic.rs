//! Bounded logical-line semantics. This is deliberately not a shell grammar or document lexer.

use std::collections::HashMap;
use std::net::Ipv6Addr;
use std::ops::Range;

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use nyaterm_core::keyword_highlight_presets::{
    BuiltinKeywordMatcher, ResolvedKeywordHighlightRule, builtin_keyword_matcher,
    builtin_keyword_rule_swatch,
};
use nyaterm_terminal::terminal_is_zero_width_mark;

use crate::keywords::{is_whole_word_match, parse_hex_rgb};

#[derive(Default)]
pub(super) struct SemanticContext {
    pub input: Vec<Range<usize>>,
    pub prompt: Vec<Range<usize>>,
    pub integrated: bool,
    pub continuation: bool,
}

pub(super) fn extend_region(regions: &mut Vec<Range<usize>>, start: usize, end: usize) {
    if let Some(last) = regions.last_mut()
        && last.end == start
    {
        last.end = end;
    } else {
        regions.push(start..end);
    }
}

#[derive(Clone, Copy)]
pub(super) struct SemanticMatch {
    pub start: usize,
    pub end: usize,
    pub color: u32,
    // Priority breaks ties only when two whole matches have identical bounds.
    pub priority: u16,
    permissions: bool,
}

struct RegexRule {
    regex: regex::Regex,
    color: u32,
    priority: u16,
    whole_word: bool,
    output_only: bool,
    shell_lexed: bool,
    option: bool,
    operator: bool,
    numeric: bool,
}

struct LiteralRule {
    color: u32,
    output_only: bool,
    whole_word: bool,
}

struct ChildColors {
    read: u32,
    write: u32,
    execute: u32,
    file_type: u32,
    punctuation: u32,
}

pub(super) struct SemanticHighlighter {
    colors: HashMap<String, u32>,
    regex: Vec<RegexRule>,
    words: Vec<LiteralRule>,
    automaton: Option<AhoCorasick>,
    ipv6_candidate: regex::Regex,
    children: ChildColors,
}

impl SemanticHighlighter {
    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }

    pub fn compile(rules: &[ResolvedKeywordHighlightRule]) -> Self {
        let mut colors = HashMap::new();
        let mut regex = Vec::new();
        let mut words = Vec::new();
        let mut patterns = Vec::new();
        let dark = rules
            .iter()
            .find(|r| r.id == "builtin-string")
            .or_else(|| rules.iter().find(|r| r.id == "builtin-permissions"))
            .or_else(|| {
                rules
                    .iter()
                    .find(|r| builtin_keyword_matcher(&r.id).is_some())
            })
            .is_none_or(|r| {
                r.color
                    .eq_ignore_ascii_case(builtin_keyword_rule_swatch(&r.id, true))
            });
        for rule in rules.iter().filter(|r| r.enabled) {
            let Some(matcher) = builtin_keyword_matcher(&rule.id) else {
                continue;
            };
            let color = parse_hex_rgb(&rule.color).unwrap_or(0x6796e6);
            colors.insert(rule.id.clone(), color);
            match matcher {
                BuiltinKeywordMatcher::LiteralWords => {
                    for pattern in &rule.patterns {
                        patterns.push(pattern.clone());
                        words.push(LiteralRule {
                            color,
                            output_only: rule.id != "builtin-constant",
                            whole_word: true,
                        });
                    }
                }
                BuiltinKeywordMatcher::Regex => {
                    let priority = match rule.id.as_str() {
                        "builtin-url" => 25,
                        "builtin-version" => 35,
                        "builtin-number" => 80,
                        "builtin-operator" => 90,
                        "builtin-option" => 40,
                        "builtin-string" => 20,
                        // Interface names and VLAN tags must outrank the number
                        // rule (80) so the whole token keeps one colour.
                        "builtin-net-interface" | "builtin-net-vlan" => 85,
                        _ => 30,
                    };
                    for pattern in &rule.patterns {
                        if let Ok(compiled) = regex::RegexBuilder::new(pattern)
                            .case_insensitive(true)
                            .build()
                        {
                            regex.push(RegexRule {
                                regex: compiled,
                                color,
                                priority,
                                whole_word: rule.id == "builtin-option",
                                output_only: false,
                                shell_lexed: matches!(
                                    rule.id.as_str(),
                                    "builtin-option" | "builtin-string"
                                ),
                                option: rule.id == "builtin-option",
                                operator: rule.id == "builtin-operator",
                                numeric: matches!(
                                    rule.id.as_str(),
                                    "builtin-number" | "builtin-duration"
                                ),
                            });
                        }
                    }
                }
                BuiltinKeywordMatcher::Structured => {}
            }
        }
        let automaton = (!patterns.is_empty()).then(|| {
            AhoCorasickBuilder::new()
                .ascii_case_insensitive(true)
                .match_kind(MatchKind::LeftmostLongest)
                .build(patterns)
                .expect("built-in literal matcher")
        });
        let swatch =
            |id| parse_hex_rgb(builtin_keyword_rule_swatch(id, dark)).expect("semantic swatch");
        Self {
            colors,
            regex,
            words,
            automaton,
            children: ChildColors {
                read: swatch("builtin-info"),
                write: swatch("builtin-warn"),
                execute: swatch("builtin-error"),
                file_type: swatch("builtin-debug"),
                punctuation: swatch("builtin-operator"),
            },
            ipv6_candidate: regex::Regex::new(r"[0-9A-Fa-f:.]*:[0-9A-Fa-f:.]+")
                .expect("IPv6 candidate"),
        }
    }

    fn color(&self, id: &str) -> Option<u32> {
        self.colors.get(id).copied()
    }

    fn child_color(&self, id: &str) -> u32 {
        match id {
            "builtin-info" => self.children.read,
            "builtin-warn" => self.children.write,
            "builtin-error" => self.children.execute,
            "builtin-debug" => self.children.file_type,
            _ => self.children.punctuation,
        }
    }

    fn add(&self, matches: &mut Vec<SemanticMatch>, range: Range<usize>, id: &str, priority: u16) {
        if let Some(color) = self.color(id)
            && range.start < range.end
        {
            matches.push(SemanticMatch {
                start: range.start,
                end: range.end,
                color,
                priority,
                permissions: false,
            });
        }
    }

    #[cfg(test)]
    pub fn matches(&self, line: &str, context: &SemanticContext) -> Vec<SemanticMatch> {
        self.whole_matches(line, context, &[])
    }

    pub fn ranges(
        &self,
        line: &str,
        context: &SemanticContext,
        user_matches: &[(usize, usize, u32)],
    ) -> Vec<(usize, usize, u32)> {
        if self.is_empty() {
            return user_matches.to_vec();
        }
        let mut ranges: Vec<(usize, usize, u32)> = Vec::new();
        let mut append = |start, end, color| {
            if let Some(last) = ranges.last_mut()
                && last.1 == start
                && last.2 == color
            {
                last.1 = end;
            } else {
                ranges.push((start, end, color));
            }
        };
        for found in self.whole_matches(line, context, user_matches) {
            if found.permissions {
                // Permissions are one validated token. Decoration does not resolve overlaps.
                for (index, byte) in line[found.start..found.end].bytes().enumerate() {
                    let id = match byte {
                        b'-' | b'.' | b'+' => "builtin-operator",
                        _ if index == 0 => "builtin-debug",
                        b'r' => "builtin-info",
                        b'w' => "builtin-warn",
                        _ => "builtin-error",
                    };
                    append(
                        found.start + index,
                        found.start + index + 1,
                        self.child_color(id),
                    );
                }
            } else {
                append(found.start, found.end, found.color);
            }
        }
        ranges
    }

    fn whole_matches(
        &self,
        line: &str,
        context: &SemanticContext,
        user_matches: &[(usize, usize, u32)],
    ) -> Vec<SemanticMatch> {
        let fallback = (!context.integrated && !context.continuation)
            .then(|| fallback_prompt(line))
            .flatten();
        let fallback_input = fallback.map(|(_, start)| start..line.len());
        let input: &[Range<usize>] = if let Some(ref range) = fallback_input {
            std::slice::from_ref(range)
        } else {
            &context.input
        };
        let in_input =
            |start: usize, end: usize| input.iter().any(|r| start < r.end && end > r.start);
        let in_prompt = |start: usize, end: usize| {
            context
                .prompt
                .iter()
                .any(|r| start < r.end && end > r.start)
                || fallback.is_some_and(|(_, input)| start < input && end > 0)
        };
        // Each source advances in text order. Keep one pending match per source,
        // choose the leftmost/longest whole token, then skip its interior.
        let mut sources: Vec<Box<dyn Iterator<Item = SemanticMatch> + '_>> = Vec::new();
        sources.push(Box::new(user_matches.iter().map(|&(start, end, color)| {
            SemanticMatch {
                start,
                end,
                color,
                priority: 0,
                permissions: false,
            }
        })));
        if let Some(color) = self.color("builtin-number") {
            let in_input = &in_input;
            let in_prompt = &in_prompt;
            // Ping sequences are counters even below ten. Keep this exception
            // in output context so ordinary single digits retain their noise filter.
            sources.push(Box::new(line.match_indices("icmp_seq=").filter_map(
                move |(field_start, field)| {
                    let start = field_start + field.len();
                    let end = start + line[start..].bytes().take_while(u8::is_ascii_digit).count();
                    (start < end
                        && semantic_word_boundary(line, field_start, end)
                        && numeric_boundary(line, start, end)
                        && !in_input(field_start, end)
                        && !in_prompt(field_start, end))
                    .then_some(SemanticMatch {
                        start,
                        end,
                        color,
                        priority: 80,
                        permissions: false,
                    })
                },
            )));
        }
        for rule in &self.regex {
            let in_input = &in_input;
            let in_prompt = &in_prompt;
            sources.push(Box::new(
                rule.regex
                    .find_iter(line)
                    .filter(move |found| {
                        !found.is_empty()
                            && (!rule.numeric || numeric_boundary(line, found.start(), found.end()))
                            && (!rule.option || option_start_boundary(line, found.start()))
                            && (!rule.whole_word
                                || semantic_word_boundary(line, found.start(), found.end()))
                            && !in_prompt(found.start(), found.end())
                            && (!(rule.output_only || rule.shell_lexed)
                                || !in_input(found.start(), found.end()))
                    })
                    .flat_map(move |found| {
                        let mut cursor = found.start();
                        std::iter::from_fn(move || {
                            let mut start = cursor;
                            while cursor < found.end() {
                                if !rule.operator {
                                    cursor = found.end();
                                    break;
                                }
                                if line.as_bytes()[cursor] != b'-' {
                                    // Keep punctuation separate from a following signed
                                    // number: at the sign, the longer numeric token wins.
                                    cursor += 1;
                                    break;
                                }
                                let hyphen = cursor;
                                while cursor < found.end() && line.as_bytes()[cursor] == b'-' {
                                    cursor += 1;
                                }
                                if embedded_name_hyphens(line, hyphen, cursor) {
                                    if start < hyphen {
                                        return Some(SemanticMatch {
                                            start,
                                            end: hyphen,
                                            color: rule.color,
                                            priority: rule.priority,
                                            permissions: false,
                                        });
                                    }
                                    start = cursor;
                                } else {
                                    break;
                                }
                            }
                            (start < cursor).then_some(SemanticMatch {
                                start,
                                end: cursor,
                                color: rule.color,
                                priority: rule.priority,
                                permissions: false,
                            })
                        })
                    }),
            ));
        }
        if let Some(automaton) = &self.automaton {
            sources.push(Box::new(automaton.find_iter(line).filter_map(|found| {
                let rule = &self.words[found.pattern().as_usize()];
                if (rule.whole_word && !semantic_word_boundary(line, found.start(), found.end()))
                    || in_prompt(found.start(), found.end())
                    || (rule.output_only && in_input(found.start(), found.end()))
                {
                    return None;
                }
                Some(SemanticMatch {
                    start: found.start(),
                    end: found.end(),
                    color: rule.color,
                    priority: 60,
                    permissions: false,
                })
            })));
        }
        if let Some(color) = self.color("builtin-address") {
            let in_prompt = &in_prompt;
            sources.push(Box::new(self.ipv6_candidate.find_iter(line).filter_map(
                move |candidate| {
                    (semantic_word_boundary(line, candidate.start(), candidate.end())
                        && candidate.as_str().parse::<Ipv6Addr>().is_ok()
                        && !in_prompt(candidate.start(), candidate.end()))
                    .then_some(SemanticMatch {
                        start: candidate.start(),
                        end: candidate.end(),
                        color,
                        priority: 30,
                        permissions: false,
                    })
                },
            )));
        }
        if let Some(color) = self.color("builtin-permissions") {
            let mut offset = 0;
            let in_input = &in_input;
            let in_prompt = &in_prompt;
            sources.push(Box::new(
                line.split_inclusive(char::is_whitespace)
                    .filter_map(move |word| {
                        let start = offset;
                        offset += word.len();
                        let token = word.trim_end();
                        let end = start + token.len();
                        (valid_permissions(token)
                            && !in_input(start, end)
                            && !in_prompt(start, end))
                        .then_some(SemanticMatch {
                            start,
                            end,
                            color,
                            priority: 20,
                            permissions: true,
                        })
                    }),
            ));
        }
        let mut prompts = Vec::new();
        for prompt in &context.prompt {
            self.prompt_sign(line, prompt.clone(), &mut prompts);
        }
        if let Some((sign, _)) = fallback {
            self.add(&mut prompts, sign..sign + 1, "builtin-prompt", 20);
        }
        sources.push(Box::new(prompts.into_iter()));
        let mut shell = Vec::new();
        for region in input {
            self.shell(line, region.clone(), &mut shell, context.continuation);
        }
        sources.push(Box::new(shell.into_iter()));

        let mut pending: Vec<_> = sources.into_iter().map(Iterator::peekable).collect();
        let mut result = Vec::new();
        let mut cursor = 0;
        loop {
            let mut best: Option<SemanticMatch> = None;
            for source in &mut pending {
                while source
                    .peek()
                    .is_some_and(|candidate| candidate.start < cursor)
                {
                    source.next();
                }
                if let Some(candidate) = source.peek().copied()
                    && best.is_none_or(|current| {
                        candidate.start < current.start
                            || (candidate.start == current.start && candidate.end > current.end)
                            || (candidate.start == current.start
                                && candidate.end == current.end
                                && candidate.priority < current.priority)
                    })
                {
                    best = Some(candidate);
                }
            }
            let Some(best) = best else { break };
            cursor = best.end;
            result.push(best);
        }
        result
    }

    fn prompt_sign(&self, line: &str, region: Range<usize>, out: &mut Vec<SemanticMatch>) {
        if let Some((offset, ch)) = line[region.clone()]
            .char_indices()
            .rev()
            .find(|(_, ch)| !ch.is_whitespace())
            && matches!(ch, '$' | '#' | '>' | '%' | '❯')
        {
            self.add(
                out,
                region.start + offset..region.start + offset + ch.len_utf8(),
                "builtin-prompt",
                20,
            );
        }
    }

    fn shell(
        &self,
        line: &str,
        region: Range<usize>,
        out: &mut Vec<SemanticMatch>,
        continuation: bool,
    ) {
        let bytes = line.as_bytes();
        let mut cursor = region.start;
        let mut command = !continuation;
        while cursor < region.end {
            if bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
                continue;
            }
            if matches!(bytes[cursor], b';' | b'|' | b'&') {
                let start = cursor;
                cursor += 1;
                if cursor < region.end && bytes[cursor] == bytes[start] {
                    cursor += 1;
                }
                self.add(out, start..cursor, "builtin-operator", 40);
                command = true;
                continue;
            }
            if matches!(bytes[cursor], b'<' | b'>') {
                // Skip a simple redirection's target without promoting it to a command.
                let start = cursor;
                cursor += 1;
                while cursor < region.end && matches!(bytes[cursor], b'<' | b'>' | b'&') {
                    cursor += 1;
                }
                self.add(out, start..cursor, "builtin-operator", 40);
                while cursor < region.end && bytes[cursor].is_ascii_whitespace() {
                    cursor += 1;
                }
                let end = shell_word_end(line, cursor, region.end);
                self.shell_word(line, cursor..end, false, out);
                cursor = end;
                continue;
            }
            if bytes[cursor] == b'#' {
                break;
            }
            let start = cursor;
            cursor = shell_word_end(line, start, region.end);
            let word = &line[start..cursor];
            let assignment = word
                .split_once('=')
                .is_some_and(|(name, _)| valid_assignment_name(name));
            self.shell_word(line, start..cursor, command && !assignment, out);
            if !assignment {
                command = false;
            }
        }
    }

    fn shell_word(
        &self,
        line: &str,
        range: Range<usize>,
        command: bool,
        out: &mut Vec<SemanticMatch>,
    ) {
        let word = &line[range.clone()];
        if command {
            self.add(out, range.clone(), "builtin-command", 22);
        }
        if !command
            && (word.starts_with('/')
                || word.starts_with("./")
                || word.starts_with("../")
                || word.starts_with("~/"))
        {
            self.add(out, range.clone(), "builtin-path", 28);
        }
        if word.starts_with('-')
            && word
                .trim_start_matches('-')
                .starts_with(|ch: char| ch.is_ascii_alphabetic())
        {
            let flag_end = word.find('=').unwrap_or(word.len());
            self.add(
                out,
                range.start..range.start + flag_end,
                "builtin-option",
                40,
            );
            if flag_end < word.len() && self.color("builtin-option").is_some() {
                self.add(
                    out,
                    range.start + flag_end..range.start + flag_end + 1,
                    "builtin-operator",
                    40,
                );
                if flag_end + 1 < word.len() {
                    out.push(SemanticMatch {
                        start: range.start + flag_end + 1,
                        end: range.end,
                        color: self.child_color("builtin-info"),
                        priority: 50,
                        permissions: false,
                    });
                }
            }
        }
        if command && self.color("builtin-command").is_some()
            || self.color("builtin-path").is_some()
                && (word.starts_with('/')
                    || word.starts_with("./")
                    || word.starts_with("../")
                    || word.starts_with("~/"))
            || self.color("builtin-option").is_some() && word.starts_with('-')
        {
            return;
        }
        let mut cursor = range.start;
        let bytes = line.as_bytes();
        while cursor < range.end {
            if bytes[cursor] == b'\\' {
                cursor = escaped_end(line, cursor, range.end);
                continue;
            }
            if !matches!(bytes[cursor], b'\'' | b'"') {
                cursor += line[cursor..]
                    .chars()
                    .next()
                    .expect("word character")
                    .len_utf8();
                continue;
            }
            let quote = bytes[cursor];
            let start = cursor;
            cursor += 1;
            while cursor < range.end {
                if bytes[cursor] == quote {
                    cursor += 1;
                    break;
                }
                if bytes[cursor] == b'\\' && quote == b'"' {
                    let end = escaped_end(line, cursor, range.end);
                    cursor = end;
                } else {
                    cursor += line[cursor..]
                        .chars()
                        .next()
                        .expect("string character")
                        .len_utf8();
                }
            }
            self.add(out, start..cursor, "builtin-string", 20);
        }
    }
}

fn semantic_word_boundary(line: &str, start: usize, end: usize) -> bool {
    let word_char = |ch: char| ch.is_alphanumeric() || ch == '-' || terminal_is_zero_width_mark(ch);
    is_whole_word_match(line, start, end)
        && !line[..start].chars().next_back().is_some_and(word_char)
        && !line[end..].chars().next().is_some_and(word_char)
}

fn numeric_boundary(line: &str, start: usize, end: usize) -> bool {
    // Do not recover numeric fragments inside names, dotted tokens or malformed
    // literals (e.g. worker-123, 0x12GG, 12e+ or 12e--34). Consuming candidates
    // before checking their neighbours also prevents retrying without their sign.
    let continuation = |ch: char| {
        ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.') || terminal_is_zero_width_mark(ch)
    };
    !line[..start]
        .chars()
        .next_back()
        .is_some_and(|ch| continuation(ch) || ch == '+')
        && !line[end..].chars().next().is_some_and(continuation)
}

fn option_start_boundary(line: &str, start: usize) -> bool {
    line[..start]
        .chars()
        .next_back()
        .is_none_or(|ch| ch.is_whitespace() || matches!(ch, '[' | '|'))
}

fn embedded_name_hyphens(line: &str, start: usize, end: usize) -> bool {
    let name_char = |ch: char| ch.is_alphanumeric() || ch == '_' || terminal_is_zero_width_mark(ch);
    line[..start].chars().next_back().is_some_and(name_char)
        && line[end..].chars().next().is_some_and(name_char)
}

fn valid_permissions(word: &str) -> bool {
    let bytes = word.as_bytes();
    if !(bytes.len() == 10 || (bytes.len() == 11 && matches!(bytes[10], b'.' | b'+')))
        || !matches!(
            bytes[0],
            b'b' | b'c'
                | b'C'
                | b'd'
                | b'D'
                | b'l'
                | b'M'
                | b'n'
                | b'p'
                | b'P'
                | b's'
                | b'?'
                | b'-'
        )
    {
        return false;
    }
    (0..3).all(|group| {
        let start = 1 + group * 3;
        matches!(bytes[start], b'r' | b'-')
            && matches!(bytes[start + 1], b'w' | b'-')
            && matches!(bytes[start + 2], b'x' | b's' | b'S' | b't' | b'T' | b'-')
    })
}

fn valid_assignment_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn escaped_end(line: &str, start: usize, end: usize) -> usize {
    let next = start + 1;
    if next >= end {
        return end;
    }
    let byte = line.as_bytes()[next];
    let digits = match byte {
        b'x' => 2,
        b'u' => 4,
        b'U' => 8,
        _ => 0,
    };
    if digits > 0
        && next + 1 + digits <= end
        && line.as_bytes()[next + 1..next + 1 + digits]
            .iter()
            .all(u8::is_ascii_hexdigit)
    {
        next + 1 + digits
    } else {
        next + line[next..]
            .chars()
            .next()
            .expect("escaped character")
            .len_utf8()
    }
}

fn shell_word_end(line: &str, start: usize, end: usize) -> usize {
    let mut cursor = start;
    let mut quote = None;
    let bytes = line.as_bytes();
    while cursor < end {
        let byte = bytes[cursor];
        if byte == b'\\' && quote != Some(b'\'') {
            cursor = escaped_end(line, cursor, end);
            continue;
        }
        if Some(byte) == quote {
            quote = None;
        } else if quote.is_none() {
            if byte.is_ascii_whitespace() || matches!(byte, b';' | b'|' | b'&' | b'<' | b'>') {
                break;
            }
            if matches!(byte, b'\'' | b'"') {
                quote = Some(byte);
            }
        }
        cursor += line[cursor..]
            .chars()
            .next()
            .expect("shell character")
            .len_utf8();
    }
    cursor
}

pub(super) fn fallback_prompt(line: &str) -> Option<(usize, usize)> {
    // Bare signs, user@host:path, or the conventional [user@host directory] sign.
    // Avoid interpreting arbitrary log text containing '$', '#' or '>' as input.
    let trimmed = line.trim_start();
    let prefix = line.len() - trimmed.len();
    let first = if let Some(bracketed) = trimmed.strip_prefix('[') {
        let (prefix, _) = bracketed.split_once(']')?;
        let host = prefix.split_whitespace().next()?;
        let (user, host) = host.split_once('@')?;
        if user.is_empty() || host.is_empty() {
            return None;
        }
        trimmed.get(..prefix.len() + 3)?
    } else {
        trimmed.split_whitespace().next()?
    };
    let sign = first.as_bytes().last().copied()?;
    if !matches!(sign, b'$' | b'#' | b'>' | b'%') || !(first.len() == 1 || first.contains('@')) {
        return None;
    }
    let end = prefix + first.len();
    if !line[end..].starts_with(char::is_whitespace) {
        return None;
    }
    Some((end - 1, end))
}

#[cfg(test)]
mod tests;
