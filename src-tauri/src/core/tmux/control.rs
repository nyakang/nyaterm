//! tmux control mode (`tmux -CC`) line-protocol parsing.
//!
//! The control channel is a line-based protocol: notifications start with `%`,
//! command responses are wrapped in `%begin`/`%end` (or `%error`) blocks, and
//! pane output is delivered as octal-escaped `%output`/`%extended-output`
//! notifications.
//!
//! This module is deliberately transport-agnostic: [`ControlParser`] accepts
//! arbitrary byte chunks and emits complete [`ControlMessage`]s, so callers
//! never have to care about line boundaries.

use serde::Serialize;
use std::collections::VecDeque;

/// A completed control-mode line.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ControlMessage {
    /// `%begin <time> <num> <flags>` — start of a command response block.
    Begin,
    /// `%end <time> <num> <flags>` — successful end of the current block.
    End,
    /// `%error <time> <num> <flags>` — failed end of the current block.
    Error,
    /// `%output %<pane> <octal-escaped data>`
    Output {
        pane: String,
        data: Vec<u8>,
    },
    /// `%extended-output %<pane> <age> <flags> : <data>` (tmux >= 3.2, `-CC`).
    ///
    /// `age` is the millisecond age of the output block on the server; it lets
    /// callers deduplicate notifications that were generated before a
    /// `capture-pane` snapshot but deferred past the enclosing `%end` marker.
    ExtendedOutput {
        pane: String,
        age: u64,
        data: Vec<u8>,
    },
    /// `%layout-change @<window> <layout> <visible-layout> <flags>`
    ///
    /// `visible_layout`/`flags` were added in tmux 3.x; when the window is
    /// zoomed (`flags` contains `Z`) the visible layout collapses to the
    /// zoomed pane while `layout` keeps the full pane tree.
    LayoutChange {
        window: String,
        layout: LayoutCell,
        visible_layout: LayoutCell,
        flags: String,
    },
    WindowAdd {
        window: String,
    },
    WindowClose {
        window: String,
    },
    WindowRenamed {
        window: String,
        name: String,
    },
    /// `%window-pane-changed @<window> %<pane>` — active pane of a window.
    WindowPaneChanged {
        window: String,
        pane: String,
    },
    /// `%pane-exited %<pane>` (tmux >= 3.2a).
    PaneExited {
        pane: String,
    },
    PaneModeChanged {
        pane: String,
    },
    /// `%session-changed $<session> <name>` — our client attached/changed session.
    SessionChanged {
        session: String,
    },
    /// `%session-window-changed $<session> @<window>` — current window changed.
    SessionWindowChanged {
        window: String,
    },
    SessionRenamed {
        session: String,
    },
    SessionsChanged,
    ClientSessionChanged,
    ClientDetached {
        client: String,
    },
    /// `%pause %<pane>` — tmux paused this pane's output (flow control).
    Pause {
        pane: String,
    },
    /// `%continue %<pane>` — the pane resumed after a pause.
    Continue {
        pane: String,
    },
    /// `%exit [reason]` — the control client is exiting.
    Exit {
        reason: Option<String>,
    },
    /// `%config-error <line>` — tmux reported a config problem.
    ConfigError {
        text: String,
    },
    /// `%message <line>` — log message from the server.
    Message {
        text: String,
    },
    /// Notifications we recognize but don't act on (`%subscription-changed`,
    /// `%paste-buffer-*`, `%unlinked-window-*`, ...).
    Ignored,
}

/// One cell of a tmux layout string such as
/// `b25f,158x48,0,0{79x48,0,0,0,78x48,80,0,1}`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LayoutCell {
    pub width: u32,
    pub height: u32,
    pub x: u32,
    pub y: u32,
    /// Present on leaf cells.
    pub pane: Option<u32>,
    /// `true` when children are arranged side by side (`{...}`),
    /// `false` when stacked vertically (`[...]`).
    pub columns: bool,
    pub children: Vec<LayoutCell>,
}

impl LayoutCell {
    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }

    /// All pane ids contained in this layout subtree.
    pub fn pane_ids(&self) -> Vec<u32> {
        if let Some(pane) = self.pane {
            return vec![pane];
        }
        self.children
            .iter()
            .flat_map(LayoutCell::pane_ids)
            .collect()
    }

    /// Look up a pane's cell geometry inside this layout.
    pub fn find_pane(&self, pane: u32) -> Option<&LayoutCell> {
        if self.pane == Some(pane) {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find_pane(pane))
    }
}

/// Incremental line-based parser for the control channel.
#[derive(Default)]
pub(crate) struct ControlParser {
    /// Bytes of the current incomplete line.
    line: Vec<u8>,
    /// Completed lines pending dispatch.
    lines: VecDeque<String>,
}

impl ControlParser {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Feed raw channel bytes; returns complete control lines in order.
    ///
    /// Control output over a PTY may arrive with `\r\n` endings, so a trailing
    /// `\r` is stripped from every completed line.
    pub(crate) fn feed(&mut self, data: &[u8]) -> Vec<String> {
        let mut start = 0;
        for (index, byte) in data.iter().enumerate() {
            if *byte == b'\n' {
                let mut end = index;
                if end > start && data[end - 1] == b'\r' {
                    end -= 1;
                }
                self.line.extend_from_slice(&data[start..end]);
                if let Ok(text) = std::str::from_utf8(&self.line) {
                    self.lines.push_back(text.to_string());
                }
                self.line.clear();
                start = index + 1;
            }
        }
        self.line.extend_from_slice(&data[start..]);
        self.lines.drain(..).collect()
    }

    /// Bytes still buffered mid-line; used to hand leftovers back when the
    /// control session ends and the channel returns to a normal shell.
    pub(crate) fn take_pending(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.line)
    }
}

/// Decode tmux's octal escaping: any byte `b` outside the printable range is
/// emitted as a literal `\` followed by three octal digits.
pub(crate) fn unescape_octal(data: &str) -> Vec<u8> {
    let bytes = data.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..=i + 3].iter().all(u8::is_ascii_digit)
        {
            let value = (bytes[i + 1] - b'0') as u32 * 64
                + (bytes[i + 2] - b'0') as u32 * 8
                + (bytes[i + 3] - b'0') as u32;
            // Octal escapes encode one byte; values > 0o377 are not emitted by
            // tmux, but clamp defensively rather than corrupting the stream.
            out.push(value.min(0xFF) as u8);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

fn id_arg(token: &str) -> Option<String> {
    if token.len() > 1 && matches!(token.as_bytes()[0], b'%' | b'@' | b'$') {
        Some(token.to_string())
    } else {
        None
    }
}

fn rest_after_keyword<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    line.strip_prefix(keyword).map(str::trim_start)
}

/// Parse one complete control line (no trailing newline or `\r`).
pub(crate) fn parse_line(line: &str) -> Option<ControlMessage> {
    let rest = line.strip_prefix('%')?;
    let keyword_end = rest
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-'))
        .unwrap_or(rest.len());
    let keyword = &rest[..keyword_end];

    match keyword {
        "begin" | "end" | "error" => Some(match keyword {
            "begin" => ControlMessage::Begin,
            "end" => ControlMessage::End,
            _ => ControlMessage::Error,
        }),
        "output" => {
            let args = rest_after_keyword(line, "%output")?;
            let (pane, data) = args.split_once(' ')?;
            Some(ControlMessage::Output {
                pane: pane.to_string(),
                data: unescape_octal(data),
            })
        }
        "extended-output" => {
            // `%extended-output %<id> <age> <flags...> : <data>`
            let args = rest_after_keyword(line, "%extended-output")?;
            let (meta, data) = args.split_once(" : ")?;
            let mut meta_parts = meta.split_whitespace();
            let pane = meta_parts.next()?;
            let age = meta_parts.next().and_then(|t| t.parse().ok()).unwrap_or(0);
            Some(ControlMessage::ExtendedOutput {
                pane: pane.to_string(),
                age,
                data: unescape_octal(data),
            })
        }
        "layout-change" => {
            let args = rest_after_keyword(line, "%layout-change")?;
            let mut parts = args.split_whitespace();
            let window = parts.next()?.to_string();
            let layout_token = parts.next()?;
            let (layout, _) = parse_layout(layout_token)?;
            // Older servers omit the trailing fields; the visible layout then
            // equals the full layout.
            let visible_layout = parts
                .next()
                .and_then(|token| parse_layout(token))
                .map(|(cell, _)| cell)
                .unwrap_or_else(|| layout.clone());
            let flags = parts.next().unwrap_or_default().to_string();
            Some(ControlMessage::LayoutChange {
                window,
                layout,
                visible_layout,
                flags,
            })
        }
        "window-add" | "unlinked-window-add" => Some(ControlMessage::WindowAdd {
            window: args_id(line, "%window-add")
                .or_else(|| args_id(line, "%unlinked-window-add"))?,
        }),
        "window-close" | "unlinked-window-close" => Some(ControlMessage::WindowClose {
            window: args_id(line, "%window-close")
                .or_else(|| args_id(line, "%unlinked-window-close"))?,
        }),
        "window-renamed" => {
            let args = rest_after_keyword(line, "%window-renamed")?;
            let (window, name) = args.split_once(' ')?;
            Some(ControlMessage::WindowRenamed {
                window: window.to_string(),
                name: name.to_string(),
            })
        }
        "window-pane-changed" => {
            let args = rest_after_keyword(line, "%window-pane-changed")?;
            let mut parts = args.split_whitespace();
            Some(ControlMessage::WindowPaneChanged {
                window: parts.next()?.to_string(),
                pane: parts.next()?.to_string(),
            })
        }
        "pane-exited" => Some(ControlMessage::PaneExited {
            pane: args_id(line, "%pane-exited")?,
        }),
        "pane-mode-changed" => Some(ControlMessage::PaneModeChanged {
            pane: args_id(line, "%pane-mode-changed")?,
        }),
        "session-changed" => {
            let args = rest_after_keyword(line, "%session-changed")?;
            Some(ControlMessage::SessionChanged {
                session: args.split_whitespace().next()?.to_string(),
            })
        }
        "session-window-changed" => {
            let args = rest_after_keyword(line, "%session-window-changed")?;
            let mut parts = args.split_whitespace();
            let _session = parts.next()?;
            Some(ControlMessage::SessionWindowChanged {
                window: parts.next()?.to_string(),
            })
        }
        "session-renamed" => {
            let args = rest_after_keyword(line, "%session-renamed")?;
            Some(ControlMessage::SessionRenamed {
                session: args.split_whitespace().next()?.to_string(),
            })
        }
        "sessions-changed" => Some(ControlMessage::SessionsChanged),
        "client-session-changed" => Some(ControlMessage::ClientSessionChanged),
        "client-detached" => Some(ControlMessage::ClientDetached {
            client: rest_after_keyword(line, "%client-detached")?.to_string(),
        }),
        "pause" => Some(ControlMessage::Pause {
            pane: args_id(line, "%pause")?,
        }),
        "continue" => Some(ControlMessage::Continue {
            pane: args_id(line, "%continue")?,
        }),
        "exit" => Some(ControlMessage::Exit {
            reason: rest_after_keyword(line, "%exit")
                .filter(|reason| !reason.is_empty())
                .map(str::to_string),
        }),
        "config-error" => Some(ControlMessage::ConfigError {
            text: rest_after_keyword(line, "%config-error")?.to_string(),
        }),
        "message" => Some(ControlMessage::Message {
            text: rest_after_keyword(line, "%message")?.to_string(),
        }),
        _ => {
            if line.starts_with('%') {
                Some(ControlMessage::Ignored)
            } else {
                None
            }
        }
    }
}

fn args_id(line: &str, keyword: &str) -> Option<String> {
    rest_after_keyword(line, keyword)
        .and_then(|args| args.split_whitespace().next().and_then(id_arg))
}

/// True when a completed line at column 0 looks like a control-mode
/// notification. Used to detect `tmux -CC` startup in a normal data stream;
/// the whitelist keeps stray `%` text from false-triggering.
pub(crate) fn is_control_notification(line: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "begin",
        "end",
        "error",
        "exit",
        "output",
        "extended-output",
        "layout-change",
        "pane-exited",
        "pane-mode-changed",
        "window-pane-changed",
        "session-changed",
        "session-window-changed",
        "session-renamed",
        "sessions-changed",
        "window-add",
        "window-close",
        "window-renamed",
        "unlinked-window-add",
        "unlinked-window-close",
        "unlinked-window-renamed",
        "client-detached",
        "client-session-changed",
        "config-error",
        "continue",
        "pause",
        "message",
        "subscription-changed",
        "paste-buffer-changed",
        "paste-buffer-deleted",
    ];

    let Some(rest) = line.strip_prefix('%') else {
        return false;
    };
    let keyword_len = rest
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-'))
        .unwrap_or(rest.len());
    // A bare `%` or a nameless line is not a notification.
    if keyword_len == 0 {
        return false;
    }
    // The keyword must be followed by a space or end the line.
    if keyword_len != rest.len() && rest.as_bytes()[keyword_len] != b' ' {
        return false;
    }
    KEYWORDS.contains(&&rest[..keyword_len])
}

/// Parse a tmux layout cell starting at `pos` in `input`.
///
/// Grammar: `WxH,xoff,yoff` then either `,pane_id` (leaf), `{children}`
/// (children side by side) or `[children]` (children stacked).
fn parse_cell(input: &[u8], pos: usize) -> Option<(LayoutCell, usize)> {
    let (width, mut i) = parse_u32(input, pos)?;
    (i < input.len() && input[i] == b'x').then_some(())?;
    let (height, next) = parse_u32(input, i + 1)?;
    i = next;
    (i < input.len() && input[i] == b',').then_some(())?;
    let (x, next) = parse_u32(input, i + 1)?;
    i = next;
    (i < input.len() && input[i] == b',').then_some(())?;
    let (y, next) = parse_u32(input, i + 1)?;
    i = next;

    let mut cell = LayoutCell {
        width,
        height,
        x,
        y,
        pane: None,
        columns: false,
        children: Vec::new(),
    };

    match input.get(i) {
        Some(b'{' | b'[') => {
            cell.columns = input[i] == b'{';
            i += 1;
            let closer = if cell.columns { b'}' } else { b']' };
            loop {
                let (child, next) = parse_cell(input, i)?;
                cell.children.push(child);
                i = next;
                match input.get(i) {
                    Some(b',') => i += 1,
                    Some(&close) if close == closer => {
                        i += 1;
                        break;
                    }
                    _ => break,
                }
            }
        }
        Some(b',') => {
            i += 1;
            if let Some((pane, next)) = parse_u32(input, i) {
                cell.pane = Some(pane);
                i = next;
            }
        }
        _ => {}
    }
    Some((cell, i))
}

/// Parse a full layout string `checksum,WxH,x,y{...}`.
/// Returns the root cell and the number of bytes consumed.
pub(crate) fn parse_layout(input: &str) -> Option<(LayoutCell, usize)> {
    let bytes = input.as_bytes();
    // Skip the hex checksum prefix.
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
        i += 1;
    }
    (i < bytes.len() && bytes[i] == b',').then_some(())?;
    parse_cell(bytes, i + 1)
}

fn parse_u32(input: &[u8], pos: usize) -> Option<(u32, usize)> {
    let mut i = pos;
    let mut value: u32 = 0;
    let mut seen = false;
    while i < input.len() && input[i].is_ascii_digit() {
        value = value
            .saturating_mul(10)
            .saturating_add(u32::from(input[i] - b'0'));
        seen = true;
        i += 1;
    }
    seen.then_some((value, i))
}

/// The pane tree for one window as reported to the frontend. Mirrors the
/// frontend `PaneNode` shape so it can be applied directly.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum TmuxUiNode {
    Pane {
        #[serde(rename = "paneId")]
        pane_id: String,
        #[serde(rename = "sessionId")]
        session_id: String,
        name: String,
    },
    Split {
        direction: &'static str,
        ratio: f64,
        first: Box<TmuxUiNode>,
        second: Box<TmuxUiNode>,
    },
}

#[cfg(test)]
mod tests {
    use super::{ControlMessage, ControlParser, parse_layout, parse_line, unescape_octal};

    #[test]
    fn octal_unescape_decodes_control_bytes() {
        assert_eq!(unescape_octal("abc"), b"abc");
        assert_eq!(unescape_octal("\\033[31mred"), b"\x1b[31mred");
        assert_eq!(unescape_octal("a\\000b"), vec![b'a', 0, b'b']);
        // A lone backslash without octal digits stays literal.
        assert_eq!(unescape_octal("a\\zb"), b"a\\zb");
        assert_eq!(unescape_octal("\\\\abc"), b"\\\\abc");
    }

    #[test]
    fn parser_splits_crlf_and_partial_lines() {
        let mut parser = ControlParser::new();
        assert!(parser.feed(b"%window-add @1\r\n%la").len() == 1);
        let lines = parser.feed(b"yout-change @1 xx,1x1,0,0,0\nrest");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("%layout-change"));
        // "rest" has no newline yet, stays pending.
        assert_eq!(parser.take_pending(), b"rest");
    }

    #[test]
    fn parses_layout_nested_cells() {
        let (root, _) = parse_layout("b25f,158x48,0,0{79x48,0,0,0,78x48,80,0,1}").unwrap();
        assert_eq!((root.width, root.height), (158, 48));
        assert!(root.columns);
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.children[0].pane, Some(0));
        assert_eq!(root.children[1].x, 80);
    }

    #[test]
    fn parses_stacked_layout() {
        let (root, _) = parse_layout("aaaa,80x24,0,0[80x12,0,0,0,80x11,0,13,1]").unwrap();
        assert!(!root.columns);
        assert_eq!(root.pane_ids(), vec![0, 1]);
        assert_eq!(root.find_pane(1).unwrap().height, 11);
    }

    #[test]
    fn parses_core_notifications() {
        assert_eq!(
            parse_line("%window-add @1"),
            Some(ControlMessage::WindowAdd {
                window: "@1".into()
            })
        );
        assert_eq!(
            parse_line("%window-renamed @2 my window"),
            Some(ControlMessage::WindowRenamed {
                window: "@2".into(),
                name: "my window".into()
            })
        );
        assert_eq!(
            parse_line("%exit"),
            Some(ControlMessage::Exit { reason: None })
        );
        assert_eq!(
            parse_line("%session-window-changed $0 @3"),
            Some(ControlMessage::SessionWindowChanged {
                window: "@3".into()
            })
        );
    }

    #[test]
    fn parses_output_notifications() {
        assert_eq!(
            parse_line("%output %5 hello\\033world"),
            Some(ControlMessage::Output {
                pane: "%5".into(),
                data: b"hello\x1bworld".to_vec()
            })
        );
        assert_eq!(
            parse_line("%extended-output %5 1234 - : data"),
            Some(ControlMessage::ExtendedOutput {
                pane: "%5".into(),
                age: 1234,
                data: b"data".to_vec()
            })
        );
    }

    #[test]
    fn non_control_lines_rejected() {
        assert_eq!(parse_line("regular output"), None);
        assert_eq!(parse_line(""), None);
    }
}
