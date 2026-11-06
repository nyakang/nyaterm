//! Width tiers for the AI panel, measured from the panel's own bounds.

use gpui::{Pixels, px};

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
