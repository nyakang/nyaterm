use nyaterm_mosh::framebuffer::{Attr, Color, ColorType, Framebuffer};
use nyaterm_terminal::prediction::{PredictedCell, TerminalPrediction};
use nyaterm_terminal::{CellStyle, RenderCell};

fn color(color: Color) -> (Option<u8>, Option<u32>) {
    match color.kind {
        ColorType::Default => (None, None),
        ColorType::Index if color.value < 16 => (Some(color.value as u8), None),
        ColorType::Index => {
            let n = color.value.min(255);
            let rgb = if n >= 232 {
                let c = 8 + (n - 232) * 10;
                c * 0x010101
            } else {
                let n = n - 16;
                let component = |v| if v == 0 { 0 } else { 55 + 40 * v };
                component(n / 36) << 16 | component(n / 6 % 6) << 8 | component(n % 6)
            };
            (None, Some(rgb))
        }
        ColorType::Rgb => (None, Some(color.value & 0xffffff)),
    }
}

fn style(attr: Attr) -> CellStyle {
    let (fg, fg_rgb) = color(attr.fg);
    let (bg, bg_rgb) = color(attr.bg);
    CellStyle {
        fg,
        bg,
        fg_rgb,
        bg_rgb,
        bold: attr.bold,
        reverse: attr.reverse,
        underline: attr.under,
        strikeout: attr.strike,
        italic: attr.italic,
        hidden: attr.conceal,
    }
}

pub(super) fn prediction(confirmed: &Framebuffer, displayed: &Framebuffer) -> TerminalPrediction {
    let mut result = TerminalPrediction {
        cols: confirmed.cols,
        rows: confirmed.rows,
        cells: Vec::new(),
        cursor: None,
    };
    if confirmed.cols != displayed.cols || confirmed.rows != displayed.rows {
        return result;
    }
    for row in 0..confirmed.rows {
        for col in 0..confirmed.cols {
            let Some(cell) = displayed.cell_at(col, row) else {
                continue;
            };
            if confirmed.cell_at(col, row) == Some(cell) {
                continue;
            }
            result.cells.push(PredictedCell {
                row,
                col,
                cell: RenderCell {
                    text: cell.text().into(),
                    style: style(cell.attr),
                    width: cell.width,
                    hyperlink: None,
                },
            });
        }
    }
    if (confirmed.cur_x, confirmed.cur_y) != (displayed.cur_x, displayed.cur_y) {
        result.cursor = Some((displayed.cur_y, displayed.cur_x));
    }
    result
}

#[cfg(test)]
mod tests {
    use nyaterm_mosh::{ansi_apply::apply_ansi, framebuffer::Framebuffer};
    use nyaterm_terminal::TerminalScreen;

    #[test]
    fn native_projection_preserves_unicode_and_styles_across_skipped_publications() {
        let mut remote = Framebuffer::new(20, 3);
        let mut screen = TerminalScreen::default();
        screen.resize(20, 3);
        apply_ansi(&mut remote, "你好 💻".as_bytes());
        screen.advance(&remote.diff(None));
        let submitted = remote.clone();
        apply_ansi(&mut remote, b"\x1b[2;1Hskipped");
        apply_ansi(&mut remote, b"\x1b[2;1H\x1b[31mfinal\x1b[K");
        screen.advance(&remote.diff(Some(&submitted)));
        let snapshot = screen.snapshot();
        assert!(snapshot.row_data[0].text.starts_with("你好 💻"));
        assert!(snapshot.row_data[1].text.starts_with("final"));
        assert!(!snapshot.row_data[1].text.contains("skipped"));
        assert_ne!(
            snapshot.row_data[0].cells[0].style.fg,
            snapshot.row_data[1].cells[0].style.fg
        );
    }
}
