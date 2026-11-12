use gpui::{
    AppContext as _, ClickEvent, Context, FocusHandle, IntoElement, Modifiers, Render,
    TestAppContext, VisualTestContext, Window, div, prelude::*, px, rgb, size,
};
use nyaterm_core::ActionLinksMatcherSettings;
use nyaterm_core::action_links::find_action_links;
use nyaterm_core::mac_address::MacAddressFormat;
use nyaterm_ui::{init_components, nya_root};

use crate::action_links::{
    ActionLinkAction, ActionLinkPlatform, ActionLinkTarget, actions_for_match_on,
};
use crate::features::view_widgets::passive_overlay_layer;
use crate::models::ActionLinkTooltipState;
use crate::theme::theme_palette;

use super::{ActionLinkTooltipPresentation, action_link_tooltip_layer};

fn actions(value: &str) -> Vec<ActionLinkAction> {
    let item = find_action_links(value, &ActionLinksMatcherSettings::default(), true).remove(0);
    actions_for_match_on(&item, ActionLinkPlatform::Posix)
}

fn default_action(value: &str) -> ActionLinkAction {
    actions(value)
        .into_iter()
        .find(|action| action.is_default)
        .unwrap()
}

#[test]
fn built_in_tooltips_describe_the_actual_default_execution_targets() {
    crate::preload_i18n().unwrap();
    for (value, title, verb, target) in [
        (
            "assets.zip",
            "解压归档文件",
            "执行",
            ActionLinkTarget::ExecuteCommand("unzip assets.zip".into()),
        ),
        (
            "192.0.2.1",
            "Ping 地址",
            "执行",
            ActionLinkTarget::ExecuteCommand("ping 192.0.2.1".into()),
        ),
        (
            "example.com:8080",
            "发起 HTTP 请求",
            "执行",
            ActionLinkTarget::ExecuteCommand("curl http://example.com:8080".into()),
        ),
        (
            "https://example.com",
            "打开链接",
            "打开",
            ActionLinkTarget::OpenUrl("https://example.com".into()),
        ),
        (
            "aa-bb-cc-dd-ee-ff",
            "复制 MAC 地址",
            "复制",
            ActionLinkTarget::CopyText("AA:BB:CC:DD:EE:FF".into()),
        ),
    ] {
        let default = default_action(value);
        let presentation = ActionLinkTooltipPresentation::for_action(&default);
        assert_eq!(default.target, target, "{value}");
        assert_eq!(presentation.title(&default, "zh-CN"), title);
        assert_eq!(
            presentation.hint(false, false, "zh-CN"),
            format!("Ctrl + 单击 {verb}"),
        );
        assert_eq!(
            presentation.hint(true, false, "zh-CN"),
            format!("Ctrl + 单击 {verb} · Alt + 单击 更多操作"),
        );
        assert_eq!(
            presentation.hint(true, true, "zh-CN"),
            format!("⌘ + 单击 {verb} · Option + 单击 更多操作"),
        );
        assert!(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../nyaterm-app/assets")
                .join(presentation.icon_path)
                .is_file()
        );
    }
}

#[test]
fn tooltip_verbs_follow_targets_and_unknown_actions_keep_their_labels() {
    crate::preload_i18n().unwrap();
    let mut action = default_action("assets.zip");
    action.target = ActionLinkTarget::CopyText("unzip assets.zip".into());
    let presentation = ActionLinkTooltipPresentation::for_action(&action);
    assert_eq!(presentation.title(&action, "en"), action.label);
    assert_eq!(presentation.hint(false, false, "en"), "Ctrl + click Copy");

    let insert = actions("aa-bb-cc-dd-ee-ff")
        .into_iter()
        .find(|action| {
            action.mac_format == Some(MacAddressFormat::OctetColon)
                && matches!(action.target, ActionLinkTarget::PasteText(_))
        })
        .unwrap();
    let presentation = ActionLinkTooltipPresentation::for_action(&insert);
    assert_eq!(presentation.title(&insert, "en"), "Insert MAC address");
    assert_eq!(presentation.hint(false, true, "en"), "⌘ + click Insert");

    action.id = "future-action".into();
    action.label = "Future action".into();
    action.target = ActionLinkTarget::ExecuteCommand("echo 'CopyText'".into());
    let presentation = ActionLinkTooltipPresentation::for_action(&action);
    assert_eq!(presentation.title(&action, "en"), "Future action");
    assert_eq!(presentation.hint(false, false, "en"), "Ctrl + click Run");
}

const LOCALES: [(&str, &str); 6] = [
    (
        "en",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/locales/en.json")),
    ),
    (
        "zh-CN",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/locales/zh-CN.json")),
    ),
    (
        "zh-TW",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/locales/zh-TW.json")),
    ),
    (
        "ja",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/locales/ja.json")),
    ),
    (
        "ko",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/locales/ko.json")),
    ),
    (
        "fr",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/locales/fr.json")),
    ),
];

#[test]
fn every_catalog_translates_tooltip_titles_and_both_modifier_hints() {
    crate::preload_i18n().unwrap();
    for (locale, catalog) in LOCALES {
        let catalog: serde_json::Value = serde_json::from_str(catalog).unwrap();
        for value in [
            "assets.zip",
            "192.0.2.1",
            "example.com:8080",
            "https://example.com",
            "aa-bb-cc-dd-ee-ff",
        ] {
            let default = default_action(value);
            let presentation = ActionLinkTooltipPresentation::for_action(&default);
            let title_key = presentation
                .title_key
                .unwrap()
                .strip_prefix("terminal.")
                .unwrap();
            assert_eq!(
                presentation.title(&default, locale),
                catalog["terminal"][title_key].as_str().unwrap()
            );
            for is_macos in [false, true] {
                let hint = presentation.hint(true, is_macos, locale);
                assert!(!hint.contains("%{"), "{locale}: {hint}");
                assert!(!hint.contains("terminal."), "{locale}: {hint}");
                let verb_key = presentation
                    .activation_key
                    .strip_prefix("terminal.")
                    .unwrap();
                assert!(hint.contains(catalog["terminal"][verb_key].as_str().unwrap()));
                assert!(hint.contains(if is_macos { "Option" } else { "Alt" }));
                assert!(hint.contains(if is_macos { "⌘" } else { "Ctrl" }));
            }
        }
    }
}

struct TooltipHost {
    tip: ActionLinkTooltipState,
    theme: &'static str,
    locale: &'static str,
    focus: FocusHandle,
    clicks: Vec<Modifiers>,
}

impl Render for TooltipHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = theme_palette(self.theme);
        div()
            .id("action-link-tooltip-test-host")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                this.clicks.push(event.modifiers());
                cx.notify();
            }))
            .child(passive_overlay_layer(action_link_tooltip_layer(
                &self.tip,
                palette,
                rgb(palette.surface_elevated),
                window.viewport_size().width.into(),
                self.locale,
            )))
    }
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
}

#[gpui::test]
fn tooltip_wraps_within_window_edges_and_passes_modifier_clicks_through(cx: &mut TestAppContext) {
    crate::preload_i18n().unwrap();
    cx.update(init_components);
    let host = cx.new(|cx| TooltipHost {
        tip: ActionLinkTooltipState {
            session_id: "fixture".into(),
            x: px(0.),
            y: px(0.),
            default_action: default_action("assets.zip"),
            has_more_actions: true,
            match_key: "fixture|Archive|assets.zip|0|10".into(),
        },
        theme: "github-dark",
        locale: "en",
        focus: cx.focus_handle(),
        clicks: Vec::new(),
    });
    let hosted = host.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let focus = hosted.read(cx).focus.clone();
        window.focus(&focus, cx);
        nya_root(hosted, window, cx)
    });
    let mut wide_height = px(0.);
    let mut narrow_height = px(0.);
    for width in [800., 230.] {
        cx.simulate_resize(size(px(width), px(500.)));
        cx.run_until_parked();
        assert_eq!(
            cx.update(|window, _| window.viewport_size()),
            size(px(width), px(500.)),
        );
        for theme in ["github-dark", "github-light"] {
            for (locale, _) in LOCALES {
                for (x, y) in [(0., 0.), (width - 1., 0.), (0., 499.), (width - 1., 499.)] {
                    host.update(cx, |host, cx| {
                        host.tip.x = px(x);
                        host.tip.y = px(y);
                        host.theme = theme;
                        host.locale = locale;
                        cx.notify();
                    });
                    draw(cx);
                    let bounds = cx.debug_bounds("action-link-tooltip").unwrap();
                    // Anchored rounds its final origin to whole pixels, while
                    // wrapped text can have fractional height. Allow one pixel
                    // of rounding around the requested eight-pixel margin.
                    assert!(bounds.left() >= px(7.));
                    assert!(bounds.top() >= px(7.));
                    assert!(
                        bounds.right() <= px(width - 7.),
                        "{theme} {locale} width={width} pointer=({x}, {y}): {bounds:?}",
                    );
                    assert!(
                        bounds.bottom() <= px(493.),
                        "{theme} {locale} width={width} pointer=({x}, {y}): {bounds:?}",
                    );
                    let hint = cx.debug_bounds("action-link-tooltip-hint").unwrap();
                    assert!(hint.left() >= bounds.left() && hint.right() <= bounds.right());
                    if theme == "github-dark" && locale == "en" && x == 0. && y == 0. {
                        if width == 800. {
                            wide_height = hint.size.height;
                        } else {
                            narrow_height = hint.size.height;
                        }
                    }
                }
            }
        }
    }
    assert!(
        narrow_height > wide_height,
        "the hint must wrap in a narrow window"
    );
    let bounds = cx.debug_bounds("action-link-tooltip").unwrap();
    let modifiers = Modifiers {
        control: true,
        ..Default::default()
    };
    cx.simulate_click(bounds.center(), modifiers);
    cx.update(|window, cx| {
        let host = host.read(cx);
        assert_eq!(host.clicks, vec![modifiers]);
        assert!(host.focus.is_focused(window));
    });
}
