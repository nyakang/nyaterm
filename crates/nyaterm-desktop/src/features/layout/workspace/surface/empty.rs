use rust_i18n::t;

use gpui::{Context, FontWeight, IntoElement, SharedString, div, prelude::*, px, rgb, svg};
use nyaterm_core::truncate_preview;

use crate::features::view_widgets::{
    connection_spinner, empty_workspace_action, nyaterm_logo_mark,
};
use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::models::BottomPanelMode;
use crate::models::NavItem;

impl NyaTermApp {
    pub(in crate::features) fn workbench_workspace_state(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Match Tauri EmptyWorkspaceState: large faded logo + label|shortcut rows.
        let temporary_ssh = self.display_shortcut_for("tab.temporarySshLink", "Ctrl+Alt+N");
        let open_chat = self.display_shortcut_for("view.openChat", "Ctrl+Alt+I");
        let show_commands = self.display_shortcut_for("view.showAllCommands", "Ctrl+Shift+P");
        let switch_terminal = self.display_shortcut_for("tab.quickSwitch", "Ctrl+Shift+S");
        let temporary_ssh_label = t!("temporarySsh.title");
        let open_chat_label = t!("app.openChat");
        let show_commands_label = t!("app.showAllCommands");
        let switch_terminal_label = t!("app.switchTerminal");

        let palette = self.theme_palette();
        let terminal_palette = self.terminal_theme_palette();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(self.shell_surface_color(terminal_palette.terminal_bg))
            .px_6()
            .child(
                div()
                    .w(px(544.))
                    .max_w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .when(!self.wallpaper_enabled(), |this| {
                        this.child(div().mb_9().child(nyaterm_logo_mark(
                            terminal_palette,
                            256.,
                            0.13,
                        )))
                    })
                    .child(
                        // Tauri EmptyWorkspaceState: grid w-fit max-w-[30rem] gap-x-4 gap-y-3
                        div()
                            .w(px(480.))
                            .max_w_full()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(empty_workspace_action(
                                palette,
                                temporary_ssh_label,
                                temporary_ssh,
                                cx.listener(|this, _, window, cx| {
                                    this.ensure_panel_open(NavItem::Connections);
                                    this.open_temporary_ssh_link_dialog(window, cx);
                                }),
                            ))
                            .child(empty_workspace_action(
                                palette,
                                open_chat_label,
                                open_chat,
                                cx.listener(|this, _, window, cx| {
                                    this.ensure_panel_open(NavItem::AiAssistant);
                                    window.focus(this.ai.chat_focus(), cx);
                                    this.ai.set_panel_status("AI assistant focused");
                                    cx.notify();
                                }),
                            ))
                            .child(empty_workspace_action(
                                palette,
                                show_commands_label,
                                show_commands,
                                cx.listener(|this, _, window, cx| {
                                    this.set_bottom_panel_mode(BottomPanelMode::QuickCommands);
                                    let search = this.commands.quick_search_draft().to_string();
                                    let field = this.text_input(
                                        "quick-command.search",
                                        &search,
                                        TextInputSetup::placeholder(t!("quickCommands.search")),
                                        cx,
                                    );
                                    window.focus(&field.read(cx).focus_handle(), cx);
                                    this.shell.set_status("quick commands opened".to_string());
                                    cx.notify();
                                }),
                            ))
                            .child(empty_workspace_action(
                                palette,
                                switch_terminal_label,
                                switch_terminal,
                                cx.listener(|this, _, window, cx| {
                                    this.open_quick_switch(window, cx);
                                }),
                            )),
                    ),
            )
    }

    pub(in crate::features) fn pending_workspace_state(&self) -> impl IntoElement {
        let palette = self.theme_palette();
        let name = self
            .session
            .start_pending_display_name()
            .unwrap_or_else(|| t!("terminal.connecting").to_string());

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .bg(self.shell_surface_color(self.terminal_theme_palette().terminal_bg))
            .child(connection_spinner(
                SharedString::from("pending-workspace-spinner"),
                rgb(palette.primary).into(),
                24.,
            ))
            .child(
                div()
                    .max_w(px(320.))
                    .px_4()
                    .text_center()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(name),
            )
    }

    pub(in crate::features) fn failed_workspace_state(&self) -> impl IntoElement {
        let palette = self.theme_palette();
        let error = self
            .session
            .start_active_failed()
            .map(|failed| failed.error.clone())
            .or_else(|| self.shell.last_connect_failure_error().map(str::to_string))
            .unwrap_or_default();

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .bg(self.shell_surface_color(self.terminal_theme_palette().terminal_bg))
            .child(
                svg()
                    .size(px(32.))
                    .path("icons/session/disconnect.svg")
                    .text_color(rgb(palette.danger)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .px_6()
                    .text_center()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight(600.))
                            .text_color(rgb(palette.text))
                            .child(t!("terminal.connectionFailed")),
                    )
                    .child(
                        div()
                            .max_w(px(320.))
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_dimmed))
                            .child(truncate_preview(&error, 160)),
                    ),
            )
    }
}
