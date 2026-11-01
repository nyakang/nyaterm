pub(crate) use nyaterm_terminal::{
    TerminalTextCell, terminal_byte_index_for_cell_col, terminal_is_zero_width_mark,
    terminal_text_cells,
};
pub(crate) use nyaterm_terminal_gpui::{
    NyaTerminalElement, NyaTerminalLayoutCache, TerminalBufferMatch, TerminalGridSelection,
    TerminalKeyMode, TerminalKeywordHighlightSnapshot, TerminalKeywordHighlighter,
    TerminalLineDecorations, compile_terminal_keyword_highlighter,
    precompute_terminal_keyword_highlights_for_rows,
    precompute_terminal_keyword_highlights_for_rows_with_stats_and_cancel, terminal_font_features,
    terminal_key_bytes_with_mode, terminal_key_release_bytes_with_mode,
    terminal_keyword_highlight_expanded_rows, terminal_keyword_rules_key,
    terminal_screen_from_output,
};

pub(crate) const INITIAL_TERMINAL_BANNER: &str = "$ nyaterm --native\nGPUI shell initialized.\nStart a local terminal or open a saved connection.\n";

pub(crate) fn initial_terminal_screen() -> nyaterm_terminal::TerminalScreen {
    nyaterm_terminal_gpui::initial_terminal_screen(INITIAL_TERMINAL_BANNER)
}
