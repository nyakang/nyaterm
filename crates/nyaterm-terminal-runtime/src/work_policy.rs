/// Pure presentation state for deciding terminal data-plane and paint work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalPresentation {
    VisibleActive,
    VisibleInactive,
    Background,
}

impl TerminalPresentation {
    pub fn resolve(is_active: bool, is_visible: bool) -> Self {
        match (is_active, is_visible) {
            (true, true) => Self::VisibleActive,
            (false, true) => Self::VisibleInactive,
            (_, false) => Self::Background,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalWorkPolicy {
    pub parse_output: bool,
    pub live_snapshot: bool,
    pub surface_notify: bool,
    pub active_decorations: bool,
}

impl TerminalWorkPolicy {
    pub const fn for_presentation(presentation: TerminalPresentation) -> Self {
        let visible = matches!(
            presentation,
            TerminalPresentation::VisibleActive | TerminalPresentation::VisibleInactive
        );
        Self {
            parse_output: true,
            live_snapshot: visible,
            surface_notify: visible,
            active_decorations: matches!(presentation, TerminalPresentation::VisibleActive),
        }
    }
}
