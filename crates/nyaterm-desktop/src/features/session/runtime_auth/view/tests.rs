use std::path::Path;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use gpui::{
    AppContext as _, Bounds, Context, Entity, IntoElement, Pixels, Render, ScrollDelta,
    ScrollWheelEvent, Subscription, TestAppContext, VisualTestContext, Window, div, point,
    prelude::*, px, size,
};
use nyaterm_core::SavedConnection;
use nyaterm_store::ConnectionStore;
use nyaterm_transport::ssh_authentication::{
    SshAuthenticationMaterial, SshAuthenticationMethod, SshAuthenticationRequest,
};
use nyaterm_transport::{
    SshCredentialPrompt, SshCredentialPromptKind, SshCredentialPromptReason,
    SshKeyboardInteractivePrompt, SshKeyboardInteractiveRequest,
};
use nyaterm_ui::nya_root;
use serde_json::json;

use crate::features::NyaTermApp;
use crate::features::session::runtime_auth::state::{
    AuthMethod, CredentialResponseSender, RuntimeAuthenticationContext, RuntimeAuthenticationForm,
    SaveDestination,
};
use crate::features::session::{CredentialPromptState, KeyboardInteractivePromptState};
use crate::features::test_support::app_with_visible_local_session;
use crate::test_support::{TestAppCleanup, TestConfigDir};

struct PromptHost {
    app: Entity<NyaTermApp>,
    _app_subscription: Subscription,
}

impl Render for PromptHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let overlay = self.app.update(cx, |app, cx| {
            app.shell
                .set_viewport_size_for_test((viewport.width.into(), viewport.height.into()));
            app.ssh_auth_prompt_overlay(cx).into_any_element()
        });
        div().relative().size_full().child(overlay)
    }
}

fn prompt() -> SshCredentialPrompt {
    SshCredentialPrompt {
        host: "example.test".into(),
        port: 22,
        username: "test".into(),
        connection_name: "Layout fixture".into(),
        kind: SshCredentialPromptKind::Password,
        reason: SshCredentialPromptReason::MissingPassword,
        attempt: 1,
        prompt_text: None,
        echo: false,
    }
}

fn authentication(root: &Path) -> CredentialPromptState {
    // A snapshot enables the real remember-credential controls, without starting
    // authentication or touching the application's own store.
    let store = ConnectionStore::open(root.join("fixture-store")).unwrap();
    let connection: SavedConnection = serde_json::from_value(json!({
        "id": "fixture", "name": "Layout fixture", "type": "ssh",
        "host": "example.test", "username": "test", "auth": {"mode": "password"}
    }))
    .unwrap();
    store.save_connection(&connection).unwrap();
    let context = Arc::new(RuntimeAuthenticationContext::new(
        Some(store.runtime_connection_snapshot("fixture").unwrap()),
        None,
        None,
        None,
        Default::default(),
    ));
    let request = SshAuthenticationRequest {
        prompt: prompt(),
        available_methods: vec![
            SshAuthenticationMethod::Password,
            SshAuthenticationMethod::Publickey,
        ],
        key_response_id: None,
    };
    let (sender, _) = mpsc::channel();
    CredentialPromptState {
        id: "layout".into(),
        prompt: request.prompt.clone(),
        response_tx: CredentialResponseSender::Authentication(sender),
        authentication: Some(RuntimeAuthenticationForm::new(request, context)),
        value: String::new(),
    }
}

fn hosted<'a>(
    cx: &'a mut TestAppContext,
    root: &Path,
    state: CredentialPromptState,
    dimensions: (f32, f32),
) -> (Entity<NyaTermApp>, &'a mut VisualTestContext) {
    let app = app_with_visible_local_session(cx, root, "terminal");
    app.update(cx, |app, cx| {
        app.sync_component_theme(cx);
        app.session.prompts.activate_credential(state);
    });
    let host_app = app.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let host = cx.new(|cx| PromptHost {
            _app_subscription: cx.observe(&host_app, |_, _, cx| cx.notify()),
            app: host_app,
        });
        nya_root(host, window, cx)
    });
    cx.simulate_resize(size(px(dimensions.0), px(dimensions.1)));
    draw(&app, cx);
    (app, cx)
}

fn draw(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        app.update(cx, |_, cx| cx.notify());
        _ = window.draw(cx);
    });
    cx.run_until_parked();
}

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("missing {selector}"))
}

fn assert_contains(outer: Bounds<Pixels>, inner: Bounds<Pixels>) {
    let tolerance = px(1.);
    assert!(inner.left() >= outer.left() - tolerance);
    assert!(inner.top() >= outer.top() - tolerance);
    assert!(inner.right() <= outer.right() + tolerance);
    assert!(inner.bottom() <= outer.bottom() + tolerance);
}

fn assert_card_fits(cx: &mut VisualTestContext) {
    let card = bounds(cx, "ssh-auth-card");
    let footer = bounds(cx, "ssh-auth-footer");
    let viewport = cx.update(|window, _| window.viewport_size());
    assert!(card.top() >= px(11.));
    assert!(card.bottom() <= viewport.height - px(11.));
    assert_contains(Bounds::new(point(px(0.), px(0.)), viewport), card);
    assert_contains(card, footer);
    assert_eq!(footer.size.height, px(32.));
    assert!(
        bounds(cx, "ssh-auth-body-layout/scrollbar-layer")
            .size
            .height
            > px(0.)
    );
}

#[test]
fn password_prompt_grows_with_its_fields_at_each_windows_scale() {
    let root = TestConfigDir::new("nyaterm-auth-layout-password");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let state = authentication(root.path());
    let (app, cx) = hosted(&mut cx, root.path(), state, (900., 900.));
    for scale in [1., 1.25, 1.5] {
        cx.simulate_scale_factor_change(scale);
        draw(&app, cx);
        assert_card_fits(cx);
        let card = bounds(cx, "ssh-auth-card");
        let body = bounds(cx, "ssh-auth-body-layout/scrollbar-layer");
        assert!(
            body.size.height < px(550.),
            "short forms should not be stretched"
        );
        assert_contains(body, bounds(cx, "ssh-auth-title"));
        assert_contains(body, bounds(cx, "ssh-auth-secret-field"));
        assert_contains(body, bounds(cx, "ssh-auth-remember-section"));
        assert_contains(card, body);
    }
}

#[test]
fn expanded_form_scrolls_without_moving_footer_and_reflows_on_resize() {
    let root = TestConfigDir::new("nyaterm-auth-layout-scroll");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let mut state = authentication(root.path());
    let form = state.authentication.as_mut().unwrap();
    form.remember = true;
    form.destination = SaveDestination::NewAccount;
    form.error = Some("Credential unavailable.\n".repeat(8));
    let (app, cx) = hosted(&mut cx, root.path(), state, (600., 400.));
    assert_card_fits(cx);
    let footer = bounds(cx, "ssh-auth-footer");
    let content_top = bounds(cx, "ssh-auth-content").top();
    let body = bounds(cx, "ssh-auth-body-layout/scrollbar-layer");
    cx.simulate_event(ScrollWheelEvent {
        position: body.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-1000.))),
        ..Default::default()
    });
    draw(&app, cx);
    assert!(bounds(cx, "ssh-auth-content").top() < content_top);
    assert_eq!(bounds(cx, "ssh-auth-footer"), footer);
    assert_contains(body, bounds(cx, "ssh-auth-error"));

    for dimensions in [(420., 320.), (900., 900.), (600., 400.)] {
        cx.simulate_resize(size(px(dimensions.0), px(dimensions.1)));
        draw(&app, cx);
        assert_card_fits(cx);
    }
}

#[test]
fn changing_authentication_sources_keeps_the_card_inside_a_small_window() {
    let root = TestConfigDir::new("nyaterm-auth-layout-sources");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let state = authentication(root.path());
    let (app, cx) = hosted(&mut cx, root.path(), state, (420., 400.));
    for (method, source) in [(true, 1), (true, 0), (false, 1), (false, 0)] {
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.change_runtime_auth_source(method, source, window, cx);
            });
        });
        draw(&app, cx);
        assert_card_fits(cx);
    }
    app.update(cx, |app, cx| {
        let form = app
            .session
            .prompts
            .active_credential_mut()
            .unwrap()
            .authentication
            .as_mut()
            .unwrap();
        assert_eq!(form.method, AuthMethod::Password);
        form.remember = true;
        form.destination = SaveDestination::NewAccount;
        cx.notify();
    });
    draw(&app, cx);
    assert_card_fits(cx);
    assert!(bounds(cx, "ssh-auth-remember-section").size.height > px(32.));
}

#[test]
fn password_field_keeps_focus_when_revealed_and_supports_cancel_and_submit() {
    let root = TestConfigDir::new("nyaterm-auth-layout-input");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let mut state = authentication(root.path());
    let (sender, cancelled) = mpsc::channel();
    state.response_tx = CredentialResponseSender::Authentication(sender);
    let (app, cx) = hosted(&mut cx, root.path(), state, (900., 900.));
    let field_bounds = bounds(cx, "ssh-auth-secret-field");
    cx.simulate_click(field_bounds.center(), Default::default());
    draw(&app, cx);
    let field = app.update(cx, |app, cx| {
        app.text_input(
            "ssh.credential.layout",
            "",
            crate::features::text_inputs::TextInputSetup::masked(),
            cx,
        )
    });
    cx.update(|window, cx| {
        assert!(field.read(cx).component_focus_handle(cx).is_focused(window));
        assert!(field.read(cx).is_masked());
    });
    cx.simulate_input("layout-secret");
    let toggle = bounds(cx, "nya-input-mask-toggle");
    cx.simulate_click(toggle.center(), Default::default());
    draw(&app, cx);
    cx.update(|window, cx| {
        assert!(field.read(cx).component_focus_handle(cx).is_focused(window));
        assert!(!field.read(cx).is_masked());
    });
    cx.simulate_keystrokes("escape");
    draw(&app, cx);
    assert!(cancelled.try_recv().unwrap().is_none());

    let mut state = authentication(root.path());
    let (sender, submitted) = mpsc::channel();
    state.response_tx = CredentialResponseSender::Authentication(sender);
    app.update(cx, |app, cx| {
        app.session.prompts.activate_credential(state);
        cx.notify();
    });
    draw(&app, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert!(app.focus_active_ssh_prompt_input(window, cx));
        });
    });
    cx.simulate_input("layout-secret");
    cx.simulate_keystrokes("enter");
    let deadline = Instant::now() + Duration::from_secs(10);
    let response = loop {
        cx.run_until_parked();
        if let Ok(response) = submitted.try_recv() {
            break response.unwrap();
        }
        assert!(
            Instant::now() < deadline,
            "authentication was not submitted"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(matches!(
        response.material,
        SshAuthenticationMaterial::Password(ref value) if value == "layout-secret"
    ));
    app.read_with(cx, |app, _| {
        assert!(app.session.prompt_active_credential().is_none());
    });
}

#[test]
fn legacy_password_and_keyboard_interactive_prompts_keep_their_content_height() {
    let root = TestConfigDir::new("nyaterm-auth-layout-legacy");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let (sender, _) = mpsc::channel();
    let state = CredentialPromptState {
        id: "legacy".into(),
        prompt: prompt(),
        response_tx: sender.into(),
        authentication: None,
        value: String::new(),
    };
    let (app, cx) = hosted(&mut cx, root.path(), state, (900., 900.));
    let legacy = bounds(cx, "ssh-credential-card");
    assert!(legacy.size.height > px(100.));
    assert!(legacy.top() >= px(12.) && legacy.bottom() <= px(888.));
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert!(app.focus_active_ssh_prompt_input(window, cx));
            let field = app.text_input(
                "ssh.credential.legacy",
                "",
                crate::features::text_inputs::TextInputSetup::masked(),
                cx,
            );
            assert!(field.read(cx).focus_handle().is_focused(window));
        });
    });
    app.update(cx, |app, cx| {
        app.session.prompts.take_credential();
        let (sender, _) = mpsc::channel();
        app.session
            .prompts
            .activate_keyboard_interactive(KeyboardInteractivePromptState {
                id: "keyboard".into(),
                request: SshKeyboardInteractiveRequest {
                    host: "example.test".into(),
                    port: 22,
                    username: "test".into(),
                    connection_name: "Layout fixture".into(),
                    name: "Verification".into(),
                    instructions: "Enter the challenge response".into(),
                    round: 1,
                    prompts: vec![SshKeyboardInteractivePrompt {
                        prompt: "Response:".into(),
                        echo: false,
                    }],
                    otp_id: None,
                },
                response_tx: sender,
                responses: vec![String::new()],
                focused_index: 0,
                otp_code: None,
                otp_type: None,
                otp_period: 30,
                otp_time_step: None,
                otp_error: None,
            });
        cx.notify();
    });
    draw(&app, cx);
    let keyboard = bounds(cx, "ssh-keyboard-card");
    assert!(keyboard.size.height > px(100.));
    assert!(keyboard.top() >= px(12.) && keyboard.bottom() <= px(888.));
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert!(app.focus_active_ssh_prompt_input(window, cx));
        });
    });
}
