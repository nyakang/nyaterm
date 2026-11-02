pub mod connection;
pub mod credentials;
pub mod network;
pub mod notes;
pub mod quick_commands;
pub mod sessions;
pub mod settings;
pub mod ssh_runtime;
pub mod ssh_runtime_auth;
pub mod window_decoration;
pub mod window_material;
pub mod window_state;
pub mod workspace;
pub mod workspace_manifest;
pub use connection::*;
pub use credentials::*;
pub use network::*;
pub use notes::*;
pub use quick_commands::*;
pub use sessions::*;
pub use settings::*;
pub use window_state::*;
pub use workspace::*;
pub use workspace_manifest::*;

fn default_ssh_port() -> u16 {
    22
}

fn default_ssh_user() -> String {
    "root".to_string()
}

fn default_backspace_mode_ssh() -> String {
    "del".to_string()
}

fn default_telnet_port() -> u16 {
    23
}

fn default_rdp_port() -> u16 {
    3389
}

fn default_vnc_port() -> u16 {
    5900
}

fn default_baud_rate() -> u32 {
    115_200
}

fn default_data_bits() -> u8 {
    8
}

fn default_parity() -> String {
    "none".to_string()
}

fn default_stop_bits() -> String {
    "1".to_string()
}

fn default_backspace_mode_serial() -> String {
    "ctrl_h".to_string()
}

fn default_tunnel_type() -> String {
    "local".to_string()
}

fn default_tunnel_target_host() -> String {
    "127.0.0.1".to_string()
}

fn default_proxy_protocol() -> String {
    "socks5".to_string()
}

fn default_proxy_host() -> String {
    "127.0.0.1".to_string()
}

fn default_proxy_port() -> u16 {
    1080
}

fn default_backspace_mode_telnet() -> String {
    "del".to_string()
}

fn default_telnet_enter_mode() -> String {
    "cr".to_string()
}

fn default_telnet_auto_login_timeout_ms() -> u64 {
    60_000
}

fn default_true() -> bool {
    true
}

fn default_rdp_backend() -> String {
    "builtin".to_string()
}

fn deserialize_rdp_backend<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = <String as serde::Deserialize>::deserialize(deserializer)?;
    Ok(match value.as_str() {
        "auto" | "internal" => "builtin".to_string(),
        "active-x" => "windows".to_string(),
        _ => value,
    })
}

fn serialize_rdp_backend<S>(value: &str, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(match value {
        "auto" | "internal" => "builtin",
        "active-x" => "windows",
        _ => value,
    })
}

fn default_rdp_certificate_policy() -> String {
    "prompt".to_string()
}

fn default_rdp_display_mode() -> String {
    "fit-window".to_string()
}

fn default_rdp_width() -> u32 {
    1920
}

fn default_rdp_height() -> u32 {
    1080
}

fn default_rdp_color_depth() -> u8 {
    32
}

fn default_rdp_clipboard_mode() -> String {
    "text-only".to_string()
}

fn default_rdp_reconnect_attempts() -> u32 {
    5
}

fn default_vnc_security_mode() -> String {
    "auto".to_string()
}

fn default_vnc_scale_mode() -> String {
    "fit".to_string()
}

fn default_vnc_reconnect_attempts() -> u32 {
    5
}

pub fn default_sftp_shell_detection_timeout_ms() -> u64 {
    3000
}

fn is_default_sftp_shell_detection_timeout_ms(value: &u64) -> bool {
    *value == default_sftp_shell_detection_timeout_ms()
}

fn is_default_sftp_settings(value: &SftpSettings) -> bool {
    value == &SftpSettings::default()
}

fn is_standard_ssh_profile(value: &SshProfile) -> bool {
    *value == SshProfile::Standard
}

fn is_default_telnet_auto_login_config(value: &TelnetAutoLoginConfig) -> bool {
    value == &TelnetAutoLoginConfig::default()
}

fn default_auth_mode() -> String {
    "password".to_string()
}

fn default_otp_type() -> String {
    "totp".to_string()
}

fn default_otp_algorithm() -> String {
    "SHA1".to_string()
}

fn default_otp_digits() -> u8 {
    6
}

fn default_otp_period() -> u64 {
    30
}

fn default_post_login_delay_ms() -> u64 {
    1000
}

fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests;
