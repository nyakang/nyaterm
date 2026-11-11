//! Width tiers for the AI panel, measured from the panel's own bounds.

use gpui::{Pixels, px};

pub(in crate::features) const COMPOSER_HEIGHT_MAX: Pixels = px(480.);
pub(super) const COMPOSER_PADDING: Pixels = px(8.);
pub(super) const COMPOSER_GAP: Pixels = px(8.);

/// Measured presentation geometry, owned only by the mounted AI panel.
#[derive(Clone, Copy, Default)]
pub(super) struct AiComposerLayout {
    pub panel_height: Pixels,
    pub banner_height: Pixels,
    pub footer_height: Pixels,
    pub context_height: Pixels,
    pub prompt_height: Pixels,
}

#[derive(Clone, Copy)]
pub(super) struct AiComposerLimits {
    pub prompt: Pixels,
    pub editable: Pixels,
    pub composer: Pixels,
}

impl AiComposerLayout {
    pub fn limits(self, single_row: Pixels) -> Option<AiComposerLimits> {
        if self.panel_height <= px(0.) || self.footer_height <= px(0.) {
            return None;
        }
        // The handle's 5px hit strip overlaps its neighbours by 2px on each
        // side. The composer adds padding, a border and the footer gap.
        let available = (self.panel_height - self.banner_height - px(1.)).max(px(0.));
        let chrome = self.footer_height + COMPOSER_PADDING * 2. + px(1.) + COMPOSER_GAP;
        let editable_room = (available - chrome).max(px(0.));
        let transcript = (editable_room - self.context_height - single_row).clamp(px(0.), px(80.));
        let editable = (editable_room - transcript).max(px(0.));
        Some(AiComposerLimits {
            // When context alone exceeds the budget, keep one editing row
            // inside the scrolling body while the footer stays outside it.
            prompt: (editable - self.context_height)
                .max(single_row)
                .min(COMPOSER_HEIGHT_MAX),
            editable,
            composer: (available - transcript).max(px(0.)),
        })
    }
}

/// The panel is usually a side panel, so the window viewport says nothing
/// about the room it has. Tiers come from the laid-out panel width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::features) enum AiPanelLayout {
    Compact,
    #[default]
    Standard,
    Wide,
}

impl AiPanelLayout {
    const STANDARD_MIN: Pixels = px(360.);
    const WIDE_MIN: Pixels = px(600.);

    pub(in crate::features) fn from_width(width: Pixels) -> Self {
        if width < Self::STANDARD_MIN {
            Self::Compact
        } else if width < Self::WIDE_MIN {
            Self::Standard
        } else {
            Self::Wide
        }
    }

    pub(in crate::features) fn is_compact(self) -> bool {
        self == Self::Compact
    }
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::AiPanelLayout;

    #[test]
    fn width_tiers_split_at_360_and_600_pixels() {
        assert_eq!(AiPanelLayout::from_width(px(280.)), AiPanelLayout::Compact);
        assert_eq!(AiPanelLayout::from_width(px(309.)), AiPanelLayout::Compact);
        assert_eq!(AiPanelLayout::from_width(px(359.)), AiPanelLayout::Compact);
        assert_eq!(AiPanelLayout::from_width(px(360.)), AiPanelLayout::Standard);
        assert_eq!(AiPanelLayout::from_width(px(599.)), AiPanelLayout::Standard);
        assert_eq!(AiPanelLayout::from_width(px(600.)), AiPanelLayout::Wide);
    }
}
