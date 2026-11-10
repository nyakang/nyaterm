use std::{collections::HashMap, path::Path, sync::Arc};

use gpui::{
    AppContext as _, Entity, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _,
    Render, RenderImage, ScrollDelta, ScrollEvent, Styled as _, TestAppContext, VisualTestContext,
    div, point, px,
};
use nyaterm_core::{AppRuntime, Group, RuntimeMode, models::sessions::ConnectionCustomIcon};

use super::groups::{connection_editor_group_menu_options, ordered_connection_groups};
use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
use crate::features::NyaTermApp;
use crate::features::selects::PENDING_CONNECTION_GROUP_VALUE;
use crate::models::{
    ConnectionEditorCredentialOverlay, ConnectionEditorField, ConnectionEditorForwardingTab,
    ConnectionEditorPasswordSource, ConnectionEditorState, ConnectionKindTab,
};
use crate::test_support::TestConfigDir;

struct EditorHost {
    app: Entity<NyaTermApp>,
    width: f32,
    height: f32,
    font_size: f32,
    native_window: bool,
}

struct AlgorithmListHost {
    app: Entity<NyaTermApp>,
    tab: crate::models::ConnectionEditorSshAlgorithmTab,
}

impl Render for AlgorithmListHost {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let tab = self.tab;
        let list = self.app.update(cx, |app, cx| {
            let supported = nyaterm_transport::supported_ssh_algorithms();
            let (options, selected) = match tab {
                crate::models::ConnectionEditorSshAlgorithmTab::KeyExchange => {
                    (&supported.kex, &supported.compatible.kex)
                }
                crate::models::ConnectionEditorSshAlgorithmTab::Ciphers => {
                    (&supported.ciphers, &supported.compatible.ciphers)
                }
                crate::models::ConnectionEditorSshAlgorithmTab::Macs => {
                    (&supported.macs, &supported.compatible.macs)
                }
                crate::models::ConnectionEditorSshAlgorithmTab::HostKeys => {
                    (&supported.host_keys, &supported.compatible.host_keys)
                }
            };
            super::ssh::ssh_algorithm_list(app.theme_palette(), tab, options, selected, cx)
                .into_any_element()
        });
        div().w(px(340.)).text_size(px(12.)).child(list)
    }
}

impl Render for EditorHost {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let editor = self.app.read(cx).connection_state.active_editor_draft();
        let editor = self.app.update(cx, |app, cx| {
            editor
                .map(|editor| {
                    if self.native_window {
                        app.connection_editor_window_view(editor, (self.width, self.height), cx)
                    } else {
                        app.connection_editor_panel(editor, (self.width, self.height), cx)
                    }
                })
                .unwrap_or_else(|| div().into_any_element())
        });
        div()
            .debug_selector(|| "connection-editor-test-host".to_string())
            .w(px(self.width))
            .h(px(self.height))
            .text_size(px(self.font_size))
            .overflow_hidden()
            .child(editor)
    }
}

fn test_app(cx: &mut TestAppContext, root: &Path) -> Entity<NyaTermApp> {
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.to_path_buf(),
        root.join("config"),
        root.join("logs"),
        root.join("cache"),
        None,
    );
    let stores = UiStoreHandles {
        startup_restore: cx.new(|_| StartupRestoreStore::default()),
        overlays: cx.new(|_| OverlayStore::default()),
    };
    cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
}

fn hosted_editor<'a>(
    cx: &'a mut TestAppContext,
    root: &Path,
    width: f32,
    height: f32,
    font_size: f32,
) -> (Entity<NyaTermApp>, &'a mut VisualTestContext) {
    let app = test_app(cx, root);
    cx.update_entity(&app, |app, cx| {
        app.sync_component_theme(cx);
        app.connection_state.begin_editor(editor(None, None));
        app.connection_state.build_editor_fields(cx);
    });
    let host_app = app.clone();
    let (_, vcx) = cx.add_window_view(move |_, _| EditorHost {
        app: host_app,
        width,
        height,
        font_size,
        native_window: false,
    });
    let vcx: &mut VisualTestContext = vcx;
    vcx.run_until_parked();
    for _ in 0..3 {
        draw_editor(&app, vcx);
    }
    (app, vcx)
}

fn draw_editor(app: &Entity<NyaTermApp>, vcx: &mut VisualTestContext) {
    vcx.update(|window, cx| {
        app.update(cx, |_, cx| cx.notify());
        _ = window.draw(cx);
        window.simulate_next_frame(cx);
    });
    vcx.run_until_parked();
}

#[gpui::test]
fn telnet_network_page_and_select_focus_work_in_raw_tcp_mode(cx: &mut TestAppContext) {
    use crate::models::ConnectionEditorSection;
    let test_dir = TestConfigDir::new("nyaterm-telnet-network-editor");
    let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 900., 12.);
    for raw in [false, true] {
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let mut draft = editor(None, None);
                draft.kind = ConnectionKindTab::Telnet;
                draft.raw_tcp_cli = raw;
                draft.proxy_id = Some("retained-proxy".into());
                draft.proxy_jump_id = Some("retained-jump".into());
                app.connection_state.begin_editor(draft);
                app.connection_state.build_editor_fields(cx);
                app.set_connection_editor_section(ConnectionEditorSection::Network, cx);
            });
        });
        draw_editor(&app, vcx);
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let draft = app.connection_state.active_editor_draft().unwrap();
                assert_eq!(draft.section, ConnectionEditorSection::Network);
                assert_eq!(draft.proxy_id.as_deref(), Some("retained-proxy"));
                assert_eq!(draft.proxy_jump_id.as_deref(), Some("retained-jump"));
                for select in ["connection-editor-proxy", "connection-editor-proxy-jump"] {
                    assert!(app.focus_select(select, window, cx));
                    assert!(app.connection_editor_select_is_focused(window, cx));
                }
            });
        });
    }
}

#[gpui::test]
fn mstsc_editor_hides_unused_controls_and_restores_them_when_switching_back(
    cx: &mut TestAppContext,
) {
    use crate::models::ConnectionEditorSection;
    let test_dir = TestConfigDir::new("nyaterm-mstsc-editor-layout");
    let (app, vcx) = hosted_editor(cx, test_dir.path(), 900., 680., 12.);
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let mut draft = editor(None, None);
            draft.kind = ConnectionKindTab::Rdp;
            draft.host = "example.test".into();
            draft.port = "3389".into();
            draft.rdp_backend = "mstsc".into();
            draft.rdp_display.mode = "fixed".into();
            draft.rdp_reconnect.enabled = true;
            app.connection_state.begin_editor(draft);
            app.connection_state.build_editor_fields(cx);
        })
    });
    for section in [
        ConnectionEditorSection::Basic,
        ConnectionEditorSection::Security,
        ConnectionEditorSection::Display,
        ConnectionEditorSection::Session,
    ] {
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| app.set_connection_editor_section(section, cx))
        });
        draw_editor(&app, vcx);
        for selector in [
            "connection-editor-field-Username",
            "connection-editor-field-Domain",
            "connection-editor-field-Password",
            "connection-editor-field-RdpDisplayWidth",
            "connection-editor-field-RdpDisplayHeight",
            "connection-editor-field-RdpReconnectAttempts",
        ] {
            assert!(
                vcx.debug_bounds(selector).is_none(),
                "{section:?}: {selector}"
            );
        }
    }
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.on_select_changed("connection-editor-rdp-backend", Some("builtin"), cx);
            app.set_connection_editor_section(ConnectionEditorSection::Basic, cx);
        })
    });
    draw_editor(&app, vcx);
    assert!(
        vcx.debug_bounds("connection-editor-field-Username")
            .is_some()
    );
}

#[test]
fn ssh_algorithm_lists_keep_rows_full_height_and_scroll_inside_the_viewport() {
    use crate::models::ConnectionEditorSshAlgorithmTab;

    for tab in [
        ConnectionEditorSshAlgorithmTab::KeyExchange,
        ConnectionEditorSshAlgorithmTab::Ciphers,
        ConnectionEditorSshAlgorithmTab::Macs,
        ConnectionEditorSshAlgorithmTab::HostKeys,
    ] {
        let test_dir = TestConfigDir::new("nyaterm-ssh-algorithm-list-scroll");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| app.sync_component_theme(cx));
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| AlgorithmListHost { app: host_app, tab });
        let vcx: &mut VisualTestContext = vcx;
        draw_editor(&app, vcx);

        let viewport = vcx
            .debug_bounds("connection-ssh-algorithm-viewport")
            .unwrap();
        let first = vcx.debug_bounds("connection-ssh-algorithm-row-0").unwrap();
        let second = vcx.debug_bounds("connection-ssh-algorithm-row-1").unwrap();
        assert_eq!(first.size.height, px(36.));
        assert_eq!(second.top() - first.top(), px(36.));
        assert!(viewport.size.height <= px(226.));
        let overlay = vcx.debug_bounds("scrollbar-overlay").unwrap();
        assert!(overlay.size.height >= viewport.size.height - px(2.));

        vcx.simulate_event(ScrollEvent {
            position: viewport.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-108.))),
            ..Default::default()
        });
        draw_editor(&app, vcx);
        let after = vcx.debug_bounds("connection-ssh-algorithm-row-0").unwrap();
        if tab == ConnectionEditorSshAlgorithmTab::Macs {
            assert_eq!(after.top(), first.top(), "short lists should not scroll");
        } else {
            assert!(after.top() < first.top(), "{tab:?} must scroll its content");
        }
        assert_eq!(
            vcx.debug_bounds("connection-ssh-algorithm-viewport")
                .unwrap(),
            viewport
        );
        assert_eq!(vcx.debug_bounds("scrollbar-overlay").unwrap(), overlay);
    }
}

fn protocol_selectors(kind: ConnectionKindTab) -> (&'static str, &'static str) {
    match kind {
        ConnectionKindTab::Ssh => (
            "connection-editor-ssh-section",
            "connection-editor-ssh-advanced",
        ),
        ConnectionKindTab::Local => (
            "connection-editor-local-section",
            "connection-editor-local-advanced",
        ),
        ConnectionKindTab::Telnet => (
            "connection-editor-telnet-section",
            "connection-editor-telnet-advanced",
        ),
        ConnectionKindTab::Serial => (
            "connection-editor-serial-section",
            "connection-editor-serial-advanced",
        ),
        ConnectionKindTab::Rdp => (
            "connection-editor-rdp-section",
            "connection-editor-rdp-advanced",
        ),
        ConnectionKindTab::Vnc => (
            "connection-editor-vnc-section",
            "connection-editor-vnc-advanced",
        ),
    }
}

fn group(id: &str, name: &str, parent_id: Option<&str>, sort_order: i32) -> Group {
    Group {
        id: id.to_string(),
        name: name.to_string(),
        parent_id: parent_id.map(ToOwned::to_owned),
        sort_order,
        created_at_ms: None,
        updated_at_ms: None,
    }
}

fn editor(group_id: Option<&str>, pending_group_name: Option<&str>) -> ConnectionEditorState {
    ConnectionEditorState {
        id: None,
        kind: ConnectionKindTab::Ssh,
        name: String::new(),
        description: String::new(),
        tags: Vec::new(),
        new_tag: String::new(),
        icon: None,
        icon_auto_detect: false,
        recording: None,
        group_id: group_id.map(ToOwned::to_owned),
        new_group_name: String::new(),
        pending_group_name: pending_group_name.map(ToOwned::to_owned),
        pending_group_parent_id: group_id.map(ToOwned::to_owned),
        host: String::new(),
        port: "22".to_string(),
        username: String::new(),
        domain: String::new(),
        auth_mode: "password".to_string(),
        rdp_backend: "builtin".to_string(),
        rdp_security: Default::default(),
        rdp_display: Default::default(),
        rdp_clipboard: Default::default(),
        rdp_reconnect: Default::default(),

        vnc_security: Default::default(),
        vnc_display: Default::default(),
        vnc_clipboard: Default::default(),
        vnc_reconnect: Default::default(),
        vnc_shared: true,
        vnc_view_only: false,
        password_source: ConnectionEditorPasswordSource::Ask,
        password_id: None,
        account_id: None,
        username_source: nyaterm_core::ConnectionUsernameSource::Connection,
        password_source_touched: false,
        password_source_demoted: false,
        password_edit: crate::models::ConnectionPasswordEdit::Replace,
        field_errors: Default::default(),
        password: nyaterm_core::SecretString::default(),
        existing_password: None,
        existing_password_locked: false,
        key_id: None,
        otp_id: None,
        auto_fill_otp: false,
        proxy_id: None,
        proxy_jump_id: None,
        host_key_alias: None,
        x11_forwarding: false,
        dynamic_tab_title: false,
        agent_endpoint: Default::default(),
        agent_forwarding_config: Default::default(),
        agent_allow_all_confirmed: false,
        agent_forwarding_endpoint_index: 0,
        agent_preview: None,
        agent_preview_loading: false,
        backspace_mode: "del".to_string(),
        encoding: String::new(),
        ssh_compatibility_mode: false,
        ssh_profile: Default::default(),
        terminal_type: None,
        sftp_enabled: true,
        sftp_compatibility_mode: false,
        sftp_cwd_follow_mode: "shell_integration".to_string(),
        sftp_shell_detection_timeout_ms: "3000".to_string(),
        sftp_pipeline_depth: None,
        sftp_extra: Default::default(),
        sftp_filename_encoding: String::new(),
        ssh_algorithm_mode: "compatible".to_string(),
        ssh_algorithm_kex: Vec::new(),
        ssh_algorithm_ciphers: Vec::new(),
        ssh_algorithm_macs: Vec::new(),
        ssh_algorithm_host_keys: Vec::new(),
        ssh_algorithm_tab: crate::models::ConnectionEditorSshAlgorithmTab::KeyExchange,
        shell_path: String::new(),
        shell_args: String::new(),
        working_dir: String::new(),
        serial_port: String::new(),
        baud_rate: "115200".to_string(),
        data_bits: "8".to_string(),
        parity: "none".to_string(),
        flow_control: Default::default(),
        stop_bits: "1".to_string(),
        raw_tcp_cli: false,
        telnet_enter_mode: "cr".to_string(),
        local_echo: false,
        local_line_edit: false,
        force_character_at_a_time: false,
        send_naws: true,
        send_sga: true,
        telnet_auto_login_enabled: true,
        telnet_auto_login_send_wake_enter: true,
        telnet_keep_alive_interval_secs:
            nyaterm_core::models::connection::DEFAULT_TELNET_KEEP_ALIVE_INTERVAL_SECS.to_string(),
        telnet_auto_login_timeout_ms: "60000".to_string(),
        telnet_auto_login_username_prompt_regex: String::new(),
        telnet_auto_login_password_prompt_regex: String::new(),
        telnet_auto_login_success_prompt_regex: String::new(),
        telnet_auto_login_failure_prompt_regex: String::new(),
        telnet_auto_login_max_retries: "0".to_string(),
        post_login_enabled: false,
        post_login_command: String::new(),
        post_login_delay_ms: "0".to_string(),

        forwarding_tab: ConnectionEditorForwardingTab::Agent,

        connect_after_save: false,
        section: crate::models::ConnectionEditorSection::Basic,
        focused_field: ConnectionEditorField::Name,
        error: None,
    }
}

#[test]
fn connection_editor_group_select_orders_tree_and_keeps_orphans_visible() {
    let groups = vec![
        group("child", "Child", Some("parent"), 0),
        group("orphan", "Orphan", Some("missing"), 2),
        group("parent", "Parent", None, 1),
    ];

    let ordered = ordered_connection_groups(&groups)
        .into_iter()
        .map(|(group, depth)| (group.id, depth))
        .collect::<Vec<_>>();

    assert_eq!(
        ordered,
        vec![
            ("parent".to_string(), 0),
            ("child".to_string(), 1),
            ("orphan".to_string(), 0),
        ]
    );
}

#[test]
fn connection_editor_group_menu_options_keep_depth_labels_and_pending_group() {
    let groups = vec![
        group("child", "Child", Some("parent"), 0),
        group("parent", "Parent", None, 1),
    ];
    let editor = editor(Some("parent"), Some("Staging"));

    let options = connection_editor_group_menu_options(&groups, &editor, "None", "New Group");
    let values = options
        .iter()
        .map(|option| {
            (
                option.value.as_deref(),
                option.label.as_str(),
                option.depth,
                option.selected,
                option.disabled,
                option.none_option,
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        values,
        vec![
            (None, "None", 0, false, false, true),
            (
                Some(PENDING_CONNECTION_GROUP_VALUE),
                "Staging (New Group)",
                0,
                true,
                true,
                false
            ),
            (Some("parent"), "Parent", 0, false, false, false),
            (Some("child"), "Child", 1, false, false, false),
        ]
    );
}

#[test]
fn connection_editor_group_menu_options_select_existing_group_without_pending_group() {
    let groups = vec![
        group("child", "Child", Some("parent"), 0),
        group("parent", "Parent", None, 1),
    ];
    let editor = editor(Some("child"), None);

    let selected = connection_editor_group_menu_options(&groups, &editor, "None", "New Group")
        .into_iter()
        .filter(|option| option.selected)
        .map(|option| option.value)
        .collect::<Vec<_>>();

    assert_eq!(selected, vec![Some("child".to_string())]);
}

fn visible_password_eye(
    app: &Entity<NyaTermApp>,
    vcx: &mut VisualTestContext,
) -> gpui::Bounds<gpui::Pixels> {
    app.read_with(vcx, |app, _| app.connection_state.editor_scroll())
        .set_offset(point(px(0.), px(-10000.)));
    draw_editor(app, vcx);
    draw_editor(app, vcx);
    vcx.debug_bounds("nya-input-mask-toggle").unwrap()
}

#[gpui::test]
fn saved_connection_password_supports_typing_paste_clear_and_locked_delete(
    cx: &mut TestAppContext,
) {
    use crate::models::ConnectionPasswordEdit;
    let test_dir = TestConfigDir::new("nyaterm-single-password-input");
    let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 900., 12.);
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let mut draft = editor(None, None);
            draft.id = Some("saved".into());
            draft.password_source = ConnectionEditorPasswordSource::Direct;
            draft.password_edit = ConnectionPasswordEdit::Keep;
            draft.password = "old".into();
            draft.existing_password = Some("old".into());
            app.connection_state.begin_editor(draft);
            app.connection_state.build_editor_fields(cx);
        })
    });
    draw_editor(&app, vcx);
    assert!(
        vcx.debug_bounds("connection-editor-field-Password")
            .is_some()
    );
    vcx.update(|window, cx| {
        let input =
            &app.read(cx).connection_state.editor_fields()[&ConnectionEditorField::Password];
        assert_eq!(input.read(cx).value(cx), "old");
        assert!(input.read(cx).is_masked());
        window.focus(&input.read(cx).component_focus_handle(cx), cx);
    });
    draw_editor(&app, vcx);
    let eye = visible_password_eye(&app, vcx);
    vcx.simulate_click(eye.center(), Modifiers::default());
    draw_editor(&app, vcx);
    assert!(app.read_with(vcx, |app, cx| {
        !app.connection_state.editor_fields()[&ConnectionEditorField::Password]
            .read(cx)
            .is_masked()
            && !app.security.unlock_prompt_open()
    }));
    let eye = visible_password_eye(&app, vcx);
    vcx.simulate_click(eye.center(), Modifiers::default());
    draw_editor(&app, vcx);
    vcx.simulate_keystrokes("secondary-a n e w");
    draw_editor(&app, vcx);
    assert!(vcx.update(|_, cx| {
        let draft = app.read(cx).connection_state.active_editor_draft().unwrap();
        draft.password_edit == ConnectionPasswordEdit::Replace
            && draft.password.expose_secret() == "new"
    }));
    vcx.simulate_keystrokes("tab");
    draw_editor(&app, vcx);
    assert!(vcx.update(|window, cx| {
        let input =
            &app.read(cx).connection_state.editor_fields()[&ConnectionEditorField::Password];
        !input.read(cx).component_focus_handle(cx).is_focused(window)
    }));
    vcx.simulate_keystrokes("shift-tab");
    draw_editor(&app, vcx);
    assert!(vcx.update(|window, cx| {
        let input =
            &app.read(cx).connection_state.editor_fields()[&ConnectionEditorField::Password];
        input.read(cx).component_focus_handle(cx).is_focused(window)
    }));
    vcx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(" pasted ".into())));
    vcx.simulate_keystrokes("secondary-a secondary-v");
    draw_editor(&app, vcx);
    assert!(vcx.update(|_, cx| {
        app.read(cx)
            .connection_state
            .active_editor_draft()
            .unwrap()
            .password
            .expose_secret()
            == " pasted "
    }));
    vcx.simulate_keystrokes("secondary-a backspace");
    draw_editor(&app, vcx);
    assert_eq!(
        vcx.update(|_, cx| app
            .read(cx)
            .connection_state
            .active_editor_draft()
            .unwrap()
            .password_edit),
        ConnectionPasswordEdit::Remove
    );

    for key in ["backspace", "delete"] {
        vcx.update(|_, cx| app.update(cx, |app, cx| app.connection_state.clear_editor_secrets(cx)));
        draw_editor(&app, vcx);
        vcx.update(|window, cx| {
            let input =
                &app.read(cx).connection_state.editor_fields()[&ConnectionEditorField::Password];
            assert!(input.read(cx).value(cx).is_empty());
            assert!(
                app.read(cx)
                    .connection_state
                    .editor_password_can_delete_stored()
            );
            window.focus(&input.read(cx).component_focus_handle(cx), cx);
        });
        draw_editor(&app, vcx);
        let keystroke = gpui::Keystroke::parse(key).unwrap();
        vcx.simulate_event(gpui::KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        vcx.simulate_event(gpui::KeyUpEvent { keystroke });
        draw_editor(&app, vcx);
        assert_eq!(
            vcx.update(|_, cx| app
                .read(cx)
                .connection_state
                .active_editor_draft()
                .unwrap()
                .password_edit),
            ConnectionPasswordEdit::Remove
        );
    }
    vcx.simulate_keystrokes("n e w");
    draw_editor(&app, vcx);
    assert_eq!(
        vcx.update(|_, cx| app
            .read(cx)
            .connection_state
            .active_editor_draft()
            .unwrap()
            .password_edit),
        ConnectionPasswordEdit::Replace
    );
}

#[gpui::test]
fn draft_password_eye_does_not_unlock_stored_secrets(cx: &mut TestAppContext) {
    use crate::models::ConnectionPasswordEdit;

    let test_dir = TestConfigDir::new("nyaterm-draft-password-eye");
    let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 900., 12.);
    for kind in [
        ConnectionKindTab::Ssh,
        ConnectionKindTab::Telnet,
        ConnectionKindTab::Rdp,
        ConnectionKindTab::Vnc,
    ] {
        for saved in [false, true] {
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    let mut summary = app.settings.summary().clone();
                    summary.has_master_password = true;
                    app.settings.replace_summary(summary);
                    app.security.lock_secrets();
                    let mut draft = editor(None, None);
                    draft.kind = kind;
                    draft.password_source = ConnectionEditorPasswordSource::Direct;
                    if saved {
                        draft.id = Some("saved".into());
                        draft.existing_password = Some("stored ciphertext".into());
                        draft.existing_password_locked = true;
                        draft.password_edit = ConnectionPasswordEdit::Keep;
                    }
                    app.connection_state.begin_editor(draft);
                    app.connection_state.build_editor_fields(cx);
                });
            });
            draw_editor(&app, vcx);
            vcx.update(|window, cx| {
                let input = &app.read(cx).connection_state.editor_fields()
                    [&ConnectionEditorField::Password];
                window.focus(&input.read(cx).component_focus_handle(cx), cx);
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(" new draft ".into()));
            });
            vcx.simulate_keystrokes("secondary-v");
            draw_editor(&app, vcx);
            for masked in [false, true] {
                let eye = visible_password_eye(&app, vcx);
                vcx.simulate_click(eye.center(), Modifiers::default());
                draw_editor(&app, vcx);
                app.read_with(vcx, |app, cx| {
                    let input =
                        &app.connection_state.editor_fields()[&ConnectionEditorField::Password];
                    assert_eq!(input.read(cx).is_masked(), masked);
                    assert_eq!(input.read(cx).value(cx), " new draft ");
                    assert!(!app.security.unlock_prompt_open());
                    assert!(app.security_secrets_locked());
                    let draft = app.connection_state.active_editor_draft().unwrap();
                    assert_eq!(draft.password_edit, ConnectionPasswordEdit::Replace);
                    assert_eq!(draft.password.expose_secret(), " new draft ");
                    if saved {
                        assert_eq!(
                            draft.existing_password.as_deref(),
                            Some("stored ciphertext")
                        );
                    }
                });
            }
        }
    }
}

#[gpui::test]
fn password_eye_unlock_failure_cancel_and_success_preserve_the_saved_password(
    cx: &mut TestAppContext,
) {
    use crate::models::ConnectionPasswordEdit;
    let test_dir = TestConfigDir::new("nyaterm-password-eye-unlock");
    let app = test_app(cx, test_dir.path());
    app.update(cx, |app, cx| {
        app.sync_component_theme(cx);
        let mut summary = app.settings.summary().clone();
        summary.has_master_password = true;
        app.settings.replace_summary(summary);
        app.security.lock_secrets();
        let mut draft = editor(None, None);
        draft.password_source = ConnectionEditorPasswordSource::Direct;
        draft.password_edit = ConnectionPasswordEdit::Keep;
        draft.password = " old saved ".into();
        draft.existing_password = Some(" old saved ".into());
        app.connection_state.begin_editor(draft);
        app.connection_state.build_editor_fields(cx);
    });
    let host_app = app.clone();
    let (_, vcx) = cx.add_window_view(move |window, cx| {
        let content = cx.new(|_| EditorHost {
            app: host_app,
            width: 640.,
            height: 900.,
            font_size: 12.,
            native_window: false,
        });
        nyaterm_ui::nya_root(content, window, cx)
    });
    let vcx: &mut VisualTestContext = vcx;
    draw_editor(&app, vcx);
    app.update(vcx, |app, _| {
        let mut summary = app.settings.summary().clone();
        summary.has_master_password = true;
        app.settings.replace_summary(summary);
        app.security.lock_secrets();
    });
    let eye = visible_password_eye(&app, vcx);
    vcx.simulate_click(eye.center(), Modifiers::default());
    draw_editor(&app, vcx);
    assert!(app.read_with(vcx, |app, _| app.security.unlock_prompt_open()));
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let request = app.security.begin_unlock_request().unwrap().0;
            let generation = app.security.unlock_dialog_generation();
            app.finish_security_unlock(
                request,
                generation,
                window.window_handle(),
                Ok(false),
                window,
                cx,
            );
            assert!(app.security.unlock_error().is_some());
            assert_eq!(
                app.connection_state
                    .active_editor_draft()
                    .unwrap()
                    .password
                    .expose_secret(),
                " old saved "
            );
        })
    });
    draw_editor(&app, vcx);
    vcx.simulate_keystrokes("escape");
    draw_editor(&app, vcx);
    assert!(!app.read_with(vcx, |app, _| app.security.unlock_prompt_open()));
    assert_eq!(
        app.read_with(vcx, |app, _| app
            .connection_state
            .active_editor_draft()
            .unwrap()
            .password_edit),
        ConnectionPasswordEdit::Keep
    );

    let eye = visible_password_eye(&app, vcx);
    vcx.simulate_click(eye.center(), Modifiers::default());
    draw_editor(&app, vcx);
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let request = app.security.begin_unlock_request().unwrap().0;
            let generation = app.security.unlock_dialog_generation();
            app.finish_security_unlock(
                request,
                generation,
                window.window_handle(),
                Ok(true),
                window,
                cx,
            );
        })
    });
    draw_editor(&app, vcx);
    vcx.update(|_, cx| {
        let input =
            &app.read(cx).connection_state.editor_fields()[&ConnectionEditorField::Password];
        assert!(!input.read(cx).is_masked());
        assert_eq!(input.read(cx).value(cx), " old saved ");
    });
    // Hiding must work even if the security owner has become locked.
    vcx.update(|_, cx| app.update(cx, |app, _| app.security.lock_secrets()));
    let eye = visible_password_eye(&app, vcx);
    vcx.simulate_click(eye.center(), Modifiers::default());
    draw_editor(&app, vcx);
    assert!(!app.read_with(vcx, |app, _| app.security.unlock_prompt_open()));
    assert!(app.read_with(vcx, |app, cx| {
        app.connection_state.editor_fields()[&ConnectionEditorField::Password]
            .read(cx)
            .is_masked()
    }));
}

#[gpui::test]
fn account_password_input_rejects_typing_and_paste_and_unlink_restores_editing(
    cx: &mut TestAppContext,
) {
    use crate::models::{ConnectionEditorSelect, ConnectionPasswordEdit};
    let test_dir = TestConfigDir::new("nyaterm-account-readonly-password");
    let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 900., 12.);
    for kind in [
        ConnectionKindTab::Ssh,
        ConnectionKindTab::Telnet,
        ConnectionKindTab::Rdp,
        ConnectionKindTab::Vnc,
    ] {
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let mut draft = editor(None, None);
                draft.kind = kind;
                draft.password_edit = ConnectionPasswordEdit::Keep;
                if matches!(kind, ConnectionKindTab::Ssh | ConnectionKindTab::Telnet) {
                    draft.account_id = Some("account".into());
                } else {
                    draft.password_id = Some("account".into());
                }
                draft.password_source = ConnectionEditorPasswordSource::Saved;
                app.connection_state.begin_editor(draft);
                app.connection_state.build_editor_fields(cx);
                let token = app.connection_state.begin_editor_password_reveal().unwrap();
                app.connection_state.complete_editor_password_reveal(
                    token,
                    Ok(Some("account secret".into())),
                    cx,
                );
            })
        });
        draw_editor(&app, vcx);
        vcx.update(|window, cx| {
            let input =
                &app.read(cx).connection_state.editor_fields()[&ConnectionEditorField::Password];
            window.focus(&input.read(cx).component_focus_handle(cx), cx);
            cx.write_to_clipboard(gpui::ClipboardItem::new_string("paste".into()));
        });
        draw_editor(&app, vcx);
        vcx.simulate_keystrokes("secondary-a x secondary-v backspace delete");
        draw_editor(&app, vcx);
        assert_eq!(
            app.read_with(vcx, |app, cx| app.connection_state.editor_fields()
                [&ConnectionEditorField::Password]
                .read(cx)
                .value(cx)),
            "account secret"
        );
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let select = if matches!(kind, ConnectionKindTab::Ssh | ConnectionKindTab::Telnet) {
                    ConnectionEditorSelect::Account
                } else {
                    ConnectionEditorSelect::SavedPassword
                };
                app.set_connection_editor_select_value(select, None, cx);
            })
        });
        draw_editor(&app, vcx);
        vcx.update(|window, cx| {
            let input =
                &app.read(cx).connection_state.editor_fields()[&ConnectionEditorField::Password];
            assert!(input.read(cx).value(cx).is_empty());
            window.focus(&input.read(cx).component_focus_handle(cx), cx);
        });
        draw_editor(&app, vcx);
        vcx.simulate_keystrokes("n e w");
        draw_editor(&app, vcx);
        assert_eq!(
            app.read_with(vcx, |app, _| app
                .connection_state
                .active_editor_draft()
                .unwrap()
                .password
                .expose_secret()
                .to_owned()),
            "new"
        );
    }
}

#[gpui::test]
fn post_login_editor_preserves_edits_across_navigation_and_saves_only_with_shortcut(
    cx: &mut TestAppContext,
) {
    use crate::features::connections::ConnectionEditorToggle;
    use crate::models::ConnectionEditorSection;

    let test_dir = TestConfigDir::new("nyaterm-post-login-editor");
    let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 720., 12.);
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let mut draft = editor(None, None);
            draft.post_login_enabled = true;
            draft.post_login_command = "echo hello".into();
            app.connection_state.begin_editor(draft);
            app.connection_state.build_editor_fields(cx);
            app.set_connection_editor_section(ConnectionEditorSection::PostLogin, cx);
        });
    });
    draw_editor(&app, vcx);
    assert_eq!(
        vcx.debug_bounds("connection-editor-post-login-command")
            .unwrap()
            .size
            .height,
        px(168.)
    );
    let input = vcx.update(|window, cx| {
        let input = app.read(cx).connection_state.editor_fields()
            [&ConnectionEditorField::PostLoginCommand]
            .clone();
        input.update(cx, |input, cx| input.focus_with_cursor_at_end(window, cx));
        input
    });
    vcx.simulate_keystrokes("enter shift-enter");
    draw_editor(&app, vcx);
    vcx.update(|_, cx| {
        let draft = app.read(cx).connection_state.active_editor_draft().unwrap();
        assert_eq!(draft.section, ConnectionEditorSection::PostLogin);
        assert!(draft.field_errors.is_empty());
        assert_eq!(draft.post_login_command, "echo hello\n\n");
    });
    for section in [
        ConnectionEditorSection::Basic,
        ConnectionEditorSection::PostLogin,
    ] {
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| app.set_connection_editor_section(section, cx))
        });
        draw_editor(&app, vcx);
    }
    for _ in 0..2 {
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.toggle_connection_editor_flag(ConnectionEditorToggle::PostLogin, cx)
            })
        });
        draw_editor(&app, vcx);
    }
    vcx.update(|window, cx| {
        assert_eq!(
            input.entity_id(),
            app.read(cx).connection_state.editor_fields()[&ConnectionEditorField::PostLoginCommand]
                .entity_id()
        );
        assert_eq!(input.read(cx).value(cx), "echo hello\n\n");
        input.update(cx, |input, cx| input.focus_with_cursor_at_end(window, cx));
    });
    vcx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-enter"
    } else {
        "ctrl-enter"
    });
    draw_editor(&app, vcx);
    vcx.update(|_, cx| {
        let draft = app.read(cx).connection_state.active_editor_draft().unwrap();
        assert_eq!(draft.post_login_command, "echo hello\n\n");
        assert!(
            draft
                .field_errors
                .contains_key(&ConnectionEditorField::Host),
            "save must validate the connection"
        );
        assert_eq!(draft.section, ConnectionEditorSection::Basic);
    });
}

#[gpui::test]
fn connection_editor_enter_creates_inline_tags_and_keeps_input_focus(cx: &mut TestAppContext) {
    let test_dir = TestConfigDir::new("nyaterm-connection-inline-tags");
    let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 720., 12.);

    for id in [None, Some("saved-local")] {
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let mut draft = editor(None, None);
                draft.id = id.map(ToOwned::to_owned);
                draft.kind = ConnectionKindTab::Local;
                app.connection_state.begin_editor(draft);
                app.connection_state.build_editor_fields(cx);
                cx.notify();
            });
        });
        draw_editor(&app, vcx);
        let tags = vcx
            .debug_bounds("connection-editor-tags")
            .expect("tag field");
        let description = vcx
            .debug_bounds("connection-editor-description")
            .expect("description field");
        assert!(tags.bottom() < description.top());
        vcx.update(|window, cx| {
            let input =
                &app.read(cx).connection_state.editor_fields()[&ConnectionEditorField::NewTag];
            window.focus(&input.read(cx).component_focus_handle(cx), cx);
        });
        draw_editor(&app, vcx);
        vcx.simulate_keystrokes("d o c k e r enter");
        draw_editor(&app, vcx);
        vcx.update(|window, cx| {
            let state = &app.read(cx).connection_state;
            let draft = state
                .active_editor_draft()
                .expect("Enter keeps the editor open");
            assert_eq!(draft.tags, ["docker"]);
            assert!(draft.new_tag.is_empty());
            let input = &state.editor_fields()[&ConnectionEditorField::NewTag];
            assert_eq!(input.read(cx).value(cx), "");
            assert!(input.read(cx).component_focus_handle(cx).is_focused(window));
        });
        vcx.simulate_keystrokes("g p u enter");
        draw_editor(&app, vcx);
        let tags_after = vcx
            .debug_bounds("connection-editor-tags")
            .expect("inline tags");
        assert_eq!(tags.size.height, tags_after.size.height);
        vcx.update(|_, cx| {
            assert_eq!(
                app.read(cx)
                    .connection_state
                    .active_editor_draft()
                    .unwrap()
                    .tags,
                ["docker", "gpu"]
            );
        });
    }
}

#[gpui::test]
fn serial_custom_baud_accepts_pointer_focus_and_keyboard_edits(cx: &mut TestAppContext) {
    let test_dir = TestConfigDir::new("nyaterm-serial-custom-baud-input");
    let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 720., 12.);

    for (id, seed) in [(None, "115200"), (Some("saved-serial"), "76800")] {
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let mut draft = editor(None, None);
                draft.id = id.map(ToOwned::to_owned);
                draft.kind = ConnectionKindTab::Serial;
                draft.baud_rate = seed.to_string();
                app.connection_state.begin_editor(draft);
                app.connection_state.build_editor_fields(cx);
                app.set_connection_editor_section(
                    crate::models::ConnectionEditorSection::Serial,
                    cx,
                );
                app.set_connection_editor_baud_popover_open(true, cx);
                cx.notify();
            });
        });
        for _ in 0..3 {
            draw_editor(&app, vcx);
        }
        let bounds = vcx
            .debug_bounds("connection-editor-custom-baud")
            .expect("custom baud input should render");

        let input = vcx.update(|_, cx| {
            app.read(cx).connection_state.editor_number_fields()[&ConnectionEditorField::BaudRate]
                .clone()
        });
        assert!(
            input.read_with(vcx, |input, _| input.component_state().is_some()),
            "the popup must render the numeric input entity"
        );
        assert_eq!(input.read_with(vcx, |input, cx| input.value(cx)), seed);
        assert!(bounds.size.width > px(0.));
        assert!(bounds.size.height >= px(32.));
        vcx.simulate_click(bounds.center(), Modifiers::default());
        draw_editor(&app, vcx);
        assert!(
            vcx.update(|window, cx| {
                input.read(cx).component_focus_handle(cx).is_focused(window)
            })
        );
        vcx.simulate_keystrokes("secondary-a 2 5 0 0 0 0");
        draw_editor(&app, vcx);
        assert_eq!(input.read_with(vcx, |input, cx| input.value(cx)), "250000");
        assert!(vcx.update(|_, cx| {
            let state = &app.read(cx).connection_state;
            assert_eq!(state.active_editor_draft().unwrap().baud_rate, "250000");
            state.editor_baud_popover_is_open()
        }));

        let apply = vcx
            .debug_bounds("connection-editor-apply-custom-baud")
            .expect("apply custom baud button should render");
        vcx.simulate_click(apply.center(), Modifiers::default());
        draw_editor(&app, vcx);
        assert!(vcx.update(|_, cx| {
            let state = &app.read(cx).connection_state;
            assert_eq!(state.active_editor_draft().unwrap().baud_rate, "250000");
            !state.editor_baud_popover_is_open()
        }));
    }
}

#[gpui::test]
fn connection_editor_icon_picker_matches_protocol_defaults_and_explicit_icons(
    cx: &mut TestAppContext,
) {
    let test_dir = TestConfigDir::new("nyaterm-connection-icon-picker-selection");
    let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 720., 12.);
    for (kind, key, selector) in [
        (
            ConnectionKindTab::Ssh,
            "server",
            "connection-editor-icon-selected-server",
        ),
        (
            ConnectionKindTab::Local,
            "local",
            "connection-editor-icon-selected-local",
        ),
        (
            ConnectionKindTab::Telnet,
            "telnet",
            "connection-editor-icon-selected-telnet",
        ),
        (
            ConnectionKindTab::Serial,
            "serial",
            "connection-editor-icon-selected-serial",
        ),
        (
            ConnectionKindTab::Rdp,
            "rdp",
            "connection-editor-icon-selected-rdp",
        ),
        (
            ConnectionKindTab::Vnc,
            "vnc",
            "connection-editor-icon-selected-vnc",
        ),
    ] {
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.set_connection_editor_kind(kind, cx);
                app.set_connection_editor_icon(None, cx);
                app.set_connection_icon_picker_open(true, cx);
            });
        });
        for _ in 0..3 {
            draw_editor(&app, vcx);
        }
        let selected = vcx
            .debug_bounds(selector)
            .expect("the protocol default must be selected in the picker");
        if kind != ConnectionKindTab::Ssh {
            assert!(
                vcx.debug_bounds("connection-editor-icon-selected-server")
                    .is_none()
            );
        }
        vcx.simulate_click(selected.center(), Modifiers::default());
        draw_editor(&app, vcx);
        assert!(vcx.update(|_, cx| {
            app.read(cx)
                .connection_state
                .active_editor_draft()
                .unwrap()
                .icon
                .as_deref()
                == Some(key)
        }));
    }
    for (stored_key, selector) in [
        ("server", "connection-editor-icon-selected-server"),
        (" Ubuntu.SVG ", "connection-editor-icon-selected-ubuntu"),
        ("kubernetes", "connection-editor-icon-selected-k8s"),
        ("unknown-icon", "connection-editor-icon-selected-vnc"),
        ("", "connection-editor-icon-selected-vnc"),
    ] {
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.set_connection_editor_icon(Some(stored_key), cx);
                app.set_connection_icon_picker_open(true, cx);
            });
        });
        for _ in 0..3 {
            draw_editor(&app, vcx);
        }
        assert!(vcx.debug_bounds(selector).is_some(), "{stored_key}");
    }
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.connection_state.custom_icons.records = vec![ConnectionCustomIcon {
                id: "test-custom-icon".into(),
                name: "Test icon".into(),
                data_url: String::new(),
                created_at_ms: 0,
                updated_at_ms: 0,
            }];
            app.set_connection_editor_icon(Some("test-custom-icon"), cx);
            app.set_connection_icon_picker_open(true, cx);
        });
    });
    for _ in 0..3 {
        draw_editor(&app, vcx);
    }
    assert!(
        vcx.debug_bounds("connection-editor-icon-selected-vnc")
            .is_none(),
        "a custom icon awaiting its image must not select the protocol fallback"
    );
}

#[gpui::test]
fn connection_editor_icon_picker_scrolls_custom_icons_without_moving_actions(
    cx: &mut TestAppContext,
) {
    let test_dir = TestConfigDir::new("nyaterm-connection-icon-picker-scroll");
    let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 720., 12.);
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.set_connection_icon_picker_open(true, cx);
        });
    });
    for _ in 0..3 {
        draw_editor(&app, vcx);
    }
    let grid_without_custom = vcx
        .debug_bounds("connection-editor-icon-grid")
        .expect("default icons should render inside the picker");
    let popup_without_custom = vcx
        .debug_bounds("connection-editor-icon-popover-content")
        .expect("icon picker should be open");

    let records = (0..14)
        .map(|index| ConnectionCustomIcon {
            id: format!("test-icon-{index}"),
            name: format!("Test icon {index}"),
            data_url: String::new(),
            created_at_ms: 0,
            updated_at_ms: 0,
        })
        .collect::<Vec<_>>();
    vcx.update(|_, cx| {
        app.update(cx, |app, _| {
            let icon = Arc::new(RenderImage::new(vec![image::Frame::new(
                image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255])),
            )]));
            let images = records
                .iter()
                .map(|record| (record.id.clone(), icon.clone()))
                .collect::<HashMap<_, _>>();
            app.connection_state.custom_icons.records = records[..7].to_vec();
            app.connection_state.custom_icons.images = Arc::new(images);
        });
    });
    for _ in 0..3 {
        draw_editor(&app, vcx);
    }
    let popup_with_seven_custom = vcx
        .debug_bounds("connection-editor-icon-popover-content")
        .expect("icon picker should grow with seven custom icons");
    let first_custom_icon = vcx
        .debug_bounds("connection-editor-custom-icon-first")
        .expect("first custom icon should render");
    let first_custom_image = vcx
        .debug_bounds("connection-editor-custom-image-first")
        .expect("image inside first custom icon should render");
    assert_eq!(first_custom_image.left() - first_custom_icon.left(), px(6.));
    assert_eq!(
        popup_with_seven_custom.size.height - popup_without_custom.size.height,
        px(28.)
    );

    vcx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.connection_state.custom_icons.records = records;
        });
    });
    for _ in 0..3 {
        draw_editor(&app, vcx);
    }

    let grid_before = vcx
        .debug_bounds("connection-editor-icon-grid")
        .expect("icon grid should render inside the picker");
    let popup_before = vcx
        .debug_bounds("connection-editor-icon-popover-content")
        .expect("icon picker should be open");
    let rightmost_icon = vcx
        .debug_bounds("connection-editor-icon-rightmost")
        .expect("rightmost icon should render clear of the scrollbar");
    let auto_detect_before = vcx
        .debug_bounds("connection-editor-icon-auto-detect")
        .expect("auto-detect action should render below the icon grid");
    assert_eq!(grid_without_custom.size.height, px(252.));
    assert!(grid_before.size.height > px(280.));
    assert!(popup_before.right() - rightmost_icon.right() >= px(25.));
    assert_eq!(
        popup_before.size.height - popup_without_custom.size.height,
        px(28.)
    );
    vcx.simulate_event(ScrollEvent {
        position: point(grid_before.left() + px(20.), grid_before.top() + px(20.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-48.))),
        ..Default::default()
    });
    draw_editor(&app, vcx);

    let grid_after = vcx
        .debug_bounds("connection-editor-icon-grid")
        .expect("icon grid should remain scrollable");
    let popup_after = vcx
        .debug_bounds("connection-editor-icon-popover-content")
        .expect("icon picker should remain open");
    let auto_detect_after = vcx
        .debug_bounds("connection-editor-icon-auto-detect")
        .expect("auto-detect action should remain below the icon grid");
    assert!(
        grid_after.top() < grid_before.top(),
        "grid: {grid_before:?} -> {grid_after:?}, popup: {popup_before:?} -> {popup_after:?}, auto-detect: {auto_detect_before:?} -> {auto_detect_after:?}"
    );
    assert_eq!(popup_after.top(), popup_before.top());
    assert_eq!(auto_detect_after.top(), auto_detect_before.top());
}

#[gpui::test]
fn vnc_credential_manager_blocks_editor_clicks_and_closes(cx: &mut TestAppContext) {
    let test_dir = TestConfigDir::new("nyaterm-vnc-credential-overlay");
    let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 720., 12.);
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.set_connection_editor_kind(ConnectionKindTab::Vnc, cx);
            app.set_connection_editor_section(crate::models::ConnectionEditorSection::Basic, cx);
            app.connection_state
                .set_editor_password_source(ConnectionEditorPasswordSource::Saved);
            app.set_connection_editor_credential_overlay(
                Some(ConnectionEditorCredentialOverlay::Passwords),
                cx,
            );
        });
    });
    draw_editor(&app, vcx);
    assert_eq!(
        vcx.update(|_, cx| app.read(cx).connection_state.editor_credential_overlay()),
        Some(ConnectionEditorCredentialOverlay::Passwords)
    );

    let editor_close = vcx
        .debug_bounds("connection-editor-close")
        .expect("editor close button should render under the overlay");
    vcx.simulate_click(editor_close.center(), Modifiers::default());
    draw_editor(&app, vcx);
    assert_eq!(
        vcx.update(|_, cx| app.read(cx).connection_state.editor_credential_overlay()),
        Some(ConnectionEditorCredentialOverlay::Passwords)
    );
    assert!(vcx.update(|_, cx| {
        app.read(cx)
            .connection_state
            .active_editor_draft()
            .is_some()
    }));

    let overlay_close = vcx
        .debug_bounds("connection-editor-credential-close")
        .expect("credential manager close button should render");
    vcx.simulate_click(overlay_close.center(), Modifiers::default());
    draw_editor(&app, vcx);
    assert_eq!(
        vcx.update(|_, cx| app.read(cx).connection_state.editor_credential_overlay()),
        None
    );
    assert!(
        vcx.debug_bounds("connection-editor-credential-overlay")
            .is_none()
    );
}

#[gpui::test]
fn account_dropdown_escape_closes_the_menu_before_the_connection_editor(cx: &mut TestAppContext) {
    use crate::models::ConnectionEditorSelect;
    let test_dir = TestConfigDir::new("nyaterm-account-dropdown-escape");
    let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 720., 12.);
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let account = nyaterm_core::SavedPassword {
                id: "identity".into(),
                name: "Identity".into(),
                username: "root".into(),
                password: None,
                has_password: false,
                sort_order: 0,
            };
            app.security
                .replace_catalog(Vec::new(), Vec::new(), vec![account], Vec::new());
            app.set_connection_editor_section(crate::models::ConnectionEditorSection::Basic, cx);
        })
    });
    draw_editor(&app, vcx);
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.focus_connection_editor_select(ConnectionEditorSelect::Account, window, cx)
        })
    });
    draw_editor(&app, vcx);
    vcx.simulate_keystrokes("enter");
    draw_editor(&app, vcx);
    assert!(vcx.update(|window, cx| {
        app.read(cx)
            .connection_editor_select_menu_is_focused(window, cx)
    }));
    vcx.simulate_keystrokes("escape");
    draw_editor(&app, vcx);
    assert!(vcx.update(|window, cx| {
        let app = app.read(cx);
        app.connection_state.active_editor_draft().is_some()
            && app.connection_editor_select_is_focused(window, cx)
            && !app.connection_editor_select_menu_is_focused(window, cx)
    }));
    vcx.simulate_keystrokes("escape");
    assert!(vcx.update(|_, cx| {
        app.read(cx)
            .connection_state
            .active_editor_draft()
            .is_none()
    }));
}

#[gpui::test]
fn vnc_no_authentication_hides_password_controls_without_losing_the_draft(cx: &mut TestAppContext) {
    let test_dir = TestConfigDir::new("nyaterm-vnc-auth-draft");
    let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 720., 12.);
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.set_connection_editor_kind(ConnectionKindTab::Vnc, cx);
            app.set_connection_editor_section(crate::models::ConnectionEditorSection::Basic, cx);
            app.connection_state
                .set_editor_password_source(ConnectionEditorPasswordSource::Direct);
            app.connection_state
                .set_editor_field_text(ConnectionEditorField::Password, "retained fixture".into());
            app.set_connection_editor_select_value(
                crate::models::ConnectionEditorSelect::VncSecurityMode,
                Some("none"),
                cx,
            );
        })
    });
    draw_editor(&app, vcx);
    assert!(vcx.debug_bounds("connection-password-controls").is_none());
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.set_connection_editor_select_value(
                crate::models::ConnectionEditorSelect::VncSecurityMode,
                Some("auto"),
                cx,
            );
            assert_eq!(
                app.connection_state
                    .active_editor_draft()
                    .unwrap()
                    .password
                    .expose_secret(),
                "retained fixture"
            );
        })
    });
    draw_editor(&app, vcx);
    assert!(vcx.debug_bounds("connection-password-controls").is_some());
}

#[test]
fn account_dialog_returns_to_the_same_connection_and_preserves_its_drafts() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    use crate::models::ConnectionEditorSelect;
    use nyaterm_core::models::credentials::PasswordUpdate;
    let test_dir = TestConfigDir::new("nyaterm-account-dialog-return");
    let app = test_app(&mut cx, test_dir.path());
    cx.update_entity(&app, |app, cx| {
        app.sync_component_theme(cx);
        let mut draft = editor(None, None);
        draft.host = "example.com".into();
        draft.username = "root".into();
        app.connection_state.begin_editor(draft);
        app.connection_state.build_editor_fields(cx);
        app.set_connection_editor_section(crate::models::ConnectionEditorSection::Basic, cx);
    });
    let host_app = app.clone();
    let (_, vcx) = cx.add_window_view(move |window, cx| {
        let content = cx.new(|_| EditorHost {
            app: host_app,
            width: 640.,
            height: 720.,
            font_size: 12.,
            native_window: false,
        });
        nyaterm_ui::nya_root(content, window, cx)
    });
    let vcx: &mut VisualTestContext = vcx;
    draw_editor(&app, vcx);
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_connection_account_editor(None, window, cx);
            app.security
                .apply_editor_input("pw-name", "Deployment".into());
            app.security
                .apply_editor_input("pw-username", "deploy".into());
            app.security
                .apply_editor_input("pw-value", " account fixture ".into());
        })
    });
    draw_editor(&app, vcx);
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.focus_text_input_if_present("security.editor.pw-name", window, cx);
        })
    });
    draw_editor(&app, vcx);
    vcx.simulate_keystrokes("tab");
    draw_editor(&app, vcx);
    assert!(vcx.update(|window, cx| {
        app.read(cx)
            .existing_text_input("security.editor.pw-username")
            .unwrap()
            .read(cx)
            .component_focus_handle(cx)
            .is_focused(window)
    }));
    vcx.simulate_keystrokes("shift-tab");
    draw_editor(&app, vcx);
    assert!(vcx.update(|window, cx| {
        app.read(cx)
            .existing_text_input("security.editor.pw-name")
            .unwrap()
            .read(cx)
            .component_focus_handle(cx)
            .is_focused(window)
    }));
    vcx.simulate_keystrokes("enter");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while vcx.update(|_, cx| app.read(cx).security.editor_busy()) {
        assert!(
            std::time::Instant::now() < deadline,
            "account save did not complete"
        );
        vcx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    draw_editor(&app, vcx);
    let id = vcx.update(|_, cx| {
        let app = app.read(cx);
        assert!(app.security.password_editor().is_none());
        let draft = app
            .connection_state
            .active_editor_draft()
            .expect("parent stays open");
        assert_eq!(draft.username, "root");
        assert_eq!(draft.password_source, ConnectionEditorPasswordSource::Saved);
        assert_eq!(draft.host, "example.com");
        draft.account_id.unwrap()
    });
    assert!(vcx.debug_bounds("connection-account-username").is_some());
    assert!(vcx.update(|window, cx| app.read(cx).connection_editor_select_is_focused(window, cx)));
    assert!(
        vcx.debug_bounds("connection-editor-saved-password")
            .is_none()
    );
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.focus_connection_editor_select(ConnectionEditorSelect::Account, window, cx);
            app.open_connection_account_editor(Some(id), window, cx);
            assert!(matches!(
                app.security.password_editor().unwrap().password_update,
                PasswordUpdate::Keep
            ));
            app.set_account_password_update(
                PasswordUpdate::Replace("replacement fixture".into()),
                cx,
            );
            app.set_account_password_update(PasswordUpdate::Keep, cx);
            assert!(
                app.existing_text_input("security.editor.pw-value")
                    .is_none()
            );
            app.set_account_password_update(PasswordUpdate::Remove, cx);
            app.set_account_password_update(PasswordUpdate::Keep, cx);
        })
    });
    draw_editor(&app, vcx);
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.focus_text_input_if_present("security.editor.pw-name", window, cx);
        })
    });
    draw_editor(&app, vcx);
    vcx.simulate_keystrokes("escape");
    draw_editor(&app, vcx);
    assert!(vcx.update(|_, cx| app.read(cx).security.password_editor().is_none()));
    assert!(vcx.update(|window, cx| app.read(cx).connection_editor_select_is_focused(window, cx)));
    assert!(vcx.update(|_, cx| {
        app.read(cx)
            .connection_state
            .active_editor_draft()
            .is_some()
    }));
}

#[test]
fn connection_editor_categories_fit_wide_and_compact_viewports() {
    use crate::models::ConnectionEditorSection;
    let mut field_selectors = std::collections::HashMap::<_, &'static str>::new();
    for native_window in [false, true] {
        for (width, height, font_size) in [
            (900., 680., 12.),
            (785., 680., 12.),
            (784., 680., 12.),
            (760., 680., 12.),
            (759., 680., 12.),
            (420., 480., 24.),
        ] {
            let test_dir = TestConfigDir::new("nyaterm-connection-editor-layout");
            let mut cx = TestAppContext::single();
            let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let (app, vcx) = hosted_editor(&mut cx, test_dir.path(), width, height, font_size);
            vcx.update(|window, cx| {
                let host = window.root::<EditorHost>().unwrap().unwrap();
                host.update(cx, |host, cx| {
                    host.native_window = native_window;
                    cx.notify();
                });
            });
            for kind in [
                ConnectionKindTab::Ssh,
                ConnectionKindTab::Local,
                ConnectionKindTab::Telnet,
                ConnectionKindTab::Serial,
                ConnectionKindTab::Rdp,
                ConnectionKindTab::Vnc,
            ] {
                vcx.update(|_, cx| {
                    app.update(cx, |app, cx| app.set_connection_editor_kind(kind, cx))
                });
                vcx.update(|_, cx| {
                    app.update(cx, |app, cx| {
                        let mut draft = app.connection_state.active_editor_draft().unwrap();
                        draft.auth_mode = "agent".into();
                        draft.agent_forwarding_config.enabled = true;
                        draft.agent_forwarding_config.sources.external_agent = true;
                        draft.agent_forwarding_config.sources.stored_keys = true;
                        draft
                            .agent_forwarding_config
                            .sources
                            .external_agent_endpoints = vec![
                            nyaterm_core::SshAgentEndpoint::Environment {
                                variable: "SSH_AUTH_SOCK".into(),
                            },
                            nyaterm_core::SshAgentEndpoint::UnixSocket {
                                path: "/tmp/agent.sock".into(),
                            },
                        ];
                        draft.post_login_enabled = true;
                        draft.ssh_algorithm_mode = "custom".into();
                        draft.rdp_display.mode = "custom".into();
                        app.connection_state.begin_editor(draft);
                        app.connection_state.build_editor_fields(cx);
                    })
                });
                for section in ConnectionEditorSection::for_kind(kind) {
                    vcx.update(|_, cx| {
                        app.update(cx, |app, cx| {
                            app.set_connection_editor_section(*section, cx)
                        })
                    });
                    draw_editor(&app, vcx);
                    let host = vcx.debug_bounds("connection-editor-test-host").unwrap();
                    let panel = vcx.debug_bounds("connection-editor-panel").unwrap();
                    let scroll = vcx.debug_bounds("connection-editor-scroll").unwrap();
                    let body = vcx.debug_bounds(protocol_selectors(kind).0).unwrap();
                    if *section == ConnectionEditorSection::Basic
                        && matches!(
                            kind,
                            ConnectionKindTab::Ssh
                                | ConnectionKindTab::Telnet
                                | ConnectionKindTab::Rdp
                                | ConnectionKindTab::Vnc
                        )
                    {
                        let host = vcx.debug_bounds("connection-editor-field-Host").unwrap();
                        let username = vcx
                            .debug_bounds("connection-editor-field-Username")
                            .unwrap();
                        let description =
                            vcx.debug_bounds("connection-editor-description").unwrap();
                        assert!(host.bottom() <= username.top(), "{kind:?}");
                        assert!(username.bottom() <= description.top(), "{kind:?}");
                    }
                    assert!(
                        panel.left() >= host.left() && panel.right() <= host.right(),
                        "{kind:?}/{section:?}: {panel:?} in {host:?}"
                    );
                    assert!(panel.top() >= host.top() && panel.bottom() <= host.bottom());
                    assert!(scroll.size.height > px(0.));
                    assert!(
                        body.left() >= scroll.left() && body.right() <= scroll.right(),
                        "{kind:?}/{section:?}: {body:?} in {scroll:?}"
                    );
                    let field_keys = vcx.update(|_, cx| {
                        let state = &app.read(cx).connection_state;
                        state
                            .editor_fields()
                            .keys()
                            .chain(state.editor_number_fields().keys())
                            .copied()
                            .collect::<Vec<_>>()
                    });
                    for field in field_keys {
                        // GPUI's test selector API requires static strings. Intern each
                        // field name once for this matrix rather than once per frame.
                        let selector = field_selectors.entry(field).or_insert_with(|| {
                            Box::leak(format!("connection-editor-field-{field:?}").into_boxed_str())
                        });
                        if let Some(bounds) = vcx.debug_bounds(selector) {
                            assert!(bounds.size.width > px(0.));
                            assert!(
                                bounds.left() >= scroll.left() && bounds.right() <= scroll.right(),
                                "{kind:?}/{section:?}/{field:?} at {width}: {bounds:?} in {scroll:?}"
                            );
                        }
                    }
                    let navigation = vcx.debug_bounds("connection-editor-navigation").unwrap();
                    assert!(navigation.right() <= scroll.left());
                    assert_eq!(navigation.top(), scroll.top());
                    assert_eq!(navigation.size.height, scroll.size.height);
                    for selector in ["connection-editor-save", "connection-editor-close"] {
                        let button = vcx.debug_bounds(selector).unwrap();
                        assert!(button.left() >= panel.left() && button.right() <= panel.right());
                        assert!(button.top() >= panel.top() && button.bottom() <= panel.bottom());
                    }
                }
            }
        }
    }
}
#[gpui::test]
fn category_navigation_preserves_input_entities_and_keyboard_focus(cx: &mut TestAppContext) {
    use crate::models::ConnectionEditorSection;
    let test_dir = TestConfigDir::new("nyaterm-editor-navigation-entities");
    let (app, vcx) = hosted_editor(cx, test_dir.path(), 900., 680., 12.);
    let original = vcx.update(|_, cx| {
        app.read(cx).connection_state.editor_fields()[&ConnectionEditorField::Name].clone()
    });
    let component = original.read_with(vcx, |input, _| input.component_state().unwrap());
    for width in [420., 759., 760., 900., 420.] {
        vcx.update(|window, cx| {
            window
                .root::<EditorHost>()
                .unwrap()
                .unwrap()
                .update(cx, |host, cx| {
                    host.width = width;
                    host.height = 480.;
                    cx.notify();
                });
        });
        vcx.simulate_resize(gpui::size(px(width), px(480.)));
        draw_editor(&app, vcx);
        assert!(vcx.debug_bounds("connection-editor-navigation").is_some());
    }
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.connection_state
                .set_editor_field_error(ConnectionEditorField::Name, "retained error".into());
            window.focus(&app.connection_state.editor_navigation_focus(), cx);
        });
    });
    vcx.simulate_keystrokes("down");
    draw_editor(&app, vcx);
    assert!(vcx.update(|window, cx| {
        let state = &app.read(cx).connection_state;
        assert_eq!(
            state.active_editor_draft().unwrap().section,
            ConnectionEditorSection::Network
        );
        state.editor_navigation_focus().is_focused(window)
    }));
    vcx.simulate_keystrokes("tab");
    draw_editor(&app, vcx);
    assert!(vcx.update(|window, cx| {
        !app.read(cx)
            .connection_state
            .editor_navigation_focus()
            .is_focused(window)
    }));
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.set_connection_editor_section(ConnectionEditorSection::Basic, cx)
        })
    });
    draw_editor(&app, vcx);
    vcx.update(|_, cx| {
        let state = &app.read(cx).connection_state;
        let current = &state.editor_fields()[&ConnectionEditorField::Name];
        assert_eq!(current.entity_id(), original.entity_id());
        assert_eq!(
            current.read(cx).component_state().unwrap().entity_id(),
            component.entity_id()
        );
        assert_eq!(
            state.active_editor_draft().unwrap().error.as_deref(),
            Some("retained error")
        );
    });
}

#[gpui::test]
fn save_reveals_focuses_and_scrolls_to_an_invalid_hidden_field(cx: &mut TestAppContext) {
    use crate::models::ConnectionEditorSection;
    let test_dir = TestConfigDir::new("nyaterm-editor-validation-navigation");
    let (app, vcx) = hosted_editor(cx, test_dir.path(), 420., 480., 24.);
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.connection_state
                .set_editor_field_text(ConnectionEditorField::Host, "example.test".into());
            app.connection_state
                .set_editor_field_text(ConnectionEditorField::Username, "root".into());
            app.connection_state.set_editor_field_text(
                ConnectionEditorField::SftpShellDetectionTimeout,
                "invalid".into(),
            );
            app.save_connection_editor(window, cx);
        })
    });
    for _ in 0..6 {
        draw_editor(&app, vcx);
    }
    vcx.update(|window, cx| {
        let state = &app.read(cx).connection_state;
        let draft = state.active_editor_draft().unwrap();
        assert_eq!(draft.section, ConnectionEditorSection::Sftp);
        assert!(
            draft
                .field_errors
                .contains_key(&ConnectionEditorField::SftpShellDetectionTimeout)
        );
        let focus = state
            .editor_fields()
            .get(&ConnectionEditorField::SftpShellDetectionTimeout)
            .map(|input| input.read(cx).component_focus_handle(cx))
            .or_else(|| {
                state
                    .editor_number_fields()
                    .get(&ConnectionEditorField::SftpShellDetectionTimeout)
                    .map(|input| input.read(cx).component_focus_handle(cx))
            })
            .unwrap();
        assert!(focus.is_focused(window));
    });
    let field = vcx
        .debug_bounds("connection-editor-field-SftpShellDetectionTimeout")
        .unwrap();
    let viewport = vcx.debug_bounds("connection-editor-scroll").unwrap();
    assert!(
        field.top() >= viewport.top() && field.top() < viewport.bottom(),
        "{field:?} in {viewport:?}"
    );
}

#[gpui::test]
fn save_returns_to_basic_and_scrolls_to_an_invalid_authentication_field(cx: &mut TestAppContext) {
    use crate::models::ConnectionEditorSection;
    let test_dir = TestConfigDir::new("nyaterm-editor-auth-validation");
    let (app, vcx) = hosted_editor(cx, test_dir.path(), 420., 480., 24.);
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let mut draft = app.connection_state.active_editor_draft().unwrap();
            draft.host = "example.test".into();
            draft.username = "root".into();
            draft.auth_mode = "agent".into();
            draft.agent_endpoint = nyaterm_core::SshAgentEndpoint::UnixSocket {
                path: String::new(),
            };
            app.connection_state.begin_editor(draft);
            app.connection_state.build_editor_fields(cx);
            app.set_connection_editor_section(ConnectionEditorSection::Network, cx);
            app.save_connection_editor(window, cx);
        });
    });
    for _ in 0..6 {
        draw_editor(&app, vcx);
    }
    vcx.update(|window, cx| {
        let state = &app.read(cx).connection_state;
        let draft = state.active_editor_draft().unwrap();
        assert_eq!(draft.section, ConnectionEditorSection::Basic);
        assert!(
            draft
                .field_errors
                .contains_key(&ConnectionEditorField::AgentUnixSocket)
        );
        let input = &state.editor_fields()[&ConnectionEditorField::AgentUnixSocket];
        assert!(input.read(cx).component_focus_handle(cx).is_focused(window));
    });
    let field = vcx
        .debug_bounds("connection-editor-field-AgentUnixSocket")
        .unwrap();
    let viewport = vcx.debug_bounds("connection-editor-scroll").unwrap();
    assert!(
        field.top() >= viewport.top() && field.top() < viewport.bottom(),
        "{field:?} in {viewport:?}"
    );
}

#[gpui::test]
fn agent_identity_picker_escape_preserves_the_connection_editor(cx: &mut TestAppContext) {
    let test_dir = TestConfigDir::new("nyaterm-editor-agent-picker-escape");
    let (app, vcx) = hosted_editor(cx, test_dir.path(), 900., 680., 12.);
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let input = &app.connection_state.editor_fields()[&ConnectionEditorField::Name];
            window.focus(&input.read(cx).component_focus_handle(cx), cx);
            app.set_connection_editor_agent_identity_picker_open(true, cx);
        });
    });
    draw_editor(&app, vcx);
    vcx.simulate_keystrokes("escape");
    draw_editor(&app, vcx);
    vcx.update(|_, cx| {
        let state = &app.read(cx).connection_state;
        assert!(!state.editor_agent_identity_picker_is_open());
        assert!(state.editor_has_draft());
    });
}

#[gpui::test]
fn invalid_hidden_baud_rate_opens_its_picker_and_focuses_the_custom_input(cx: &mut TestAppContext) {
    use crate::models::ConnectionEditorSection;
    let test_dir = TestConfigDir::new("nyaterm-editor-baud-validation");
    let (app, vcx) = hosted_editor(cx, test_dir.path(), 420., 480., 24.);
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.set_connection_editor_kind(ConnectionKindTab::Serial, cx);
            app.connection_state
                .set_editor_field_text(ConnectionEditorField::SerialPort, "COM1".into());
            app.connection_state
                .set_editor_field_text(ConnectionEditorField::BaudRate, "0".into());
            app.save_connection_editor(window, cx);
        });
    });
    for _ in 0..6 {
        draw_editor(&app, vcx);
    }
    vcx.update(|window, cx| {
        let state = &app.read(cx).connection_state;
        assert_eq!(
            state.active_editor_draft().unwrap().section,
            ConnectionEditorSection::Serial
        );
        assert!(state.editor_baud_popover_is_open());
        let input = &state.editor_number_fields()[&ConnectionEditorField::BaudRate];
        assert!(input.read(cx).component_focus_handle(cx).is_focused(window));
    });
}
