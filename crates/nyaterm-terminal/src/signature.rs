//! Row change fingerprints.
//!
//! A signature only answers "did this row's rendered content change?", so it
//! lives in memory and is never persisted; its value may change between
//! releases. It is computed for every row that scrolls past, so cost matters:
//! the previous `DefaultHasher` (SipHash) over four separate fields per cell
//! took roughly 85% of the time spent ingesting plain terminal output.
//!
//! Each cell is packed into one word and folded with a 128-bit multiply, which
//! keeps collisions negligible for 64-bit change detection. Trailing cells that
//! are default-styled blanks are left out and the significant length is hashed
//! instead, so a mostly empty 200-column row costs a few cells, not 200.

use crate::CellStyle;

const SEED: u64 = 0x243F_6A88_85A3_08D3;
const MULTIPLIER: u64 = 0x9E37_79B9_7F4A_7C15;
/// Outside the Unicode range, so a blank can never equal a real character.
const BLANK_BASE: u64 = 0x11_0000;
const EXTRA_END: u64 = u64::MAX;

pub(crate) struct RowHasher(u64);

impl RowHasher {
    /// `significant_cells` is the row length after trailing trivial blanks.
    pub(crate) fn new(significant_cells: usize) -> Self {
        let mut hasher = Self(SEED);
        hasher.mix(significant_cells as u64);
        hasher
    }

    #[inline]
    fn mix(&mut self, word: u64) {
        let product = u128::from(self.0 ^ word).wrapping_mul(u128::from(MULTIPLIER));
        self.0 = (product as u64) ^ ((product >> 64) as u64);
    }

    /// `base` is the cell's character, or `None` for a blank. `extra` holds the
    /// zero-width characters combined onto it; they are ignored for blanks.
    #[inline]
    pub(crate) fn cell(
        &mut self,
        base: Option<char>,
        extra: impl Iterator<Item = char>,
        style: &CellStyle,
        width: u8,
        hyperlink: Option<&str>,
    ) {
        let mut extra = extra.peekable();
        let has_extra = base.is_some() && extra.peek().is_some();
        let has_rgb = style.fg_rgb.is_some() || style.bg_rgb.is_some();
        let style_flags = u64::from(style.bold)
            | u64::from(style.reverse) << 1
            | u64::from(style.underline) << 2
            | u64::from(style.strikeout) << 3
            | u64::from(style.italic) << 4
            | u64::from(style.hidden) << 5;
        let word = base.map_or(BLANK_BASE, |ch| u64::from(ch as u32))
            | u64::from(width) << 21
            | style_flags << 25
            | style.fg.map_or(0, |index| u64::from(index) + 1) << 31
            | style.bg.map_or(0, |index| u64::from(index) + 1) << 40
            | u64::from(has_rgb) << 60
            | u64::from(has_extra) << 61
            | u64::from(hyperlink.is_some()) << 62;
        self.mix(word);
        if has_rgb {
            self.mix(style.fg_rgb.map_or(0, |rgb| u64::from(rgb) + 1));
            self.mix(style.bg_rgb.map_or(0, |rgb| u64::from(rgb) + 1));
        }
        if has_extra {
            for ch in extra {
                self.mix(u64::from(ch as u32));
            }
            self.mix(EXTRA_END);
        }
        if let Some(uri) = hyperlink {
            for chunk in uri.as_bytes().chunks(8) {
                let mut bytes = [0_u8; 8];
                bytes[..chunk.len()].copy_from_slice(chunk);
                self.mix(u64::from_le_bytes(bytes));
            }
            self.mix(uri.len() as u64);
        }
    }

    pub(crate) fn finish(self) -> u64 {
        self.0
    }
}

/// A blank, single-width, default-styled, link-free cell. Trailing runs of
/// these are equivalent to the row simply being shorter.
#[inline]
pub(crate) fn is_trivial_blank(
    blank: bool,
    style: &CellStyle,
    width: u8,
    hyperlink: Option<&str>,
) -> bool {
    blank && width == 1 && hyperlink.is_none() && *style == CellStyle::default()
}

#[cfg(test)]
mod tests {
    use super::{RowHasher, is_trivial_blank};
    use crate::CellStyle;

    type TestCell<'a> = (Option<char>, &'a str, CellStyle, u8, Option<&'a str>);

    fn sign(cells: &[TestCell<'_>]) -> u64 {
        let mut hasher = RowHasher::new(cells.len());
        for (base, extra, style, width, link) in cells {
            hasher.cell(*base, extra.chars(), style, *width, *link);
        }
        hasher.finish()
    }

    fn plain(
        ch: char,
    ) -> (
        Option<char>,
        &'static str,
        CellStyle,
        u8,
        Option<&'static str>,
    ) {
        (Some(ch), "", CellStyle::default(), 1, None)
    }

    #[test]
    fn every_rendered_attribute_changes_the_signature() {
        let base = sign(&[plain('a')]);
        let styled = |edit: fn(&mut CellStyle)| {
            let mut style = CellStyle::default();
            edit(&mut style);
            sign(&[(Some('a'), "", style, 1, None)])
        };

        assert_eq!(base, sign(&[plain('a')]));
        assert_ne!(base, sign(&[plain('b')]));
        assert_ne!(base, styled(|s| s.bold = true));
        assert_ne!(base, styled(|s| s.fg = Some(1)));
        assert_ne!(base, styled(|s| s.bg = Some(1)));
        assert_ne!(base, styled(|s| s.fg_rgb = Some(0x00ff00)));
        assert_ne!(
            styled(|s| s.fg_rgb = Some(0x00ff00)),
            styled(|s| s.bg_rgb = Some(0x00ff00))
        );
        assert_ne!(
            base,
            sign(&[(Some('a'), "", CellStyle::default(), 2, None)])
        );
        assert_ne!(
            base,
            sign(&[(Some('a'), "", CellStyle::default(), 1, Some("https://a"))])
        );
        assert_ne!(
            sign(&[(Some('a'), "", CellStyle::default(), 1, Some("https://a"))]),
            sign(&[(Some('a'), "", CellStyle::default(), 1, Some("https://b"))])
        );
        assert_ne!(
            base,
            sign(&[(Some('a'), "\u{301}", CellStyle::default(), 1, None)])
        );
    }

    #[test]
    fn a_blank_cell_never_equals_a_nul_character() {
        let blank = sign(&[(None, "", CellStyle::default(), 1, None)]);

        assert_ne!(blank, sign(&[plain('\0')]));
        assert_ne!(blank, sign(&[plain(' ')]), "space text differs from blank");
    }

    #[test]
    fn row_length_is_part_of_the_signature() {
        assert_ne!(sign(&[plain('a')]), sign(&[plain('a'), plain('a')]));
    }

    #[test]
    fn only_default_single_width_blanks_are_trivial() {
        let default = CellStyle::default();
        let colored = CellStyle {
            bg: Some(4),
            ..CellStyle::default()
        };

        assert!(is_trivial_blank(true, &default, 1, None));
        assert!(!is_trivial_blank(false, &default, 1, None));
        assert!(!is_trivial_blank(true, &colored, 1, None));
        assert!(!is_trivial_blank(true, &default, 0, None), "wide spacer");
        assert!(!is_trivial_blank(true, &default, 1, Some("x")));
    }
}
