/// Check a growing response without allocating its body or reasoning text.
pub(in crate::features) fn think_content_presence(content: &str) -> (bool, bool) {
    let mut has_visible = false;
    let mut before_last_open = false;
    let mut last_open = false;
    let mut after_open = None;
    let mut feed = |segment: &str| {
        for character in segment.chars() {
            if character == '<' {
                before_last_open = has_visible;
                last_open = true;
                after_open = None;
            } else if last_open && after_open.is_none() {
                after_open = Some(character);
            }
            has_visible |= !character.is_whitespace();
        }
    };
    let mut rest = content;
    let mut has_thought = false;
    while let Some(start) = rest.find("<think>") {
        feed(&rest[..start]);
        let after = &rest[start + 7..];
        if let Some(end) = after.find("</think>") {
            has_thought |= !after[..end].trim().is_empty();
            rest = &after[end + 8..];
        } else {
            has_thought |= !after.trim().is_empty();
            rest = "";
            break;
        }
    }
    feed(rest);
    // Match extraction's incomplete trailing-tag rule across removed think segments.
    let truncated = last_open && (after_open.is_none() || after_open == Some('t'));
    (
        if truncated {
            before_last_open
        } else {
            has_visible
        },
        has_thought,
    )
}

/// Strip `<think>…</think>` segments (Tauri `extractThinkContent`).
pub(in crate::features) fn extract_think_content(content: &str) -> (String, Option<String>) {
    let mut reasoning_parts: Vec<String> = Vec::new();
    let mut visible = String::new();
    let mut rest = content;
    while let Some(start) = rest.find("<think>") {
        visible.push_str(&rest[..start]);
        let after = &rest[start + 7..];
        if let Some(end) = after.find("</think>") {
            let part = after[..end].trim();
            if !part.is_empty() {
                reasoning_parts.push(part.to_string());
            }
            rest = &after[end + 8..];
        } else {
            let trailing = after.trim();
            if !trailing.is_empty() {
                reasoning_parts.push(trailing.to_string());
            }
            rest = "";
            break;
        }
    }
    visible.push_str(rest);
    // Drop incomplete trailing open-tag prefix fragments.
    if let Some(idx) = visible.rfind('<') {
        let tail = &visible[idx..];
        if "<think>".starts_with(tail) || tail == "<" || tail.starts_with("<t") {
            visible.truncate(idx);
        }
    }
    let visible = visible.trim().to_string();
    let reasoning = if reasoning_parts.is_empty() {
        None
    } else {
        Some(reasoning_parts.join("\n\n"))
    };
    (visible, reasoning)
}

#[cfg(test)]
mod tests {
    use super::{extract_think_content, think_content_presence};
    #[test]
    fn extract_think_keeps_visible() {
        let (visible, think) = extract_think_content("hi <think>secret</think> there");
        assert_eq!(visible, "hi  there");
        assert_eq!(think.as_deref(), Some("secret"));
    }

    #[test]
    fn think_presence_matches_extraction_for_streaming_prefixes() {
        for source in [
            "",
            "answer",
            "<",
            "<t",
            "<think>",
            "<think>reason",
            "<think>reason</think>answer",
            "answer<think>reason</think>",
            " <think> </think> ",
            "<think>a</think><think>b</think>",
            "<t<think>a</think>",
            "<t<think>a</think>>",
            "<<think>a</think>t",
            "answer<th",
            "中文<think>推理</think>回答",
            "<think>x</think> <thi",
            "<other>",
        ] {
            for end in source
                .char_indices()
                .map(|(index, _)| index)
                .chain(std::iter::once(source.len()))
            {
                let prefix = &source[..end];
                let (display, reasoning) = extract_think_content(prefix);
                assert_eq!(
                    think_content_presence(prefix),
                    (!display.is_empty(), reasoning.is_some()),
                    "prefix={prefix:?}"
                );
            }
        }
    }
}
