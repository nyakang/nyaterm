//! Screen construction and bounded output processing shared with runtime adapters.
use crate::{TerminalEffects, TerminalOutputDecoder, TerminalScreen};

pub const TERMINAL_OUTPUT_VISIBLE_BACKLOG_CAP: usize = 1_000_000;

pub fn initial_terminal_screen(banner: &str) -> TerminalScreen {
    terminal_screen_from_output(banner)
}

pub fn terminal_screen_from_output(output: &str) -> TerminalScreen {
    let mut screen = TerminalScreen::default();
    screen.advance(output.as_bytes());
    screen
}

pub fn protect_terminal_output_burst<'a>(
    screen: &mut TerminalScreen,
    output_decoder: &mut TerminalOutputDecoder,
    data: &'a [u8],
) -> (&'a [u8], usize) {
    if data.len() <= TERMINAL_OUTPUT_VISIBLE_BACKLOG_CAP {
        return (data, 0);
    }
    let skip = data.len() - TERMINAL_OUTPUT_VISIBLE_BACKLOG_CAP;
    screen.reset_stream_state();
    output_decoder.reset_decoder();
    (&data[skip..], skip)
}

pub fn trim_string_to_tail(output: &mut String, max_bytes: usize) {
    if max_bytes == 0 {
        output.clear();
        return;
    }
    if output.len() <= max_bytes {
        return;
    }
    let min_start = output.len() - max_bytes;
    let drain_to = output
        .char_indices()
        .find_map(|(index, _)| (index >= min_start).then_some(index))
        .unwrap_or(output.len());
    output.drain(..drain_to);
}

pub fn merge_terminal_effects(target: &mut TerminalEffects, mut incoming: TerminalEffects) {
    if incoming.title.is_some() {
        target.title = incoming.title.take();
    }
    target.reset_title |= incoming.reset_title;
    target.bell |= incoming.bell;
    if incoming.cwd.is_some() {
        target.cwd = incoming.cwd.take();
    }
    target.shell_command_started |= incoming.shell_command_started;
    target.shell_command_finished |= incoming.shell_command_finished;
    target.pty_write.append(&mut incoming.pty_write);
    if incoming.clipboard_store.is_some() {
        target.clipboard_store = incoming.clipboard_store.take();
    }
    target.clipboard_loads.append(&mut incoming.clipboard_loads);
}
