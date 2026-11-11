use gpui::{
    AppContext as _, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div,
    prelude::*, px, rgb,
};
use nyaterm_ui::text::{NyaTextFormat, NyaTextState, NyaTextStyle};
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaDialogHandle, NyaDialogWindowExt as _, NyaSelectableText,
};
use rust_i18n::t;
use tokio_util::sync::CancellationToken;

use super::McpApprovalDecision;
use crate::features::NyaTermApp;
use crate::features::shell::gpui_code_font_family;

#[derive(Default)]
pub(super) struct McpApprovalPresentation {
    current: Option<PresentedApproval>,
    reconcile_scheduled: bool,
}

struct PresentedApproval {
    request_id: String,
    handle: NyaDialogHandle,
    text: NyaTextState,
    // A client may reuse its request ID immediately after cancellation. Retain
    // the displayed request's liveness so that cannot authorize its replacement.
    cancellation: CancellationToken,
}

/// The queue owns authorization. These views only observe it and render either
/// the scrolling body or the fixed footer of the same tracked dialog.
struct ApprovalPart {
    app: Entity<NyaTermApp>,
    request_id: String,
    footer: bool,
    _subscription: Subscription,
}

impl ApprovalPart {
    fn new(
        app: Entity<NyaTermApp>,
        request_id: String,
        footer: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.observe(&app, |_, _, cx| cx.notify());
        Self {
            app,
            request_id,
            footer,
            _subscription: subscription,
        }
    }
}

impl Render for ApprovalPart {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.app
            .update(cx, |app, cx| app.schedule_mcp_approval_dialog(window, cx));
        if self.footer {
            return self.app.update(cx, |app, cx| {
                app.mcp_approval_footer(&self.app, &self.request_id, window, cx)
                    .into_any_element()
            });
        }
        self.app
            .read(cx)
            .mcp_approval_body(&self.request_id)
            .into_any_element()
    }
}

impl NyaTermApp {
    fn mcp_approval_is_blocked(&self) -> bool {
        self.security.screen_locked()
            || self.session.prompt_has_active_ssh_auth()
            || self.modal_child_window_is_open_or_pending()
    }

    pub(in crate::features) fn schedule_mcp_approval_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mcp.approval_dialog.reconcile_scheduled
            || (self.mcp.approval_dialog.current.is_none()
                && self.mcp_pending_approval_requests().is_empty())
        {
            return;
        }
        self.mcp.approval_dialog.reconcile_scheduled = true;
        let app = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            let _ = app.update(cx, |app, cx| {
                app.mcp.approval_dialog.reconcile_scheduled = false;
                app.reconcile_mcp_approval_dialog(window, cx);
            });
        });
    }

    fn reconcile_mcp_approval_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let blocked = self.mcp_approval_is_blocked();
        if let Some(current) = &self.mcp.approval_dialog.current {
            let active = current.handle.is_active();
            let valid = self
                .mcp
                .capabilities
                .external_approval_request(&current.request_id)
                .is_some();
            if active && valid && !current.cancellation.is_cancelled() && !blocked {
                return;
            }
            if active {
                // Never close another feature's dialog, even when our request
                // expires while covered by a newer dialog.
                if !current.handle.is_top(window, cx) {
                    return;
                }
                window.close_nya_dialog(cx);
            }
            if let Some(current) = self.mcp.approval_dialog.current.take() {
                current.text.invalidate_selection(cx);
            }
            cx.notify();
        }
        if blocked || window.has_active_nya_dialog(cx) {
            return;
        }
        let Some(request) = self.mcp_pending_approval_requests().into_iter().next() else {
            return;
        };
        let Some(view) = self
            .mcp
            .capabilities
            .external_approval_view(&request.request_id)
        else {
            return;
        };
        let text = NyaTextState::new(
            NyaTextFormat::Plain,
            view.command
                .as_deref()
                .unwrap_or(&view.approval.parameter_summary),
            cx,
        );
        let app = cx.entity();
        let body =
            cx.new(|cx| ApprovalPart::new(app.clone(), request.request_id.clone(), false, cx));
        let footer =
            cx.new(|cx| ApprovalPart::new(app.clone(), request.request_id.clone(), true, cx));
        let handle = window.open_tracked_nya_dialog(cx, move |dialog, _, _| {
            dialog
                .title(t!("ai.mcp.approvalTitle"))
                .width(720.)
                .overlay_closable(false)
                .keyboard(false)
                .close_button(false)
                // Unfocused Confirm/Cancel actions must never consume a request.
                .on_ok(|_, _, _| false)
                .on_cancel(|_, _, _| false)
                .content(body.clone())
                .footer(footer.clone())
        });
        self.mcp.approval_dialog.current = Some(PresentedApproval {
            request_id: request.request_id,
            handle,
            text,
            cancellation: view.cancellation,
        });
        cx.notify();
    }

    fn decide_presented_mcp_approval(
        &mut self,
        request_id: &str,
        handle: &NyaDialogHandle,
        decision: McpApprovalDecision,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mcp_approval_is_blocked() {
            return;
        }
        let Some(current) = &self.mcp.approval_dialog.current else {
            return;
        };
        if current.request_id != request_id
            || current.cancellation.is_cancelled()
            || !handle.is_top(window, cx)
            || !current.handle.is_top(window, cx)
            || self
                .mcp
                .capabilities
                .external_approval_request(request_id)
                .is_none()
        {
            return;
        }
        self.respond_to_mcp_approval(request_id, decision, cx);
        window.close_nya_dialog(cx);
        if let Some(current) = self.mcp.approval_dialog.current.take() {
            current.text.invalidate_selection(cx);
        }
        // Open the next request only after closing this dialog and restoring
        // the component's focus chain.
        self.schedule_mcp_approval_dialog(window, cx);
        cx.notify();
    }

    fn mcp_approval_body(&self, request_id: &str) -> impl IntoElement {
        let mut body = div()
            .id("mcp-approval-body")
            .debug_selector(|| "mcp-approval-body".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_3();
        let Some(request) = self
            .mcp
            .capabilities
            .external_approval_request(request_id)
            .cloned()
        else {
            return body;
        };
        let Some(current) = self
            .mcp
            .approval_dialog
            .current
            .as_ref()
            .filter(|current| current.request_id == request_id)
        else {
            return body;
        };
        let palette = self.theme_palette();
        let risk: SharedString = match request.risk_level.as_str() {
            "low" => t!("ai.riskLow").into(),
            "medium" => t!("ai.riskMedium").into(),
            "high" => t!("ai.riskHigh").into(),
            "critical" => t!("ai.riskCritical").into(),
            _ => request.risk_level.into(),
        };
        body = body
            .child(detail_row(t!("ai.mcp.client"), request.client))
            .child(detail_row(t!("ai.mcp.capability"), request.capability))
            .when_some(request.target, |body, target| {
                body.child(detail_row(t!("ai.mcp.target"), target))
            })
            .child(
                div()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("ai.mcp.parameters")),
            )
            .child(
                div()
                    .debug_selector(|| "mcp-approval-parameters".into())
                    .w_full()
                    .min_w_0()
                    .p_3()
                    .rounded_md()
                    .bg(rgb(palette.bg))
                    .font_family(gpui_code_font_family())
                    .text_size(px(13.))
                    .child(
                        NyaSelectableText::from_state(&current.text)
                            .style(NyaTextStyle::default().foreground(palette.text)),
                    ),
            )
            .child(detail_row(t!("ai.mcp.risk"), risk));
        if request.destructive {
            body = body.child(
                div()
                    .text_color(rgb(palette.danger))
                    .child(t!("ai.mcp.destructiveWarning")),
            );
        }
        body
    }

    fn mcp_approval_footer(
        &self,
        app: &Entity<Self>,
        request_id: &str,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> impl IntoElement {
        let view = self.mcp.capabilities.external_approval_request(request_id);
        let enabled = view.is_some()
            && !self.mcp_approval_is_blocked()
            && self
                .mcp
                .approval_dialog
                .current
                .as_ref()
                .is_some_and(|current| {
                    current.request_id == request_id
                        && !current.cancellation.is_cancelled()
                        && current.handle.is_top(window, cx)
                });
        let mut footer = div()
            .debug_selector(|| "mcp-approval-footer".into())
            .flex()
            .flex_wrap()
            .justify_end()
            .gap_2();
        let mut choices = vec![
            (
                "deny",
                t!("ai.mcp.deny"),
                McpApprovalDecision::Deny,
                NyaButtonVariant::Danger,
            ),
            (
                "once",
                t!("ai.mcp.allowOnce"),
                McpApprovalDecision::AllowOnce,
                NyaButtonVariant::Secondary,
            ),
        ];
        if view.is_some_and(|request| request.allow_session) {
            choices.push((
                "session",
                t!("ai.mcp.allowSession"),
                McpApprovalDecision::AllowSession,
                NyaButtonVariant::Secondary,
            ));
        }
        for (key, label, decision, variant) in choices {
            let app = app.clone();
            let request_id = request_id.to_string();
            let handle = self
                .mcp
                .approval_dialog
                .current
                .as_ref()
                .map(|current| current.handle.clone());
            let button = NyaButton::new(format!("mcp-approval-{key}-{request_id}"), label)
                .variant(variant)
                .disabled(!enabled)
                .when(key == "deny", NyaButton::autofocus)
                .on_click(move |_, window, cx| {
                    let Some(handle) = &handle else { return };
                    app.update(cx, |app, cx| {
                        app.decide_presented_mcp_approval(
                            &request_id,
                            handle,
                            decision,
                            window,
                            cx,
                        );
                    });
                });
            footer = footer.child(
                div()
                    .debug_selector(move || format!("mcp-approval-{key}"))
                    .child(button),
            );
        }
        footer
    }
}

fn detail_row(label: impl Into<SharedString>, value: impl Into<SharedString>) -> impl IntoElement {
    div()
        .flex()
        .flex_wrap()
        .gap_2()
        .child(div().w(px(112.)).flex_shrink_0().child(label.into()))
        .child(div().flex_1().min_w_0().child(value.into()))
}

#[cfg(test)]
mod tests;
