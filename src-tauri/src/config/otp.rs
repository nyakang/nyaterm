use super::{get_config_dir, load_json, save_json, uuid_v4};
use crate::error::{AppError, AppResult};
use crate::utils::crypto;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

fn default_digits() -> u8 {
    6
}

fn default_period() -> u64 {
    30
}

/// Stored OTP entry. The secret is AES-256-GCM encrypted on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtpEntry {
    #[serde(default = "uuid_v4")]
    pub id: String,
    /// `"totp"` or `"hotp"`.
    pub otp_type: String,
    pub issuer: String,
    pub username: String,
    /// Encrypted secret on disk; plaintext only after explicit decryption.
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default = "default_algorithm")]
    pub algorithm: String,
    #[serde(default = "default_digits")]
    pub digits: u8,
    /// Time step in seconds (TOTP only).
    #[serde(default = "default_period")]
    pub period: u64,
    /// Counter value (HOTP only).
    #[serde(default)]
    pub counter: u64,
    /// Transient: true when encrypted secret data exists on disk.
    #[serde(default, skip_serializing)]
    pub has_secret: bool,
}

fn default_algorithm() -> String {
    "SHA1".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OtpConfig {
    #[serde(default)]
    pub entries: Vec<OtpEntry>,
}

pub fn load_otp_entries(app: &AppHandle) -> AppResult<OtpConfig> {
    let dir = get_config_dir(app)?;
    let path = dir.join("otp.json");
    let mut config: OtpConfig = load_json(&path)?;
    for entry in &mut config.entries {
        entry.has_secret = entry.secret.is_some();
    }
    Ok(config)
}

pub fn save_otp_entries(app: &AppHandle, config: &OtpConfig) -> AppResult<()> {
    let dir = get_config_dir(app)?;
    save_json(&dir.join("otp.json"), config)
}

pub fn load_otp_entry_by_id(app: &AppHandle, id: &str) -> AppResult<OtpEntry> {
    let cfg = load_otp_entries(app)?;
    let mut entry = cfg
        .entries
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| AppError::Config(format!("OTP entry '{}' not found", id)))?;
    if let Some(ct) = entry.secret.clone() {
        entry.secret = crypto::decrypt(&ct).ok();
    }
    Ok(entry)
}
