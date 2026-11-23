//! One visual frame for every tool call: agent steps, terminal commands, and
//! command cards share the border, padding, and status color.

use gpui::{Div, Styled, div, rgb};
use rust_i18n::t;

use crate::presentation::AiAgentStepView;
use crate::presentation::AiCommandPhase;
use nyaterm_ui::ThemePalette;

/// Chrome title for a terminal tool call, e.g. `Terminal · production-ssh`.
pub fn tool_call_title(target: &str) -> String {
    let kind = t!("ai.terminalCommand");
    if target.trim().is_empty() {
        kind.to_string()
    } else {
        format!("{kind} · {}", target.trim())
    }
}

/// What a tool call looks like, independent of where its data came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiToolCallView {
    pub phase: AiCommandPhase,
}

impl AiToolCallView {
    pub fn from_phase(phase: AiCommandPhase) -> Self {
        Self { phase }
    }

    pub fn from_step(step: &AiAgentStepView) -> Self {
        Self {
            phase: AiCommandPhase::from_step(step),
        }
    }

    pub fn status_color(self, palette: ThemePalette) -> u32 {
        match self.phase {
            AiCommandPhase::NeedsApproval => palette.warning,
            AiCommandPhase::Failed => palette.danger,
            AiCommandPhase::Running | AiCommandPhase::Preparing => palette.link,
            AiCommandPhase::Completed => palette.success,
            _ => palette.text_muted,
        }
    }

    /// An approval is a decision, so its frame is the only one that is tinted.
    fn border_color(self, palette: ThemePalette) -> u32 {
        if self.phase == AiCommandPhase::NeedsApproval {
            palette.warning
        } else {
            palette.border
        }
    }

    pub fn frame(self, palette: ThemePalette) -> Div {
        self.apply_frame(div(), palette)
    }

    pub fn apply_frame<E: Styled>(self, element: E, palette: ThemePalette) -> E {
        element
            .min_w_0()
            .w_full()
            .rounded_md()
            .border_1()
            .border_color(rgb(self.border_color(palette)))
            .bg(rgb(palette.surface))
            .p_2()
    }
}

#[cfg(test)]
mod tests {
    use super::AiToolCallView;
    use crate::presentation::AiCommandPhase;
    use nyaterm_ui::theme_palette;

    #[test]
    fn tool_call_title_joins_the_kind_and_target() {
        let titled = super::tool_call_title("production-ssh");
        assert!(titled.contains("production-ssh"), "{titled}");
        assert_ne!(titled, super::tool_call_title("  "));
    }

    #[test]
    fn only_an_approval_tints_the_tool_call_frame() {
        let palette = theme_palette("github-dark");
        let approval = AiToolCallView::from_phase(AiCommandPhase::NeedsApproval);
        let running = AiToolCallView::from_phase(AiCommandPhase::Running);
        assert_eq!(approval.border_color(palette), palette.warning);
        assert_eq!(running.border_color(palette), palette.border);
        assert_eq!(running.status_color(palette), palette.link);
        assert_eq!(
            AiToolCallView::from_phase(AiCommandPhase::Failed).status_color(palette),
            palette.danger
        );
    }
}
