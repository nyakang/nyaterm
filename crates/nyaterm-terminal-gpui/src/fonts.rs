//! Terminal font validation and fixed-width measurement.
use crate::terminal_font_features;
use gpui::{Font, FontFallbacks, FontWeight, Pixels, SharedString, font};

/// Families that GPUI resolves to a platform-specific concrete font.
pub fn is_generic_terminal_font_family(family: &str) -> bool {
    let family = family.trim();
    family.eq_ignore_ascii_case("monospace")
        || family.eq_ignore_ascii_case("system-monospace")
        || family.eq_ignore_ascii_case("ui-monospace")
}

/// Resolved terminal font descriptor retaining the primary family and ordered fallbacks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedAppearanceFont {
    pub family: String,
    pub fallbacks: Option<FontFallbacks>,
    /// Ordered family names retained from user configuration so runtime promotion
    /// can rebuild the complete descriptor.
    pub fallback_families: Vec<String>,
}

impl ResolvedAppearanceFont {
    pub fn font(&self) -> Font {
        let mut font = font(SharedString::from(self.family.clone()));
        font.fallbacks = self.fallbacks.clone();
        font
    }

    pub fn with_primary_family(&self, family: &str) -> Self {
        let fallback_families = self
            .fallback_families
            .iter()
            .filter(|candidate| !candidate.eq_ignore_ascii_case(family))
            .cloned()
            .collect::<Vec<_>>();
        let fallbacks = (!fallback_families.is_empty())
            .then(|| FontFallbacks::from_fonts(fallback_families.clone()));
        Self {
            family: family.to_string(),
            fallbacks,
            fallback_families,
        }
    }
}

/// Measured metrics and the concrete family selected by GPUI.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalFontMeasurement {
    pub cell_width: f32,
    pub resolved_family: String,
}

/// Reason why a terminal font could not be validated for fixed-width painting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalFontMeasurementFailure {
    FontNotResolved,
    ResolvedFamilyMismatch,
    MissingGlyphAdvance,
    NotMonospaced,
    ShapingMismatch,
}
/// Terminal font carrying the font features the grid disables, so text shaped
/// against it advances on the grid's cells instead of using ligatures.
pub fn terminal_font_with_features(
    descriptor: &ResolvedAppearanceFont,
    weight: FontWeight,
) -> gpui::Font {
    let mut font = descriptor.font();
    font.features = terminal_font_features();
    font.weight = weight;
    font
}

pub fn measure_terminal_font(
    text_system: &gpui::TextSystem,
    descriptor: &ResolvedAppearanceFont,
    size: Pixels,
    weight: FontWeight,
) -> Result<TerminalFontMeasurement, TerminalFontMeasurementFailure> {
    let font = terminal_font_with_features(descriptor, weight);
    let font_id = text_system.resolve_font(&font);
    let resolved = text_system
        .get_font_for_id(font_id)
        .ok_or(TerminalFontMeasurementFailure::FontNotResolved)?;
    let resolved_family = resolved.family.to_string();

    // TextSystem silently falls back to the global UI font when the requested family is
    // missing. Reject that result before a proportional font reaches the fixed-width
    // terminal painter. Generic families are intentionally resolved to a concrete
    // platform family, so they are validated by their measured widths below.
    if !resolved_family.eq_ignore_ascii_case(font.family.as_str())
        && !is_generic_terminal_font_family(font.family.as_str())
    {
        return Err(TerminalFontMeasurementFailure::ResolvedFamilyMismatch);
    }

    let widths = ['i', 'W', '0', 'm']
        .into_iter()
        .filter_map(|ch| {
            text_system
                .advance(font_id, size, ch)
                .ok()
                .map(|size| size.width)
        })
        .map(f32::from)
        .collect::<Vec<_>>();
    if widths.len() != 4 {
        return Err(TerminalFontMeasurementFailure::MissingGlyphAdvance);
    }
    if widths
        .iter()
        .any(|width| !width.is_finite() || *width <= 1.0)
    {
        return Err(TerminalFontMeasurementFailure::MissingGlyphAdvance);
    }

    let min_width = widths.iter().copied().fold(f32::INFINITY, f32::min);
    let max_width = widths.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if max_width - min_width > (max_width * 0.02).max(0.25) {
        return Err(TerminalFontMeasurementFailure::NotMonospaced);
    }

    // Forced-width layout must agree with the raw glyph advance; a large discrepancy
    // indicates that this platform shaping path is unsuitable for terminal cells.
    let shaped_zero_width = f32::from(text_system.layout_width(font_id, size, '0'));
    if !shaped_zero_width.is_finite()
        || (shaped_zero_width - widths[2]).abs() > (widths[2] * 0.05).max(0.5)
    {
        return Err(TerminalFontMeasurementFailure::ShapingMismatch);
    }

    Ok(TerminalFontMeasurement {
        cell_width: widths[2],
        resolved_family,
    })
}
