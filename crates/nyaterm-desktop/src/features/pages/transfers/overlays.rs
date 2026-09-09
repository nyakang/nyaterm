use rust_i18n::t;

use gpui::{
    App, ClickEvent, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px, rgb,
};
use nyaterm_ui::NyaScrollable;

use crate::features::NyaTermApp;
use crate::models::{TransferJobMenuState, TransferJobStatus};
use crate::theme::ThemePalette;

use super::{transfer_job_can_retry, transfer_job_has_local_target, transfer_menu_position};

impl NyaTermApp {
    pub(in crate::features) fn transfer_job_menu_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let state = self
            .transfer
            .transfer_job_menu()
            .cloned()
            .unwrap_or(TransferJobMenuState {
                job_id: String::new(),
                x: px(24.),
                y: px(24.),
            });
        let job = self.transfer.transfer_job(&state.job_id).cloned();
        let can_pause = job
            .as_ref()
            .is_some_and(|job| job.status == TransferJobStatus::Running && job.control.is_some());
        let can_resume = job
            .as_ref()
            .is_some_and(|job| job.status == TransferJobStatus::Paused && job.control.is_some());
        let can_cancel = job.as_ref().is_some_and(|job| {
            matches!(
                job.status,
                TransferJobStatus::Running | TransferJobStatus::Paused
            ) && job.control.is_some()
        });
        let can_retry = job.as_ref().is_some_and(transfer_job_can_retry);
        let can_open_target = job.as_ref().is_some_and(transfer_job_has_local_target);
        let can_delete = self.can_delete_transfer_job(&state.job_id);
        let pause_id = state.job_id.clone();
        let resume_id = state.job_id.clone();
        let cancel_id = state.job_id.clone();
        let retry_id = state.job_id.clone();
        let open_id = state.job_id.clone();
        let delete_id = state.job_id.clone();
        let (viewport_w, viewport_h) = self.shell.viewport_size();
        let (menu_x, menu_y, menu_max_height) = transfer_menu_position(
            f32::from(state.x),
            f32::from(state.y),
            190.,
            250.,
            viewport_w,
            viewport_h,
        );

        div()
            .id(SharedString::from("transfer-job-menu-overlay"))
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_transfer_job_menu(cx);
            }))
            .child(
                div()
                    .id(SharedString::from("transfer-job-menu"))
                    .absolute()
                    .top(px(menu_y))
                    .left(px(menu_x))
                    .w(px(190.))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(self.shell_surface_color(palette.bg))
                    .shadow_lg()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .max_h(px(menu_max_height))
                            .overflow_y_scrollbar()
                            .p_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(transfer_job_menu_button(
                                palette,
                                "transfer-job-menu-pause",
                                t!("fileTransfer.pause"),
                                can_pause,
                                cx.listener(move |this, _, _, cx| {
                                    this.transfer.close_transfer_job_menu();
                                    this.pause_transfer_job(&pause_id, cx);
                                    this.defer_transfer_panel_snapshot_flush(cx);
                                }),
                            ))
                            .child(transfer_job_menu_button(
                                palette,
                                "transfer-job-menu-resume",
                                t!("fileTransfer.resume"),
                                can_resume,
                                cx.listener(move |this, _, _, cx| {
                                    this.transfer.close_transfer_job_menu();
                                    this.resume_transfer_job(&resume_id, cx);
                                    this.defer_transfer_panel_snapshot_flush(cx);
                                }),
                            ))
                            .child(transfer_job_menu_button(
                                palette,
                                "transfer-job-menu-retry",
                                t!("fileTransfer.retry"),
                                can_retry,
                                cx.listener(move |this, _, window, cx| {
                                    this.transfer.close_transfer_job_menu();
                                    this.retry_transfer_job(retry_id.clone(), window, cx);
                                    this.defer_transfer_panel_snapshot_flush(cx);
                                }),
                            ))
                            .child(transfer_job_menu_button(
                                palette,
                                "transfer-job-menu-cancel",
                                t!("fileTransfer.cancel"),
                                can_cancel,
                                cx.listener(move |this, _, _, cx| {
                                    this.transfer.close_transfer_job_menu();
                                    this.cancel_transfer_job(&cancel_id, cx);
                                    this.defer_transfer_panel_snapshot_flush(cx);
                                }),
                            ))
                            .child(div().h(px(1.)).mx_1().my_1().bg(rgb(palette.border)))
                            .child(transfer_job_menu_button(
                                palette,
                                "transfer-job-menu-open-target",
                                t!("fileTransfer.openTargetDirectory"),
                                can_open_target,
                                cx.listener(move |this, _, _, cx| {
                                    this.transfer.close_transfer_job_menu();
                                    this.reveal_transfer_job_target_directory(open_id.clone(), cx);
                                }),
                            ))
                            .child(div().h(px(1.)).mx_1().my_1().bg(rgb(palette.border)))
                            .child(transfer_job_menu_button(
                                palette,
                                "transfer-job-menu-delete",
                                t!("fileTransfer.delete"),
                                can_delete,
                                cx.listener(move |this, _, window, cx| {
                                    this.transfer.close_transfer_job_menu();
                                    this.request_delete_transfer_job(delete_id.clone(), window, cx);
                                }),
                            )),
                    ),
            )
    }
}

fn transfer_job_menu_button(
    palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<SharedString>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .id(SharedString::from(id.into()))
        .h(px(28.))
        .px_2()
        .flex()
        .items_center()
        .rounded_sm()
        .text_size(px(12.))
        .text_color(if enabled {
            rgb(palette.text)
        } else {
            rgb(palette.text_dimmed)
        })
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(rgb(palette.hover)))
                .on_click(on_click)
        })
        .when(!enabled, |this| this.opacity(0.45))
        .child(label)
}
