use super::super::{default_false, default_true};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecuritySettings {
    #[serde(default = "default_true")]
    pub use_os_keyring: bool,
    #[serde(default = "default_false")]
    pub enable_startup_lock: bool,
    #[serde(default = "default_false")]
    pub enable_idle_lock: bool,
    #[serde(default)]
    pub idle_lock_minutes: u32,
    /// Legacy combined lock switch. It is read for migration and never written back.
    #[serde(rename = "enable_screen_lock", default, skip_serializing)]
    legacy_enable_screen_lock: Option<bool>,
    /// Master password used to derive the wrapping key for `master.key`.
    /// Also serves as the lock-screen password when set.
    #[serde(default)]
    pub master_password: Option<String>,
    #[serde(default = "default_host_key_policy")]
    pub host_key_policy: String,
}

fn default_host_key_policy() -> String {
    "prompt".to_string()
}

impl Default for SecuritySettings {
    fn default() -> Self {
        Self {
            use_os_keyring: true,
            enable_startup_lock: false,
            enable_idle_lock: false,
            idle_lock_minutes: 0,
            legacy_enable_screen_lock: None,
            master_password: None,
            host_key_policy: default_host_key_policy(),
        }
    }
}

impl SecuritySettings {
    /// Migrates the legacy combined screen-lock setting into both independent modes.
    ///
    /// The legacy field is intentionally not serialized, so this is only applied when
    /// loading settings or importing an older portable snapshot.
    pub fn migrate_legacy_screen_lock(&mut self) -> bool {
        let Some(enabled) = self.legacy_enable_screen_lock.take() else {
            return false;
        };

        self.enable_startup_lock = enabled;
        self.enable_idle_lock = enabled;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::SecuritySettings;

    #[test]
    fn defaults_disable_both_lock_modes() {
        let settings = SecuritySettings::default();

        assert!(!settings.enable_startup_lock);
        assert!(!settings.enable_idle_lock);
        assert_eq!(settings.idle_lock_minutes, 0);
    }

    #[test]
    fn migrates_legacy_enabled_screen_lock_to_both_modes() {
        let mut settings: SecuritySettings = serde_json::from_value(serde_json::json!({
            "enable_screen_lock": true,
            "idle_lock_minutes": 15
        }))
        .expect("legacy settings should deserialize");

        assert!(settings.migrate_legacy_screen_lock());
        assert!(settings.enable_startup_lock);
        assert!(settings.enable_idle_lock);
        assert_eq!(settings.idle_lock_minutes, 15);
    }

    #[test]
    fn migrates_legacy_disabled_screen_lock_to_both_modes() {
        let mut settings: SecuritySettings = serde_json::from_value(serde_json::json!({
            "enable_screen_lock": false
        }))
        .expect("legacy settings should deserialize");

        assert!(settings.migrate_legacy_screen_lock());
        assert!(!settings.enable_startup_lock);
        assert!(!settings.enable_idle_lock);
    }

    #[test]
    fn new_lock_modes_are_serialized_without_legacy_field() {
        let settings = SecuritySettings {
            enable_startup_lock: true,
            enable_idle_lock: false,
            ..SecuritySettings::default()
        };
        let value = serde_json::to_value(settings).expect("settings should serialize");

        assert_eq!(value["enable_startup_lock"], true);
        assert_eq!(value["enable_idle_lock"], false);
        assert!(value.get("enable_screen_lock").is_none());
    }
}
