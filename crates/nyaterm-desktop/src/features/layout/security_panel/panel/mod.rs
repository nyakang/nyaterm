use rust_i18n::t;

use gpui::{
    App, ClickEvent, Context, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px,
    rgb,
};
use nyaterm_ui::{NyaDropdownMenu, NyaMenuItem, NyaScrollable, NyaTabItem, NyaTabs};

use crate::features::NyaTermApp;
use crate::models::{NavItem, PanelSide, SecurityAuthTab};
use crate::theme::ThemePalette;

mod credentials;
mod keys;
mod known_hosts;
mod otp;
mod passwords;

const SECURITY_LIST_HORIZONTAL_PADDING: f32 = 24.;
const SECURITY_LIST_COMPACT_BREAKPOINT: f32 = 224.;
const SECURITY_TABS_MIN_WIDTH: f32 = 520.;
const SECURITY_AUTH_TABS: [SecurityAuthTab; 5] = [
    SecurityAuthTab::Keys,
    SecurityAuthTab::Passwords,
    SecurityAuthTab::Otp,
    SecurityAuthTab::Credentials,
    SecurityAuthTab::KnownHosts,
];

impl NyaTermApp {
    pub(in crate::features) fn security_auth_panel(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active_tab = self.security.auth_tab();
        let palette = self.theme_palette();

        let body = match active_tab {
            SecurityAuthTab::Keys => self.security_keys_body(palette, cx),
            SecurityAuthTab::Passwords => self.security_passwords_body(palette, cx),
            SecurityAuthTab::Credentials => self.security_credentials_body(palette, cx),
            SecurityAuthTab::Otp => self.security_otp_body(palette, cx),
            SecurityAuthTab::KnownHosts => self.security_known_hosts_body(palette, cx),
        }
        .overflow_y_scrollbar();

        let tab_switcher = if self.security_panel_width() < SECURITY_TABS_MIN_WIDTH {
            NyaDropdownMenu::new("security-auth-tab-menu")
                .label(t!(active_tab.i18n_key()))
                .min_width(px(180.))
                .items(SECURITY_AUTH_TABS.map(|tab| {
                    NyaMenuItem::action(t!(tab.i18n_key()))
                        .checked(tab == active_tab)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.set_security_auth_tab(tab, window, cx);
                        }))
                }))
                .into_any_element()
        } else {
            NyaTabs::new("security-auth-tabs")
                .items(SECURITY_AUTH_TABS.map(|tab| NyaTabItem::new(t!(tab.i18n_key()))))
                .selected_index(
                    SECURITY_AUTH_TABS
                        .iter()
                        .position(|tab| *tab == active_tab)
                        .unwrap_or(0),
                )
                .on_select(cx.listener(|this, index, window, cx| {
                    if let Some(tab) = SECURITY_AUTH_TABS.get(*index) {
                        this.set_security_auth_tab(*tab, window, cx);
                    }
                }))
                .into_any_element()
        };

        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(self.shell_transparent_color(palette.surface))
            .child(
                div()
                    .px_3()
                    .pt_3()
                    .pb_0()
                    .flex()
                    .flex_col()
                    .child(tab_switcher),
            )
            .child(body)
            .when(
                matches!(
                    active_tab,
                    SecurityAuthTab::Keys
                        | SecurityAuthTab::Passwords
                        | SecurityAuthTab::Credentials
                ),
                |this| this.child(self.security_secret_footer(cx)),
            )
            .when(self.security.unlock_prompt_open(), |this| {
                this.child(self.security_unlock_prompt(cx))
            })
            .when(self.security.master_required_prompt_open(), |this| {
                this.child(self.security_master_required_prompt(cx))
            })
    }

    fn security_list_compact(&self) -> bool {
        security_list_compact_for_panel_width(self.security_panel_width())
    }

    fn security_panel_width(&self) -> f32 {
        let side = self
            .panel_side_for_item(NavItem::SecurityAuth)
            .unwrap_or(PanelSide::Left);
        let viewport_width = self.shell.viewport_size().0;
        let panel_width = match side {
            PanelSide::Left => {
                let width = self.shell.left_panel_width().clamp(160., 720.);
                if !cfg!(target_os = "macos") && viewport_width < 1024. {
                    width.min((viewport_width - 80.).max(120.))
                } else {
                    width
                }
            }
            PanelSide::Right => {
                let width = self.shell.right_panel_width().clamp(200., 720.);
                if !cfg!(target_os = "macos") && viewport_width < 768. {
                    width.min((viewport_width - 80.).max(120.))
                } else {
                    width
                }
            }
        };
        panel_width
    }
}

fn security_list_compact_for_panel_width(panel_width: f32) -> bool {
    (panel_width - SECURITY_LIST_HORIZONTAL_PADDING).max(0.) < SECURITY_LIST_COMPACT_BREAKPOINT
}

fn security_auth_body_base(id: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .pt_3()
        .pb_3()
}

fn security_tab_toolbar(
    palette: ThemePalette,
    title: impl Into<SharedString>,
    add_id: impl Into<String>,
    add_label: impl Into<SharedString>,
    enabled: bool,
    on_add: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Div {
    let title: SharedString = title.into();
    let add_label: SharedString = add_label.into();
    div()
        .flex_none()
        .h(px(28.))
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight(600.))
                .text_color(rgb(palette.text))
                .child(title),
        )
        .child(
            nyaterm_ui::NyaIconButton::new(add_id.into(), "icons/plus.svg")
                .tooltip(add_label)
                .disabled(!enabled)
                .on_click(on_add),
        )
}

#[cfg(test)]
mod tests {
    use super::security_list_compact_for_panel_width;

    #[test]
    fn security_list_layout_wraps_actions_only_below_available_width_breakpoint() {
        assert!(security_list_compact_for_panel_width(160.));
        assert!(security_list_compact_for_panel_width(240.));
        assert!(!security_list_compact_for_panel_width(320.));
    }
}
