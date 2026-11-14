//! Icon lookup tables.
//!
//! The catalogs resolve assets; [`mono_icon`] and [`color_icon`] construct
//! their GPUI elements. Keeping them apart is what stops a
//! ~1000-line brand table from being rebuilt inside a render closure.
//!
//! The tables are a port of the pre-GPUI `src/components/icons.tsx`. Their keys
//! are **persisted user data** — `SavedConnection::icon` and
//! `QuickCommand::icon_tag` are free-form strings written by both the old and the
//! new app and carried in `.nya` backups — so keys may be added but never
//! repurposed or removed.

mod aliases;
mod connection;
mod file_kind;
mod quick;
mod remote_system;
mod search;

pub use connection::{CONNECTION_ICON_OPTIONS, DEFAULT_CONNECTION_ICON, resolve_connection_icon};
pub use file_kind::file_entry_icon;
pub use quick::{QUICK_COMMAND_ICON_OPTIONS, quick_command_icon};
pub use remote_system::infer_connection_icon_key_from_remote_system;
pub use search::{SEARCH_ENGINE_ICON_IDS, known_search_engine_icon, search_engine_icon};

use crate::theme::ThemePalette;

/// How an icon asset must be painted.
///
/// The two GPUI elements are not interchangeable: `svg()` reduces the asset to an
/// alpha mask (so one glyph serves seven tints, but a multi-color logo is lost),
/// while `img()` keeps the pixels (so a distro logo survives, but nothing can
/// tint it). The path prefix encodes the same split — `icons/**` vs `color/**`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconPaint {
    /// Monochrome mask, tinted with this vendor brand color.
    Mono(u32),
    /// Full-color raster, painted as authored. The old UI rendered these through
    /// an `<img>` whose `color` was inert, so losing the tint is parity, not loss.
    FullColor,
}

/// A resolved icon: which asset, and how to paint it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconDef {
    pub path: &'static str,
    pub paint: IconPaint,
}

impl IconDef {
    pub const fn mono(path: &'static str, color: u32) -> Self {
        Self {
            path,
            paint: IconPaint::Mono(color),
        }
    }

    pub const fn full_color(path: &'static str) -> Self {
        Self {
            path,
            paint: IconPaint::FullColor,
        }
    }

    /// Tint for a monochrome icon, lifted off dark surfaces when needed.
    /// Full-color icons return `None` — they are painted as authored.
    ///
    /// Several published brand colors are near-black (`rust` is literally
    /// `#000000`, `github` `#181717`), which disappears on a dark surface. The old
    /// UI shipped that bug; rather than corrupting the vendor values in the table,
    /// the correction happens here so light themes still get the true color and
    /// any brand added later is covered automatically.
    pub fn tint(self, palette: ThemePalette) -> Option<u32> {
        match self.paint {
            IconPaint::Mono(color) => Some(legible_on(color, palette)),
            IconPaint::FullColor => None,
        }
    }
}

/// Relative luminance (WCAG), used only to decide whether a brand color has
/// collapsed into the background.
fn relative_luminance(color: u32) -> f32 {
    fn channel(value: u32) -> f32 {
        let value = (value & 0xff) as f32 / 255.0;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(color >> 16) + 0.7152 * channel(color >> 8) + 0.0722 * channel(color)
}

fn blend(from: u32, to: u32, amount: f32) -> u32 {
    let mix = |shift: u32| {
        let a = ((from >> shift) & 0xff) as f32;
        let b = ((to >> shift) & 0xff) as f32;
        (a + (b - a) * amount).round().clamp(0.0, 255.0) as u32
    };
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

/// Contrast ratio floor below which a brand color is blended toward the theme's
/// text color. 1.6 is deliberately low: it rescues `#000000` on a dark surface
/// without washing out colors that merely look dim, such as `debian`'s `#a81d33`.
const MIN_CONTRAST_RATIO: f32 = 1.6;

fn legible_on(color: u32, palette: ThemePalette) -> u32 {
    let surface = relative_luminance(palette.surface);
    // Only lift colors on dark themes. Blending bright icons toward dark text
    // on light themes turns the Tauri folder yellow into an olive brown.
    if relative_luminance(palette.text) <= surface {
        return color;
    }
    let icon = relative_luminance(color);
    let (lighter, darker) = if icon > surface {
        (icon, surface)
    } else {
        (surface, icon)
    };
    let ratio = (lighter + 0.05) / (darker + 0.05);
    if ratio >= MIN_CONTRAST_RATIO {
        return color;
    }
    // Pull toward the theme's own text color rather than a fixed white/black, so
    // the lift lands inside the palette instead of next to it.
    blend(color, palette.text, 0.65)
}

#[cfg(test)]
mod tests {
    use super::{
        IconDef, ThemePalette, file_entry_icon, legible_on, relative_luminance,
        resolve_connection_icon,
    };

    fn dark() -> ThemePalette {
        crate::theme::theme_palette("github-dark")
    }

    #[test]
    fn near_black_brand_colors_are_lifted_off_a_dark_surface() {
        let palette = dark();
        // simple-icons publishes Rust as pure black; unlifted it is invisible.
        let lifted = legible_on(0x000000, palette);
        assert_ne!(lifted, 0x000000);
        assert!(relative_luminance(lifted) > relative_luminance(palette.surface));
    }

    #[test]
    fn legible_brand_colors_are_left_alone() {
        let palette = dark();
        for color in [0x2496ed, 0xe95420, 0xfcc624, 0x326ce5] {
            assert_eq!(
                legible_on(color, palette),
                color,
                "{color:#08x} reads fine and must keep its published value"
            );
        }
    }

    #[test]
    fn light_themes_preserve_tauri_folder_and_brand_colors() {
        for theme in [
            "nya-high-contrast-white",
            "solarized-light",
            "github-light",
            "catppuccin-latte",
            "rose-pine-dawn",
            "nord-light",
            "one-light",
            "dracula-light",
        ] {
            let palette = crate::theme::theme_palette(theme);
            for def in [
                file_entry_icon("directory", true, false, palette),
                resolve_connection_icon(Some("folder"), "SSH"),
            ] {
                assert_eq!(def.tint(palette), Some(0xfbbf24), "{theme} folder");
            }
            for color in [0x000000, 0x181717, 0xfcc624, 0x67e8f9] {
                assert_eq!(legible_on(color, palette), color, "{theme} {color:#08x}");
            }
        }
    }

    #[test]
    fn full_color_icons_report_no_tint() {
        assert_eq!(
            IconDef::full_color("color/os/ubuntu.svg").tint(dark()),
            None
        );
    }
}

mod render;
pub use render::{
    activity_icon, color_icon, connection_spinner, connection_type_icon, mono_icon,
    nyaterm_app_icon, nyaterm_logo_mark, themed_icon, transfer_entry_icon,
};
