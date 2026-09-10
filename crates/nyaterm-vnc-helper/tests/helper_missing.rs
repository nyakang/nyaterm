//! The missing-helper path needs `NYATERM_VNC_HELPER` unset, so it gets its own
//! test binary rather than racing the tests that need it pointed at a real build.

use nyaterm_remote_desktop::{
    VncClipboardConfig, VncDisplayConfig, VncErrorKind, VncReconnectConfig, VncSecurityConfig,
    VncSessionConfig, VncSessionManager,
};

#[test]
fn a_missing_helper_binary_names_the_command_to_build_it() {
    let manager = VncSessionManager::new();
    let error = manager
        .create_session_with_id(
            "absent".to_string(),
            VncSessionConfig {
                relay: None,
                name: "absent".to_string(),
                host: "127.0.0.1".to_string(),
                port: 5900,
                password: None,
                security: VncSecurityConfig::default(),
                display: VncDisplayConfig::default(),
                clipboard: VncClipboardConfig::default(),
                reconnect: VncReconnectConfig::default(),
                shared: true,
                view_only: false,
            },
        )
        // A test binary lives in deps/, so the sibling lookup cannot succeed.
        .expect_err("the helper cannot be resolved beside a test binary");
    assert_eq!(error.kind, VncErrorKind::HelperMissing);
    assert!(
        error.message.contains("cargo build -p nyaterm-vnc-helper"),
        "the message should name the command to run: {}",
        error.message
    );
    assert_eq!(manager.state("absent"), None);
}
