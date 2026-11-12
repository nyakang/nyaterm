/// Lightweight GFM-ish blocks for AI transcript (closer to Tauri MarkdownContent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkdownBlock {
    Paragraph(String),
    Bullet(String),
    Numbered {
        index: u32,
        text: String,
    },
    Code {
        language: String,
        code: String,
    },
    Quote(String),
    Heading {
        level: u8,
        text: String,
    },
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    ThematicBreak,
}

/// Inline style span after markdown markers are stripped (byte ranges into `text`).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineMdStyle {
    Bold,
    Italic,
    BoldItalic,
    Code,
    Link,
    Strike,
    Underline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineMarkdown {
    pub text: String,
    pub highlights: Vec<(std::ops::Range<usize>, InlineMdStyle)>,
}

fn is_table_separator_row(line: &str) -> bool {
    let cells = split_table_row(line);
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let t = cell.trim();
            !t.is_empty() && t.chars().all(|ch| matches!(ch, '-' | ':' | ' ')) && t.contains('-')
        })
}

fn split_table_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    if !trimmed.contains('|') {
        return Vec::new();
    }
    let body = trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed)
        .strip_suffix('|')
        .unwrap_or(trimmed);
    body.split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn looks_like_table_header(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.contains('|') && split_table_row(trimmed).len() >= 2
}

fn is_thematic_break(line: &str) -> bool {
    let t = line.trim();
    if t.len() < 3 {
        return false;
    }
    let chars: Vec<char> = t.chars().collect();
    let first = chars[0];
    if !matches!(first, '-' | '*' | '_') {
        return false;
    }
    chars.iter().all(|ch| *ch == first || ch.is_whitespace())
        && chars.iter().filter(|ch| **ch == first).count() >= 3
}

/// Parse common GFM-ish inline markers into plain text + highlight ranges.
pub fn parse_inline_markdown(input: &str) -> InlineMarkdown {
    let bytes = input.as_bytes();
    let mut text = String::new();
    let mut highlights = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        // Markdown permits inline HTML; Notes emits the conservative <u> tag
        // because CommonMark has no underline delimiter.
        if input[i..].starts_with("<u>")
            && let Some(end) = input[i + 3..].find("</u>")
        {
            let inner = &input[i + 3..i + 3 + end];
            if !inner.is_empty() && !inner.contains('\n') {
                let start = text.len();
                text.push_str(inner);
                highlights.push((start..text.len(), InlineMdStyle::Underline));
                i = i + 3 + end + 4;
                continue;
            }
        }

        // Fenced-style inline code: `code`
        if bytes[i] == b'`'
            && let Some(end) = input[i + 1..].find('`')
        {
            let inner = &input[i + 1..i + 1 + end];
            if !inner.is_empty() && !inner.contains('\n') {
                let start = text.len();
                text.push_str(inner);
                highlights.push((start..text.len(), InlineMdStyle::Code));
                i = i + 1 + end + 1;
                continue;
            }
        }

        // Links: [label](url)
        if bytes[i] == b'['
            && let Some(label_end) = input[i + 1..].find(']')
        {
            let after_label = i + 1 + label_end + 1;
            if input[after_label..].starts_with('(')
                && let Some(url_end) = input[after_label + 1..].find(')')
            {
                let label = &input[i + 1..i + 1 + label_end];
                let start = text.len();
                text.push_str(label);
                highlights.push((start..text.len(), InlineMdStyle::Link));
                i = after_label + 1 + url_end + 1;
                continue;
            }
        }

        // Bold / bold-italic / italic with * or _
        if bytes[i] == b'*' || bytes[i] == b'_' {
            let marker = bytes[i] as char;
            let rest = &input[i..];
            if rest.starts_with("***") || rest.starts_with("___") {
                let close = format!("{marker}{marker}{marker}");
                if let Some(end) = input[i + 3..].find(&close) {
                    let inner = &input[i + 3..i + 3 + end];
                    if !inner.is_empty() && !inner.contains('\n') {
                        let start = text.len();
                        text.push_str(inner);
                        highlights.push((start..text.len(), InlineMdStyle::BoldItalic));
                        i = i + 3 + end + 3;
                        continue;
                    }
                }
            }
            if rest.starts_with("**") || rest.starts_with("__") {
                let close = format!("{marker}{marker}");
                if let Some(end) = input[i + 2..].find(&close) {
                    let inner = &input[i + 2..i + 2 + end];
                    if !inner.is_empty() && !inner.contains('\n') {
                        let start = text.len();
                        text.push_str(inner);
                        highlights.push((start..text.len(), InlineMdStyle::Bold));
                        i = i + 2 + end + 2;
                        continue;
                    }
                }
            }
            // Single marker italic; avoid matching mid-word underscores when possible.
            if let Some(end) = input[i + 1..].find(marker) {
                let inner = &input[i + 1..i + 1 + end];
                let ok = !inner.is_empty()
                    && !inner.contains('\n')
                    && (marker != '_'
                        || (!inner
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_alphanumeric())
                            || i == 0
                            || !input[..i]
                                .chars()
                                .next_back()
                                .is_some_and(|c| c.is_ascii_alphanumeric())));
                // Simpler italic rule: single * always; single _ when not mid-word.
                let mid_word_underscore = marker == '_'
                    && i > 0
                    && input[..i]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_ascii_alphanumeric())
                    && inner
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphanumeric());
                if ok && !mid_word_underscore {
                    let start = text.len();
                    text.push_str(inner);
                    highlights.push((start..text.len(), InlineMdStyle::Italic));
                    i = i + 1 + end + 1;
                    continue;
                }
            }
        }

        // Strikethrough: ~~text~~
        if input[i..].starts_with("~~")
            && let Some(end) = input[i + 2..].find("~~")
        {
            let inner = &input[i + 2..i + 2 + end];
            if !inner.is_empty() && !inner.contains('\n') {
                let start = text.len();
                text.push_str(inner);
                highlights.push((start..text.len(), InlineMdStyle::Strike));
                i = i + 2 + end + 2;
                continue;
            }
        }

        let ch = input[i..].chars().next().unwrap();
        text.push(ch);
        i += ch.len_utf8();
    }

    InlineMarkdown { text, highlights }
}

pub fn parse_markdown_blocks(content: &str) -> Vec<MarkdownBlock> {
    let mut blocks = Vec::new();
    let mut lines = content.lines().peekable();
    let mut paragraph: Vec<String> = Vec::new();

    let flush_paragraph = |paragraph: &mut Vec<String>, blocks: &mut Vec<MarkdownBlock>| {
        if paragraph.is_empty() {
            return;
        }
        let text = paragraph.join(" ").trim().to_string();
        paragraph.clear();
        if !text.is_empty() {
            blocks.push(MarkdownBlock::Paragraph(text));
        }
    };

    while let Some(line) = lines.next() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            flush_paragraph(&mut paragraph, &mut blocks);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("```") {
            flush_paragraph(&mut paragraph, &mut blocks);
            let language = rest.trim().to_string();
            let mut code_lines = Vec::new();
            for code_line in lines.by_ref() {
                if code_line.trim_start().starts_with("```") {
                    break;
                }
                code_lines.push(code_line.to_string());
            }
            blocks.push(MarkdownBlock::Code {
                language,
                code: code_lines.join("\n"),
            });
            continue;
        }

        // GFM pipe table: header + separator + body rows.
        if looks_like_table_header(trimmed)
            && let Some(next) = lines.peek().copied()
            && is_table_separator_row(next)
        {
            flush_paragraph(&mut paragraph, &mut blocks);
            let headers = split_table_row(trimmed);
            lines.next(); // consume separator
            let mut rows = Vec::new();
            while let Some(body) = lines.peek().copied() {
                if body.trim().is_empty() || !body.trim().contains('|') {
                    break;
                }
                if body.trim_start().starts_with("```")
                    || body.trim_start().starts_with('#')
                    || body.trim_start().starts_with('>')
                {
                    break;
                }
                rows.push(split_table_row(body));
                lines.next();
            }
            blocks.push(MarkdownBlock::Table { headers, rows });
            continue;
        }

        if is_thematic_break(trimmed) {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(MarkdownBlock::ThematicBreak);
            continue;
        }

        if trimmed.trim_start().starts_with('>') {
            flush_paragraph(&mut paragraph, &mut blocks);
            let mut quote_lines = Vec::new();
            let first = trimmed
                .trim_start()
                .strip_prefix('>')
                .map(|s| s.strip_prefix(' ').unwrap_or(s))
                .unwrap_or("")
                .to_string();
            quote_lines.push(first);
            while let Some(next) = lines.peek().copied() {
                let nt = next.trim_end();
                if nt.trim_start().starts_with('>') {
                    let part = nt
                        .trim_start()
                        .strip_prefix('>')
                        .map(|s| s.strip_prefix(' ').unwrap_or(s))
                        .unwrap_or("")
                        .to_string();
                    quote_lines.push(part);
                    lines.next();
                } else {
                    break;
                }
            }
            blocks.push(MarkdownBlock::Quote(quote_lines.join("\n")));
            continue;
        }

        let heading_level = trimmed.chars().take_while(|ch| *ch == '#').count().min(6) as u8;
        if heading_level > 0 && trimmed.as_bytes().get(heading_level as usize) == Some(&b' ') {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(MarkdownBlock::Heading {
                level: heading_level,
                text: trimmed[heading_level as usize + 1..].trim().to_string(),
            });
            continue;
        }
        let bullet = trimmed.trim_start();
        if let Some(rest) = bullet
            .strip_prefix("- ")
            .or_else(|| bullet.strip_prefix("* "))
            .or_else(|| bullet.strip_prefix("+ "))
        {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(MarkdownBlock::Bullet(rest.to_string()));
            continue;
        }
        if let Some((num, rest)) = bullet.split_once(". ")
            && !num.is_empty()
            && num.chars().all(|ch| ch.is_ascii_digit())
        {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(MarkdownBlock::Numbered {
                index: num.parse().unwrap_or(1),
                text: rest.to_string(),
            });
            continue;
        }
        paragraph.push(trimmed.trim().to_string());
    }
    flush_paragraph(&mut paragraph, &mut blocks);
    if blocks.is_empty() && !content.trim().is_empty() {
        blocks.push(MarkdownBlock::Paragraph(content.trim().to_string()));
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::{InlineMdStyle, MarkdownBlock, parse_inline_markdown, parse_markdown_blocks};
    #[test]
    fn parse_table_and_inline() {
        let md = "\
# Title

Hello **bold** and `code` and [link](https://example.com).

| A | B |
| --- | --- |
| 1 | 2 |
| 3 | 4 |

> quote line 1
> quote line 2

---
";
        let blocks = parse_markdown_blocks(md);
        assert!(matches!(blocks[0], MarkdownBlock::Heading { level: 1, .. }));
        assert!(matches!(blocks[1], MarkdownBlock::Paragraph(_)));
        match &blocks[2] {
            MarkdownBlock::Table { headers, rows } => {
                assert_eq!(headers, &["A".to_string(), "B".to_string()]);
                assert_eq!(rows.len(), 2);
            }
            other => panic!("expected table, got {other:?}"),
        }
        match &blocks[3] {
            MarkdownBlock::Quote(q) => assert_eq!(q, "quote line 1\nquote line 2"),
            other => panic!("expected quote, got {other:?}"),
        }
        assert!(matches!(blocks[4], MarkdownBlock::ThematicBreak));

        let inline = parse_inline_markdown("Hello **bold** and `code` and [link](https://x)");
        assert_eq!(inline.text, "Hello bold and code and link");
        assert!(
            inline
                .highlights
                .iter()
                .any(|(_, s)| *s == InlineMdStyle::Bold)
        );
        assert!(
            inline
                .highlights
                .iter()
                .any(|(_, s)| *s == InlineMdStyle::Code)
        );
        assert!(
            inline
                .highlights
                .iter()
                .any(|(_, s)| *s == InlineMdStyle::Link)
        );
    }
}
