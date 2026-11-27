//! Explicit conversion from persisted models to the independent helper contract.
use crate::RdpRedirectionSettings;
use nyaterm_native_protocol::protocol::{
    RdpDisplayConfig, RdpRedirectionConfig, parse_rdp_audio_mode, parse_rdp_display_mode,
};
impl From<&crate::RdpDisplaySettings> for RdpDisplayConfig {
    fn from(settings: &crate::RdpDisplaySettings) -> Self {
        Self {
            mode: parse_rdp_display_mode(&settings.mode),
            width: settings.width,
            height: settings.height,
            desktop_scale_factor: 0,
            color_depth: settings.color_depth,
            graphics_pipeline: settings.graphics_pipeline,
            display_wallpaper: settings.display_wallpaper,
            display_themes: settings.display_themes,
            enable_font_smoothing: settings.enable_font_smoothing,
            enable_desktop_composition: settings.enable_desktop_composition,
            disable_full_window_drag: settings.disable_full_window_drag,
            disable_menu_animations: settings.disable_menu_animations,
            disable_cursor_shadow: settings.disable_cursor_shadow,
            disable_cursor_blinking: settings.disable_cursor_blinking,
        }
    }
}
impl From<&RdpRedirectionSettings> for RdpRedirectionConfig {
    fn from(settings: &RdpRedirectionSettings) -> Self {
        Self {
            drives: settings.drives,
            // Old saved sessions deserialize to `"*"` via serde's default, so
            // an explicit empty string here really means "no drives selected".
            drive_letters: settings.drive_letters.clone(),
            custom_drive_path: settings.custom_drive_path.clone(),
            printers: settings.printers,
            ports: settings.ports,
            smartcards: settings.smartcards,
            microphone: settings.microphone,
            webauthn: settings.webauthn,
            clipboard: settings.clipboard,
            audio: parse_rdp_audio_mode(&settings.audio),
        }
    }
}

impl From<crate::secret::SecretString> for nyaterm_native_protocol::secret::SecretString {
    fn from(value: crate::secret::SecretString) -> Self {
        Self::new(value.into_secret())
    }
}
impl From<nyaterm_native_protocol::secret::SecretString> for crate::secret::SecretString {
    fn from(value: nyaterm_native_protocol::secret::SecretString) -> Self {
        Self::new(value.into_secret())
    }
}
impl From<crate::connection_route::RelayEndpoint>
    for nyaterm_native_protocol::relay::RelayEndpoint
{
    fn from(value: crate::connection_route::RelayEndpoint) -> Self {
        Self {
            address: value.address,
            token: value.token.into(),
        }
    }
}
impl From<crate::spice::settings::SpiceChannel>
    for nyaterm_native_protocol::spice::settings::SpiceChannel
{
    fn from(value: crate::spice::settings::SpiceChannel) -> Self {
        match value {
            crate::spice::settings::SpiceChannel::Main => Self::Main,
            crate::spice::settings::SpiceChannel::Display => Self::Display,
            crate::spice::settings::SpiceChannel::Inputs => Self::Inputs,
            crate::spice::settings::SpiceChannel::Cursor => Self::Cursor,
            crate::spice::settings::SpiceChannel::Playback => Self::Playback,
            crate::spice::settings::SpiceChannel::Record => Self::Record,
            crate::spice::settings::SpiceChannel::Smartcard => Self::Smartcard,
            crate::spice::settings::SpiceChannel::Usbredir => Self::Usbredir,
            crate::spice::settings::SpiceChannel::Port => Self::Port,
            crate::spice::settings::SpiceChannel::Webdav => Self::Webdav,
        }
    }
}
impl From<nyaterm_native_protocol::spice::settings::SpiceChannel>
    for crate::spice::settings::SpiceChannel
{
    fn from(value: nyaterm_native_protocol::spice::settings::SpiceChannel) -> Self {
        match value {
            nyaterm_native_protocol::spice::settings::SpiceChannel::Main => Self::Main,
            nyaterm_native_protocol::spice::settings::SpiceChannel::Display => Self::Display,
            nyaterm_native_protocol::spice::settings::SpiceChannel::Inputs => Self::Inputs,
            nyaterm_native_protocol::spice::settings::SpiceChannel::Cursor => Self::Cursor,
            nyaterm_native_protocol::spice::settings::SpiceChannel::Playback => Self::Playback,
            nyaterm_native_protocol::spice::settings::SpiceChannel::Record => Self::Record,
            nyaterm_native_protocol::spice::settings::SpiceChannel::Smartcard => Self::Smartcard,
            nyaterm_native_protocol::spice::settings::SpiceChannel::Usbredir => Self::Usbredir,
            nyaterm_native_protocol::spice::settings::SpiceChannel::Port => Self::Port,
            nyaterm_native_protocol::spice::settings::SpiceChannel::Webdav => Self::Webdav,
        }
    }
}
impl From<crate::spice::settings::SpiceClipboardMode>
    for nyaterm_native_protocol::spice::settings::SpiceClipboardMode
{
    fn from(value: crate::spice::settings::SpiceClipboardMode) -> Self {
        match value {
            crate::spice::settings::SpiceClipboardMode::Disabled => Self::Disabled,
            crate::spice::settings::SpiceClipboardMode::ToGuest => Self::ToGuest,
            crate::spice::settings::SpiceClipboardMode::FromGuest => Self::FromGuest,
            crate::spice::settings::SpiceClipboardMode::Bidirectional => Self::Bidirectional,
        }
    }
}
impl From<nyaterm_native_protocol::spice::settings::SpiceClipboardMode>
    for crate::spice::settings::SpiceClipboardMode
{
    fn from(value: nyaterm_native_protocol::spice::settings::SpiceClipboardMode) -> Self {
        match value {
            nyaterm_native_protocol::spice::settings::SpiceClipboardMode::Disabled => {
                Self::Disabled
            }
            nyaterm_native_protocol::spice::settings::SpiceClipboardMode::ToGuest => Self::ToGuest,
            nyaterm_native_protocol::spice::settings::SpiceClipboardMode::FromGuest => {
                Self::FromGuest
            }
            nyaterm_native_protocol::spice::settings::SpiceClipboardMode::Bidirectional => {
                Self::Bidirectional
            }
        }
    }
}
impl From<crate::spice::settings::SpiceEndpoint>
    for nyaterm_native_protocol::spice::settings::SpiceEndpoint
{
    fn from(value: crate::spice::settings::SpiceEndpoint) -> Self {
        Self {
            host: value.host.into(),
            port: value.port,
            tls_port: value.tls_port,
            proxy: value.proxy.map(Into::into),
            extra: value.extra,
        }
    }
}
impl From<crate::spice::settings::SpiceSecuritySettings>
    for nyaterm_native_protocol::spice::settings::SpiceSecuritySettings
{
    fn from(value: crate::spice::settings::SpiceSecuritySettings) -> Self {
        Self {
            require_tls: value.require_tls,
            ca_pem: value.ca_pem,
            certificate_subject: value.certificate_subject,
            tls_ciphers: value.tls_ciphers,
            secure_channels: value.secure_channels.into_iter().map(Into::into).collect(),
            extra: value.extra,
        }
    }
}
impl From<crate::spice::settings::SpiceSettings>
    for nyaterm_native_protocol::spice::settings::SpiceSettings
{
    fn from(value: crate::spice::settings::SpiceSettings) -> Self {
        Self {
            security: value.security.into(),
            clipboard: value.clipboard.into(),
            dynamic_resolution: value.dynamic_resolution,
            fit_window: value.fit_window,
            view_only: value.view_only,
            audio_playback: value.audio_playback,
            file_transfer: value.file_transfer,
            reconnect_attempts: value.reconnect_attempts,
            disabled_channels: value
                .disabled_channels
                .into_iter()
                .map(Into::into)
                .collect(),
            extra: value.extra,
        }
    }
}

impl From<crate::models::android::AndroidScreenSettings>
    for nyaterm_native_protocol::android::settings::AndroidScreenSettings
{
    fn from(value: crate::models::android::AndroidScreenSettings) -> Self {
        use crate::models::android::CaptureOrientation as Source;
        use nyaterm_native_protocol::android::settings::CaptureOrientation as Target;
        Self {
            max_size: value.max_size,
            max_fps: value.max_fps,
            bit_rate: value.bit_rate,
            power_on: value.power_on,
            capture_orientation: match value.capture_orientation {
                Source::FollowDevice => Target::FollowDevice,
                Source::LockInitial => Target::LockInitial,
                Source::Degrees0 => Target::Degrees0,
                Source::Degrees90 => Target::Degrees90,
                Source::Degrees180 => Target::Degrees180,
                Source::Degrees270 => Target::Degrees270,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn spice_wire_conversion_preserves_unknown_settings_and_policy() {
        let mut settings = crate::spice::settings::SpiceSettings {
            view_only: true,
            ..Default::default()
        };
        settings
            .extra
            .insert("future_setting".into(), serde_json::json!({"value": 7}));
        settings
            .security
            .extra
            .insert("future_tls".into(), serde_json::json!(true));
        let persisted = serde_json::to_value(&settings).unwrap();
        let wire: nyaterm_native_protocol::spice::settings::SpiceSettings = settings.into();
        assert_eq!(serde_json::to_value(wire).unwrap(), persisted);
    }

    #[test]
    fn android_wire_conversion_preserves_all_screen_fields() {
        let settings = crate::models::android::AndroidScreenSettings::default();
        let persisted = serde_json::to_value(settings).unwrap();
        let wire: nyaterm_native_protocol::android::settings::AndroidScreenSettings =
            settings.into();
        assert_eq!(serde_json::to_value(wire).unwrap(), persisted);
    }
}
