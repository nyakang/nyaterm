//! Service-neutral form widgets shared by every network tool window.

use gpui::{AnyElement, FontWeight, IntoElement, SharedString, div, prelude::*, px, rgb};
use nyaterm_core::models::networktools::DEFAULT_TFTP_BIND_ADDRESS;
use nyaterm_remote_ops::local_network::LocalInterfaceAddress;
use nyaterm_ui::{NyaIconButton, NyaSwitch, NyaTooltip};
use rust_i18n::t;

use crate::theme::ThemePalette;

/// One selectable bind address. `value` is both the address the listener binds
/// and the option label shown on the left; the interface name or role is the
/// subtitle rendered on the right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::features) struct BindOption {
    pub value: String,
    pub subtitle: String,
}

/// Build the bind-address options from one interface snapshot.
///
/// The fixed IPv4 and IPv6 wildcard and loopback entries stay first. Loopback,
/// unspecified, unscoped link-local IPv6 and non-up addresses are skipped. A
/// saved address not detected on this machine is appended so the selector never blanks it or
/// silently rewrites the user's setting.
///
/// Pure on purpose: the interface list is snapshotted at the window-open
/// boundary, never from a render path.
pub(in crate::features) fn bind_options(
    interfaces: &[LocalInterfaceAddress],
    saved: &str,
) -> Vec<BindOption> {
    let mut options = vec![
        BindOption {
            value: DEFAULT_TFTP_BIND_ADDRESS.to_string(),
            subtitle: t!("networktools.common.bind.allInterfaces").to_string(),
        },
        BindOption {
            value: "127.0.0.1".to_string(),
            subtitle: t!("networktools.common.bind.loopback").to_string(),
        },
        BindOption {
            value: "::".to_string(),
            subtitle: t!("networktools.common.bind.allInterfaces").to_string(),
        },
        BindOption {
            value: "::1".to_string(),
            subtitle: t!("networktools.common.bind.loopback").to_string(),
        },
    ];
    for interface in interfaces {
        if !interface.up
            || matches!(interface.address, std::net::IpAddr::V6(address) if address.is_unicast_link_local())
            || interface.address.is_loopback()
            || interface.address.is_unspecified()
        {
            continue;
        }
        let value = interface.address.to_string();
        if options.iter().any(|option| option.value == value) {
            continue;
        }
        options.push(BindOption {
            value,
            subtitle: interface.interface.clone(),
        });
    }
    let saved = saved.trim();
    if !saved.is_empty() && !options.iter().any(|option| option.value == saved) {
        options.push(BindOption {
            value: saved.to_string(),
            subtitle: t!("networktools.common.bind.savedUnavailable").to_string(),
        });
    }
    options
}

pub(in crate::features) fn section(
    title: String,
    palette: ThemePalette,
    children: Vec<AnyElement>,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div().flex().items_center().gap_2().child(
                div()
                    .text_sm()
                    .font_weight(FontWeight(600.))
                    .text_color(rgb(palette.text))
                    .child(title),
            ),
        )
        .children(children)
        .into_any_element()
}

pub(in crate::features) fn switch_row(label: AnyElement, switch: NyaSwitch) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(div().min_w_0().flex_1().child(label))
        .child(switch)
        .into_any_element()
}

/// A field caption with an info icon whose tooltip carries the hint. The hint
/// is never rendered as standing text.
///
/// `NyaIconButton::tooltip` hands the text to gpui-kit without a width bound,
/// so it lays out on one line and the tooltip's overflow-hidden wrapper clips
/// it. The tooltip component does wrap when its content has a bounded width
/// (gpui-kit's `long_text_wraps_within_max_width` covers exactly that), so the
/// hint is attached as a width-limited custom element instead. Newlines in the
/// catalog split semantic segments onto their own lines.
pub(in crate::features) fn caption_row(
    label: String,
    hint: String,
    id: String,
    palette: ThemePalette,
) -> AnyElement {
    let tooltip_id = SharedString::from(format!("{id}.info-tooltip"));
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(label),
        )
        .child(
            div()
                .id(tooltip_id)
                .flex()
                .tooltip(move |window, cx| {
                    let hint = hint.clone();
                    NyaTooltip::element(move |_, _| {
                        div()
                            .max_w(px(320.))
                            .flex()
                            .flex_col()
                            .children(hint.split('\n').map(|line| div().child(line.to_string())))
                    })
                    .build(window, cx)
                })
                .child(
                    NyaIconButton::new(id, "icons/info.svg")
                        .size(px(16.))
                        .icon_size(px(12.)),
                ),
        )
        .into_any_element()
}

pub(in crate::features) fn plain_caption(label: String, palette: ThemePalette) -> AnyElement {
    div()
        .text_xs()
        .text_color(rgb(palette.text_muted))
        .child(label)
        .into_any_element()
}

pub(in crate::features) fn field_with_error(
    field: AnyElement,
    error: Option<String>,
    palette: ThemePalette,
) -> AnyElement {
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(field)
        .when_some(error, |this, message| {
            this.child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.danger))
                    .child(message),
            )
        })
        .into_any_element()
}

pub(in crate::features) fn field_with_caption(
    caption: AnyElement,
    input: AnyElement,
    error: Option<String>,
    palette: ThemePalette,
) -> AnyElement {
    field_with_error(
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .child(caption)
            .child(input)
            .into_any_element(),
        error,
        palette,
    )
}

#[cfg(test)]
mod bind_option_tests {
    use nyaterm_remote_ops::local_network::LocalInterfaceAddress;

    use super::{BindOption, bind_options};

    fn interface_address(interface: &str, address: &str, up: bool) -> LocalInterfaceAddress {
        LocalInterfaceAddress {
            interface: interface.to_string(),
            address: address.parse().expect("parse test address"),
            prefix: 24,
            up,
        }
    }

    fn values(options: &[BindOption]) -> Vec<&str> {
        options.iter().map(|option| option.value.as_str()).collect()
    }

    #[test]
    fn fixed_entries_come_first_and_unusable_addresses_are_skipped() {
        let interfaces = vec![
            interface_address("lo", "127.0.0.1", true),
            interface_address("lo", "::1", true),
            interface_address("any", "0.0.0.0", true),
            interface_address("eth1", "10.0.0.5", false),
            interface_address("eth0", "192.168.1.10", true),
        ];
        let options = bind_options(&interfaces, "");
        assert_eq!(
            values(&options),
            ["0.0.0.0", "127.0.0.1", "::", "::1", "192.168.1.10"]
        );
        assert_eq!(options[4].subtitle, "eth0");
    }

    #[test]
    fn every_address_on_a_multi_address_interface_is_listed_once() {
        let interfaces = vec![
            interface_address("eth0", "192.168.1.10", true),
            interface_address("eth0", "10.0.0.10", true),
            interface_address("wlan0", "192.168.1.10", true),
        ];
        let options = bind_options(&interfaces, "");
        assert_eq!(
            values(&options),
            [
                "0.0.0.0",
                "127.0.0.1",
                "::",
                "::1",
                "192.168.1.10",
                "10.0.0.10"
            ]
        );
        assert_eq!(options[4].subtitle, "eth0");
        assert_eq!(options[5].subtitle, "eth0");
    }

    #[test]
    fn global_ipv6_addresses_are_offered_without_unscoped_link_local_addresses() {
        let interfaces = vec![
            interface_address("lo", "::1", true),
            interface_address("any", "::", true),
            interface_address("eth0", "2001:db8::1", true),
            interface_address("eth0", "fe80::1", true),
            interface_address("eth0", "192.168.1.10", true),
        ];
        let options = bind_options(&interfaces, "");
        assert_eq!(
            values(&options),
            [
                "0.0.0.0",
                "127.0.0.1",
                "::",
                "::1",
                "2001:db8::1",
                "192.168.1.10"
            ]
        );
    }

    #[test]
    fn a_saved_address_missing_from_this_machine_is_kept_selectable() {
        let interfaces = vec![interface_address("eth0", "192.168.1.10", true)];

        let missing = bind_options(&interfaces, " 192.168.50.7 ");
        assert_eq!(
            values(&missing),
            [
                "0.0.0.0",
                "127.0.0.1",
                "::",
                "::1",
                "192.168.1.10",
                "192.168.50.7"
            ]
        );
        assert!(!missing[5].subtitle.is_empty());

        let present = bind_options(&interfaces, "192.168.1.10");
        assert_eq!(
            values(&present),
            ["0.0.0.0", "127.0.0.1", "::", "::1", "192.168.1.10"]
        );

        // A saved IPv6 loopback value selects the fixed entry without duplication.
        let legacy_ipv6 = bind_options(&interfaces, "::1");
        assert_eq!(
            values(&legacy_ipv6),
            ["0.0.0.0", "127.0.0.1", "::", "::1", "192.168.1.10"]
        );
    }
}
