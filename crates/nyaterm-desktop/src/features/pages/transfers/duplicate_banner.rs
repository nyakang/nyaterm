//! 文件传输重名确认使用窗口级对话框，由后台请求的激活事件直接打开。

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui::{Context, IntoElement, Render, Subscription, Window, div, prelude::*, px, rgb};
use nyaterm_transport::SftpDuplicateDecision;
use nyaterm_ui::NyaDialogWindowExt as _;
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::formatting::download_file_name_from_remote_path;
use crate::features::session::SftpDuplicatePromptState;
use crate::features::view_widgets::bounded_dialog_width;
use crate::theme::ThemePalette;

struct DuplicatePromptDialogContent {
    app: gpui::WeakEntity<NyaTermApp>,
    prompt: SftpDuplicatePromptState,
    palette: ThemePalette,
    answered: Arc<AtomicBool>,
    _release_subscription: Subscription,
}

impl DuplicatePromptDialogContent {
    fn new(
        app: gpui::WeakEntity<NyaTermApp>,
        prompt: SftpDuplicatePromptState,
        palette: ThemePalette,
        answered: Arc<AtomicBool>,
        cx: &mut Context<Self>,
    ) -> Self {
        let release_app = app.clone();
        let release_id = prompt.id.clone();
        let release_answered = answered.clone();
        let release_subscription = cx.on_release(move |_, cx| {
            if release_answered.swap(true, Ordering::AcqRel) {
                return;
            }
            let _ = release_app.update(cx, |app, cx| {
                // 对话框可能被其它后台流程直接弹出栈，此时不会触发 on_close。
                // 释放内容实体时补发 Skip，确保阻塞的传输线程一定能继续。
                if app
                    .session
                    .prompt_active_duplicate()
                    .is_some_and(|active| active.id == release_id)
                {
                    app.resolve_duplicate_prompt(
                        release_id.clone(),
                        SftpDuplicateDecision::Skip,
                        cx,
                    );
                }
            });
        });
        Self {
            app,
            prompt,
            palette,
            answered,
            _release_subscription: release_subscription,
        }
    }
}

impl Render for DuplicatePromptDialogContent {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let kind = if self.prompt.request.is_directory {
            t!("fileTransfer.duplicateKindFolder")
        } else {
            t!("fileTransfer.duplicateKindFile")
        };
        let target_name = download_file_name_from_remote_path(&self.prompt.request.target_path);
        let description = t!(
            "fileTransfer.duplicateDescription",
            kind = kind,
            name = target_name
        );
        let mut actions = div().flex().flex_wrap().justify_end().gap_2();
        for (id, label, decision) in [
            (
                "duplicate-overwrite-action",
                t!("fileTransfer.duplicateOverwrite"),
                SftpDuplicateDecision::Overwrite,
            ),
            (
                "duplicate-skip-action",
                t!("fileTransfer.duplicateSkip"),
                SftpDuplicateDecision::Skip,
            ),
            (
                "duplicate-rename-action",
                t!("common.rename"),
                SftpDuplicateDecision::Rename,
            ),
        ] {
            let app = self.app.clone();
            let answered = self.answered.clone();
            let request = self.prompt.clone();
            actions = actions.child(div().debug_selector(move || id.to_string()).child(
                crate::widgets::small_button(
                    self.palette,
                    id,
                    label.to_string(),
                    move |_, window, cx| {
                        if answered.swap(true, Ordering::AcqRel) {
                            return;
                        }
                        if let Some(app) = app.upgrade() {
                            let request_id = request.id.clone();
                            app.update(cx, |app, cx| {
                                app.resolve_duplicate_prompt(request_id, decision, cx);
                            });
                        }
                        window.close_nya_dialog(cx);
                    },
                ),
            ));
        }
        div()
            .id("duplicate-prompt-dialog")
            .debug_selector(|| "duplicate-prompt-dialog".to_string())
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_xs()
                    .line_height(px(17.))
                    .text_color(rgb(self.palette.text_muted))
                    .child(description),
            )
            .child(
                div()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(self.palette.border))
                    .px_2()
                    .py_1()
                    .font_family(crate::features::shell::gpui_code_font_family())
                    .text_size(px(11.))
                    .text_color(rgb(self.palette.text_muted))
                    .child(self.prompt.request.target_path.clone()),
            )
            .child(actions)
    }
}

impl NyaTermApp {
    pub(in crate::features) fn open_duplicate_prompt_dialog(
        &mut self,
        prompt: SftpDuplicatePromptState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let palette = self.theme_palette();
        let app = cx.weak_entity();
        let width = bounded_dialog_width(f32::from(window.viewport_size().width), 32., 280., 448.);
        let answered = Arc::new(AtomicBool::new(false));
        let content = cx.new(|cx| {
            DuplicatePromptDialogContent::new(
                app.clone(),
                prompt.clone(),
                palette,
                answered.clone(),
                cx,
            )
        });

        window.open_nya_dialog(cx, move |dialog, _, _| {
            let close_app = app.clone();
            let close_id = prompt.id.clone();
            let close_answered = answered.clone();
            dialog
                .title(t!("fileTransfer.duplicateTitle").to_string())
                .width(width)
                .overlay_closable(false)
                // 重名决策没有默认确认项，Enter 不能被解释为 Skip。
                .on_ok(|_, _, _| false)
                .content(content.clone())
                .on_close(move |_, _window, cx| {
                    if close_answered.swap(true, Ordering::AcqRel) {
                        return;
                    }
                    let _ = close_app.update(cx, |app, cx| {
                        // 关闭按钮和 Escape 都按跳过处理，确保后台不会继续等待。
                        // 已回答的旧对话框不得误处理随后激活的另一条冲突请求。
                        if app
                            .session
                            .prompt_active_duplicate()
                            .is_some_and(|active| active.id == close_id)
                        {
                            app.resolve_duplicate_prompt(
                                close_id.clone(),
                                SftpDuplicateDecision::Skip,
                                cx,
                            );
                        }
                    });
                })
        });
    }
}
