use super::super::{default_false, default_true};
use serde::{Deserialize, Serialize};

const DEFAULT_RECORDING_MEMORY_LIMIT_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferSettings {
    #[serde(default = "default_editor_type")]
    pub editor_type: String,
    #[serde(default = "default_transfer_threads")]
    pub download_threads: u32,
    #[serde(default = "default_transfer_threads")]
    pub upload_threads: u32,
    #[serde(default = "default_duplicate_strategy")]
    pub duplicate_strategy: String,
    #[serde(default = "default_true")]
    pub preserve_timestamps: bool,
    #[serde(default = "default_true")]
    pub resume_broken_transfer: bool,
    #[serde(default = "default_file_permissions")]
    pub default_file_permissions: String,
    #[serde(default = "default_max_retries")]
    pub max_transfer_retries: u32,
    #[serde(default = "default_buffer_size")]
    pub transfer_buffer_size: u32,
    #[serde(default)]
    pub download_path: String,
    #[serde(default = "default_false")]
    pub ask_save_location: bool,
    #[serde(default)]
    pub default_editor: String,
    #[serde(default)]
    pub recording_path: String,
    #[serde(default = "default_true")]
    pub recording_include_io_labels: bool,
    #[serde(default = "default_true")]
    pub recording_include_timestamps: bool,
    #[serde(default = "default_false")]
    pub recording_auto_start: bool,
    #[serde(default = "default_recording_memory_limit_bytes")]
    pub recording_memory_limit_bytes: u64,
    #[serde(default = "default_true")]
    pub zmodem_enabled: bool,
}

fn default_transfer_threads() -> u32 {
    3
}
fn default_editor_type() -> String {
    "external".to_string()
}
fn default_duplicate_strategy() -> String {
    "ask".to_string()
}
fn default_file_permissions() -> String {
    "644".to_string()
}
fn default_max_retries() -> u32 {
    2
}
fn default_buffer_size() -> u32 {
    32
}
fn default_recording_memory_limit_bytes() -> u64 {
    DEFAULT_RECORDING_MEMORY_LIMIT_BYTES
}

impl Default for TransferSettings {
    fn default() -> Self {
        Self {
            editor_type: default_editor_type(),
            download_threads: default_transfer_threads(),
            upload_threads: default_transfer_threads(),
            duplicate_strategy: default_duplicate_strategy(),
            preserve_timestamps: true,
            resume_broken_transfer: true,
            default_file_permissions: default_file_permissions(),
            max_transfer_retries: default_max_retries(),
            transfer_buffer_size: default_buffer_size(),
            download_path: String::new(),
            ask_save_location: false,
            default_editor: String::new(),
            recording_path: String::new(),
            recording_include_io_labels: true,
            recording_include_timestamps: true,
            recording_auto_start: false,
            recording_memory_limit_bytes: default_recording_memory_limit_bytes(),
            zmodem_enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TransferSettings;

    #[test]
    fn defaults_auto_recording_to_disabled_for_legacy_settings() {
        let settings: TransferSettings = serde_json::from_value(serde_json::json!({
            "recording_path": "",
            "recording_include_io_labels": true,
            "recording_include_timestamps": true,
            "recording_memory_limit_bytes": 5242880
        }))
        .unwrap();

        assert!(!settings.recording_auto_start);
        assert_eq!(settings.editor_type, "external");
    }
}
