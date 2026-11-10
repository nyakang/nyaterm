//! Persisted Android settings and UI-independent presentation policies.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdbDeviceSelector {
    Usb { serial: String },
    Tcp { host: String, port: u16 },
}

impl AdbDeviceSelector {
    pub fn serial(&self) -> String {
        match self {
            Self::Usb { serial } => serial.clone(),
            Self::Tcp { host, port } if host.contains(':') => format!("[{host}]:{port}"),
            Self::Tcp { host, port } => format!("{host}:{port}"),
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Usb { serial } if serial.is_empty() || serial.chars().any(char::is_control) => {
                Err("Select an authorized Android device")
            }
            Self::Tcp { host, port }
                if host.trim().is_empty()
                    || host.chars().any(char::is_whitespace)
                    || *port == 0 =>
            {
                Err("Enter a valid Android host and port")
            }
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AndroidScreenSettings {
    pub max_size: u32,
    pub max_fps: u32,
    pub bit_rate: u32,
    pub view_only: bool,
}

impl Default for AndroidScreenSettings {
    fn default() -> Self {
        Self {
            max_size: 1280,
            max_fps: 30,
            bit_rate: 8_000_000,
            view_only: false,
        }
    }
}

impl AndroidScreenSettings {
    pub fn validate(self) -> Result<(), &'static str> {
        if !(128..=2560).contains(&self.max_size)
            || !(1..=120).contains(&self.max_fps)
            || !(100_000..=100_000_000).contains(&self.bit_rate)
        {
            return Err("Android screen settings are out of range");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AndroidViewKind {
    Files,
    Shell,
    Screen,
}

impl AndroidViewKind {
    pub const fn restore_type(self) -> &'static str {
        match self {
            Self::Files => "adb_files",
            Self::Shell => "adb_shell",
            Self::Screen => "adb_screen",
        }
    }

    pub fn from_restore_type(value: &str) -> Option<Self> {
        match value {
            "adb_files" => Some(Self::Files),
            "adb_shell" => Some(Self::Shell),
            "adb_screen" => Some(Self::Screen),
            _ => None,
        }
    }
}

pub fn default_android_directory() -> String {
    "/sdcard/".into()
}
pub fn next_paste_sequence() -> u64 {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
pub fn android_keycode(key: &str) -> Option<u32> {
    if key.len() == 1 {
        let byte = key.as_bytes()[0].to_ascii_lowercase();
        if byte.is_ascii_lowercase() {
            return Some(29 + u32::from(byte - b'a'));
        }
        if byte.is_ascii_digit() {
            return Some(7 + u32::from(byte - b'0'));
        }
    }
    Some(match key {
        "enter" => 66,
        "backspace" => 67,
        "delete" => 112,
        "escape" => 111,
        "tab" => 61,
        "space" => 62,
        "left" => 21,
        "right" => 22,
        "up" => 19,
        "down" => 20,
        "home" => 122,
        "end" => 123,
        "pageup" => 92,
        "pagedown" => 93,
        "shift" => 59,
        "control" => 113,
        "alt" => 57,
        "cmd" | "super" => 117,
        _ => return None,
    })
}

/// Reject letterboxing rather than clamping a click onto the device edge.
pub fn screen_point(
    x: f32,
    y: f32,
    viewport_width: f32,
    viewport_height: f32,
    width: u32,
    height: u32,
) -> Option<(u32, u32)> {
    if width == 0
        || height == 0
        || viewport_width <= 0.0
        || viewport_height <= 0.0
        || ![x, y, viewport_width, viewport_height]
            .iter()
            .all(|v| v.is_finite())
    {
        return None;
    }
    let scale = (viewport_width / width as f32).min(viewport_height / height as f32);
    let left = (viewport_width - width as f32 * scale) / 2.0;
    let top = (viewport_height - height as f32 * scale) / 2.0;
    let px = (x - left) / scale;
    let py = (y - top) / scale;
    (px >= 0.0 && py >= 0.0 && px < width as f32 && py < height as f32)
        .then_some((px as u32, py as u32))
}

#[cfg(test)]
mod tests {
    use crate::models::android::{
        AdbDeviceSelector, AndroidScreenSettings, AndroidViewKind, screen_point,
    };

    #[test]
    fn letterbox_clicks_are_rejected_and_center_maps_to_screen() {
        assert_eq!(screen_point(0.0, 50.0, 200.0, 100.0, 100, 100), None);
        assert_eq!(
            screen_point(100.0, 50.0, 200.0, 100.0, 100, 100),
            Some((50, 50))
        );
        assert_eq!(screen_point(f32::NAN, 0.0, 200.0, 100.0, 100, 100), None);
    }

    #[test]
    fn device_identity_does_not_include_a_runtime_transport_id() {
        let device = AdbDeviceSelector::Tcp {
            host: "::1".into(),
            port: 5555,
        };
        assert_eq!(device.serial(), "[::1]:5555");
        assert_eq!(serde_json::to_value(device).unwrap()["kind"], "tcp");
        assert_eq!(AndroidScreenSettings::default().max_fps, 30);
        for view in [
            AndroidViewKind::Files,
            AndroidViewKind::Shell,
            AndroidViewKind::Screen,
        ] {
            assert_eq!(
                AndroidViewKind::from_restore_type(view.restore_type()),
                Some(view)
            );
        }
    }
}

#[cfg(test)]
mod compatibility_tests {
    use crate::models::android::{AdbDeviceSelector, AndroidScreenSettings};
    use crate::models::{ConnectionType, SavedConnection};
    use serde_json::json;
    #[test]
    fn usb_tcp_connections_preserve_unknown_fields_and_screen_default_reset() {
        for device in [
            json!({"kind":"usb","serial":"fixture-usb"}),
            json!({"kind":"tcp","host":"example.test","port":5555}),
        ] {
            let raw = json!({"id":"android","name":"Android","type":"adb","device":device,"screen":{"max_fps":60,"future_option":"kept"},"future_root":{"x":1}});
            let mut connection: SavedConnection = serde_json::from_value(raw).unwrap();
            if let ConnectionType::Adb { screen, .. } = &mut connection.config {
                *screen = AndroidScreenSettings::default();
            } else {
                panic!("Android expected");
            }
            let serialized = serde_json::to_value(&connection).unwrap();
            assert_eq!(serialized["screen"]["max_fps"], 30);
            assert_eq!(serialized["screen"]["future_option"], "kept");
            assert_eq!(serialized["future_root"]["x"], 1);
            let loaded: SavedConnection = serde_json::from_value(serialized).unwrap();
            assert!(
                matches!(loaded.config, ConnectionType::Adb { directory, .. } if directory == "/sdcard/")
            );
        }
        let selector = AdbDeviceSelector::Tcp {
            host: "::1".into(),
            port: 5555,
        };
        assert_eq!(selector.serial(), "[::1]:5555");
    }
}
