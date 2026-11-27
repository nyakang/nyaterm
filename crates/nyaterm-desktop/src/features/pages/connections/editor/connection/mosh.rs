use super::super::super::list::editor_field;
use super::ConnectionEditorSectionContext;
use crate::features::NyaTermApp;
use crate::models::ConnectionEditorField;
use gpui::{
    Context, IntoElement, div,
    prelude::{ParentElement, Styled},
};
use nyaterm_core::mosh::MoshPrediction;
use nyaterm_ui::{NyaTabItem, NyaTabs};
use rust_i18n::t;

pub(super) fn settings(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let mut panel = div().flex().flex_col().gap_3();
    for (key, field) in [
        ("mosh.serverPath", ConnectionEditorField::MoshServerPath),
        ("mosh.udpHost", ConnectionEditorField::MoshUdpHost),
        ("mosh.portStart", ConnectionEditorField::MoshPortStart),
        ("mosh.portEnd", ConnectionEditorField::MoshPortEnd),
    ] {
        panel = panel.child(editor_field(
            section.palette,
            t!(key),
            field,
            section.fields,
            cx,
        ));
    }
    panel
        .child(div().text_xs().child(t!("mosh.routing")))
        .child(div().text_xs().child(t!("mosh.prediction")))
        .child(
            NyaTabs::new("mosh-prediction")
                .items([
                    NyaTabItem::new(t!("mosh.adaptive")),
                    NyaTabItem::new(t!("mosh.always")),
                    NyaTabItem::new(t!("mosh.never")),
                ])
                .selected_index(match section.editor.mosh.prediction {
                    MoshPrediction::Adaptive => 0,
                    MoshPrediction::Always => 1,
                    MoshPrediction::Never => 2,
                })
                .on_select(cx.listener(|app, index, _, cx| {
                    app.connection_state.set_mosh_prediction(match *index {
                        1 => MoshPrediction::Always,
                        2 => MoshPrediction::Never,
                        _ => MoshPrediction::Adaptive,
                    });
                    cx.notify();
                }))
                .into_any_element(),
        )
}

pub(super) fn terminal(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    use super::super::super::list::{
        ConnectionEditorRenderContext, connection_editor_select, editor_switch_row,
    };
    use crate::features::connections::ConnectionEditorToggle;
    use crate::models::ConnectionEditorSelect;
    let mut panel = div()
        .flex()
        .flex_col()
        .gap_3()
        .child(t!("mosh.terminal"))
        .child(connection_editor_select(
            ConnectionEditorRenderContext {
                palette: section.palette,
                fields: section.fields,
                cx,
            },
            "mosh-backspace",
            t!("dialog.backspaceMode"),
            ConnectionEditorSelect::Backspace,
        ))
        .child(editor_switch_row(
            section.palette,
            "mosh-dynamic-title",
            t!("dialog.dynamicTabTitle"),
            section.editor.dynamic_tab_title,
            cx.listener(|app, _, _, cx| {
                app.toggle_connection_editor_flag(ConnectionEditorToggle::DynamicTabTitle, cx)
            }),
        ))
        .child(super::recording::connection_editor_recording_section(
            section, cx,
        ));
    if section.editor.defaults_mode {
        panel = panel.child(settings(section, cx));
    }
    panel
}
