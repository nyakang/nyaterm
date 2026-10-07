use std::fmt;
use std::time::Duration;

use regex::Regex;

const MAX_REPEAT_COUNT: u32 = 10_000;

#[derive(Debug, Clone)]
pub(super) struct Program {
    pub statements: Vec<Statement>,
}

#[derive(Debug, Clone)]
pub(super) enum Statement {
    Connect {
        target: String,
        alias: Option<String>,
        line: usize,
    },
    Use {
        alias: String,
        line: usize,
    },
    Send {
        value: Value,
        newline: bool,
        line: usize,
    },
    Wait {
        matcher: WaitMatcher,
        line: usize,
    },
    Timeout {
        duration: Duration,
        line: usize,
    },
    If {
        condition: WaitCondition,
        then_body: Vec<Statement>,
        else_body: Vec<Statement>,
        line: usize,
    },
    Repeat {
        count: u32,
        body: Vec<Statement>,
        line: usize,
    },
    Log {
        message: String,
        line: usize,
    },
}

impl Statement {
    pub fn line(&self) -> usize {
        match self {
            Self::Connect { line, .. }
            | Self::Use { line, .. }
            | Self::Send { line, .. }
            | Self::Wait { line, .. }
            | Self::Timeout { line, .. }
            | Self::If { line, .. }
            | Self::Repeat { line, .. }
            | Self::Log { line, .. } => *line,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum Value {
    Literal(String),
    Secret(String),
}

#[derive(Debug, Clone)]
pub(super) enum WaitMatcher {
    Literal(String),
    Regex(Regex),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WaitCondition {
    Matched,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "NyaScript parse error at line {}, column {}: {}",
            self.line, self.column, self.message
        )
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockEnd {
    Eof,
    End { line: usize, column: usize },
    Else { line: usize, column: usize },
}

pub(super) fn parse(source: &str) -> Result<Program, ParseError> {
    let lines: Vec<&str> = source.lines().collect();
    let mut index = 0;
    let (statements, end) = parse_block(&lines, &mut index, false)?;
    match end {
        BlockEnd::Eof => Ok(Program { statements }),
        BlockEnd::End { line, column } => Err(error(line, column, "unexpected 'end'")),
        BlockEnd::Else { line, column } => Err(error(line, column, "unexpected 'else'")),
    }
}

fn parse_block(
    lines: &[&str],
    index: &mut usize,
    allow_else: bool,
) -> Result<(Vec<Statement>, BlockEnd), ParseError> {
    let mut statements = Vec::new();
    while *index < lines.len() {
        let line_number = *index + 1;
        let raw = lines[*index];
        let trimmed = raw.trim();
        let column = raw.find(|ch: char| !ch.is_whitespace()).unwrap_or(0) + 1;
        *index += 1;

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed == "end" {
            return Ok((
                statements,
                BlockEnd::End {
                    line: line_number,
                    column,
                },
            ));
        }
        if trimmed == "else" {
            if allow_else {
                return Ok((
                    statements,
                    BlockEnd::Else {
                        line: line_number,
                        column,
                    },
                ));
            }
            return Err(error(line_number, column, "unexpected 'else'"));
        }

        let (keyword, rest) = split_keyword(trimmed);
        let argument_column = column + keyword.len() + 1;
        let statement = match keyword {
            "connect" => {
                if rest.trim().is_empty() {
                    return Err(error(
                        line_number,
                        argument_column,
                        "connect target is required",
                    ));
                }
                let (target_raw, alias_raw) = split_alias_clause(rest);
                let target = parse_text(target_raw, line_number, argument_column)?;
                if target.trim().is_empty() {
                    return Err(error(
                        line_number,
                        argument_column,
                        "connect target is required",
                    ));
                }
                let alias = alias_raw
                    .map(|value| {
                        let value = value.trim();
                        validate_alias(value, line_number, argument_column + target_raw.len() + 4)?;
                        Ok(value.to_string())
                    })
                    .transpose()?;
                Statement::Connect {
                    target,
                    alias,
                    line: line_number,
                }
            }
            "use" => {
                let alias = rest.trim();
                validate_alias(alias, line_number, argument_column)?;
                Statement::Use {
                    alias: alias.to_string(),
                    line: line_number,
                }
            }
            "send" | "sendln" => Statement::Send {
                value: parse_value(rest, line_number, argument_column)?,
                newline: keyword == "sendln",
                line: line_number,
            },
            "wait" => {
                let literal = parse_text(rest, line_number, argument_column)?;
                if literal.is_empty() {
                    return Err(error(
                        line_number,
                        argument_column,
                        "wait value cannot be empty",
                    ));
                }
                Statement::Wait {
                    matcher: WaitMatcher::Literal(literal),
                    line: line_number,
                }
            }
            "wait_regex" => {
                let pattern = parse_text(rest, line_number, argument_column)?;
                if pattern.is_empty() {
                    return Err(error(
                        line_number,
                        argument_column,
                        "wait_regex pattern cannot be empty",
                    ));
                }
                let regex = Regex::new(&pattern).map_err(|err| {
                    error(
                        line_number,
                        argument_column,
                        format!("invalid regular expression: {err}"),
                    )
                })?;
                Statement::Wait {
                    matcher: WaitMatcher::Regex(regex),
                    line: line_number,
                }
            }
            "timeout" => Statement::Timeout {
                duration: parse_duration(rest, line_number, argument_column)?,
                line: line_number,
            },
            "if" => {
                let condition = match rest.trim() {
                    "matched" => WaitCondition::Matched,
                    "timeout" => WaitCondition::Timeout,
                    _ => {
                        return Err(error(
                            line_number,
                            argument_column,
                            "if condition must be 'matched' or 'timeout'",
                        ));
                    }
                };
                let (then_body, end) = parse_block(lines, index, true)?;
                let else_body = match end {
                    BlockEnd::Else { .. } => {
                        let (body, end) = parse_block(lines, index, false)?;
                        if !matches!(end, BlockEnd::End { .. }) {
                            return Err(error(line_number, column, "if block is missing 'end'"));
                        }
                        body
                    }
                    BlockEnd::End { .. } => Vec::new(),
                    BlockEnd::Eof => {
                        return Err(error(line_number, column, "if block is missing 'end'"));
                    }
                };
                Statement::If {
                    condition,
                    then_body,
                    else_body,
                    line: line_number,
                }
            }
            "repeat" => {
                let count = rest.trim().parse::<u32>().map_err(|_| {
                    error(
                        line_number,
                        argument_column,
                        "repeat count must be a positive integer",
                    )
                })?;
                if count == 0 || count > MAX_REPEAT_COUNT {
                    return Err(error(
                        line_number,
                        argument_column,
                        format!("repeat count must be between 1 and {MAX_REPEAT_COUNT}"),
                    ));
                }
                let (body, end) = parse_block(lines, index, false)?;
                if !matches!(end, BlockEnd::End { .. }) {
                    return Err(error(line_number, column, "repeat block is missing 'end'"));
                }
                Statement::Repeat {
                    count,
                    body,
                    line: line_number,
                }
            }
            "log" => {
                if rest.trim().starts_with("secret(") {
                    return Err(error(
                        line_number,
                        argument_column,
                        "log does not accept secret() values",
                    ));
                }
                Statement::Log {
                    message: parse_text(rest, line_number, argument_column)?,
                    line: line_number,
                }
            }
            _ => {
                return Err(error(
                    line_number,
                    column,
                    format!("unknown command '{keyword}'"),
                ));
            }
        };
        statements.push(statement);
    }
    Ok((statements, BlockEnd::Eof))
}

fn split_keyword(line: &str) -> (&str, &str) {
    match line.find(char::is_whitespace) {
        Some(index) => (&line[..index], line[index..].trim_start()),
        None => (line, ""),
    }
}

fn split_alias_clause(input: &str) -> (&str, Option<&str>) {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let bytes = input.as_bytes();
    let mut index = 0;
    while index + 1 < bytes.len() {
        let ch = input[index..].chars().next().unwrap();
        if escaped {
            escaped = false;
            index += ch.len_utf8();
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            index += 1;
            continue;
        }
        if matches!(ch, '\'' | '"') {
            if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(ch);
            }
            index += ch.len_utf8();
            continue;
        }
        if quote.is_none()
            && input[index..].starts_with("as")
            && index > 0
            && input[..index]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
            && input[index + 2..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
        {
            return (
                input[..index].trim_end(),
                Some(input[index + 2..].trim_start()),
            );
        }
        index += ch.len_utf8();
    }
    (input.trim(), None)
}

fn parse_value(input: &str, line: usize, column: usize) -> Result<Value, ParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(error(line, column, "value is required"));
    }
    if let Some(inner) = input
        .strip_prefix("secret(")
        .and_then(|value| value.strip_suffix(')'))
    {
        let reference = parse_text(inner, line, column + "secret(".len())?;
        if reference.trim().is_empty() {
            return Err(error(line, column, "secret reference cannot be empty"));
        }
        return Ok(Value::Secret(reference));
    }
    Ok(Value::Literal(parse_text(input, line, column)?))
}

fn parse_text(input: &str, line: usize, column: usize) -> Result<String, ParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(String::new());
    }
    let Some(quote) = input.chars().next().filter(|ch| matches!(ch, '\'' | '"')) else {
        return Ok(input.to_string());
    };

    let mut output = String::new();
    let mut escaped = false;
    let mut closed_at = None;
    for (offset, ch) in input[quote.len_utf8()..].char_indices() {
        let absolute = quote.len_utf8() + offset;
        if escaped {
            match ch {
                'n' => output.push('\n'),
                'r' => output.push('\r'),
                't' => output.push('\t'),
                '\\' => output.push('\\'),
                '\'' => output.push('\''),
                '"' => output.push('"'),
                other => {
                    output.push('\\');
                    output.push(other);
                }
            }
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != '\'' {
            escaped = true;
            continue;
        }
        if ch == quote {
            closed_at = Some(absolute + ch.len_utf8());
            break;
        }
        output.push(ch);
    }
    let Some(closed_at) = closed_at else {
        return Err(error(line, column, "unterminated quoted string"));
    };
    if !input[closed_at..].trim().is_empty() {
        return Err(error(
            line,
            column + closed_at,
            "unexpected text after quoted string",
        ));
    }
    Ok(output)
}

fn parse_duration(input: &str, line: usize, column: usize) -> Result<Duration, ParseError> {
    let input = input.trim();
    let (number, multiplier) = if let Some(value) = input.strip_suffix("ms") {
        (value, 1_u64)
    } else if let Some(value) = input.strip_suffix('s') {
        (value, 1_000)
    } else if let Some(value) = input.strip_suffix('m') {
        (value, 60_000)
    } else {
        (input, 1_000)
    };
    let amount = number
        .trim()
        .parse::<u64>()
        .map_err(|_| error(line, column, "invalid timeout duration"))?;
    let millis = amount
        .checked_mul(multiplier)
        .ok_or_else(|| error(line, column, "timeout duration is too large"))?;
    if millis == 0 || millis > 24 * 60 * 60 * 1_000 {
        return Err(error(
            line,
            column,
            "timeout duration must be between 1ms and 24h",
        ));
    }
    Ok(Duration::from_millis(millis))
}

fn validate_alias(alias: &str, line: usize, column: usize) -> Result<(), ParseError> {
    let mut chars = alias.chars();
    let Some(first) = chars.next() else {
        return Err(error(line, column, "alias is required"));
    };
    if !(first.is_ascii_alphabetic() || first == '_')
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    {
        return Err(error(
            line,
            column,
            "alias must start with a letter or '_' and contain only letters, digits, '_' or '-'",
        ));
    }
    Ok(())
}

fn error(line: usize, column: usize, message: impl Into<String>) -> ParseError {
    ParseError {
        line,
        column,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Statement, WaitCondition, WaitMatcher, parse};

    #[test]
    fn nyascript_parser_builds_control_flow_and_connect_targets() {
        let program = parse(
            r#"
connect ssh user@example.com -p 2222 as remote
timeout 250ms
repeat 2
  sendln "show status"
  wait_regex "ready\s*>"
  if matched
    log "ready"
  else
    log "timed out"
  end
end
"#,
        )
        .unwrap();

        assert_eq!(program.statements.len(), 3);
        let Statement::Connect { target, alias, .. } = &program.statements[0] else {
            panic!("expected connect");
        };
        assert_eq!(target, "ssh user@example.com -p 2222");
        assert_eq!(alias.as_deref(), Some("remote"));
        let Statement::Repeat { count, body, .. } = &program.statements[2] else {
            panic!("expected repeat");
        };
        assert_eq!(*count, 2);
        assert!(matches!(
            body[1],
            Statement::Wait {
                matcher: WaitMatcher::Regex(_),
                ..
            }
        ));
        assert!(matches!(
            body[2],
            Statement::If {
                condition: WaitCondition::Matched,
                ..
            }
        ));
    }

    #[test]
    fn nyascript_parser_reports_line_and_column_for_invalid_regex() {
        let error = parse("sendln \"hi\"\n  wait_regex \"[\"").unwrap_err();
        assert_eq!(error.line, 2);
        assert!(error.column >= 3);
        assert!(error.to_string().contains("invalid regular expression"));
    }

    #[test]
    fn nyascript_parser_preserves_regex_backslash_escapes() {
        let program = parse(r#"wait_regex "ready\s*>""#).unwrap();
        let Statement::Wait {
            matcher: WaitMatcher::Regex(regex),
            ..
        } = &program.statements[0]
        else {
            panic!("expected regex wait");
        };

        assert!(regex.is_match("ready \t>"));
        assert!(!regex.is_match("readys>"));
    }

    #[test]
    fn nyascript_parser_rejects_unbounded_or_unknown_constructs() {
        let error = parse("while matched\nend").unwrap_err();
        assert_eq!(error.line, 1);
        assert!(error.message.contains("unknown command"));
        assert!(parse("repeat 0\nend").is_err());
    }
}
