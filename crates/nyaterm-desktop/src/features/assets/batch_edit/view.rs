//! Batch editor body.
//!
//! The window, tabs, navigation, section order, field order and control sizes
//! mirror the connection editor. Fields the connection editor has but the batch
//! editor may not write are still rendered in their original position, disabled.
//! A field shows what the selection holds: a uniform value as-is, a mixed value
//! as `<保持不变>`, and an empty value stays empty. Editing the control is what
//! records a change; disabled fields never record one.

use gpui::{AnyElement, Context, FontWeight, IntoElement, SharedString, div, prelude::*, px, rgb};
use nyaterm_core::{
    ConnectionBatchKind, Group, OtpEntry, ProxyConfig, RecordingMode, SavedConnection,
    SavedPassword, SshAgentEndpoint, SshAgentForwardingPolicy, SshKey,
};
use nyaterm_ui::{
    NYA_FORM_CONTROL_HEIGHT_PX, NyaButton, NyaButtonVariant, NyaIconButton, NyaInputShell,
    NyaPopover, NyaScrollable, NyaSelect, NyaSelectOption, NyaSwitch, NyaTabItem, NyaTabs,
};
use rust_i18n::t;

use super::{
    ASSET_BATCH_SELECT_PREFIX, ASSET_BATCH_TEXT_PREFIX, AssetBatchEditDraft, BatchFieldId,
    FieldBasis, batch_field_in_kind, batch_kind_tab, batch_kind_value,
};
use crate::features::NyaTermApp;
use crate::features::icons::{
    CONNECTION_ICON_OPTIONS, DEFAULT_CONNECTION_ICON, resolve_connection_icon,
};
use crate::features::pages::connections::editor::connection::options;
use crate::features::pages::connections::list::ConnectionEditorChoice;
use crate::features::view_widgets::themed_icon;
use crate::models::ConnectionEditorSection;

const BATCH_NAV_WIDTH_PX: f32 = 180.;
const BATCH_NAV_WIDTH_NARROW_PX: f32 = 128.;
const BATCH_DESCRIPTION_HEIGHT_PX: f32 = 56.;
const BATCH_COMMAND_HEIGHT_PX: f32 = 168.;
const BATCH_WIDE_WIDTH_PX: f32 = 760.;
const BATCH_NARROW_CONTENT_WIDTH_PX: f32 = 500.;

/// Tabs in the same order as the connection editor.
const BATCH_TAB_KINDS: [ConnectionBatchKind; 6] = [
    ConnectionBatchKind::Ssh,
    ConnectionBatchKind::LocalTerminal,
    ConnectionBatchKind::Telnet,
    ConnectionBatchKind::Serial,
    ConnectionBatchKind::Rdp,
    ConnectionBatchKind::Vnc,
];

/// Maps the connection editor's shared choices onto batch select options.
/// A `None` choice (none / manual identity / automatic) keeps the same label
/// and uses the empty value so selecting it clears the reference.
fn choice_options(choices: Vec<ConnectionEditorChoice>) -> Vec<NyaSelectOption> {
    choices
        .into_iter()
        .map(|choice| {
            let value = choice.value.unwrap_or_default();
            let mut option = NyaSelectOption::new(value, choice.label);
            if let Some(search_text) = choice.search_text {
                option = option.search_text(search_text);
            }
            if let Some(subtitle) = choice.subtitle {
                option = option.subtitle(subtitle);
            }
            option
        })
        .collect()
}

pub(super) fn group_select_options(groups: &[Group]) -> Vec<NyaSelectOption> {
    let root = t!("assets.title").to_string();
    let mut options = vec![NyaSelectOption::new("", root.clone())];
    options.extend(
        nyaterm_core::build_group_options(groups, &root)
            .into_iter()
            .map(|option| {
                NyaSelectOption::new(option.group.id, option.path).search_text(option.search_text)
            }),
    );
    options
}

pub(super) fn icon_select_options() -> Vec<NyaSelectOption> {
    CONNECTION_ICON_OPTIONS
        .iter()
        .map(|icon| NyaSelectOption::new(*icon, *icon))
        .collect()
}

pub(super) fn default_icon_value() -> Option<String> {
    Some(DEFAULT_CONNECTION_ICON.to_string())
}

pub(super) fn kind_select_options() -> Vec<NyaSelectOption> {
    BATCH_TAB_KINDS
        .into_iter()
        .map(|kind| NyaSelectOption::new(batch_kind_value(kind), batch_kind_label(kind)))
        .collect()
}

pub(super) fn encoding_select_options() -> Vec<NyaSelectOption> {
    choice_options(options::encoding_choices(""))
}

pub(super) fn backspace_select_options() -> Vec<NyaSelectOption> {
    choice_options(options::backspace_choices(""))
}

pub(super) fn ssh_profile_select_options() -> Vec<NyaSelectOption> {
    choice_options(options::ssh_profile_choices(
        nyaterm_core::SshProfile::Standard,
    ))
}

pub(super) fn terminal_type_select_options(
    profile: nyaterm_core::SshProfile,
) -> Vec<NyaSelectOption> {
    choice_options(options::ssh_terminal_type_choices(profile, None))
}

pub(super) fn sftp_cwd_follow_select_options() -> Vec<NyaSelectOption> {
    choice_options(options::sftp_cwd_follow_choices(""))
}

pub(super) fn sftp_filename_encoding_select_options() -> Vec<NyaSelectOption> {
    choice_options(options::sftp_filename_encoding_choices(""))
}

pub(super) fn sftp_pipeline_depth_select_options(selected: &str) -> Vec<NyaSelectOption> {
    let depth = selected.trim().parse::<u32>().ok();
    choice_options(options::sftp_pipeline_depth_choices(depth))
}

pub(super) fn ssh_algorithm_mode_select_options() -> Vec<NyaSelectOption> {
    choice_options(options::ssh_algorithm_mode_choices(""))
}

pub(super) fn rdp_certificate_policy_select_options() -> Vec<NyaSelectOption> {
    choice_options(options::rdp_certificate_policy_choices("prompt"))
}

pub(super) fn vnc_security_select_options() -> Vec<NyaSelectOption> {
    choice_options(options::vnc_security_choices("auto"))
}

pub(super) fn account_select_options(
    passwords: &[SavedPassword],
    kind: Option<ConnectionBatchKind>,
    selected: &str,
) -> Vec<NyaSelectOption> {
    let selected = (!selected.trim().is_empty()).then_some(selected);
    let choices = match kind {
        Some(ConnectionBatchKind::Rdp | ConnectionBatchKind::Vnc) => {
            options::password_choices(passwords, selected, selected.is_none(), selected)
        }
        _ => options::account_choices(passwords, selected, selected.is_none(), selected),
    };
    choice_options(choices)
}

pub(super) fn key_select_options(keys: &[SshKey], selected: &str) -> Vec<NyaSelectOption> {
    let selected = (!selected.trim().is_empty()).then_some(selected);
    choice_options(options::ssh_key_choices(keys, selected))
}

pub(super) fn otp_select_options(entries: &[OtpEntry], selected: &str) -> Vec<NyaSelectOption> {
    let selected = (!selected.trim().is_empty()).then_some(selected);
    choice_options(options::otp_choices(entries, selected))
}

pub(super) fn proxy_select_options(
    proxies: &[ProxyConfig],
    selected: &str,
) -> Vec<NyaSelectOption> {
    let selected = (!selected.trim().is_empty()).then_some(selected);
    choice_options(options::proxy_choices(proxies, selected))
}

pub(super) fn proxy_jump_select_options(
    connections: &[SavedConnection],
    groups: &[Group],
    selected: &str,
    selected_ids: &[String],
) -> Vec<NyaSelectOption> {
    let selected = (!selected.trim().is_empty()).then_some(selected);
    let mut choices = options::proxy_jump_choices(connections, groups, None, selected);
    choices.retain(|choice| {
        choice
            .value
            .as_deref()
            .is_none_or(|value| !selected_ids.iter().any(|id| id == value))
    });
    choice_options(choices)
}

pub(super) fn ssh_agent_endpoint_select_options(selected: &str) -> Vec<NyaSelectOption> {
    let endpoint = match selected {
        "environment" => SshAgentEndpoint::Environment {
            variable: String::new(),
        },
        "unix_socket" => SshAgentEndpoint::UnixSocket {
            path: String::new(),
        },
        "pageant" => SshAgentEndpoint::Pageant,
        "windows_openssh" => SshAgentEndpoint::WindowsOpenSsh,
        _ => SshAgentEndpoint::Auto,
    };
    choice_options(options::ssh_agent_endpoint_choices(&endpoint))
}

pub(super) fn ssh_agent_policy_select_options(selected: &str) -> Vec<NyaSelectOption> {
    let policy = if selected == "all" {
        SshAgentForwardingPolicy::All
    } else {
        SshAgentForwardingPolicy::Allowlist {
            fingerprints: Vec::new(),
        }
    };
    choice_options(options::ssh_agent_forwarding_policy_choices(&policy))
}

pub(super) fn rdp_backend_select_options(selected: &str) -> Vec<NyaSelectOption> {
    choice_options(options::rdp_backend_choices(selected, selected == "mstsc"))
}

pub(super) fn rdp_display_mode_select_options(selected: &str) -> Vec<NyaSelectOption> {
    choice_options(options::rdp_display_mode_choices(selected))
}

pub(super) fn vnc_scale_select_options(selected: &str) -> Vec<NyaSelectOption> {
    choice_options(options::vnc_scale_choices(selected))
}

pub(super) fn recording_mode_select_options(selected: &str) -> Vec<NyaSelectOption> {
    let mode = if selected == "raw" {
        RecordingMode::Raw
    } else {
        RecordingMode::Transcript
    };
    choice_options(options::recording_mode_choices(mode))
}

pub(super) fn serial_port_select_options(ports: &[String], selected: &str) -> Vec<NyaSelectOption> {
    choice_options(options::serial_port_choices(ports, selected))
}

pub(super) fn baud_rate_select_options(selected: &str) -> Vec<NyaSelectOption> {
    choice_options(options::baud_rate_choices(selected))
}

pub(super) fn data_bits_select_options(selected: &str) -> Vec<NyaSelectOption> {
    choice_options(options::data_bits_choices(selected))
}

pub(super) fn parity_select_options(selected: &str) -> Vec<NyaSelectOption> {
    choice_options(options::parity_choices(selected))
}

pub(super) fn stop_bits_select_options(selected: &str) -> Vec<NyaSelectOption> {
    choice_options(options::stop_bits_choices(selected))
}

pub(super) fn flow_control_select_options(selected: &str) -> Vec<NyaSelectOption> {
    choice_options(options::flow_control_choices(selected))
}

pub(super) fn telnet_enter_select_options(selected: &str) -> Vec<NyaSelectOption> {
    choice_options(options::telnet_enter_choices(selected))
}

fn batch_field_is_select(field: BatchFieldId) -> bool {
    matches!(
        field,
        BatchFieldId::Group
            | BatchFieldId::Icon
            | BatchFieldId::Encoding
            | BatchFieldId::Backspace
            | BatchFieldId::SshProfile
            | BatchFieldId::TerminalType
            | BatchFieldId::SftpCwdFollow
            | BatchFieldId::SftpFilenameEncoding
            | BatchFieldId::SftpPipelineDepth
            | BatchFieldId::SshAlgorithmMode
            | BatchFieldId::RdpCertificatePolicy
            | BatchFieldId::VncSecurityMode
            | BatchFieldId::SshAuthAgentEndpoint
            | BatchFieldId::Account
            | BatchFieldId::Key
            | BatchFieldId::Otp
            | BatchFieldId::Proxy
            | BatchFieldId::ProxyJump
            | BatchFieldId::SerialPort
            | BatchFieldId::BaudRate
            | BatchFieldId::DataBits
            | BatchFieldId::Parity
            | BatchFieldId::StopBits
            | BatchFieldId::FlowControl
            | BatchFieldId::TelnetEnterMode
            | BatchFieldId::RdpBackend
            | BatchFieldId::RdpDisplayMode
            | BatchFieldId::VncScaleMode
            | BatchFieldId::RecordingMode
            | BatchFieldId::AgentPolicy
    )
}

/// Current displayed value when the draft can determine one, mirroring the
/// editor's own visibility checks (mixed and untouched means "unknown").
fn batch_current_text(draft: &AssetBatchEditDraft, field: BatchFieldId) -> Option<&str> {
    (draft.is_edited(field) || draft.basis(field) == FieldBasis::Uniform)
        .then(|| draft.value(field))
}

fn batch_current_bool(draft: &AssetBatchEditDraft, field: BatchFieldId) -> Option<bool> {
    (draft.is_edited(field) || draft.basis(field) == FieldBasis::Uniform)
        .then(|| draft.bool_value(field))
}

fn batch_rdp_uses_mstsc(draft: &AssetBatchEditDraft) -> bool {
    matches!(
        batch_current_text(draft, BatchFieldId::RdpBackend),
        Some("mstsc")
    )
}

fn batch_rdp_display_fixed(draft: &AssetBatchEditDraft) -> bool {
    // The editor shows the size fields only for the stored `fixed` mode, not
    // for the `native` alias (whose select entry normalizes to `fixed`).
    matches!(
        batch_current_text(draft, BatchFieldId::RdpDisplayMode),
        Some("fixed")
    )
}

/// The editor hides the password source when authentication is explicitly not
/// password-based. Mixed/unknown drafts stay editable.
fn batch_auth_hides_password(draft: &AssetBatchEditDraft) -> bool {
    matches!(
        batch_current_text(draft, BatchFieldId::AuthMode),
        Some(mode) if mode != "password"
    )
}

fn batch_vnc_requires_no_auth(draft: &AssetBatchEditDraft) -> bool {
    matches!(
        batch_current_text(draft, BatchFieldId::VncSecurityMode),
        Some("none")
    )
}

fn batch_dynamic_disabled_reason(
    field: BatchFieldId,
    draft: &AssetBatchEditDraft,
) -> Option<&'static str> {
    if matches!(
        field,
        BatchFieldId::TelnetSendNaws | BatchFieldId::TelnetSendSga
    ) && batch_current_bool(draft, BatchFieldId::TelnetRawTcp) == Some(true)
    {
        return Some("dialog.telnetRawTcpCliDesc");
    }
    None
}

pub(super) fn batch_kind_label(kind: ConnectionBatchKind) -> SharedString {
    match kind {
        ConnectionBatchKind::Ssh => SharedString::from("SSH"),
        ConnectionBatchKind::LocalTerminal => t!("dialog.localTerminal").into(),
        ConnectionBatchKind::Telnet => SharedString::from("Telnet"),
        ConnectionBatchKind::Serial => t!("dialog.serial").into(),
        ConnectionBatchKind::Rdp => SharedString::from("RDP"),
        ConnectionBatchKind::Vnc => SharedString::from("VNC"),
    }
}

fn tab_index(kind: ConnectionBatchKind) -> usize {
    BATCH_TAB_KINDS
        .iter()
        .position(|candidate| *candidate == kind)
        .unwrap_or(0)
}

/// Disabled fields that the connection editor paints as a select keep the
/// chevron, even when the batch editor cannot write them.
fn batch_field_has_select_appearance(field: BatchFieldId) -> bool {
    batch_field_is_select(field)
}

fn batch_display_value_label(field: BatchFieldId, raw: &str) -> String {
    match field {
        BatchFieldId::RdpBackend => match raw {
            "builtin" => t!("dialog.rdpBackendBuiltin").to_string(),
            "windows" => t!("dialog.rdpBackendWindows").to_string(),
            "mstsc" => t!("dialog.rdpBackendMstsc").to_string(),
            _ => raw.to_string(),
        },
        BatchFieldId::RdpDisplayMode => match raw {
            "fit" => t!("dialog.rdpDisplayFitWindow").to_string(),
            "fixed" => t!("dialog.rdpDisplayFixed").to_string(),
            _ => raw.to_string(),
        },
        BatchFieldId::VncScaleMode => match raw {
            "actual" => t!("dialog.vncScaleActual").to_string(),
            "fit" => t!("dialog.vncScaleFit").to_string(),
            "stretch" => t!("dialog.vncScaleStretch").to_string(),
            _ => raw.to_string(),
        },
        BatchFieldId::RecordingMode => match raw {
            "raw" => t!("dialog.recordingModeRaw").to_string(),
            "transcript" => t!("dialog.recordingModeTranscript").to_string(),
            _ => raw.to_string(),
        },
        BatchFieldId::SshAuthAgentEndpoint => match raw {
            "auto" => t!("dialog.sshAgentAuto").to_string(),
            "environment" => t!("dialog.sshAgentEnvironment").to_string(),
            "unix_socket" => t!("dialog.sshAgentUnixSocket").to_string(),
            "pageant" => "Pageant".to_string(),
            "windows_openssh" => "Windows OpenSSH".to_string(),
            _ => raw.to_string(),
        },
        _ => raw.to_string(),
    }
}

impl NyaTermApp {
    /// The child-window body. Reads only; widget entities were created when the
    /// editor opened or when a protocol was picked.
    pub(in crate::features) fn asset_batch_edit_window_view(
        &mut self,
        viewport: (f32, f32),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let draft = self.start_workspace.asset_batch();
        let section = draft.section();
        let error = draft.error().cloned();
        let saving = draft.saving();
        let has_edits = draft.has_edits();
        let display_kind = draft.display_kind();
        let awaiting_kind = draft.awaiting_kind_pick();
        let kind_picked = draft.picked_kind().is_some();
        let wide = viewport.0 >= BATCH_WIDE_WIDTH_PX;
        let navigation_width = if wide {
            BATCH_NAV_WIDTH_PX
        } else {
            BATCH_NAV_WIDTH_NARROW_PX
        };
        let narrow = viewport.0 - navigation_width < BATCH_NARROW_CONTENT_WIDTH_PX;

        let tabs = NyaTabs::new("asset-batch-kind-tabs")
            .items(BATCH_TAB_KINDS.map(|kind| NyaTabItem::new(batch_kind_label(kind))))
            .selected_index_if_visible(display_kind.map(tab_index))
            .full_width(true)
            .on_select(cx.listener(|this, index, _, cx| {
                if let Some(kind) = BATCH_TAB_KINDS.get(*index).copied() {
                    this.set_asset_batch_kind(kind, cx);
                }
            }));

        let kind_select_id = SharedString::from(format!("{ASSET_BATCH_SELECT_PREFIX}kind"));
        let kind_control = if wide {
            tabs.into_any_element()
        } else {
            div()
                .w_full()
                .h(px(NYA_FORM_CONTROL_HEIGHT_PX))
                .children(
                    self.selects
                        .field(&kind_select_id)
                        .map(|entity| NyaSelect::new(&entity)),
                )
                .into_any_element()
        };

        let reset_note = div()
            .text_xs()
            .text_color(rgb(if kind_picked {
                palette.danger
            } else {
                palette.text_muted
            }))
            .child(t!("assets.batch.kindResetNote"));

        let navigation = if awaiting_kind {
            div()
                .id("asset-batch-navigation")
                .flex()
                .flex_col()
                .gap_1()
                .p_3()
                .w(px(navigation_width))
                .flex_none()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(t!("assets.batch.kindPickToNavigate"))
                .into_any_element()
        } else {
            let sections = display_kind
                .map(|kind| ConnectionEditorSection::for_kind(batch_kind_tab(kind)))
                .unwrap_or_default();
            let current = if sections.contains(&section) {
                section
            } else {
                sections
                    .first()
                    .copied()
                    .unwrap_or(ConnectionEditorSection::Basic)
            };
            div()
                .id("asset-batch-navigation")
                .flex()
                .flex_col()
                .gap_1()
                .p_3()
                .w(px(navigation_width))
                .flex_none()
                .overflow_y_scrollbar()
                .children(sections.iter().map(|item| {
                    NyaButton::new(format!("asset-batch-nav-{}", item.id()), item.label())
                        .variant(NyaButtonVariant::Ghost)
                        .full_width()
                        .height(px(40.))
                        .label_left()
                        .tooltip(item.label().to_string())
                        .selected(current == *item)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_asset_batch_section(*item, cx);
                        }))
                }))
                .into_any_element()
        };

        let current_section = display_kind
            .map(|kind| {
                let sections = ConnectionEditorSection::for_kind(batch_kind_tab(kind));
                if sections.contains(&section) {
                    section
                } else {
                    ConnectionEditorSection::Basic
                }
            })
            .unwrap_or(ConnectionEditorSection::Basic);

        let mut content: Vec<AnyElement> = Vec::new();
        if let Some(kind) = display_kind {
            content.push(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight(600.))
                    .child(current_section.label().to_string())
                    .into_any_element(),
            );
            content.extend(self.asset_batch_section_content(
                kind,
                current_section,
                narrow,
                palette,
                cx,
            ));
        } else {
            content.push(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight(600.))
                    .child(t!("assets.batch.title"))
                    .into_any_element(),
            );
            content.push(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("assets.batch.kindPickToNavigate"))
                    .into_any_element(),
            );
        }

        let scroll_handle = self.start_workspace.asset_batch().scroll().clone();
        let scroll = div()
            .id("asset-batch-scroll")
            .min_w_0()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&scroll_handle)
            .p(px(20.))
            .flex()
            .flex_col()
            .gap_3()
            .children(content);

        let mut card = div()
            .id("asset-batch-panel")
            .flex()
            .flex_col()
            .size_full()
            .child(
                div().flex_none().px(px(20.)).pt(px(20.)).child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .pb_3()
                        .child(kind_control)
                        .child(reset_note),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(navigation)
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .flex_col()
                            .min_h_0()
                            .min_w_0()
                            .vertical_scrollbar(&scroll_handle)
                            .child(scroll),
                    ),
            );
        if let Some(error) = error {
            card = card.child(
                div()
                    .id("asset-batch-error")
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(palette.danger))
                    .bg(gpui::rgba((palette.danger << 8) | 0x18))
                    .p_2()
                    .text_size(px(12.))
                    .text_color(rgb(palette.danger))
                    .child(error),
            );
        }
        card.child(
            div()
                .h(px(60.))
                .flex_none()
                .border_t_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.section_header))
                .px_5()
                .py_3()
                .flex()
                .items_center()
                .justify_end()
                .gap_3()
                .child(
                    NyaButton::new("asset-batch-cancel", t!("common.cancel"))
                        .variant(NyaButtonVariant::Secondary)
                        .height(px(36.))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_asset_batch_edit(cx);
                        })),
                )
                .child(
                    NyaButton::new("asset-batch-apply", t!("common.save"))
                        .variant(NyaButtonVariant::Primary)
                        .height(px(36.))
                        .disabled(saving || !has_edits)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.save_asset_batch_edit(cx);
                        })),
                ),
        )
        .into_any_element()
    }

    fn asset_batch_basic_prefix(
        &self,
        kind: ConnectionBatchKind,
        narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .gap_3()
            .items_end()
            .when(!narrow, |this| this.flex_wrap())
            .when(narrow, |this| this.flex_col().items_stretch())
            .child(self.asset_batch_icon_picker(kind, palette, cx))
            .child(
                div()
                    .min_w(px(192.))
                    .flex_1()
                    .min_w_0()
                    .when(narrow, |this| this.w_full())
                    .child(self.asset_batch_row(BatchFieldId::Name, kind, palette, cx)),
            )
            .child(
                div()
                    .min_w(px(192.))
                    .flex_1()
                    .when(!narrow, |this| this.max_w(px(288.)))
                    .when(narrow, |this| this.w_full())
                    .min_w_0()
                    .child(self.asset_batch_row(BatchFieldId::Group, kind, palette, cx)),
            )
            .into_any_element()
    }

    fn asset_batch_row(
        &self,
        field: BatchFieldId,
        kind: ConnectionBatchKind,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let draft = self.start_workspace.asset_batch();
        let basis = draft.basis(field);
        let mixed = basis == FieldBasis::Mixed && !draft.is_edited(field);
        let disabled_reason = batch_dynamic_disabled_reason(field, draft);
        let editable = field.is_editable()
            && batch_field_in_kind(kind, field)
            && basis != FieldBasis::Inapplicable
            && disabled_reason.is_none();

        if field.is_bool() {
            let checked = draft.bool_value(field) && !mixed;
            let mut trailing = div().flex().items_center().gap_2();
            if mixed {
                trailing = trailing.child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("assets.batch.mixed")),
                );
            } else if let Some(reason) = disabled_reason {
                trailing = trailing.child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!(reason)),
                );
            }
            trailing = trailing.child(
                NyaSwitch::new(field.bool_id())
                    .checked(checked)
                    .disabled(!editable)
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        this.set_asset_batch_bool(field, *checked, cx);
                    })),
            );
            return div()
                .min_h(px(NYA_FORM_CONTROL_HEIGHT_PX))
                .py_1()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_xs()
                        .text_color(rgb(if editable {
                            palette.text
                        } else {
                            palette.text_muted
                        }))
                        .child(t!(field.label_key())),
                )
                .child(trailing)
                .into_any_element();
        }

        let control = self.asset_batch_editable_control(field, editable, palette, cx);

        let caption = div()
            .flex()
            .items_center()
            .gap_0p5()
            .text_xs()
            .text_color(rgb(palette.text_muted))
            .child(t!(field.label_key()));

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(caption)
            .child(control)
            .into_any_element()
    }

    /// A disabled control keeps the connection editor's shell metrics so the
    /// field stays visibly in place without being focusable.
    fn asset_batch_disabled_control(
        &self,
        field: BatchFieldId,
        palette: crate::theme::ThemePalette,
    ) -> AnyElement {
        let draft = self.start_workspace.asset_batch();
        let text = batch_dynamic_disabled_reason(field, draft)
            .map(|reason| t!(reason).to_string())
            .unwrap_or_else(|| batch_display_value_label(field, &draft.disabled_text(field)));
        let mut shell = div()
            .h(px(NYA_FORM_CONTROL_HEIGHT_PX))
            .w_full()
            .min_w_0()
            .px(px(
                crate::features::text_inputs::ORDINARY_INPUT_SHELL_PADDING_X_PX,
            ))
            .flex()
            .items_center()
            .gap_2()
            .rounded_sm()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.input))
            .text_size(px(13.))
            .text_color(rgb(palette.text_muted))
            .cursor_default()
            .child(div().flex_1().min_w_0().truncate().child(text));
        if batch_field_has_select_appearance(field) {
            shell = shell.child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_muted))
                    .child("▾"),
            );
        }
        shell.into_any_element()
    }

    fn asset_batch_section_content(
        &self,
        kind: ConnectionBatchKind,
        section: ConnectionEditorSection,
        narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        use ConnectionBatchKind as K;
        use ConnectionEditorSection as S;
        match (kind, section) {
            (K::Ssh, S::Basic) => self.asset_batch_ssh_basic(kind, narrow, palette, cx),
            (K::Ssh, S::Network) => self.asset_batch_ssh_network(kind, narrow, palette, cx),
            (K::Ssh, S::Terminal) => self.asset_batch_ssh_terminal(kind, narrow, palette, cx),
            (K::Ssh, S::Sftp) => self.asset_batch_ssh_sftp(kind, palette, cx),
            (K::Ssh, S::Forwarding) => self.asset_batch_ssh_forwarding(kind, palette, cx),
            (K::Ssh, S::PostLogin) => self.asset_batch_ssh_post_login(kind, palette, cx),
            (K::Ssh, S::Algorithms) => self.asset_batch_ssh_algorithms(kind, palette, cx),
            (K::LocalTerminal, S::Basic) => self.asset_batch_local_basic(kind, narrow, palette, cx),
            (K::LocalTerminal, S::Terminal) => {
                self.asset_batch_local_terminal(kind, narrow, palette, cx)
            }
            (K::Telnet, S::Basic) => self.asset_batch_telnet_basic(kind, narrow, palette, cx),
            (K::Telnet, S::AutoLogin) => {
                self.asset_batch_telnet_auto_login(kind, narrow, palette, cx)
            }
            (K::Telnet, S::Terminal) => self.asset_batch_telnet_terminal(kind, narrow, palette, cx),
            (K::Telnet, S::Compatibility) => {
                self.asset_batch_telnet_compatibility(kind, narrow, palette, cx)
            }
            (K::Serial, S::Basic) => self.asset_batch_serial_basic(kind, narrow, palette, cx),
            (K::Serial, S::Serial) => self.asset_batch_serial_section(kind, narrow, palette, cx),
            (K::Serial, S::Terminal) => self.asset_batch_serial_terminal(kind, narrow, palette, cx),
            (K::Rdp, S::Basic) => self.asset_batch_rdp_basic(kind, narrow, palette, cx),
            (K::Rdp, S::Network) => self.asset_batch_network_fields(kind, palette, cx),
            (K::Rdp, S::Security) => self.asset_batch_rdp_security(kind, palette, cx),
            (K::Rdp, S::Display) => self.asset_batch_rdp_display(kind, narrow, palette, cx),
            (K::Rdp, S::Session) => self.asset_batch_rdp_session(kind, palette, cx),
            (K::Vnc, S::Basic) => self.asset_batch_vnc_basic(kind, narrow, palette, cx),
            (K::Vnc, S::Network) => self.asset_batch_network_fields(kind, palette, cx),
            (K::Vnc, S::Security) => self.asset_batch_vnc_security(kind, palette, cx),
            (K::Vnc, S::Display) => self.asset_batch_vnc_display(kind, palette, cx),
            (K::Vnc, S::Session) => self.asset_batch_vnc_session(kind, narrow, palette, cx),
            _ => Vec::new(),
        }
    }

    fn asset_batch_basic_tail(
        &self,
        kind: ConnectionBatchKind,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        vec![
            self.asset_batch_field(BatchFieldId::Tags, kind, palette, cx),
            self.asset_batch_field(BatchFieldId::Description, kind, palette, cx),
        ]
    }

    fn asset_batch_ssh_basic(
        &self,
        kind: ConnectionBatchKind,
        narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = vec![self.asset_batch_basic_prefix(kind, narrow, palette, cx)];
        out.push(self.asset_batch_switch_row(
            BatchFieldId::IconAutoDetect,
            Some("dialog.iconAutoDetectTooltip"),
            palette,
            true,
            cx,
        ));
        out.push(self.asset_batch_host_port_row(kind, 132., narrow, palette, cx));
        out.push(self.asset_batch_account_row(kind, true, palette, cx));
        out.push(self.asset_batch_field(BatchFieldId::Username, kind, palette, cx));
        out.push(self.asset_batch_auth_tabs(kind, palette, cx));
        let draft = self.start_workspace.asset_batch();
        match batch_current_text(draft, BatchFieldId::AuthMode) {
            Some("password") => out.push(self.asset_batch_password_field(palette, cx)),
            Some("agent") => out.push(self.asset_batch_agent_auth_card(palette, cx)),
            Some("key" | "certificate") => out.push(self.asset_batch_key_row(kind, palette, cx)),
            Some("none") => {
                out.push(
                    div()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(palette.border))
                        .bg(rgb(palette.bg))
                        .px_3()
                        .py_2()
                        .text_size(px(10.))
                        .text_color(rgb(palette.text_muted))
                        .child(t!("dialog.noAuthenticationDescription"))
                        .into_any_element(),
                );
            }
            _ => {}
        }
        out.push(self.asset_batch_otp_card(kind, palette, cx));
        out.extend(self.asset_batch_basic_tail(kind, palette, cx));
        out
    }

    fn asset_batch_local_basic(
        &self,
        kind: ConnectionBatchKind,
        narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut preset_row = div()
            .flex()
            .gap_2()
            .items_end()
            .when(narrow, |this| this.flex_col().items_stretch());
        preset_row = preset_row
            .child(
                div()
                    .when(!narrow, |this| this.w(px(144.)).flex_none())
                    .min_w_0()
                    .child(self.asset_batch_blank_shell(palette, true)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(narrow, |this| this.w_full())
                    .child(self.asset_batch_editable_control(
                        BatchFieldId::ShellPath,
                        BatchFieldId::ShellPath.is_editable(),
                        palette,
                        cx,
                    )),
            )
            .child(
                NyaIconButton::new("asset-batch-shell-browse", "icons/folder.svg")
                    .form_control()
                    .disabled(true),
            );
        let shell_path = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_0p5()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("dialog.shellPath")),
            )
            .child(preset_row);
        let working_dir = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_0p5()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("dialog.workingDir")),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_end()
                    .when(narrow, |this| this.flex_col().items_stretch())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(self.asset_batch_editable_control(
                                BatchFieldId::WorkingDir,
                                BatchFieldId::WorkingDir.is_editable(),
                                palette,
                                cx,
                            )),
                    )
                    .child(
                        NyaButton::new("asset-batch-working-dir-browse", t!("dialog.browse"))
                            .variant(NyaButtonVariant::Secondary)
                            .form_control()
                            .disabled(true),
                    ),
            );
        vec![
            self.asset_batch_basic_prefix(kind, narrow, palette, cx),
            shell_path.into_any_element(),
            self.asset_batch_field(BatchFieldId::ShellArgs, kind, palette, cx),
            working_dir.into_any_element(),
            self.asset_batch_field(BatchFieldId::Tags, kind, palette, cx),
            self.asset_batch_field(BatchFieldId::Description, kind, palette, cx),
        ]
    }

    fn asset_batch_telnet_basic(
        &self,
        kind: ConnectionBatchKind,
        narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = vec![self.asset_batch_basic_prefix(kind, narrow, palette, cx)];
        out.push(self.asset_batch_host_port_row(kind, 150., narrow, palette, cx));
        out.push(self.asset_batch_account_row(kind, true, palette, cx));
        out.push(self.asset_batch_field(BatchFieldId::Username, kind, palette, cx));
        out.push(self.asset_batch_auth_tabs(kind, palette, cx));
        let draft = self.start_workspace.asset_batch();
        if !batch_auth_hides_password(draft) {
            out.push(self.asset_batch_password_field(palette, cx));
        }
        out.extend(self.asset_batch_basic_tail(kind, palette, cx));
        out
    }

    fn asset_batch_telnet_terminal(
        &self,
        kind: ConnectionBatchKind,
        narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let input_behavior = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.asset_batch_columns(
                vec![
                    self.asset_batch_field(BatchFieldId::Backspace, kind, palette, cx),
                    self.asset_batch_field(BatchFieldId::TelnetEnterMode, kind, palette, cx),
                ],
                narrow,
            ));
        let terminal = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.asset_batch_field(BatchFieldId::Encoding, kind, palette, cx))
            .child(self.asset_batch_recording_section(kind, palette, cx));
        vec![
            self.asset_batch_advanced_card(
                SharedString::from(t!("dialog.telnetInputBehavior").to_string()),
                None,
                input_behavior.into_any_element(),
                palette,
            ),
            self.asset_batch_plain_card(terminal.into_any_element(), palette),
        ]
    }

    fn asset_batch_telnet_compatibility(
        &self,
        kind: ConnectionBatchKind,
        narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let body = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.asset_batch_switch_card(
                BatchFieldId::TelnetRawTcp,
                Some("dialog.telnetRawTcpCliDesc"),
                palette,
                true,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(self.asset_batch_field(BatchFieldId::TelnetKeepAlive, kind, palette, cx))
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(palette.text_muted))
                            .child(t!("dialog.telnetKeepAliveIntervalDesc")),
                    ),
            )
            .child(self.asset_batch_columns(
                vec![
                    self.asset_batch_switch_row(
                        BatchFieldId::TelnetLocalEcho,
                        Some("dialog.telnetLocalEchoDesc"),
                        palette,
                        true,
                        cx,
                    ),
                    self.asset_batch_switch_row(
                        BatchFieldId::TelnetLocalLineEdit,
                        Some("dialog.telnetLocalLineEditDesc"),
                        palette,
                        true,
                        cx,
                    ),
                    self.asset_batch_switch_row(
                        BatchFieldId::TelnetForceChar,
                        Some("dialog.telnetForceCharAtATimeDesc"),
                        palette,
                        true,
                        cx,
                    ),
                    self.asset_batch_switch_row(
                        BatchFieldId::TelnetSendNaws,
                        Some("dialog.telnetSendNawsDesc"),
                        palette,
                        true,
                        cx,
                    ),
                    self.asset_batch_switch_row(
                        BatchFieldId::TelnetSendSga,
                        Some("dialog.telnetSendSgaDesc"),
                        palette,
                        true,
                        cx,
                    ),
                ],
                narrow,
            ));
        vec![self.asset_batch_plain_card(body.into_any_element(), palette)]
    }

    fn asset_batch_telnet_auto_login(
        &self,
        kind: ConnectionBatchKind,
        narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        vec![
            div()
                .text_size(px(12.))
                .font_weight(FontWeight(600.))
                .text_color(rgb(palette.text))
                .child(t!("dialog.telnetAutoLogin"))
                .into_any_element(),
            self.asset_batch_columns(
                vec![
                    self.asset_batch_switch_row(
                        BatchFieldId::TelnetAutoLoginEnabled,
                        None,
                        palette,
                        true,
                        cx,
                    ),
                    self.asset_batch_switch_row(
                        BatchFieldId::TelnetAutoLoginWake,
                        Some("dialog.telnetAutoLoginWakeEnterDesc"),
                        palette,
                        true,
                        cx,
                    ),
                ],
                narrow,
            ),
            self.asset_batch_columns(
                vec![
                    self.asset_batch_field(BatchFieldId::TelnetAutoLoginTimeout, kind, palette, cx),
                    self.asset_batch_field(BatchFieldId::TelnetAutoLoginRetries, kind, palette, cx),
                ],
                narrow,
            ),
            self.asset_batch_field(BatchFieldId::TelnetAutoLoginUserPrompt, kind, palette, cx),
            self.asset_batch_field(
                BatchFieldId::TelnetAutoLoginPasswordPrompt,
                kind,
                palette,
                cx,
            ),
            self.asset_batch_field(
                BatchFieldId::TelnetAutoLoginSuccessPrompt,
                kind,
                palette,
                cx,
            ),
            self.asset_batch_field(
                BatchFieldId::TelnetAutoLoginFailurePrompt,
                kind,
                palette,
                cx,
            ),
        ]
    }

    fn asset_batch_serial_basic(
        &self,
        kind: ConnectionBatchKind,
        narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = vec![self.asset_batch_basic_prefix(kind, narrow, palette, cx)];
        out.extend(self.asset_batch_basic_tail(kind, palette, cx));
        out
    }

    fn asset_batch_serial_section(
        &self,
        kind: ConnectionBatchKind,
        narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        vec![
            self.asset_batch_columns(
                vec![
                    self.asset_batch_field(BatchFieldId::SerialPort, kind, palette, cx),
                    self.asset_batch_field(BatchFieldId::BaudRate, kind, palette, cx),
                ],
                narrow,
            ),
            self.asset_batch_columns(
                vec![
                    self.asset_batch_field(BatchFieldId::DataBits, kind, palette, cx),
                    self.asset_batch_field(BatchFieldId::Parity, kind, palette, cx),
                    self.asset_batch_field(BatchFieldId::StopBits, kind, palette, cx),
                ],
                narrow,
            ),
            self.asset_batch_field(BatchFieldId::FlowControl, kind, palette, cx),
        ]
    }

    fn asset_batch_serial_terminal(
        &self,
        kind: ConnectionBatchKind,
        narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let body = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.asset_batch_columns(
                vec![
                    self.asset_batch_field(BatchFieldId::Backspace, kind, palette, cx),
                    self.asset_batch_field(BatchFieldId::Encoding, kind, palette, cx),
                ],
                narrow,
            ))
            .child(self.asset_batch_recording_section(kind, palette, cx));
        vec![self.asset_batch_plain_card(body.into_any_element(), palette)]
    }

    fn asset_batch_local_terminal(
        &self,
        kind: ConnectionBatchKind,
        _narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let body = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.asset_batch_field(BatchFieldId::Encoding, kind, palette, cx))
            .child(self.asset_batch_switch_row(
                BatchFieldId::DynamicTabTitle,
                Some("dialog.dynamicTabTitleDesc"),
                palette,
                true,
                cx,
            ))
            .child(self.asset_batch_recording_section(kind, palette, cx));
        vec![self.asset_batch_plain_card(body.into_any_element(), palette)]
    }

    fn asset_batch_ssh_terminal(
        &self,
        kind: ConnectionBatchKind,
        narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let terminal = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.asset_batch_columns(
                vec![
                    self.asset_batch_field(BatchFieldId::SshProfile, kind, palette, cx),
                    self.asset_batch_field(BatchFieldId::TerminalType, kind, palette, cx),
                ],
                narrow,
            ))
            .child(self.asset_batch_field(BatchFieldId::Encoding, kind, palette, cx))
            .child(self.asset_batch_switch_row(
                BatchFieldId::DynamicTabTitle,
                Some("dialog.dynamicTabTitleDesc"),
                palette,
                true,
                cx,
            ))
            .child(self.asset_batch_recording_section(kind, palette, cx));
        vec![
            self.asset_batch_advanced_card(
                SharedString::from(t!("dialog.encodingSettings").to_string()),
                Some(SharedString::from(
                    t!("connection.encodingFollowGlobal").to_string(),
                )),
                terminal.into_any_element(),
                palette,
            ),
            self.asset_batch_advanced_card(
                SharedString::from(t!("dialog.backspaceMode").to_string()),
                Some(SharedString::from(
                    t!("dialog.sshBackspaceModeDesc").to_string(),
                )),
                self.asset_batch_field(BatchFieldId::Backspace, kind, palette, cx),
                palette,
            ),
        ]
    }

    fn asset_batch_ssh_sftp(
        &self,
        kind: ConnectionBatchKind,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let body = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.asset_batch_switch_row(BatchFieldId::SftpEnabled, None, palette, true, cx))
            .child(self.asset_batch_switch_row(
                BatchFieldId::SftpCompatibility,
                Some("dialog.sftpCompatibilityMode"),
                palette,
                true,
                cx,
            ))
            .child(self.asset_batch_field(BatchFieldId::SftpCwdFollow, kind, palette, cx))
            .child(self.asset_batch_field(BatchFieldId::SftpShellTimeout, kind, palette, cx))
            .child(self.asset_batch_field(BatchFieldId::SftpFilenameEncoding, kind, palette, cx))
            .child(self.asset_batch_field(BatchFieldId::SftpPipelineDepth, kind, palette, cx));
        vec![self.asset_batch_advanced_card(
            SharedString::from(t!("dialog.sftpAdvanced").to_string()),
            Some(SharedString::from(
                t!("dialog.sftpAdvancedDesc").to_string(),
            )),
            body.into_any_element(),
            palette,
        )]
    }

    fn asset_batch_ssh_network(
        &self,
        kind: ConnectionBatchKind,
        narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let draft = self.start_workspace.asset_batch();
        let summary = format!(
            "{} · {}",
            batch_preview(&draft.disabled_text(BatchFieldId::Proxy)),
            batch_preview(&draft.disabled_text(BatchFieldId::ProxyJump))
        );
        let body = self.asset_batch_columns(
            vec![
                self.asset_batch_field(BatchFieldId::Proxy, kind, palette, cx),
                self.asset_batch_field(BatchFieldId::ProxyJump, kind, palette, cx),
            ],
            narrow,
        );
        vec![self.asset_batch_advanced_card(
            SharedString::from(t!("panel.network").to_string()),
            Some(SharedString::from(summary)),
            body,
            palette,
        )]
    }

    fn asset_batch_network_fields(
        &self,
        kind: ConnectionBatchKind,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        vec![
            self.asset_batch_field(BatchFieldId::Proxy, kind, palette, cx),
            self.asset_batch_field(BatchFieldId::ProxyJump, kind, palette, cx),
        ]
    }

    fn asset_batch_ssh_forwarding(
        &self,
        kind: ConnectionBatchKind,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let x11_selected = self.start_workspace.asset_batch().forwarding_x11();
        let tabs = NyaTabs::new("asset-batch-forwarding-tabs")
            .items([
                NyaTabItem::new(t!("dialog.sshAgentForwardingTab")),
                NyaTabItem::new(t!("dialog.x11Forwarding")),
            ])
            .selected_index_if_visible(Some(usize::from(x11_selected)))
            .on_select(cx.listener(|this, index: &usize, _, cx| {
                this.set_asset_batch_forwarding_x11(*index != 0, cx);
            }));
        if x11_selected {
            let content = self.asset_batch_switch_row(BatchFieldId::X11, None, palette, true, cx);
            vec![
                tabs.into_any_element(),
                self.asset_batch_advanced_card(
                    SharedString::from(t!("dialog.x11Forwarding").to_string()),
                    Some(SharedString::from(
                        t!("dialog.x11ForwardingDesc").to_string(),
                    )),
                    content,
                    palette,
                ),
            ]
        } else {
            let agent_switch =
                self.asset_batch_switch_control(BatchFieldId::AgentForwarding, true, palette, cx);
            let header = div()
                .flex()
                .items_start()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight(600.))
                                .text_color(rgb(palette.text))
                                .child(t!("dialog.sshAgentForwarding")),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(palette.text_muted))
                                .child(t!("dialog.sshAgentForwardingDescription")),
                        ),
                )
                .child(agent_switch);
            let body = div()
                .border_t_1()
                .border_color(rgb(palette.border))
                .pt_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight(600.))
                        .text_color(rgb(palette.text))
                        .child(t!("dialog.sshAgentForwardingSources")),
                )
                .child(self.asset_batch_switch_row(
                    BatchFieldId::AgentExternal,
                    Some("dialog.sshAgentEndpointListDescription"),
                    palette,
                    true,
                    cx,
                ))
                .child(self.asset_batch_switch_row(
                    BatchFieldId::AgentStoredKeys,
                    None,
                    palette,
                    true,
                    cx,
                ))
                .child(self.asset_batch_field(BatchFieldId::AgentEndpoints, kind, palette, cx))
                .child(self.asset_batch_field(BatchFieldId::AgentPolicy, kind, palette, cx));
            let card = div()
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.bg))
                .p_3()
                .flex()
                .flex_col()
                .gap_3()
                .child(header)
                .child(body)
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(palette.warning))
                        .child(t!("dialog.sshAgentForwardingWarning")),
                );
            vec![tabs.into_any_element(), card.into_any_element()]
        }
    }

    fn asset_batch_ssh_post_login(
        &self,
        kind: ConnectionBatchKind,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.asset_batch_switch_row(
                BatchFieldId::PostLoginEnabled,
                None,
                palette,
                true,
                cx,
            ))
            .child(self.asset_batch_field(BatchFieldId::PostLoginCommand, kind, palette, cx))
            .child(self.asset_batch_field(BatchFieldId::PostLoginDelay, kind, palette, cx));
        vec![self.asset_batch_advanced_card(
            SharedString::from(t!("dialog.postLoginCommand").to_string()),
            Some(SharedString::from(
                t!("dialog.postLoginCommandDesc").to_string(),
            )),
            body.into_any_element(),
            palette,
        )]
    }

    fn asset_batch_ssh_algorithms(
        &self,
        kind: ConnectionBatchKind,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let draft = self.start_workspace.asset_batch();
        let mode = draft.value(BatchFieldId::SshAlgorithmMode).to_string();
        let mode_description = match mode.as_str() {
            "secure" => t!("dialog.algorithmModeSecureDesc").to_string(),
            "custom" => t!("dialog.algorithmModeCustomDesc").to_string(),
            _ => t!("dialog.algorithmModeCompatibleDesc").to_string(),
        };
        let active_tab = draft.algorithm_tab();
        let tabs = NyaTabs::new("asset-batch-algorithm-tabs")
            .items([
                NyaTabItem::new(t!("dialog.algorithmKexTab")),
                NyaTabItem::new(t!("dialog.algorithmCiphersTab")),
                NyaTabItem::new(t!("dialog.algorithmMacsTab")),
                NyaTabItem::new(t!("dialog.algorithmHostKeysTab")),
            ])
            .selected_index_if_visible(Some(active_tab))
            .on_select(cx.listener(|this, index: &usize, _, cx| {
                this.set_asset_batch_algorithm_tab(*index, cx);
            }));
        let field = match active_tab {
            0 => BatchFieldId::SshKex,
            1 => BatchFieldId::SshCiphers,
            2 => BatchFieldId::SshMacs,
            _ => BatchFieldId::SshHostKeys,
        };
        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.asset_batch_field(BatchFieldId::SshAlgorithmMode, kind, palette, cx))
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(palette.text_muted))
                    .child(mode_description),
            )
            .child(tabs)
            .child(self.asset_batch_field(field, kind, palette, cx));
        vec![self.asset_batch_advanced_card(
            SharedString::from(t!("dialog.sshAlgorithms").to_string()),
            Some(SharedString::from(
                t!("dialog.sshAlgorithmsDesc").to_string(),
            )),
            body.into_any_element(),
            palette,
        )]
    }

    fn asset_batch_note(
        &self,
        text: impl Into<SharedString>,
        palette: crate::theme::ThemePalette,
    ) -> AnyElement {
        div()
            .text_size(px(10.))
            .text_color(rgb(palette.text_muted))
            .child(text.into())
            .into_any_element()
    }

    fn asset_batch_rdp_basic(
        &self,
        kind: ConnectionBatchKind,
        narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = vec![self.asset_batch_basic_prefix(kind, narrow, palette, cx)];
        let draft = self.start_workspace.asset_batch();
        let mstsc = batch_rdp_uses_mstsc(draft);
        // The editor's section wrapper puts the mstsc note before the section
        // content, right after the shared name/icon/group header.
        if mstsc {
            out.push(self.asset_batch_note(t!("dialog.rdpMstscDesc"), palette));
        }
        out.push(self.asset_batch_host_port_row(kind, 150., narrow, palette, cx));
        if !mstsc {
            out.push(self.asset_batch_columns(
                vec![
                    self.asset_batch_field(BatchFieldId::Username, kind, palette, cx),
                    self.asset_batch_field(BatchFieldId::RdpDomain, kind, palette, cx),
                ],
                narrow,
            ));
            out.push(self.asset_batch_auth_tabs(kind, palette, cx));
            if !batch_auth_hides_password(draft) {
                out.push(self.asset_batch_account_row(kind, false, palette, cx));
                out.push(self.asset_batch_password_field(palette, cx));
            }
        }
        out.extend(self.asset_batch_basic_tail(kind, palette, cx));
        out
    }

    fn asset_batch_rdp_security(
        &self,
        kind: ConnectionBatchKind,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let draft = self.start_workspace.asset_batch();
        if batch_rdp_uses_mstsc(draft) {
            return vec![self.asset_batch_note(t!("dialog.rdpMstscDesc"), palette)];
        }
        vec![
            self.asset_batch_switch_card(
                BatchFieldId::RdpUseNla,
                Some("dialog.rdpUseNlaDesc"),
                palette,
                true,
                cx,
            ),
            self.asset_batch_switch_card(
                BatchFieldId::RdpAdminSession,
                Some("dialog.rdpAdministrativeSessionDesc"),
                palette,
                true,
                cx,
            ),
            self.asset_batch_field(BatchFieldId::RdpCertificatePolicy, kind, palette, cx),
        ]
    }

    fn asset_batch_rdp_display(
        &self,
        kind: ConnectionBatchKind,
        narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let draft = self.start_workspace.asset_batch();
        if batch_rdp_uses_mstsc(draft) {
            return vec![self.asset_batch_note(t!("dialog.rdpMstscDesc"), palette)];
        }
        let mut out = vec![self.asset_batch_field(BatchFieldId::RdpDisplayMode, kind, palette, cx)];
        if batch_rdp_display_fixed(draft) {
            out.push(self.asset_batch_columns(
                vec![
                    self.asset_batch_field(BatchFieldId::RdpWidth, kind, palette, cx),
                    self.asset_batch_field(BatchFieldId::RdpHeight, kind, palette, cx),
                ],
                narrow,
            ));
        }
        out.push(self.asset_batch_note(t!("dialog.rdpGraphicsPipelineUnavailable"), palette));
        for field in [
            BatchFieldId::RdpWallpaper,
            BatchFieldId::RdpThemes,
            BatchFieldId::RdpFontSmoothing,
            BatchFieldId::RdpDesktopComposition,
            BatchFieldId::RdpDisableFullWindowDrag,
            BatchFieldId::RdpDisableMenuAnimations,
            BatchFieldId::RdpDisableCursorShadow,
            BatchFieldId::RdpDisableCursorBlinking,
        ] {
            out.push(self.asset_batch_switch_card(
                field,
                Some(asset_batch_switch_description(field)),
                palette,
                true,
                cx,
            ));
        }
        out
    }

    fn asset_batch_rdp_session(
        &self,
        kind: ConnectionBatchKind,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let draft = self.start_workspace.asset_batch();
        let mut out = Vec::new();
        if batch_rdp_uses_mstsc(draft) {
            out.push(self.asset_batch_note(t!("dialog.rdpMstscDesc"), palette));
            out.push(self.asset_batch_field(BatchFieldId::RdpBackend, kind, palette, cx));
            return out;
        }
        out.push(self.asset_batch_field(BatchFieldId::RdpBackend, kind, palette, cx));
        out.push(self.asset_batch_note(t!("dialog.rdpBackendDesc"), palette));
        out.push(self.asset_batch_switch_card(
            BatchFieldId::RdpAutoReconnect,
            Some("dialog.rdpAutoReconnectDesc"),
            palette,
            true,
            cx,
        ));
        out.push(self.asset_batch_field(BatchFieldId::RdpReconnectAttempts, kind, palette, cx));
        out
    }

    fn asset_batch_vnc_basic(
        &self,
        kind: ConnectionBatchKind,
        narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = vec![self.asset_batch_basic_prefix(kind, narrow, palette, cx)];
        out.push(self.asset_batch_host_port_row(kind, 150., narrow, palette, cx));
        out.push(self.asset_batch_auth_tabs(kind, palette, cx));
        let draft = self.start_workspace.asset_batch();
        if !batch_auth_hides_password(draft) && !batch_vnc_requires_no_auth(draft) {
            out.push(self.asset_batch_account_row(kind, false, palette, cx));
            out.push(self.asset_batch_password_field(palette, cx));
        }
        out.push(self.asset_batch_field(BatchFieldId::Username, kind, palette, cx));
        out.extend(self.asset_batch_basic_tail(kind, palette, cx));
        out
    }

    fn asset_batch_vnc_security(
        &self,
        kind: ConnectionBatchKind,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let draft = self.start_workspace.asset_batch();
        let shows_warning =
            batch_current_text(draft, BatchFieldId::VncSecurityMode) == Some("none");
        let mut out =
            vec![self.asset_batch_field(BatchFieldId::VncSecurityMode, kind, palette, cx)];
        if shows_warning {
            out.push(
                div()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(palette.warning))
                    .p_2()
                    .text_size(px(10.))
                    .text_color(rgb(palette.warning))
                    .child(t!("dialog.vncUnencryptedWarning"))
                    .into_any_element(),
            );
        }
        out
    }

    fn asset_batch_vnc_display(
        &self,
        kind: ConnectionBatchKind,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        vec![self.asset_batch_field(BatchFieldId::VncScaleMode, kind, palette, cx)]
    }

    fn asset_batch_vnc_session(
        &self,
        kind: ConnectionBatchKind,
        narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut grid = div().grid().grid_cols(if narrow { 1 } else { 2 }).gap_2();
        for field in [
            BatchFieldId::VncClipboard,
            BatchFieldId::VncShared,
            BatchFieldId::VncViewOnly,
            BatchFieldId::VncReconnect,
        ] {
            grid = grid.child(self.asset_batch_switch_card(
                field,
                Some(asset_batch_switch_description(field)),
                palette,
                true,
                cx,
            ));
        }
        vec![
            grid.into_any_element(),
            self.asset_batch_field(BatchFieldId::VncReconnectAttempts, kind, palette, cx),
        ]
    }

    fn asset_batch_recording_section(
        &self,
        kind: ConnectionBatchKind,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let use_global =
            self.asset_batch_switch_control(BatchFieldId::RecordingUseGlobal, true, palette, cx);
        let show_details = batch_current_bool(
            self.start_workspace.asset_batch(),
            BatchFieldId::RecordingUseGlobal,
        ) != Some(true);
        div()
            .border_t_1()
            .border_color(rgb(palette.border))
            .pt_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight(600.))
                                    .text_color(rgb(palette.text))
                                    .child(t!("dialog.connectionRecording")),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(palette.text_muted))
                                    .child(t!("dialog.connectionRecordingDesc")),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(palette.text_muted))
                                    .child(t!("dialog.recordingUseGlobal")),
                            )
                            .child(use_global),
                    ),
            )
            .when(show_details, |this| {
                this.child(self.asset_batch_switch_row(
                    BatchFieldId::RecordingAutoStart,
                    Some("dialog.recordingAutoStartDesc"),
                    palette,
                    true,
                    cx,
                ))
                .child(self.asset_batch_field(
                    BatchFieldId::RecordingMode,
                    kind,
                    palette,
                    cx,
                ))
            })
            .into_any_element()
    }

    fn asset_batch_icon_picker(
        &self,
        kind: ConnectionBatchKind,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let draft = self.start_workspace.asset_batch();
        let icon_value = draft.value(BatchFieldId::Icon).to_string();
        let kind_label = batch_kind_tab(kind).label().to_string();
        let icon_def = if icon_value.trim().is_empty() {
            resolve_connection_icon(None, &kind_label)
        } else {
            resolve_connection_icon(Some(icon_value.as_str()), &kind_label)
        };
        let trigger = div()
            .flex_none()
            .size(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.input))
            .child(themed_icon(palette, icon_def, false, 18.));
        let options = CONNECTION_ICON_OPTIONS
            .iter()
            .map(|key| {
                let key = *key;
                let selected = icon_value == key;
                let kind_label = batch_kind_tab(kind).label().to_string();
                div()
                    .id(SharedString::from(format!("asset-batch-icon-{key}")))
                    .size(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .when(selected, |this| this.bg(rgb(palette.hover)))
                    .hover(|this| this.bg(rgb(palette.hover)))
                    .child(themed_icon(
                        palette,
                        resolve_connection_icon(Some(key), &kind_label),
                        selected,
                        16.,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_asset_batch_icon(key, cx);
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let grid = div()
            .id("asset-batch-icon-grid")
            .grid()
            .grid_cols(7)
            .gap_1()
            .p_2()
            .w(px(236.))
            .h(px(196.))
            .overflow_y_scrollbar()
            .children(options);
        let open = draft.icon_picker_open();
        let popover = NyaPopover::new("asset-batch-icon-popover", trigger, grid)
            .open(open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.set_asset_batch_icon_picker_open(*open, cx);
            }));
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("dialog.icon")),
            )
            .child(popover)
            .into_any_element()
    }

    fn asset_batch_field(
        &self,
        field: BatchFieldId,
        kind: ConnectionBatchKind,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.asset_batch_row(field, kind, palette, cx)
    }

    fn asset_batch_editable_control(
        &self,
        field: BatchFieldId,
        editable: bool,
        palette: crate::theme::ThemePalette,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        if editable && batch_field_is_select(field) {
            let entity = self.selects.field(&field.select_id());
            return div()
                .h(px(NYA_FORM_CONTROL_HEIGHT_PX))
                .min_w_0()
                .children(entity.map(|entity| NyaSelect::new(&entity)))
                .into_any_element();
        }
        if editable {
            let id = format!("{ASSET_BATCH_TEXT_PREFIX}{}", field.key());
            let entity = self.existing_text_input(&id);
            let height = match field {
                BatchFieldId::Description => BATCH_DESCRIPTION_HEIGHT_PX,
                BatchFieldId::PostLoginCommand => BATCH_COMMAND_HEIGHT_PX,
                _ => NYA_FORM_CONTROL_HEIGHT_PX,
            };
            let multi_line = matches!(
                field,
                BatchFieldId::Description | BatchFieldId::PostLoginCommand
            );
            return div()
                .min_w_0()
                .children(entity.map(|entity| {
                    let shell =
                        NyaInputShell::new(SharedString::from(id), &entity).height(px(height));
                    if multi_line {
                        shell.multi_line()
                    } else {
                        shell
                    }
                }))
                .into_any_element();
        }
        self.asset_batch_disabled_control(field, palette)
    }

    fn asset_batch_switch_control(
        &self,
        field: BatchFieldId,
        editable: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let draft = self.start_workspace.asset_batch();
        let disabled_reason = batch_dynamic_disabled_reason(field, draft);
        let editable = editable && disabled_reason.is_none();
        let mixed = draft.basis(field) == FieldBasis::Mixed && !draft.is_edited(field);
        let checked = if field.is_bool() {
            draft.bool_value(field)
        } else {
            draft.value(field) == "true"
        };
        let id = if field.is_bool() {
            field.bool_id().to_string()
        } else {
            format!("asset-batch.display.bool.{}", field.key())
        };
        let mut trailing = div().flex().items_center().gap_2();
        if mixed {
            trailing = trailing.child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("assets.batch.mixed")),
            );
        } else if let Some(reason) = disabled_reason {
            trailing = trailing.child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!(reason)),
            );
        }
        let mut switch = NyaSwitch::new(SharedString::from(id))
            .checked(checked && !mixed)
            .disabled(!editable);
        if editable {
            switch = switch.on_click(cx.listener(move |this, checked: &bool, _, cx| {
                this.set_asset_batch_bool(field, *checked, cx);
            }));
        }
        trailing.child(switch).into_any_element()
    }

    fn asset_batch_switch_row(
        &self,
        field: BatchFieldId,
        description: Option<&'static str>,
        palette: crate::theme::ThemePalette,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut left = div().min_w_0().flex_1().flex().flex_col().gap_0p5().child(
            div()
                .text_xs()
                .text_color(rgb(if editable {
                    palette.text
                } else {
                    palette.text_muted
                }))
                .child(t!(field.label_key())),
        );
        if let Some(key) = description {
            left = left.child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!(key)),
            );
        }
        div()
            .min_h(px(36.))
            .py_1()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(left)
            .child(self.asset_batch_switch_control(field, editable, palette, cx))
            .into_any_element()
    }

    fn asset_batch_switch_card(
        &self,
        field: BatchFieldId,
        description: Option<&'static str>,
        palette: crate::theme::ThemePalette,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut left = div().min_w_0().flex_1().child(
            div()
                .text_xs()
                .font_weight(FontWeight(500.))
                .text_color(rgb(if editable {
                    palette.text
                } else {
                    palette.text_muted
                }))
                .child(t!(field.label_key())),
        );
        if let Some(key) = description {
            left = left.child(
                div()
                    .mt_1()
                    .text_size(px(10.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!(key)),
            );
        }
        div()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .px_3()
            .py_2()
            .flex()
            .items_start()
            .justify_between()
            .gap_3()
            .child(left)
            .child(self.asset_batch_switch_control(field, editable, palette, cx))
            .into_any_element()
    }

    fn asset_batch_columns(&self, children: Vec<AnyElement>, narrow: bool) -> AnyElement {
        let wrappers = children
            .into_iter()
            .map(|child| {
                div()
                    .min_w(px(180.))
                    .flex_1()
                    .when(narrow, |this| this.w_full())
                    .child(child)
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        div()
            .flex()
            .gap_3()
            .when(!narrow, |this| this.flex_wrap())
            .when(narrow, |this| this.flex_col())
            .children(wrappers)
            .into_any_element()
    }

    fn asset_batch_advanced_card(
        &self,
        title: SharedString,
        description: Option<SharedString>,
        content: AnyElement,
        palette: crate::theme::ThemePalette,
    ) -> AnyElement {
        let mut header = div().flex().flex_col().gap_1().child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight(600.))
                .text_color(rgb(palette.text))
                .child(title),
        );
        if let Some(description) = description {
            header = header.child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(palette.text_muted))
                    .child(description),
            );
        }
        div()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.bg))
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(header)
            .child(content)
            .into_any_element()
    }

    fn asset_batch_plain_card(
        &self,
        content: AnyElement,
        palette: crate::theme::ThemePalette,
    ) -> AnyElement {
        div()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.bg))
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(content)
            .into_any_element()
    }

    fn asset_batch_blank_shell(
        &self,
        palette: crate::theme::ThemePalette,
        chevron: bool,
    ) -> AnyElement {
        let mut shell = div()
            .h(px(NYA_FORM_CONTROL_HEIGHT_PX))
            .w_full()
            .min_w_0()
            .px(px(
                crate::features::text_inputs::ORDINARY_INPUT_SHELL_PADDING_X_PX,
            ))
            .flex()
            .items_center()
            .gap_2()
            .rounded_sm()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.input))
            .text_size(px(13.))
            .text_color(rgb(palette.text_muted))
            .cursor_default()
            .child(div().flex_1().min_w_0());
        if chevron {
            shell = shell.child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_muted))
                    .child("▾"),
            );
        }
        shell.into_any_element()
    }

    fn asset_batch_host_port_row(
        &self,
        kind: ConnectionBatchKind,
        port_width: f32,
        narrow: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .gap_3()
            .when(narrow, |this| this.flex_col())
            .child(
                div()
                    .min_w(px(180.))
                    .flex_1()
                    .min_w_0()
                    .when(narrow, |this| this.w_full())
                    .child(self.asset_batch_field(BatchFieldId::Host, kind, palette, cx)),
            )
            .child(
                div()
                    .when(!narrow, |this| this.w(px(port_width)).flex_none())
                    .when(narrow, |this| this.w_full())
                    .child(self.asset_batch_field(BatchFieldId::Port, kind, palette, cx)),
            )
            .into_any_element()
    }

    fn asset_batch_account_row(
        &self,
        kind: ConnectionBatchKind,
        identity: bool,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut row = div().flex().flex_wrap().items_end().gap_2().child(
            div()
                .min_w(px(180.))
                .flex_1()
                .min_w_0()
                .child(self.asset_batch_field(BatchFieldId::Account, kind, palette, cx)),
        );
        if identity {
            row = row
                .child(
                    NyaButton::new("asset-batch-account-new", t!("passwordManager.add"))
                        .form_control()
                        .disabled(true),
                )
                .child(
                    NyaButton::new("asset-batch-account-edit", t!("common.edit"))
                        .form_control()
                        .disabled(true),
                );
        } else {
            row = row.child(
                NyaButton::new("asset-batch-manage-passwords", t!("dialog.managePasswords"))
                    .form_control()
                    .disabled(true),
            );
        }
        row.into_any_element()
    }

    fn asset_batch_password_field(
        &self,
        palette: crate::theme::ThemePalette,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let draft = self.start_workspace.asset_batch();
        let disabled = draft.basis(BatchFieldId::Password) == FieldBasis::Inapplicable
            || batch_dynamic_disabled_reason(BatchFieldId::Password, draft).is_some();
        let control = if disabled {
            self.asset_batch_disabled_control(BatchFieldId::Password, palette)
        } else {
            let id = format!("{ASSET_BATCH_TEXT_PREFIX}{}", BatchFieldId::Password.key());
            div()
                .min_w_0()
                .children(self.existing_text_input(&id).map(|entity| {
                    NyaInputShell::new(SharedString::from(id), &entity)
                        .height(px(NYA_FORM_CONTROL_HEIGHT_PX))
                }))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("authForm.passwordOptional")),
            )
            .child(control)
            .into_any_element()
    }

    fn asset_batch_auth_tabs(
        &self,
        kind: ConnectionBatchKind,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let draft = self.start_workspace.asset_batch();
        let mixed = draft.basis(BatchFieldId::AuthMode) == FieldBasis::Mixed;
        let mode = draft.value(BatchFieldId::AuthMode).to_string();
        let (choices, selected) = if kind == ConnectionBatchKind::Ssh {
            let choices = options::ssh_auth_choices(&mode);
            let selected = choices
                .iter()
                .position(|choice| choice.selected)
                .unwrap_or(0);
            (choices, selected)
        } else {
            let password_disabled =
                kind == ConnectionBatchKind::Vnc && batch_vnc_requires_no_auth(draft);
            let choices = options::password_auth_choices(&mode, password_disabled);
            let selected = choices
                .iter()
                .position(|choice| choice.selected)
                .unwrap_or(0);
            (choices, selected)
        };
        let values: Vec<Option<String>> =
            choices.iter().map(|choice| choice.value.clone()).collect();
        let items = choices
            .into_iter()
            .map(|choice| NyaTabItem::new(choice.label).disabled(choice.disabled))
            .collect::<Vec<_>>();
        let tabs = NyaTabs::new("asset-batch-auth-tabs")
            .items(items)
            .selected_index_if_visible((!mixed).then_some(selected))
            .on_select(cx.listener(move |this, index, _, cx| {
                if let Some(value) = values
                    .get(*index)
                    .and_then(|value: &Option<String>| value.as_deref())
                {
                    this.start_workspace
                        .set_asset_batch_value(BatchFieldId::AuthMode, value);
                    cx.notify();
                }
            }));
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("dialog.authentication")),
            )
            .child(tabs)
            .into_any_element()
    }

    fn asset_batch_key_row(
        &self,
        kind: ConnectionBatchKind,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_wrap()
            .items_end()
            .gap_2()
            .child(
                div()
                    .min_w(px(180.))
                    .flex_1()
                    .min_w_0()
                    .child(self.asset_batch_field(BatchFieldId::Key, kind, palette, cx)),
            )
            .child(
                NyaButton::new("asset-batch-manage-keys", t!("dialog.manageKeys"))
                    .form_control()
                    .disabled(true),
            )
            .into_any_element()
    }

    fn asset_batch_otp_card(
        &self,
        kind: ConnectionBatchKind,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let draft = self.start_workspace.asset_batch();
        let otp_preview = if draft.basis(BatchFieldId::Otp) == FieldBasis::Mixed {
            Some(SharedString::from(t!("assets.batch.mixed").to_string()))
        } else {
            let value = draft.value(BatchFieldId::Otp);
            (!value.is_empty()).then(|| SharedString::from(value.to_string()))
        };
        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.asset_batch_field(BatchFieldId::Otp, kind, palette, cx))
            .child(self.asset_batch_switch_row(BatchFieldId::OtpAutoFill, None, palette, true, cx));
        self.asset_batch_advanced_card(
            SharedString::from(t!("dialog.twoFactorAuth").to_string()),
            otp_preview,
            body.into_any_element(),
            palette,
        )
    }

    fn asset_batch_agent_auth_card(
        &self,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let header = div()
            .flex()
            .items_start()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight(600.))
                            .text_color(rgb(palette.text))
                            .child(t!("dialog.sshAgent")),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(palette.text_muted))
                            .child(t!("dialog.sshAgentAuthDesc")),
                    ),
            )
            .child(div().w(px(176.)).flex_none().child(self.asset_batch_field(
                BatchFieldId::SshAuthAgentEndpoint,
                ConnectionBatchKind::Ssh,
                palette,
                cx,
            )));
        let mut rows = div().flex().flex_col().gap_2().child(header);
        let draft = self.start_workspace.asset_batch();
        match batch_current_text(draft, BatchFieldId::SshAuthAgentEndpoint) {
            Some("environment") => {
                rows = rows.child(self.asset_batch_field(
                    BatchFieldId::SshAuthAgentEnvironment,
                    ConnectionBatchKind::Ssh,
                    palette,
                    cx,
                ))
            }
            Some("unix_socket") => {
                rows = rows.child(self.asset_batch_field(
                    BatchFieldId::SshAuthAgentSocket,
                    ConnectionBatchKind::Ssh,
                    palette,
                    cx,
                ))
            }
            Some(_) => {}
            // Mixed endpoint types: keep both rows so the user can type a
            // replacement value after picking a type.
            None => {
                rows = rows
                    .child(self.asset_batch_field(
                        BatchFieldId::SshAuthAgentEnvironment,
                        ConnectionBatchKind::Ssh,
                        palette,
                        cx,
                    ))
                    .child(self.asset_batch_field(
                        BatchFieldId::SshAuthAgentSocket,
                        ConnectionBatchKind::Ssh,
                        palette,
                        cx,
                    ));
            }
        }
        self.asset_batch_plain_card(rows.into_any_element(), palette)
    }
}

fn batch_preview(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        "—".to_string()
    } else {
        value.to_string()
    }
}

fn asset_batch_switch_description(field: BatchFieldId) -> &'static str {
    match field {
        BatchFieldId::RdpWallpaper => "dialog.rdpWallpaperDesc",
        BatchFieldId::RdpThemes => "dialog.rdpThemesDesc",
        BatchFieldId::RdpFontSmoothing => "dialog.rdpFontSmoothingDesc",
        BatchFieldId::RdpDesktopComposition => "dialog.rdpDesktopCompositionDesc",
        BatchFieldId::RdpDisableFullWindowDrag => "dialog.rdpDisableFullWindowDragDesc",
        BatchFieldId::RdpDisableMenuAnimations => "dialog.rdpDisableMenuAnimationsDesc",
        BatchFieldId::RdpDisableCursorShadow => "dialog.rdpDisableCursorShadowDesc",
        BatchFieldId::RdpDisableCursorBlinking => "dialog.rdpDisableCursorBlinkingDesc",
        BatchFieldId::VncClipboard => "dialog.vncClipboardDesc",
        BatchFieldId::VncShared => "dialog.vncSharedSessionDesc",
        BatchFieldId::VncViewOnly => "dialog.vncViewOnlyDesc",
        BatchFieldId::VncReconnect => "dialog.vncReconnectDesc",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AssetBatchEditDraft, BatchFieldId, batch_auth_hides_password, batch_current_bool,
        batch_rdp_display_fixed, batch_rdp_uses_mstsc, batch_vnc_requires_no_auth,
    };

    fn edited(field: BatchFieldId, value: &str) -> AssetBatchEditDraft {
        let mut draft = AssetBatchEditDraft::default();
        draft.set_value(field, value.to_string());
        draft
    }

    #[test]
    fn rdp_mstsc_backend_withholds_the_reconnect_and_display_fields() {
        assert!(batch_rdp_uses_mstsc(&edited(
            BatchFieldId::RdpBackend,
            "mstsc"
        )));
        assert!(!batch_rdp_uses_mstsc(&edited(
            BatchFieldId::RdpBackend,
            "ironrdp"
        )));
        assert!(!batch_rdp_uses_mstsc(&AssetBatchEditDraft::default()));
    }

    #[test]
    fn rdp_size_fields_follow_the_fixed_mode_only() {
        assert!(batch_rdp_display_fixed(&edited(
            BatchFieldId::RdpDisplayMode,
            "fixed"
        )));
        assert!(!batch_rdp_display_fixed(&edited(
            BatchFieldId::RdpDisplayMode,
            "fit"
        )));
        assert!(!batch_rdp_display_fixed(&edited(
            BatchFieldId::RdpDisplayMode,
            "native"
        )));
        assert!(!batch_rdp_display_fixed(&AssetBatchEditDraft::default()));
    }

    #[test]
    fn password_source_is_hidden_for_non_password_authentication() {
        assert!(!batch_auth_hides_password(&edited(
            BatchFieldId::AuthMode,
            "password"
        )));
        for mode in ["none", "key", "agent", "certificate"] {
            assert!(
                batch_auth_hides_password(&edited(BatchFieldId::AuthMode, mode)),
                "{mode} must not show the password source"
            );
        }
        assert!(!batch_auth_hides_password(&AssetBatchEditDraft::default()));
    }

    #[test]
    fn vnc_none_security_hides_every_password_source() {
        assert!(batch_vnc_requires_no_auth(&edited(
            BatchFieldId::VncSecurityMode,
            "none"
        )));
        assert!(!batch_vnc_requires_no_auth(&edited(
            BatchFieldId::VncSecurityMode,
            "vnc-auth"
        )));
        assert!(!batch_vnc_requires_no_auth(&AssetBatchEditDraft::default()));
    }

    #[test]
    fn recording_details_are_hidden_when_the_global_policy_is_uniform_true() {
        let mut draft = AssetBatchEditDraft::default();
        draft.set_bool_value(BatchFieldId::RecordingUseGlobal, true);
        assert_eq!(
            batch_current_bool(&draft, BatchFieldId::RecordingUseGlobal),
            Some(true)
        );
        draft.set_bool_value(BatchFieldId::RecordingUseGlobal, false);
        assert_eq!(
            batch_current_bool(&draft, BatchFieldId::RecordingUseGlobal),
            Some(false)
        );
        assert_eq!(
            batch_current_bool(
                &AssetBatchEditDraft::default(),
                BatchFieldId::RecordingUseGlobal
            ),
            None
        );
    }
}
