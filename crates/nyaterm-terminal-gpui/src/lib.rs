//! GPUI terminal painting helpers (element + line rendering).

use std::sync::{Arc, OnceLock};

use gpui::FontFeatures;

mod types;

mod ansi;
mod element;
mod glyphs;
mod images;
mod input;
mod keywords;
mod paint;
mod semantic;

static TERMINAL_FONT_FEATURES: OnceLock<FontFeatures> = OnceLock::new();

/// Returns the OpenType feature set used by the fixed-width terminal.
///
/// Terminal columns are determined by a character grid, so shaping must disable
/// features that merge characters or otherwise change glyph count during both
/// measurement and painting.
pub fn terminal_font_features() -> FontFeatures {
    TERMINAL_FONT_FEATURES
        .get_or_init(|| {
            FontFeatures(Arc::new(
                [("calt", 0), ("clig", 0), ("liga", 0)]
                    .into_iter()
                    .map(|(tag, value)| (tag.to_string(), value))
                    .collect(),
            ))
        })
        .clone()
}

fn resolve_cell_fg(
    palette: nyaterm_ui::ThemePalette,
    style: nyaterm_terminal::CellStyle,
    bold_default_foreground: bool,
) -> u32 {
    if style.reverse {
        if let Some(rgb) = style.bg_rgb {
            return rgb;
        }
        return style
            .bg
            .map(|index| palette.terminal_ansi_color(index))
            .unwrap_or(palette.terminal_bg);
    }
    if let Some(rgb) = style.fg_rgb {
        return rgb;
    }
    match style.fg {
        Some(index) if style.bold && index < 8 => palette.terminal_ansi_color(index + 8),
        Some(index) => palette.terminal_ansi_color(index),
        None if style.bold && bold_default_foreground => palette.terminal_ansi_color(15),
        None => palette.terminal_fg,
    }
}

fn resolve_cell_bg(
    palette: nyaterm_ui::ThemePalette,
    style: nyaterm_terminal::CellStyle,
) -> Option<u32> {
    if style.reverse {
        if let Some(rgb) = style.fg_rgb {
            return Some(rgb);
        }
        return Some(match style.fg {
            Some(index) if style.bold && index < 8 => palette.terminal_ansi_color(index + 8),
            Some(index) => palette.terminal_ansi_color(index),
            None => palette.terminal_fg,
        });
    }
    style
        .bg_rgb
        .or_else(|| style.bg.map(|index| palette.terminal_ansi_color(index)))
}

pub use element::{
    NyaTerminalElement, NyaTerminalLayoutCache, NyaTerminalLayoutCacheStats, TerminalBufferMatch,
    TerminalGridSelection, TerminalLineDecorations, TerminalSearchFlags,
};
pub use input::{
    TerminalKeyMode, initial_terminal_screen, terminal_key_bytes, terminal_key_bytes_with_mode,
    terminal_key_release_bytes_with_mode, terminal_screen_from_output, trim_terminal_output,
};
pub use keywords::terminal_buffer_matches;
pub use keywords::{
    TerminalKeywordHighlightPrecomputeStats, TerminalKeywordHighlightSnapshot,
    TerminalKeywordHighlighter, compile_terminal_keyword_highlighter,
    precompute_terminal_keyword_highlights, precompute_terminal_keyword_highlights_for_rows,
    precompute_terminal_keyword_highlights_for_rows_with_stats,
    precompute_terminal_keyword_highlights_for_rows_with_stats_and_cancel,
    terminal_keyword_highlight_expanded_rows, terminal_keyword_rules_key,
};

#[cfg(test)]
mod tests;
