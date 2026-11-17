//! Host colors used for terminal color queries, independent of presentation frameworks.

use alacritty_terminal::vte::ansi::Rgb;

/// The colors the host paints, expressed as opaque 0xRRGGBB values.
/// Program-set OSC colors take precedence without mutating this palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalColorPalette {
    pub foreground: u32,
    pub background: u32,
    pub cursor: u32,
    pub ansi: [u32; 16],
    pub foreground_intense: Option<u32>,
}

impl Default for TerminalColorPalette {
    fn default() -> Self {
        Self {
            foreground: 0xcccccc,
            background: 0x121212,
            cursor: 0xcccccc,
            ansi: std::array::from_fn(|index| {
                let color = ansi16_rgb(index as u8);
                (u32::from(color.r) << 16) | (u32::from(color.g) << 8) | u32::from(color.b)
            }),
            foreground_intense: None,
        }
    }
}

impl TerminalColorPalette {
    pub(crate) fn resolve(self, index: usize) -> Rgb {
        match index {
            0..=15 => rgb(self.ansi[index]),
            16..=255 => rgb(indexed_color_rgb(index as u8)),
            256 => rgb(self.foreground),
            257 => rgb(self.background),
            258 => rgb(self.cursor),
            259..=266 => dim_rgb(rgb(self.ansi[index - 259])),
            267 => rgb(self.foreground_intense.unwrap_or(self.foreground)),
            268 => dim_rgb(rgb(self.foreground)),
            _ => Rgb { r: 0, g: 0, b: 0 },
        }
    }
}

fn rgb(value: u32) -> Rgb {
    Rgb {
        r: ((value >> 16) & 0xff) as u8,
        g: ((value >> 8) & 0xff) as u8,
        b: (value & 0xff) as u8,
    }
}

fn ansi16_rgb(index: u8) -> Rgb {
    const TABLE: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x00, 0x00),
        (0x00, 0xcd, 0x00),
        (0xcd, 0xcd, 0x00),
        (0x00, 0x00, 0xee),
        (0xcd, 0x00, 0xcd),
        (0x00, 0xcd, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x7f, 0x7f, 0x7f),
        (0xff, 0x00, 0x00),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x5c, 0x5c, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];
    let (r, g, b) = TABLE[index as usize % 16];
    Rgb { r, g, b }
}

fn dim_rgb(color: Rgb) -> Rgb {
    Rgb {
        r: color.r / 2,
        g: color.g / 2,
        b: color.b / 2,
    }
}

pub(crate) fn indexed_color_rgb(index: u8) -> u32 {
    if index < 16 {
        return u32::from(index);
    }
    if index >= 232 {
        let gray = 8 + (u32::from(index) - 232) * 10;
        return (gray << 16) | (gray << 8) | gray;
    }
    let idx = u32::from(index) - 16;
    let r = idx / 36;
    let g = (idx / 6) % 6;
    let b = idx % 6;
    let expand = |v: u32| if v == 0 { 0 } else { 55 + v * 40 };
    (expand(r) << 16) | (expand(g) << 8) | expand(b)
}
