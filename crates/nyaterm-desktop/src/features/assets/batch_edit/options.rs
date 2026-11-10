use super::basis::batch_kind_value;
use super::draft::AssetBatchEditDraft;
use crate::features::icons::{CONNECTION_ICON_OPTIONS, DEFAULT_CONNECTION_ICON};
use crate::features::pages::connections::editor::connection::options;
use crate::features::pages::connections::list::ConnectionEditorChoice;
use gpui::SharedString;
use nyaterm_core::{
    ConnectionBatchKind, Group, OtpEntry, ProxyConfig, RecordingMode, SavedConnection,
    SavedPassword, SshAgentEndpoint, SshAgentForwardingPolicy, SshKey,
};
use nyaterm_ui::NyaSelectOption;
use rust_i18n::t;
fn choice_options(choices: Vec<ConnectionEditorChoice>) -> Vec<NyaSelectOption> {
    choices
        .into_iter()
        .filter_map(|choice| {
            let value = choice.value?;
            if value.is_empty() {
                return None;
            }
            let mut option = NyaSelectOption::new(value, choice.label);
            if let Some(search_text) = choice.search_text {
                option = option.search_text(search_text);
            }
            if let Some(subtitle) = choice.subtitle {
                option = option.subtitle(subtitle);
            }
            Some(option)
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

pub(super) fn batch_kind_label(kind: ConnectionBatchKind) -> SharedString {
    match kind {
        ConnectionBatchKind::Ssh => "SSH".into(),
        ConnectionBatchKind::Telnet => "Telnet".into(),
        ConnectionBatchKind::Rdp => "RDP".into(),
        ConnectionBatchKind::Vnc => "VNC".into(),
        ConnectionBatchKind::Adb => "Android".into(),
        ConnectionBatchKind::LocalTerminal => t!("dialog.localTerminal").into(),
        ConnectionBatchKind::Serial => t!("dialog.serial").into(),
    }
}
pub(super) fn scope_select_options(draft: &AssetBatchEditDraft) -> Vec<NyaSelectOption> {
    let mut options = vec![NyaSelectOption::new(
        "common",
        t!("assets.batch.commonScope", count = draft.ids().len()),
    )];
    options.extend(draft.counts().iter().map(|(kind, count)| {
        NyaSelectOption::new(
            batch_kind_value(*kind),
            format!("{} ({count})", batch_kind_label(*kind)),
        )
    }));
    options
}

pub(super) fn default_icon_value() -> Option<String> {
    Some(DEFAULT_CONNECTION_ICON.to_string())
}
