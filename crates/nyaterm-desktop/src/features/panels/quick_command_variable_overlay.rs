use rust_i18n::t;

use gpui::{
    Context, FontWeight, IntoElement, KeyDownEvent, SharedString, div, prelude::*, px, rgb, rgba,
};
use nyaterm_ui::{NyaScrollable, NyaSelectOption};

use crate::features::view_widgets::dialog_action_button;
use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::models::QuickCommandVariablePromptState;
use crate::widgets::small_button;

impl NyaTermApp {
    pub(in crate::features) fn quick_command_variable_prompt_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let prompt = self.commands.quick_variable_prompt().cloned().unwrap_or(
            QuickCommandVariablePromptState {
                command_id: String::new(),
                label: "command".to_string(),
                command: String::new(),
                execute: true,
                send_to_all: false,
                variables: Vec::new(),
            },
        );
        let mut preview = prompt.command.clone();
        for variable in &prompt.variables {
            preview = preview.replace(&variable.raw, &variable.value);
        }

        let mut rows = div()
            .id("quick-command-variable-fields")
            .flex()
            .flex_col()
            .gap_4();
        for (index, variable) in prompt.variables.iter().cloned().enumerate() {
            let variable_name = variable.name.clone();
            let field_id = format!("quick-command-variable-{index}");
            let field = if variable.options.is_empty() {
                self.text_input_box(
                    format!("quick-command.variable.{index}"),
                    &variable.value,
                    TextInputSetup::default(),
                    cx,
                )
                .into_any_element()
            } else {
                // Tauri offers the whole option list at once. Stepping through it with
                // arrows hid every value but the current one.
                let options = variable
                    .options
                    .iter()
                    .map(|option| {
                        let label = if option.is_empty() {
                            "-"
                        } else {
                            option.as_str()
                        };
                        NyaSelectOption::new(option.clone(), label.to_string())
                    })
                    .collect::<Vec<_>>();
                self.form_select_control(
                    format!("quick-command.variable.{index}"),
                    options,
                    Some(variable.value.clone()),
                    false,
                    cx,
                )
                .into_any_element()
            };
            rows = rows.child(
                div()
                    .id(SharedString::from(field_id.clone()))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_muted))
                            .truncate()
                            .child(variable_name),
                    )
                    .child(field),
            );
        }

        div()
            .id(SharedString::from("quick-command-variable-overlay"))
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .bg(rgba(0x00000080))
            .flex()
            .items_center()
            .justify_center()
            .track_focus(self.commands.quick_variable_focus())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.handle_quick_command_variable_key_down(event, cx) {
                    this.focus_terminal_input(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .id("quick-command-variable-backdrop")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .on_click(cx.listener(|this, _, window, cx| {
                        window.focus(this.commands.quick_variable_focus(), cx);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id(SharedString::from("quick-command-variable-dialog"))
                    // Occlude the backdrop so its click handler cannot steal focus
                    // from a variable field the pointer just landed on. GPUI reports
                    // every hitbox under the pointer as hovered, even when another
                    // element is painted on top.
                    .occlude()
                    .w(px((self.shell.viewport_size().0 - 32.).clamp(280., 400.)))
                    .max_w_full()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(self.shell_surface_color(palette.bg))
                    .shadow_lg()
                    .overflow_hidden()
                    .child(
                        div()
                            .px_5()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(palette.border))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight(800.))
                                    .text_color(rgb(palette.text))
                                    .child(t!("quickCommands.fillVariables")),
                            ),
                    )
                    .child(
                        div()
                            .id("quick-command-variable-body")
                            .p_5()
                            .max_h(px((self.shell.viewport_size().1 * 0.6).clamp(180., 420.)))
                            .overflow_y_scrollbar()
                            .child(rows)
                            .child(
                                div()
                                    .mt_4()
                                    .rounded_sm()
                                    .bg(rgb(palette.input))
                                    .p_2()
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(rgb(palette.text_muted))
                                            .child(t!("quickCommands.preview")),
                                    )
                                    .child(
                                        // The resolved command in full: this is the last
                                        // look before it runs, so nothing is cut off.
                                        div()
                                            .mt_1()
                                            .font_family(
                                                crate::features::shell::gpui_code_font_family(),
                                            )
                                            .text_xs()
                                            .line_height(px(18.))
                                            .text_color(rgb(palette.text_muted))
                                            .child(if preview.trim().is_empty() {
                                                t!("quickCommands.emptyCommand").to_string()
                                            } else {
                                                preview
                                            }),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .px_5()
                            .py_3()
                            .border_t_1()
                            .border_color(rgb(palette.border))
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(
                                div()
                                    .debug_selector(|| "quick-command-variable-cancel".into())
                                    .child(small_button(
                                        palette,
                                        "quick-command-variable-cancel",
                                        t!("common.cancel"),
                                        cx.listener(|this, _, window, cx| {
                                            this.cancel_quick_command_variable_prompt(cx);
                                            this.focus_terminal_input(window, cx);
                                        }),
                                    )),
                            )
                            .child(dialog_action_button(
                                palette,
                                "quick-command-variable-submit",
                                if prompt.execute {
                                    t!("quickCommands.run")
                                } else {
                                    t!("quickCommands.appendOnly")
                                },
                                false,
                                cx.listener(|this, _, window, cx| {
                                    this.submit_quick_command_variable_prompt(cx);
                                    this.focus_terminal_input(window, cx);
                                }),
                            )),
                    ),
            )
    }
}
