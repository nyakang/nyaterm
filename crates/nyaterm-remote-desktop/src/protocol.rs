//! Compatibility facade for the independently versioned native contract.
pub use nyaterm_native_protocol::protocol::{
    CommittedTextError, CursorPosition, CursorShape, CursorVisibility, MAX_COMMITTED_TEXT_BYTES,
    MAX_HEIGHT, MAX_VNC_CLIPBOARD_TEXT_BYTES, MAX_VNC_FRAMEBUFFER_HEIGHT,
    MAX_VNC_FRAMEBUFFER_WIDTH, MAX_VNC_INPUT_BATCH, MAX_WIDTH, MIN_HEIGHT, MIN_WIDTH,
    PROTOCOL_VERSION, PixelFormat, QueueWaker, RDP_ALL_DRIVES, RdpAudioMode, RdpBackend,
    RdpCapability, RdpCertificatePolicy, RdpCertificateRequest, RdpCertificateResponse,
    RdpClipboardConfig, RdpClipboardMode, RdpClipboardTransferProgress, RdpClipboardTransferStatus,
    RdpControlMessage, RdpDisplayConfig, RdpDisplayMetrics, RdpDisplayMode, RdpError, RdpErrorKind,
    RdpFrameEvent, RdpInputEvent, RdpPointerButton, RdpReconnectConfig, RdpRedirectionConfig,
    RdpRuntimeEvent, RdpServerCapabilities, RdpSessionConfig, RdpSessionDrain, RdpSessionState,
    RemoteCursorEvent, RemoteDesktopError, RemoteDesktopErrorCategory, RemoteDesktopViewState,
    RemoteFrameEvent, RemotePoint, RemotePointerButton, RemotePointerEvent, RemoteWheelAxis,
    VNC_PROTOCOL_VERSION, VncClipboardConfig, VncControlMessage, VncDisplayConfig, VncError,
    VncErrorKind, VncInputEvent, VncReconnectConfig, VncRuntimeEvent, VncScaleMode,
    VncSecurityConfig, VncSecurityMode, VncServerCapabilities, VncServerKeyRequest,
    VncSessionConfig, VncSessionDrain, VncSessionState, parse_rdp_audio_mode, parse_rdp_backend,
    parse_rdp_certificate_policy, parse_rdp_clipboard_mode, parse_rdp_display_mode,
    parse_rdp_drive_letters, parse_vnc_scale_mode, parse_vnc_security_mode,
    toggle_rdp_drive_letter, validate_committed_text,
};
#[cfg(test)]
mod tests {
    use super::{
        CommittedTextError, MAX_COMMITTED_TEXT_BYTES, MAX_WIDTH, MIN_HEIGHT, RDP_ALL_DRIVES,
        RdpAudioMode, RdpBackend, RdpCertificatePolicy, RdpClipboardConfig, RdpClipboardMode,
        RdpDisplayConfig, RdpDisplayMetrics, RdpDisplayMode, RdpReconnectConfig,
        RdpRedirectionConfig, RdpSessionConfig, parse_rdp_audio_mode, parse_rdp_backend,
        parse_rdp_certificate_policy, parse_rdp_clipboard_mode, parse_rdp_display_mode,
        parse_rdp_drive_letters, toggle_rdp_drive_letter, validate_committed_text,
    };
    use nyaterm_core::RdpRedirectionSettings;

    #[test]
    fn vnc_password_hint_is_optional_and_defaults_to_no_prompt() {
        use super::{VncError, VncErrorKind};

        let old = serde_json::json!({"kind": "Authentication", "message": "failed"});
        let decoded: VncError = serde_json::from_value(old.clone()).unwrap();
        assert!(!decoded.password_required);
        assert_eq!(serde_json::to_value(&decoded).unwrap(), old);
        let missing = VncError {
            password_required: true,
            ..VncError::new(VncErrorKind::Authentication, "password required")
        };
        assert_eq!(
            serde_json::from_value::<VncError>(serde_json::to_value(&missing).unwrap()).unwrap(),
            missing
        );
    }

    #[test]
    fn panel_metrics_become_a_negotiable_desktop() {
        let metrics = RdpDisplayMetrics {
            width: 9_000,
            height: 7,
            desktop_scale_factor: 40,
            physical_size_mm: Some((310, 170)),
        }
        .clamped();
        assert_eq!((metrics.width, metrics.height), (MAX_WIDTH, MIN_HEIGHT));
        assert_eq!(metrics.desktop_scale_factor, 100);
        assert_eq!(metrics.physical_size_mm, Some((310, 170)));

        let mut display = RdpDisplayConfig::default();
        display.fit_to_panel(RdpDisplayMetrics {
            width: 1_281,
            height: 719,
            desktop_scale_factor: 125,
            physical_size_mm: None,
        });
        assert_eq!((display.width, display.height), (1_280, 718));
        assert_eq!(display.desktop_scale_factor, 125);
    }

    #[test]
    fn legacy_display_ipc_and_saved_defaults_agree() {
        let display: RdpDisplayConfig = serde_json::from_value(serde_json::json!({
            "mode": "fit_window", "width": 1920, "height": 1080, "color_depth": 32
        }))
        .unwrap();
        assert_eq!(display, RdpDisplayConfig::default());
        assert_eq!(
            display,
            RdpDisplayConfig::from(&nyaterm_core::RdpDisplaySettings::default())
        );
        let document = serde_json::json!({
            "name":"legacy", "host":"example.invalid", "port":3389,
            "username":"", "domain":"", "password":null, "use_nla":true,
            "certificate_policy":"prompt",
            "display":{"mode":"fit_window","width":1920,"height":1080,"color_depth":32},
            "clipboard":{"mode":"text_only"}, "reconnect":{"enabled":true,"max_attempts":5}
        });
        let session: RdpSessionConfig = serde_json::from_value(document).unwrap();
        assert!(!session.administrative_session);
        assert_eq!(session.backend, RdpBackend::Builtin);
        assert_eq!(session.display, display);
        let encoded = serde_json::to_value(&session).unwrap();
        assert_eq!(
            serde_json::from_value::<RdpSessionConfig>(encoded).unwrap(),
            session
        );
    }

    #[test]
    fn certificate_policy_accepts_public_and_legacy_names() {
        assert_eq!(
            parse_rdp_certificate_policy("accept-temporarily"),
            RdpCertificatePolicy::Insecure
        );
        assert_eq!(
            parse_rdp_certificate_policy("insecure"),
            RdpCertificatePolicy::Insecure
        );
        assert_eq!(
            parse_rdp_certificate_policy("tofu"),
            RdpCertificatePolicy::TrustOnFirstUse
        );
        assert_eq!(
            parse_rdp_certificate_policy("reject-changed"),
            RdpCertificatePolicy::Strict
        );
    }

    #[test]
    fn legacy_native_display_mode_is_fixed() {
        assert_eq!(parse_rdp_display_mode("native"), RdpDisplayMode::Fixed);
    }

    #[test]
    fn rdp_backend_accepts_public_and_legacy_names() {
        for name in ["auto", "builtin", "internal"] {
            assert_eq!(parse_rdp_backend(name).unwrap(), RdpBackend::Builtin);
        }
        for name in ["windows", "active-x"] {
            assert_eq!(parse_rdp_backend(name).unwrap(), RdpBackend::Windows);
        }
        assert!(parse_rdp_backend("unknown").is_err());
        assert_eq!(RdpBackend::default(), RdpBackend::Builtin);
    }

    #[test]
    fn clipboard_mode_keeps_legacy_values_and_defaults_to_text_and_files() {
        assert_eq!(
            parse_rdp_clipboard_mode("disabled"),
            RdpClipboardMode::Disabled
        );
        assert_eq!(
            parse_rdp_clipboard_mode("text-only"),
            RdpClipboardMode::TextOnly
        );
        assert_eq!(
            parse_rdp_clipboard_mode("text-and-files"),
            RdpClipboardMode::TextAndFiles
        );
        assert_eq!(
            parse_rdp_clipboard_mode("unknown"),
            RdpClipboardMode::TextOnly
        );
        assert_eq!(
            RdpClipboardConfig::default().mode,
            RdpClipboardMode::TextAndFiles
        );
        assert_eq!(
            serde_json::to_string(&RdpClipboardMode::TextAndFiles).unwrap(),
            "\"text_and_files\""
        );
    }

    #[test]
    fn committed_text_accepts_unicode_at_the_byte_limit() {
        let text = "界".repeat(MAX_COMMITTED_TEXT_BYTES / 3);
        assert!(text.len() <= MAX_COMMITTED_TEXT_BYTES);
        assert_eq!(validate_committed_text(&text), Ok(()));
    }

    #[test]
    fn committed_text_rejects_oversize_nul_and_control_characters() {
        assert_eq!(
            validate_committed_text(&"a".repeat(MAX_COMMITTED_TEXT_BYTES + 1)),
            Err(CommittedTextError::TooLong)
        );
        assert_eq!(
            validate_committed_text("before\0after"),
            Err(CommittedTextError::ContainsNul)
        );
        assert_eq!(
            validate_committed_text("line\nfeed"),
            Err(CommittedTextError::ContainsControl)
        );
    }

    fn rdp_session_config() -> RdpSessionConfig {
        RdpSessionConfig {
            relay: None,
            backend: RdpBackend::Builtin,
            windows_authentication: None,
            name: "test".into(),
            host: "127.0.0.1".into(),
            port: 3389,
            username: "user".into(),
            domain: String::new(),
            password: None,
            use_nla: true,
            certificate_policy: RdpCertificatePolicy::default(),
            administrative_session: false,
            display: RdpDisplayConfig::default(),
            clipboard: RdpClipboardConfig::default(),
            redirection: RdpRedirectionConfig::default(),
            reconnect: RdpReconnectConfig::default(),
        }
    }

    #[test]
    fn redirection_defaults_keep_devices_off_and_clipboard_on() {
        let default = RdpRedirectionConfig::default();
        assert!(!default.drives);
        assert!(!default.printers);
        assert!(!default.ports);
        assert!(!default.smartcards);
        assert!(!default.microphone);
        assert!(!default.webauthn);
        assert!(default.clipboard);
        assert_eq!(default.audio, RdpAudioMode::NoAudio);
        assert_eq!(default.drive_letters, RDP_ALL_DRIVES);
        assert!(default.custom_drive_path.is_empty());

        let core = RdpRedirectionSettings::default();
        assert!(core.clipboard);
        assert_eq!(core.audio, "no_audio");
        assert!(!core.drives && !core.printers && !core.ports && !core.smartcards);
        assert_eq!(core.drive_letters, RDP_ALL_DRIVES);
        assert!(core.custom_drive_path.is_empty());
    }

    #[test]
    fn drive_letters_parse_and_toggle() {
        assert_eq!(parse_rdp_drive_letters("*").len(), 26);
        assert_eq!(parse_rdp_drive_letters("dcD,"), vec!['C', 'D']);
        assert!(parse_rdp_drive_letters("").is_empty());
        assert_eq!(
            toggle_rdp_drive_letter("*", 'b'),
            "ACDEFGHIJKLMNOPQRSTUVWXYZ"
        );
        assert_eq!(toggle_rdp_drive_letter("C", 'd'), "CD");
        assert_eq!(toggle_rdp_drive_letter("CD", 'c'), "D");
        assert_eq!(toggle_rdp_drive_letter("D", '1'), "D");
    }

    #[test]
    fn audio_mode_parses_editor_values() {
        assert_eq!(parse_rdp_audio_mode("local"), RdpAudioMode::Local);
        assert_eq!(parse_rdp_audio_mode("remote"), RdpAudioMode::Remote);
        assert_eq!(parse_rdp_audio_mode("no_audio"), RdpAudioMode::NoAudio);
        assert_eq!(parse_rdp_audio_mode("unexpected"), RdpAudioMode::NoAudio);
    }

    #[test]
    fn redirection_conversion_preserves_every_field() {
        let settings = RdpRedirectionSettings {
            drives: true,
            drive_letters: "CD".into(),
            custom_drive_path: r"C:\Users\name\Documents".into(),
            printers: true,
            ports: true,
            smartcards: true,
            microphone: true,
            webauthn: true,
            clipboard: false,
            audio: "remote".into(),
        };
        let config = RdpRedirectionConfig::from(&settings);
        assert!(config.drives && config.printers && config.ports && config.smartcards);
        assert!(config.microphone && config.webauthn);
        assert!(!config.clipboard);
        assert_eq!(config.drive_letters, "CD");
        assert_eq!(config.custom_drive_path, r"C:\Users\name\Documents");
        assert_eq!(config.audio, RdpAudioMode::Remote);
    }

    #[test]
    fn normalize_follows_clipboard_switch_and_drops_webauthn_on_windows() {
        let mut config = rdp_session_config();
        config.normalize_backend_settings();
        assert_eq!(config.clipboard.mode, RdpClipboardMode::TextAndFiles);

        config.redirection.clipboard = false;
        config.normalize_backend_settings();
        assert_eq!(config.clipboard.mode, RdpClipboardMode::Disabled);

        config.backend = RdpBackend::Windows;
        config.redirection.webauthn = true;
        config.normalize_backend_settings();
        assert!(!config.redirection.webauthn);
    }
}
