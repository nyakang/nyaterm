use crate::models::ConnectionEditorFocusTarget;
use std::borrow::Cow;
use std::collections::HashMap;

use gpui::{
    App, Context, Entity, IntoElement, SharedString, div,
    prelude::{
        FluentBuilder, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
    },
    px, rgb, svg,
};
use nyaterm_core::SavedConnection;
use rust_i18n::t;

use crate::features::{
    NyaTermApp, text_inputs::ORDINARY_INPUT_SHELL_PADDING_X_PX,
    text_inputs::ordinary_input_focus_ring, text_inputs::ordinary_input_shell_border_color,
};
use crate::models::{ConnectionEditorField, ConnectionEditorSelect};
use nyaterm_ui::{
    NYA_FORM_CONTROL_HEIGHT_PX, NyaInput, NyaInputShell, NyaInputState, NyaNumberInput,
    NyaNumberInputState, NyaSelect, NyaSelectState, NyaSwitch,
};

pub(in crate::features) use crate::features::connections::state::list_model::{
    ConnectionListRow, ConnectionSectionHeader,
};

pub(super) fn connection_tree_indent_px(depth: usize) -> f32 {
    if depth == 0 {
        8.
    } else {
        8. + depth as f32 * 16. + 16.
    }
}

#[derive(Clone)]
pub(in crate::features) struct ConnectionEditorChoice {
    pub value: Option<String>,
    pub label: String,
    pub search_text: Option<String>,
    pub subtitle: Option<String>,
    pub selected: bool,
    pub disabled: bool,
}

impl ConnectionEditorChoice {
    pub(super) fn new(value: Option<String>, label: impl Into<String>, selected: bool) -> Self {
        Self {
            value,
            label: label.into(),
            search_text: None,
            subtitle: None,
            selected,
            disabled: false,
        }
    }

    pub(super) fn search_text(mut self, search_text: impl Into<String>) -> Self {
        self.search_text = Some(search_text.into());
        self
    }

    pub(super) fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }
}

pub(super) struct ConnectionEditorRenderContext<'a, 'cx> {
    pub palette: crate::theme::ThemePalette,
    pub fields: &'a ConnectionEditorFields,
    pub cx: &'a mut Context<'cx, NyaTermApp>,
}

pub(super) fn connection_editor_select(
    render: ConnectionEditorRenderContext<'_, '_>,
    id: &'static str,
    label: impl Into<FieldLabel>,
    select: ConnectionEditorSelect,
) -> impl IntoElement {
    let ConnectionEditorRenderContext {
        palette,
        fields,
        cx,
    } = render;
    let _ = cx;
    let label = label.into();
    let show_label = !label.is_empty();
    div()
        .id(SharedString::from(id))
        .anchor_scroll(
            fields
                .anchors
                .get(&ConnectionEditorFocusTarget::Select(select))
                .cloned(),
        )
        .min_w_0()
        .flex_1()
        .flex()
        .flex_col()
        .gap_1()
        .when(show_label, |this| {
            this.child(field_caption(palette, &label))
        })
        .child(
            div()
                .id(SharedString::from(format!("{id}-container")))
                .h(px(EDITOR_CONTROL_HEIGHT_PX))
                .min_w_0()
                .child(NyaSelect::new(&fields.select(select))),
        )
}

pub(super) fn editor_switch_row(
    palette: crate::theme::ThemePalette,
    id: &'static str,
    label: impl Into<SharedString>,
    checked: bool,
    on_click: impl Fn(&bool, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .w_full()
        .min_h(px(EDITOR_CONTROL_HEIGHT_PX))
        .flex_shrink_0()
        .py_1()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .text_xs()
        .text_color(rgb(palette.text))
        .child(div().min_w_0().flex_1().child(label.clone()))
        .child(
            div().flex_shrink_0().child(
                NyaSwitch::new(id)
                    .checked(checked)
                    .tooltip(label)
                    .on_click(on_click),
            ),
        )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use nyaterm_core::SavedConnection;

    #[test]
    fn saved_connection_tooltip_matches_tauri_fields_for_every_session_type() {
        let cases = [
            (
                serde_json::json!({
                    "type": "ssh",
                    "host": " ssh.example.com ",
                    "port": 2222,
                    "username": " root "
                }),
                vec![
                    "savedConnections.host",
                    "savedConnections.port",
                    "savedConnections.user",
                    "savedConnections.description",
                ],
            ),
            (
                serde_json::json!({
                    "type": "local_terminal",
                    "shell_path": " pwsh.exe ",
                    "shell_args": " -NoLogo ",
                    "working_dir": " C:\\\\Work "
                }),
                vec![
                    "savedConnections.terminalPath",
                    "savedConnections.shellArgs",
                    "savedConnections.workingDir",
                    "savedConnections.description",
                ],
            ),
            (
                serde_json::json!({
                    "type": "telnet",
                    "host": "switch.example.com",
                    "port": 2323,
                    "username": "operator"
                }),
                vec![
                    "savedConnections.host",
                    "savedConnections.port",
                    "savedConnections.user",
                    "savedConnections.description",
                ],
            ),
            (
                serde_json::json!({
                    "type": "serial",
                    "port_name": "COM3",
                    "baud_rate": 115200,
                    "data_bits": 8,
                    "parity": "none",
                    "stop_bits": "1"
                }),
                vec![
                    "savedConnections.serialPort",
                    "savedConnections.baudRate",
                    "savedConnections.dataBits",
                    "savedConnections.parity",
                    "savedConnections.stopBits",
                    "savedConnections.description",
                ],
            ),
            (
                serde_json::json!({
                    "type": "rdp",
                    "host": "desktop.example.com",
                    "port": 3390,
                    "username": "alice"
                }),
                vec![
                    "savedConnections.host",
                    "savedConnections.port",
                    "savedConnections.user",
                    "savedConnections.description",
                ],
            ),
            (
                serde_json::json!({
                    "type": "vnc",
                    "host": "screen.example.com",
                    "port": 5901
                }),
                vec![
                    "savedConnections.host",
                    "savedConnections.port",
                    "savedConnections.user",
                    "savedConnections.description",
                ],
            ),
        ];

        for (config, expected_keys) in cases {
            let mut connection = connection("test", "Test", None, 0);
            connection.config = serde_json::from_value(config).expect("valid connection type");
            connection.description = Some(" production host ".to_string());

            let rows = super::connection_detail_rows(&connection, &HashMap::new());
            let labels = rows
                .iter()
                .map(|row| row.label.as_str())
                .collect::<Vec<_>>();
            let expected = expected_keys
                .into_iter()
                .map(|key| rust_i18n::t!(key).into_owned())
                .collect::<Vec<_>>();

            assert_eq!(
                labels,
                expected,
                "unexpected fields for {}",
                connection.kind_label()
            );
            assert_eq!(
                rows.last().map(|row| row.value.as_str()),
                Some("production host")
            );
            let expected_copy_count = match connection.kind_label() {
                "SSH" | "Telnet" | "RDP" => 3,
                "VNC" => 2,
                _ => 0,
            };
            assert_eq!(
                rows.iter().filter(|row| row.copy_value.is_some()).count(),
                expected_copy_count,
                "unexpected copy buttons for {}",
                connection.kind_label()
            );
        }
    }

    #[test]
    fn saved_connection_tooltip_uses_tauri_missing_value_rules() {
        let mut local = connection("local", "Local", None, 0);
        local.config = serde_json::from_value(serde_json::json!({
            "type": "local_terminal",
            "shell_path": " ",
            "shell_args": " ",
            "working_dir": null
        }))
        .expect("valid local terminal");

        let local_rows = super::connection_detail_rows(&local, &HashMap::new());
        assert_eq!(local_rows.len(), 3, "blank shell args must be omitted");
        assert_eq!(
            local_rows[0].value,
            rust_i18n::t!("savedConnections.notSet").to_string()
        );
        assert_eq!(
            local_rows[1].value,
            rust_i18n::t!("savedConnections.notSet").to_string()
        );
        assert_eq!(
            local_rows[2].value,
            rust_i18n::t!("savedConnections.noDescription").to_string()
        );

        assert!(
            local_rows.iter().all(|row| row.copy_value.is_none()),
            "Tauri does not offer copy buttons for local terminal fields"
        );

        let mut vnc = connection("vnc", "VNC", None, 0);
        vnc.config = serde_json::from_value(serde_json::json!({
            "type": "vnc",
            "host": "screen.example.com"
        }))
        .expect("valid VNC connection");
        let vnc_rows = super::connection_detail_rows(&vnc, &HashMap::new());
        assert_eq!(
            vnc_rows[2].value,
            rust_i18n::t!("savedConnections.notSet").to_string(),
            "Tauri renders the unavailable VNC username as not set"
        );
        assert_eq!(
            vnc_rows[0].copy_value.as_deref(),
            Some("screen.example.com")
        );
        assert_eq!(vnc_rows[1].copy_value.as_deref(), Some("5900"));
        assert_eq!(vnc_rows[2].copy_value, None);

        let mut ssh = connection("ssh", "SSH", None, 0);
        ssh.config = serde_json::from_value(serde_json::json!({
            "type": "ssh",
            "host": " ",
            "port": 22,
            "username": " "
        }))
        .expect("valid SSH connection");
        let ssh_rows = super::connection_detail_rows(&ssh, &HashMap::new());
        assert_eq!(ssh_rows[0].copy_value, None);
        assert_eq!(ssh_rows[1].copy_value.as_deref(), Some("22"));
        assert_eq!(ssh_rows[2].copy_value, None);
    }

    fn connection(
        id: &str,
        name: &str,
        group_id: Option<&str>,
        sort_order: i32,
    ) -> SavedConnection {
        SavedConnection {
            extensions: Default::default(),
            tags: Vec::new(),
            id: id.to_string(),
            name: name.to_string(),
            config: nyaterm_core::ConnectionType::LocalTerminal {
                shell_path: String::new(),
                shell_args: String::new(),
                working_dir: None,
                ai_execution_profile: nyaterm_core::AiExecutionProfile::Auto,
                encoding: String::new(),
                dynamic_tab_title: false,
            },
            group_id: group_id.map(ToOwned::to_owned),
            description: None,
            sort_order,
            icon: None,
            icon_auto_detect: None,
            auth: None,
            recording: None,
            ssh_algorithms: None,
            ssh_compatibility_mode: false,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            network: None,
            post_login: None,
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        }
    }
}

/// What the editor's inputs and selects need from the app, resolved once per
/// render.
///
/// Sections receive this rather than the app, because they are free functions
/// that already hold a `Context` — reading the app entity again from inside its
/// own update panics.
pub(super) struct ConnectionEditorFields {
    pub(super) account: Option<nyaterm_core::SavedPassword>,
    pub(super) account_password_locked: bool,
    pub(super) errors: HashMap<ConnectionEditorField, String>,
    pub(super) anchors: HashMap<ConnectionEditorFocusTarget, gpui::ScrollAnchor>,
    fields: HashMap<ConnectionEditorField, Entity<NyaInputState>>,
    number_fields: HashMap<ConnectionEditorField, Entity<NyaNumberInputState>>,
    selects: HashMap<ConnectionEditorSelect, Entity<NyaSelectState>>,
    forwarding_endpoint_selects: HashMap<usize, Entity<NyaSelectState>>,
    forwarding_endpoint_fields: HashMap<(usize, ConnectionEditorField), Entity<NyaInputState>>,
}

impl ConnectionEditorFields {
    pub(super) fn new(
        fields: HashMap<ConnectionEditorField, Entity<NyaInputState>>,
        number_fields: HashMap<ConnectionEditorField, Entity<NyaNumberInputState>>,
        selects: HashMap<ConnectionEditorSelect, Entity<NyaSelectState>>,
        forwarding_endpoint_selects: HashMap<usize, Entity<NyaSelectState>>,
        forwarding_endpoint_fields: HashMap<(usize, ConnectionEditorField), Entity<NyaInputState>>,
    ) -> Self {
        Self {
            account: None,
            account_password_locked: false,
            errors: HashMap::new(),
            anchors: HashMap::new(),
            fields,
            number_fields,
            selects,
            forwarding_endpoint_selects,
            forwarding_endpoint_fields,
        }
    }

    pub(super) fn get(&self, field: &ConnectionEditorField) -> Option<&Entity<NyaInputState>> {
        self.fields.get(field)
    }

    pub(super) fn get_number(
        &self,
        field: &ConnectionEditorField,
    ) -> Option<&Entity<NyaNumberInputState>> {
        self.number_fields.get(field)
    }

    pub(super) fn select(&self, select: ConnectionEditorSelect) -> Entity<NyaSelectState> {
        self.selects
            .get(&select)
            .cloned()
            .expect("connection editor select registered before rendering")
    }

    pub(super) fn forwarding_endpoint_select(&self, index: usize) -> Entity<NyaSelectState> {
        self.forwarding_endpoint_selects
            .get(&index)
            .cloned()
            .expect("forwarding endpoint select registered before rendering")
    }

    pub(super) fn forwarding_endpoint_field(
        &self,
        index: usize,
        field: ConnectionEditorField,
    ) -> Entity<NyaInputState> {
        self.forwarding_endpoint_fields
            .get(&(index, field))
            .cloned()
            .expect("forwarding endpoint field registered before rendering")
    }
}

/// A field's caption, and whether it carries the required marker.
#[derive(Clone, Default)]
pub(super) struct FieldLabel {
    text: SharedString,
    required: bool,
}

impl FieldLabel {
    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

impl From<&'static str> for FieldLabel {
    fn from(text: &'static str) -> Self {
        Self {
            text: SharedString::from(text),
            required: false,
        }
    }
}

impl From<String> for FieldLabel {
    fn from(text: String) -> Self {
        Self {
            text: SharedString::from(text),
            required: false,
        }
    }
}

impl<'a> From<Cow<'a, str>> for FieldLabel {
    fn from(text: Cow<'a, str>) -> Self {
        Self {
            text: SharedString::from(text),
            required: false,
        }
    }
}

impl From<SharedString> for FieldLabel {
    fn from(text: SharedString) -> Self {
        Self {
            text,
            required: false,
        }
    }
}

/// Mark a caption as required, so it renders with the red asterisk.
pub(super) fn required(label: impl Into<FieldLabel>) -> FieldLabel {
    let mut label = label.into();
    label.required = true;
    label
}

/// The caption above an input or a select.
pub(super) fn field_caption(
    palette: crate::theme::ThemePalette,
    label: &FieldLabel,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_0p5()
        .text_xs()
        .text_color(rgb(palette.text_muted))
        .child(label.text.clone())
        .when(label.required, |this| {
            this.child(div().text_color(rgb(palette.danger)).child("*"))
        })
}

/// The height every input, select and stepper in the editor shares.
pub(super) const EDITOR_CONTROL_HEIGHT_PX: f32 = NYA_FORM_CONTROL_HEIGHT_PX;

/// A caption above one editable field.
///
/// Tauri puts the caption above the box rather than inside it; a box that also
/// holds its label has half the room for what is typed into it, which is what
/// made long hosts and paths unreadable here.
pub(super) fn editor_field(
    palette: crate::theme::ThemePalette,
    label: impl Into<FieldLabel>,
    field: ConnectionEditorField,
    fields: &ConnectionEditorFields,
    cx: &Context<NyaTermApp>,
) -> impl IntoElement {
    let label = label.into();
    div()
        .id(SharedString::from(format!(
            "connection-editor-field-{field:?}"
        )))
        .debug_selector(move || format!("connection-editor-field-{field:?}"))
        .anchor_scroll(
            fields
                .anchors
                .get(&ConnectionEditorFocusTarget::Field(field))
                .cloned(),
        )
        .flex()
        .flex_col()
        .gap_1()
        .when(!label.is_empty(), |this| {
            this.child(field_caption(palette, &label))
        })
        .child(editor_field_box(palette, field, fields, cx))
        .when_some(fields.errors.get(&field), |this, error| {
            this.child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.danger))
                    .child(error.clone()),
            )
        })
}

pub(super) fn connection_editor_select_on_open(
    render: ConnectionEditorRenderContext<'_, '_>,
    id: &'static str,
    label: impl Into<FieldLabel>,
    select: ConnectionEditorSelect,
    on_open: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let ConnectionEditorRenderContext {
        palette,
        fields,
        cx,
    } = render;
    let _ = cx;
    let label = label.into();
    let show_label = !label.is_empty();
    div()
        .id(SharedString::from(id))
        .anchor_scroll(
            fields
                .anchors
                .get(&ConnectionEditorFocusTarget::Select(select))
                .cloned(),
        )
        .min_w_0()
        .flex_1()
        .flex()
        .flex_col()
        .gap_1()
        .when(show_label, |this| {
            this.child(field_caption(palette, &label))
        })
        .child(
            div()
                .id(SharedString::from(format!("{id}-container")))
                .h(px(EDITOR_CONTROL_HEIGHT_PX))
                .min_w_0()
                .child(NyaSelect::new(&fields.select(select)).on_open(on_open)),
        )
}

pub(super) fn forwarding_endpoint_editor_field(
    palette: crate::theme::ThemePalette,
    label: impl Into<FieldLabel>,
    index: usize,
    field: ConnectionEditorField,
    fields: &ConnectionEditorFields,
    cx: &App,
) -> impl IntoElement {
    let label = label.into();
    let entity = fields.forwarding_endpoint_field(index, field);
    let handle = entity.read(cx).focus_handle();
    let focused = entity.read(cx).has_focus();
    div()
        .id(SharedString::from(format!(
            "connection-editor-forwarding-{index}-{field:?}"
        )))
        .anchor_scroll(
            fields
                .anchors
                .get(&ConnectionEditorFocusTarget::ForwardingEndpoint { index, field })
                .cloned(),
        )
        .flex()
        .flex_col()
        .gap_1()
        .when(!label.is_empty(), |this| {
            this.child(field_caption(palette, &label))
        })
        .child(
            div()
                .h(px(EDITOR_CONTROL_HEIGHT_PX))
                .id(SharedString::from(format!(
                    "connection-agent-endpoint-{index}-field"
                )))
                .min_w_0()
                .px(px(ORDINARY_INPUT_SHELL_PADDING_X_PX))
                .flex()
                .items_center()
                .rounded_sm()
                .border_1()
                .border_color(ordinary_input_shell_border_color(palette, focused))
                .when(focused, |this| {
                    this.shadow(ordinary_input_focus_ring(palette))
                })
                .bg(rgb(palette.input_surface()))
                .cursor_text()
                .on_click(move |_, window, cx| {
                    window.focus(&handle, cx);
                })
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_xs()
                        .text_color(rgb(palette.text))
                        .child(NyaInput::new(&entity)),
                ),
        )
}

/// The bordered box holding one field, without a caption.
pub(super) fn editor_field_box(
    palette: crate::theme::ThemePalette,
    field: ConnectionEditorField,
    fields: &ConnectionEditorFields,
    cx: &Context<NyaTermApp>,
) -> impl IntoElement {
    let entity = fields.get(&field);
    if field == ConnectionEditorField::PostLoginCommand {
        let app = cx.weak_entity();
        return div()
            .w_full()
            .min_w_0()
            .flex_none()
            .children(entity.map(|field| {
                NyaInputShell::new("connection-editor-post-login-command", field)
                    .multi_line()
                    .height(px(168.))
                    .tab_navigation()
                    .on_secondary_enter(move |window, cx| {
                        let _ = app.update(cx, |app, cx| app.save_connection_editor(window, cx));
                    })
            }))
            .into_any_element();
    }
    let handle = entity.map(|field| field.read(cx).focus_handle());
    let focused = entity.is_some_and(|field| field.read(cx).has_focus());
    div()
        .h(px(if field == ConnectionEditorField::Description {
            112.
        } else {
            EDITOR_CONTROL_HEIGHT_PX
        }))
        .id("connection-list-search-input-shell")
        .min_w_0()
        .px(px(ORDINARY_INPUT_SHELL_PADDING_X_PX))
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(ordinary_input_shell_border_color(palette, focused))
        .when(focused, |this| {
            this.shadow(ordinary_input_focus_ring(palette))
        })
        .bg(rgb(palette.input_surface()))
        .cursor_text()
        .when_some(handle, |row, handle| {
            row.on_click(move |_, window, cx| {
                window.focus(&handle, cx);
            })
        })
        .children(entity.map(|field| {
            div()
                .min_w_0()
                .flex_1()
                .text_xs()
                .text_color(rgb(palette.text))
                .child(NyaInput::new(field))
        }))
        .into_any_element()
}

/// A numeric field backed by gpui-kit's spinner control.
pub(super) fn editor_stepper_field(
    palette: crate::theme::ThemePalette,
    label: impl Into<FieldLabel>,
    field: ConnectionEditorField,
    fields: &ConnectionEditorFields,
    _cx: &App,
) -> impl IntoElement {
    let label = label.into();
    let entity = fields.get_number(&field);
    div()
        .id(SharedString::from(format!(
            "connection-editor-field-{field:?}"
        )))
        .debug_selector(move || format!("connection-editor-field-{field:?}"))
        .anchor_scroll(
            fields
                .anchors
                .get(&ConnectionEditorFocusTarget::Field(field))
                .cloned(),
        )
        .flex()
        .flex_col()
        .gap_1()
        .when(!label.is_empty(), |this| {
            this.child(field_caption(palette, &label))
        })
        .child(
            div()
                .h(px(EDITOR_CONTROL_HEIGHT_PX))
                .min_w_0()
                .flex()
                .items_center()
                .children(entity.map(|field| {
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_xs()
                        .text_color(rgb(palette.text))
                        .child(NyaNumberInput::new(field))
                })),
        )
        .when_some(fields.errors.get(&field), |this, error| {
            this.child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.danger))
                    .child(error.clone()),
            )
        })
}

pub(super) fn icon_action_button(
    palette: crate::theme::ThemePalette,
    id: impl Into<String>,
    label: &'static str,
    tooltip: impl Into<String>,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    icon_action_button_styled(palette, id, label, tooltip, None, false, on_click)
}

/// [`icon_action_button`] with the two knobs the sort control needs: a tint that
/// marks the button as active, and a vertical flip that turns an ascending glyph
/// into a descending one.
pub(super) fn icon_action_button_styled(
    palette: crate::theme::ThemePalette,
    id: impl Into<String>,
    label: &'static str,
    tooltip: impl Into<String>,
    tint: Option<u32>,
    flip_vertical: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    // label may be a glyph fallback or an icons/*.svg path.
    let is_svg = label.starts_with("icons/") && label.ends_with(".svg");
    let tooltip = tooltip.into();
    let id = SharedString::from(id.into());
    let base_color = tint.unwrap_or(palette.text_muted);
    let hover_color = tint.unwrap_or(palette.text);
    // `svg()` takes its tint from its *own* computed style — GPUI starts every
    // element from `Style::default()`, so a glyph with no `text_color` of its own
    // resolves to `None` and is skipped entirely at paint time. The parent's
    // `text_color` only reaches real text. Hence the explicit color here, and the
    // group so hover still brightens the icon rather than only the background.
    div()
        .id(id.clone())
        .group(id.clone())
        .size(px(24.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_size(px(11.))
        .text_color(rgb(base_color))
        .cursor_pointer()
        .hover(move |this| {
            this.bg(rgb(palette.surface_elevated))
                .text_color(rgb(hover_color))
        })
        .on_click(on_click)
        .tooltip(move |window, cx| nyaterm_ui::NyaTooltip::new(tooltip.clone()).build(window, cx))
        .when(is_svg, |this| {
            let mut icon = svg()
                .size(px(14.))
                .flex_none()
                .path(label)
                .text_color(rgb(base_color))
                .group_hover(id.clone(), move |this| this.text_color(rgb(hover_color)));
            if flip_vertical {
                icon = icon.with_transformation(gpui::Transformation::scale(gpui::size(1., -1.)));
            }
            this.child(icon)
        })
        .when(!is_svg, |this| this.child(label))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ConnectionDetailRow {
    pub(super) label: String,
    pub(super) value: String,
    pub(super) copy_value: Option<String>,
    pub(super) multiline: bool,
}

impl ConnectionDetailRow {
    fn new(label: String, value: String) -> Self {
        Self {
            label,
            value,
            copy_value: None,
            multiline: false,
        }
    }

    fn copyable(mut self, copy_value: Option<String>) -> Self {
        self.copy_value = copy_value;
        self
    }

    fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }
}

pub(super) fn connection_detail_rows(
    connection: &SavedConnection,
    all_connections: &HashMap<String, SavedConnection>,
) -> Vec<ConnectionDetailRow> {
    let description = connection
        .description
        .as_deref()
        .and_then(non_empty_detail_value)
        .unwrap_or_else(|| t!("savedConnections.noDescription").to_string());

    let mut rows = match &connection.config {
        nyaterm_core::ConnectionType::Adb {
            device, directory, ..
        } => vec![
            copyable_value_detail_row("Android".into(), device.serial()),
            copyable_text_detail_row(t!("android.directory").into_owned(), directory),
        ],
        nyaterm_core::ConnectionType::Ssh {
            host,
            port,
            username,
            ..
        } => vec![
            copyable_text_detail_row(t!("savedConnections.host").to_string(), host),
            copyable_value_detail_row(t!("savedConnections.port").to_string(), port.to_string()),
            copyable_text_detail_row(t!("savedConnections.user").to_string(), username),
        ],
        nyaterm_core::ConnectionType::LocalTerminal {
            shell_path,
            shell_args,
            working_dir,
            ..
        } => {
            let mut rows = vec![ConnectionDetailRow::new(
                t!("savedConnections.terminalPath").to_string(),
                required_detail_value(shell_path),
            )];
            if let Some(shell_args) = non_empty_detail_value(shell_args) {
                rows.push(ConnectionDetailRow::new(
                    t!("savedConnections.shellArgs").to_string(),
                    shell_args,
                ));
            }
            rows.push(ConnectionDetailRow::new(
                t!("savedConnections.workingDir").to_string(),
                working_dir
                    .as_deref()
                    .map(required_detail_value)
                    .unwrap_or_else(|| t!("savedConnections.notSet").to_string()),
            ));
            rows
        }
        nyaterm_core::ConnectionType::Telnet {
            host,
            port,
            username,
            ..
        } => vec![
            copyable_text_detail_row(t!("savedConnections.host").to_string(), host),
            copyable_value_detail_row(t!("savedConnections.port").to_string(), port.to_string()),
            copyable_text_detail_row(t!("savedConnections.user").to_string(), username),
        ],
        nyaterm_core::ConnectionType::Serial {
            port_name,
            baud_rate,
            data_bits,
            parity,
            stop_bits,
            ..
        } => {
            let mut rows = vec![
                ConnectionDetailRow::new(
                    t!("savedConnections.serialPort").to_string(),
                    required_detail_value(port_name),
                ),
                ConnectionDetailRow::new(
                    t!("savedConnections.baudRate").to_string(),
                    baud_rate.to_string(),
                ),
                ConnectionDetailRow::new(
                    t!("savedConnections.dataBits").to_string(),
                    data_bits.to_string(),
                ),
            ];
            if let Some(parity) = non_empty_detail_value(parity) {
                rows.push(ConnectionDetailRow::new(
                    t!("savedConnections.parity").to_string(),
                    parity,
                ));
            }
            if let Some(stop_bits) = non_empty_detail_value(stop_bits) {
                rows.push(ConnectionDetailRow::new(
                    t!("savedConnections.stopBits").to_string(),
                    stop_bits,
                ));
            }
            rows
        }
        nyaterm_core::ConnectionType::Rdp {
            host,
            port,
            username,
            ..
        } => vec![
            copyable_text_detail_row(t!("savedConnections.host").to_string(), host),
            copyable_value_detail_row(t!("savedConnections.port").to_string(), port.to_string()),
            copyable_text_detail_row(t!("savedConnections.user").to_string(), username),
        ],
        nyaterm_core::ConnectionType::Vnc { host, port, .. } => vec![
            copyable_text_detail_row(t!("savedConnections.host").to_string(), host),
            copyable_value_detail_row(t!("savedConnections.port").to_string(), port.to_string()),
            ConnectionDetailRow::new(
                t!("savedConnections.user").to_string(),
                t!("savedConnections.notSet").to_string(),
            ),
        ],
    };

    if matches!(&connection.config, nyaterm_core::ConnectionType::Ssh { .. })
        && connection
            .network
            .as_ref()
            .and_then(|network| network.proxy_jump_id.as_deref())
            .is_some()
    {
        rows.push(
            ConnectionDetailRow::new(
                t!("savedConnections.jumpHostChain").to_string(),
                format_jump_host_chain(connection, all_connections),
            )
            .multiline(),
        );
    }
    rows.push(
        ConnectionDetailRow::new(t!("savedConnections.description").to_string(), description)
            .multiline(),
    );
    rows
}

fn required_detail_value(value: &str) -> String {
    non_empty_detail_value(value).unwrap_or_else(|| t!("savedConnections.notSet").to_string())
}

fn copyable_text_detail_row(label: String, value: &str) -> ConnectionDetailRow {
    let copy_value = non_empty_detail_value(value);
    ConnectionDetailRow::new(label, required_detail_value(value)).copyable(copy_value)
}

fn copyable_value_detail_row(label: String, value: String) -> ConnectionDetailRow {
    ConnectionDetailRow::new(label, value.clone()).copyable(Some(value))
}

fn non_empty_detail_value(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

pub(super) fn format_jump_host_chain(
    connection: &SavedConnection,
    by_id: &HashMap<String, SavedConnection>,
) -> String {
    let Some(mut jump_id) = connection
        .network
        .as_ref()
        .and_then(|network| network.proxy_jump_id.clone())
    else {
        return t!("savedConnections.notSet").to_string();
    };
    let mut seen = std::collections::HashSet::new();
    seen.insert(connection.id.clone());
    let mut labels = Vec::new();
    loop {
        if !seen.insert(jump_id.clone()) {
            labels.push(t!("savedConnections.jumpHostCycle").to_string());
            break;
        }
        let Some(jump) = by_id.get(jump_id.as_str()) else {
            labels.push(t!("savedConnections.jumpHostMissing").to_string());
            break;
        };
        labels.push(jump.name.clone());
        match jump
            .network
            .as_ref()
            .and_then(|network| network.proxy_jump_id.clone())
        {
            Some(next) => jump_id = next,
            None => break,
        }
    }
    if labels.is_empty() {
        t!("savedConnections.notSet").to_string()
    } else {
        labels.join(" -> ")
    }
}
