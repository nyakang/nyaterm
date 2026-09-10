use rust_i18n::t;

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FontWeight, IntoElement, KeyDownEvent,
    SharedString, Window, div, prelude::*, px, rgb, rgba, svg,
};
use nyaterm_core::truncate_preview;
use nyaterm_transport::RemoteTextGeneration;
use nyaterm_ui::NyaScrollable;

use crate::features::transfers::RemoteTextEditor;
use crate::features::view_widgets::full_window_input_layer;
use crate::features::{NyaTermApp, view_widgets::dialog_action_button};
use crate::models::{
    TransferEditorField, TransferEditorState, TransferEditorWorkspaceState,
    TransferExternalSyncPromptState,
};
use crate::widgets::small_button;

use super::helpers::{editor_content_preview, editor_search_matches};

#[derive(Clone, Copy)]
enum ExternalSyncButtonStyle {
    Ghost,
    Outline,
    Primary,
}

fn external_sync_button(
    palette: crate::theme::ThemePalette,
    id: &'static str,
    label: impl Into<SharedString>,
    style: ExternalSyncButtonStyle,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let transparent = rgba(0x00000000);
    let (background, border, text) = match style {
        ExternalSyncButtonStyle::Ghost => (transparent, transparent, rgb(palette.text_muted)),
        ExternalSyncButtonStyle::Outline => {
            (rgb(palette.bg), rgb(palette.border), rgb(palette.text))
        }
        ExternalSyncButtonStyle::Primary => (rgb(palette.link), rgb(palette.link), rgb(palette.bg)),
    };
    div()
        .id(SharedString::from(id))
        .h(px(28.))
        .px_3()
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(border)
        .bg(background)
        .text_color(text)
        .text_xs()
        .cursor_pointer()
        .hover(move |this| match style {
            ExternalSyncButtonStyle::Primary => this.bg(rgba((palette.link << 8) | 0xe8)),
            ExternalSyncButtonStyle::Ghost | ExternalSyncButtonStyle::Outline => {
                this.bg(rgb(palette.hover)).text_color(rgb(palette.text))
            }
        })
        .child(label)
        .on_click(on_click)
}

impl NyaTermApp {
    pub(in crate::features) fn transfer_external_sync_prompt_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some((prompt_id, prompt)) = self.active_external_editor_sync_prompt() else {
            return div().into_any_element();
        };
        self.transfer_external_sync_prompt_surface(prompt_id, prompt, false, cx)
    }

    pub(in crate::features) fn transfer_external_sync_window_view(
        &mut self,
        prompt_id: String,
        prompt: TransferExternalSyncPromptState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.transfer_external_sync_prompt_surface(prompt_id, prompt, true, cx)
    }

    fn transfer_external_sync_prompt_surface(
        &mut self,
        prompt_id: String,
        prompt: TransferExternalSyncPromptState,
        standalone: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let title = t!("fileExplorer.fileModified");
        let prompt_label = t!("fileExplorer.uploadPrompt");
        let cancel_label = t!("common.cancel");
        let always_label = t!("fileExplorer.alwaysUpload");
        let upload_once_label = t!("fileExplorer.uploadOnce");
        let ignore_prompt_id = prompt_id.clone();
        let always_prompt_id = prompt_id.clone();
        let upload_prompt_id = prompt_id.clone();

        div()
            .id(SharedString::from("transfer-external-sync-overlay"))
            .when(!standalone, |this| {
                this.absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .bg(rgba(0x00000080))
                    .p_3()
            })
            .when(standalone, |this| this.size_full().bg(rgb(palette.bg)))
            .flex()
            .items_center()
            .justify_center()
            .track_focus(self.transfer.external_sync_focus())
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(this.transfer.external_sync_focus(), cx);
                cx.notify();
            }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                cx.stop_propagation();
                if event.keystroke.key.as_str() == "escape" {
                    this.ignore_external_editor_sync_prompt(&ignore_prompt_id, cx);
                }
            }))
            .child(
                div()
                    .w_full()
                    .when(standalone, |this| this.size_full())
                    .when(!standalone, |this| {
                        this.max_w(px(440.))
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(palette.border))
                            .shadow_lg()
                    })
                    .bg(if standalone {
                        rgb(palette.bg)
                    } else {
                        self.shell_surface_color(palette.bg)
                    })
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when(!standalone, |this| {
                        this.child(div().text_sm().font_weight(FontWeight(700.)).child(title))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(rgb(palette.text))
                                    .child(prompt_label),
                            )
                            .child(
                                div()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(palette.border))
                                    .bg(rgb(palette.input))
                                    .px_3()
                                    .py_2()
                                    .font_family(crate::features::shell::gpui_code_font_family())
                                    .text_xs()
                                    .text_color(rgb(palette.text_muted))
                                    .child(truncate_preview(&prompt.remote_path, 120)),
                            ),
                    )
                    .child(
                        div()
                            .pt_3()
                            .border_t_1()
                            .border_color(rgb(palette.border))
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(external_sync_button(
                                palette,
                                "transfer-external-sync-ignore",
                                cancel_label,
                                ExternalSyncButtonStyle::Ghost,
                                cx.listener(move |this, _, _, cx| {
                                    this.ignore_external_editor_sync_prompt(&prompt_id, cx);
                                }),
                            ))
                            .child(external_sync_button(
                                palette,
                                "transfer-external-sync-always",
                                always_label,
                                ExternalSyncButtonStyle::Outline,
                                cx.listener(move |this, _, _, cx| {
                                    this.upload_external_editor_sync_prompt(
                                        &always_prompt_id,
                                        true,
                                        cx,
                                    );
                                }),
                            ))
                            .child(external_sync_button(
                                palette,
                                "transfer-external-sync-upload",
                                upload_once_label,
                                ExternalSyncButtonStyle::Primary,
                                cx.listener(move |this, _, _, cx| {
                                    this.upload_external_editor_sync_prompt(
                                        &upload_prompt_id,
                                        false,
                                        cx,
                                    );
                                }),
                            )),
                    ),
            )
            .into_any_element()
    }

    pub(in crate::features) fn transfer_editor_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.transfer_editor_surface(false, None, None, cx)
    }

    pub(in crate::features) fn transfer_editor_window_view(
        &mut self,
        editor: Entity<RemoteTextEditor>,
        cursor_position: (usize, usize),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.transfer_editor_surface(true, Some(editor), Some(cursor_position), cx)
            .into_any_element()
    }

    fn transfer_editor_surface(
        &mut self,
        standalone: bool,
        native_editor: Option<Entity<RemoteTextEditor>>,
        cursor_position: Option<(usize, usize)>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let loading_label = t!("common.loading");
        let saving_label = t!("common.saving");
        let save_label = t!("common.save");
        let cancel_label = t!("common.cancel");
        let saved_label = t!("fileEditor.saved");
        let unsaved_label = t!("fileEditor.unsaved");
        let conflict_label = t!("fileEditor.conflictTitle");
        let conflict_desc = t!("fileEditor.conflictDesc");
        let unsaved_title = t!("fileEditor.unsavedTitle");
        let unsaved_desc = t!("fileEditor.unsavedDesc");
        let reload_dirty_title = t!("fileEditor.reloadDirtyTitle");
        let reload_dirty_desc = t!("fileEditor.reloadDirtyDesc");
        let reload_label = t!("fileEditor.reload");
        let confirm_reload_label = t!("fileEditor.discardAndReload");
        let open_external_label = t!("fileEditor.openExternal");
        let force_save_label = t!("fileEditor.forceSave");
        let save_all_label = t!("fileEditor.saveAll");
        let save_close_label = t!("fileEditor.saveAndClose");
        let discard_label = t!("fileEditor.discard");
        let bytes_label = t!("fileEditor.bytes");
        let encoding_label = t!("fileEditor.encodingUtf8");
        let line_ending_label = t!("fileEditor.lineEndingLf");
        let plain_text_label = t!("fileEditor.plainText");
        let workspace = self.transfer.editor_workspace_snapshot();
        let state = workspace
            .as_ref()
            .and_then(TransferEditorWorkspaceState::active_tab)
            .cloned()
            .unwrap_or(TransferEditorState {
                id: String::new(),
                session_id: None,
                remote_path: String::new(),
                raw_path_token: None,
                name: String::new(),
                content: String::new(),
                search_query: String::new(),
                active_match: 0,
                revision: None,
                generation: RemoteTextGeneration::next(),
                loading: false,
                saving: false,
                dirty: false,
                conflict: false,
                close_after_save: false,
                reload_confirm: false,
                error: None,
                focused_field: TransferEditorField::Content,
            });
        let close_confirm = workspace
            .as_ref()
            .is_some_and(|workspace| workspace.close_confirm);
        let tabs = workspace
            .as_ref()
            .map(|workspace| workspace.tabs.clone())
            .unwrap_or_default();
        let status = if state.loading {
            loading_label.clone()
        } else if state.saving {
            saving_label.clone()
        } else if state.conflict {
            conflict_label.clone()
        } else if state.dirty {
            unsaved_label
        } else {
            saved_label
        };
        let byte_count = state.content.len();
        let language = std::path::Path::new(if state.name.is_empty() {
            &state.remote_path
        } else {
            &state.name
        })
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
        let language_label = if language.is_empty() {
            plain_text_label.to_string()
        } else {
            language.to_ascii_uppercase()
        };
        let (cursor_line, cursor_column) = cursor_position.unwrap_or_else(|| {
            let before = state.content.as_str();
            let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
            let line_start = before.rfind('\n').map(|index| index + 1).unwrap_or(0);
            (line, before[line_start..].chars().count() + 1)
        });
        let cursor_label = t!(
            "fileEditor.lineColumn",
            line = cursor_line,
            column = cursor_column
        );
        let search_matches = editor_search_matches(&state.content, &state.search_query);
        let active_match = state
            .active_match
            .min(search_matches.len().saturating_sub(1));
        let content_preview =
            editor_content_preview(&state.content, &state.search_query, active_match);
        let active_tab_id = state.id.clone();
        let has_native_editor = native_editor.is_some();
        let tabs_menu_open = self.transfer.editor_tabs_menu_is_open() && tabs.len() > 1;
        let mut tab_list = div()
            .id("transfer-editor-tab-list")
            .h_full()
            .flex_1()
            .min_w_0()
            .flex()
            .overflow_x_scrollbar();
        let tabs_menu_max_height = (self.shell.viewport_size().1 - 48.).clamp(160., 360.);
        let tabs_menu_bg = if standalone {
            rgb(palette.surface)
        } else {
            self.shell_surface_color(palette.surface)
        };
        let mut tabs_menu_content = div()
            .max_h(px(tabs_menu_max_height))
            .overflow_y_scrollbar()
            .py_1()
            .flex()
            .flex_col();
        for (index, tab) in tabs.iter().enumerate() {
            let tab_id = tab.id.clone();
            let close_tab_id = tab.id.clone();
            let active = tab.id == active_tab_id;
            let base_label = if tab.name.trim().is_empty() {
                tab.remote_path
                    .rsplit('/')
                    .next()
                    .filter(|name| !name.is_empty())
                    .unwrap_or(tab.remote_path.as_str())
            } else {
                tab.name.as_str()
            };
            let duplicate_name = tabs.iter().filter(|other| other.name == tab.name).count() > 1;
            let label = if duplicate_name {
                let parent = tab
                    .remote_path
                    .rsplit_once('/')
                    .map(|(parent, _)| parent.rsplit('/').next().unwrap_or(parent))
                    .filter(|parent| !parent.is_empty())
                    .unwrap_or("/");
                format!("{base_label} · {parent}")
            } else {
                base_label.to_string()
            };
            let tab_group_name = SharedString::from(format!("transfer-editor-tab-group-{index}"));
            tab_list = tab_list.child(
                div()
                    .id(SharedString::from(format!("transfer-editor-tab-{index}")))
                    .group(tab_group_name.clone())
                    .h_full()
                    .min_w(px(96.))
                    .max_w(px(240.))
                    .px_3()
                    .relative()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_r_1()
                    .border_color(rgb(palette.border))
                    .bg(if active {
                        rgb(palette.bg)
                    } else {
                        rgb(palette.surface)
                    })
                    .text_color(if active {
                        rgb(palette.text)
                    } else {
                        rgb(palette.text_muted)
                    })
                    .cursor_pointer()
                    .hover(|this| this.bg(rgb(palette.hover)).text_color(rgb(palette.text)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.activate_transfer_editor_tab(&tab_id, cx);
                    }))
                    .when(active, |this| {
                        this.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .right_0()
                                .h(px(1.))
                                .bg(rgb(palette.link)),
                        )
                    })
                    .when(tab.dirty, |this| {
                        this.child(
                            div()
                                .size(px(6.))
                                .flex_none()
                                .rounded_full()
                                .bg(rgb(palette.link)),
                        )
                    })
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .font_family(crate::features::shell::gpui_code_font_family())
                            .text_xs()
                            .child(truncate_preview(&label, 28)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "transfer-editor-tab-close-{index}"
                            )))
                            .size(px(20.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .text_size(px(14.))
                            .text_color(rgb(palette.text_muted))
                            .when(active, |this| this.opacity(0.7))
                            .when(!active, |this| {
                                this.opacity(0.)
                                    .group_hover(tab_group_name, |style| style.opacity(0.7))
                            })
                            .hover(|this| this.bg(rgb(palette.hover)).text_color(rgb(palette.text)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_transfer_editor_tab(&close_tab_id, cx);
                            }))
                            .child(
                                svg()
                                    .size(px(13.))
                                    .path("icons/window/close.svg")
                                    .text_color(rgb(palette.text_muted)),
                            ),
                    ),
            );

            let menu_tab_id = tab.id.clone();
            tabs_menu_content = tabs_menu_content.child(
                div()
                    .id(SharedString::from(format!(
                        "transfer-editor-tabs-menu-item-{index}"
                    )))
                    .px_3()
                    .py_2()
                    .flex()
                    .items_start()
                    .gap_2()
                    .cursor_pointer()
                    .bg(if active {
                        rgb(palette.hover)
                    } else {
                        rgb(palette.surface_elevated)
                    })
                    .hover(|this| this.bg(rgb(palette.hover)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.activate_transfer_editor_tab(&menu_tab_id, cx);
                    }))
                    .child(div().mt(px(5.)).size(px(6.)).flex_none().rounded_full().bg(
                        if tab.dirty {
                            rgb(palette.link)
                        } else {
                            rgba(0x00000000)
                        },
                    ))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .overflow_hidden()
                                    .font_family(crate::features::shell::gpui_code_font_family())
                                    .text_xs()
                                    .text_color(rgb(palette.text))
                                    .child(truncate_preview(&label, 38)),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .font_family(crate::features::shell::gpui_code_font_family())
                                    .text_size(px(10.))
                                    .text_color(rgb(palette.text_muted))
                                    .child(truncate_preview(&tab.remote_path, 58)),
                            ),
                    ),
            );
        }
        let tabs_menu = div()
            .id("transfer-editor-tabs-menu")
            .absolute()
            .top(px(40.))
            .right_0()
            .w(px(320.))
            .rounded_bl_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(tabs_menu_bg)
            .shadow_lg()
            .child(tabs_menu_content);
        let tab_strip = div()
            .id("transfer-editor-tabs")
            .h(px(40.))
            .flex_none()
            .flex()
            .overflow_hidden()
            .border_b_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.surface))
            .child(tab_list)
            .when(tabs.len() > 1, |this| {
                this.child(
                    div()
                        .id("transfer-editor-tabs-menu-trigger")
                        .h_full()
                        .w(px(36.))
                        .flex_none()
                        .border_l_1()
                        .border_color(rgb(palette.border))
                        .bg(rgb(palette.surface))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|this| this.bg(rgb(palette.hover)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.transfer.toggle_editor_tabs_menu();
                            cx.notify();
                        }))
                        .child(
                            svg()
                                .size(px(15.))
                                .path("icons/chevron-down.svg")
                                .text_color(rgb(palette.text_muted)),
                        ),
                )
            });

        div()
            .id(SharedString::from("transfer-editor-overlay"))
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .bg(if standalone {
                rgb(palette.bg)
            } else {
                rgba(0x00000080)
            })
            .flex()
            .items_center()
            .justify_center()
            .track_focus(self.transfer.editor_focus())
            .on_click(cx.listener(|this, _, window, cx| {
                this.transfer.close_editor_tabs_menu();
                window.focus(this.transfer.editor_focus(), cx);
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.transfer.editor_focus().is_focused(window) {
                    cx.stop_propagation();
                    this.handle_transfer_editor_key_down(event, window, cx);
                }
            }))
            .child(
                div()
                    .id(SharedString::from("transfer-editor-dialog"))
                    .when(standalone, |this| this.size_full())
                    .when(!standalone, |this| {
                        this.w(px(780.))
                            .h(px(620.))
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(palette.border))
                            .shadow_lg()
                    })
                    .bg(if standalone {
                        rgb(palette.bg)
                    } else {
                        self.shell_surface_color(palette.bg)
                    })
                    .overflow_hidden()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(tab_strip)
                    .child(
                        div()
                            .h(px(44.))
                            .flex_none()
                            .px_3()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .border_b_1()
                            .border_color(rgb(palette.border))
                            .bg(rgb(palette.surface))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .overflow_hidden()
                                    .font_family(crate::features::shell::gpui_code_font_family())
                                    .text_xs()
                                    .text_color(rgb(palette.text_muted))
                                    .child(truncate_preview(&state.remote_path, 96)),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .when(!close_confirm, |this| this.child(small_button(
                                        palette,
                                        "transfer-editor-reload",
                                        reload_label.clone(),
                                        cx.listener(|this, _, window, cx| {
                                            if let Some(state) =
                                                this.transfer.active_editor_tab_mut()
                                            {
                                                if state.dirty && !state.reload_confirm {
                                                    state.reload_confirm = true;
                                                    this.shell.set_status("confirm remote editor reload".to_string());
                                                    cx.notify();
                                                    return;
                                                }
                                                if state.saving {
                                                    return;
                                                }
                                                state.generation = RemoteTextGeneration::next();
                                                state.loading = true;
                                                state.error = None;
                                                state.conflict = false;
                                                state.reload_confirm = false;
                                                let session_id = state.session_id.clone();
                                                let tab_id = state.id.clone();
                                                let remote_path = state.remote_file_path();
                                                this.start_sftp_editor_load_job(
                                                    session_id,
                                                    tab_id,
                                                    remote_path,
                                                    window,
                                                    cx,
                                                );
                                            }
                                        }),
                                    )))
                                    .when(!close_confirm, |this| this.child(small_button(
                                        palette,
                                        "transfer-editor-open-external",
                                        open_external_label,
                                        cx.listener(|this, _, _, cx| {
                                            this.open_active_transfer_editor_external(cx);
                                        }),
                                    )))
                                    .when(!close_confirm, |this| this.child(small_button(
                                        palette,
                                        "transfer-editor-save",
                                        if state.saving {
                                            saving_label.clone()
                                        } else {
                                            save_label
                                        },
                                        cx.listener(|this, _, window, cx| {
                                            this.save_transfer_editor(false, window, cx);
                                        }),
                                    )))
                                    .when(!close_confirm && tabs.len() > 1, |this| {
                                        this.child(small_button(
                                            palette,
                                            "transfer-editor-save-all",
                                            save_all_label,
                                            cx.listener(|this, _, window, cx| {
                                                this.save_all_transfer_editor_tabs(window, cx);
                                            }),
                                        ))
                                    }),
                            ),
                    )
                    .when_some(state.error.clone(), |this, error| {
                        this.child(
                            div()
                                .flex_none()
                                .border_b_1()
                                .border_color(rgb(palette.border))
                                .bg(rgb(0x351216))
                                .px_3()
                                .py_2()
                                .text_xs()
                                .text_color(rgb(palette.danger))
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .id(SharedString::from("transfer-editor-content"))
                            .flex_1()
                            .min_h_0()
                            .bg(rgb(palette.input))
                            .relative()
                            .overflow_hidden()
                            .when_some(native_editor, |this, editor| this.child(editor))
                            .when(has_native_editor && state.loading, |this| {
                                this.child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .bg(rgba((palette.bg << 8) | 0xe6))
                                        .text_sm()
                                        .text_color(rgb(palette.text_muted))
                                        .child(loading_label.clone()),
                                )
                            })
                            .when(!has_native_editor, |this| {
                                this.p_3()
                                    .font_family(crate::features::shell::gpui_code_font_family())
                                    .text_xs()
                                    .text_color(if state.loading {
                                        rgb(palette.text_muted)
                                    } else {
                                        rgb(palette.text)
                                    })
                                    .child(if state.loading {
                                        SharedString::from(loading_label)
                                    } else if content_preview.is_empty() {
                                        SharedString::from("")
                                    } else {
                                        SharedString::from(content_preview)
                                    })
                            })
                    )
                    .child(
                        div()
                            .h(px(24.))
                            .flex_none()
                            .px_3()
                            .border_t_1()
                            .border_color(rgb(palette.border))
                            .bg(rgb(palette.surface))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .font_family(crate::features::shell::gpui_code_font_family())
                                    .text_size(px(11.))
                                    .text_color(rgb(palette.text_muted))
                                    .child(format!(
                                        "{language_label} · {cursor_label} · {status}"
                                    )),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .font_family(crate::features::shell::gpui_code_font_family())
                                    .text_size(px(11.))
                                    .text_color(rgb(palette.text_muted))
                                    .child(format!(
                                        "{byte_count} {bytes_label} · {encoding_label} · {line_ending_label}"
                                    )),
                            ),
                    )
                    .when(state.conflict, |this| {
                        this.child(transfer_editor_alert_dialog(
                            palette,
                            "transfer-editor-conflict-dialog",
                            480.,
                            conflict_label,
                            conflict_desc,
                            div()
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap_2()
                                .child(small_button(
                                    palette,
                                    "transfer-editor-conflict-cancel",
                                    cancel_label.clone(),
                                    cx.listener(|this, _, _, cx| {
                                        this.cancel_transfer_editor_conflict(cx);
                                    }),
                                ))
                                .child(small_button(
                                    palette,
                                    "transfer-editor-conflict-reload",
                                    reload_label,
                                    cx.listener(|this, _, window, cx| {
                                        let Some(state) = this.transfer.active_editor_tab_mut()
                                        else {
                                            return;
                                        };
                                        if state.saving {
                                            return;
                                        }
                                        state.generation = RemoteTextGeneration::next();
                                        state.loading = true;
                                        state.error = None;
                                        state.conflict = false;
                                        state.reload_confirm = false;
                                        let session_id = state.session_id.clone();
                                        let tab_id = state.id.clone();
                                        let remote_path = state.remote_file_path();
                                        this.start_sftp_editor_load_job(
                                            session_id,
                                            tab_id,
                                            remote_path,
                                            window,
                                            cx,
                                        );
                                    }),
                                ))
                                .child(dialog_action_button(
                                    palette,
                                    "transfer-editor-conflict-force-save",
                                    force_save_label,
                                    false,
                                    cx.listener(|this, _, window, cx| {
                                        this.save_transfer_editor(true, window, cx);
                                    }),
                                )),
                        ))
                    })
                    .when(close_confirm && !state.conflict, |this| {
                        this.child(transfer_editor_alert_dialog(
                            palette,
                            "transfer-editor-unsaved-dialog",
                            384.,
                            unsaved_title,
                            unsaved_desc,
                            div()
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap_2()
                                .child(small_button(
                                    palette,
                                    "transfer-editor-unsaved-cancel",
                                    cancel_label.clone(),
                                    cx.listener(|this, _, _, cx| {
                                        this.cancel_transfer_editor_close_confirm(cx);
                                    }),
                                ))
                                .child(small_button(
                                    palette,
                                    "transfer-editor-unsaved-save",
                                    if state.saving {
                                        saving_label
                                    } else {
                                        save_close_label
                                    },
                                    cx.listener(|this, _, window, cx| {
                                        this.save_transfer_editor_and_close(window, cx);
                                    }),
                                ))
                                .child(dialog_action_button(
                                    palette,
                                    "transfer-editor-unsaved-discard",
                                    discard_label,
                                    true,
                                    cx.listener(|this, _, _, cx| {
                                        this.discard_transfer_editor(cx);
                                    }),
                                )),
                        ))
                    })
                    .when(
                        state.reload_confirm && !state.conflict && !close_confirm,
                        |this| {
                            this.child(transfer_editor_alert_dialog(
                                palette,
                                "transfer-editor-reload-dirty-dialog",
                                384.,
                                reload_dirty_title,
                                reload_dirty_desc,
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .gap_2()
                                    .child(small_button(
                                        palette,
                                        "transfer-editor-reload-dirty-cancel",
                                        cancel_label,
                                        cx.listener(|this, _, _, cx| {
                                            this.cancel_transfer_editor_reload_confirm(cx);
                                        }),
                                    ))
                                    .child(dialog_action_button(
                                        palette,
                                        "transfer-editor-reload-dirty-confirm",
                                        confirm_reload_label,
                                        true,
                                        cx.listener(|this, _, window, cx| {
                                            let Some(state) =
                                                this.transfer.active_editor_tab_mut()
                                            else {
                                                return;
                                            };
                                            if state.saving {
                                                return;
                                            }
                                            state.generation = RemoteTextGeneration::next();
                                            state.loading = true;
                                            state.error = None;
                                            state.conflict = false;
                                            state.reload_confirm = false;
                                            let session_id = state.session_id.clone();
                                            let tab_id = state.id.clone();
                                            let remote_path = state.remote_file_path();
                                            this.start_sftp_editor_load_job(
                                                session_id,
                                                tab_id,
                                                remote_path,
                                                window,
                                                cx,
                                            );
                                        }),
                                    )),
                            ))
                        },
                    )
                    .when(tabs_menu_open, |this| this.child(tabs_menu)),
            )
    }
}

fn transfer_editor_alert_dialog(
    palette: crate::theme::ThemePalette,
    id: &'static str,
    width: f32,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    actions: impl IntoElement,
) -> impl IntoElement {
    let title: SharedString = title.into();
    let description: SharedString = description.into();
    full_window_input_layer(format!("{id}-backdrop"))
        .bg(rgba(0x00000099))
        .flex()
        .items_center()
        .justify_center()
        .p_3()
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(
            div()
                .id(SharedString::from(id))
                .w(px(width))
                .max_w_full()
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.surface_elevated))
                .shadow_lg()
                .p_6()
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight(700.))
                                .text_color(rgb(palette.text))
                                .child(title),
                        )
                        .child(
                            div()
                                .text_xs()
                                .line_height(px(18.))
                                .text_color(rgb(palette.text_muted))
                                .child(description),
                        ),
                )
                .child(actions),
        )
}
