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
}

impl Default for AndroidScreenSettings {
    fn default() -> Self {
        Self {
            max_size: 1280,
            max_fps: 30,
            bit_rate: 8_000_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AndroidScreenValidationError {
    #[error("Android screen maximum edge must be between 128 and 2560")]
    MaxSize,
    #[error("Android screen maximum FPS must be between 1 and 120")]
    MaxFps,
    #[error("Android screen bit rate must be between 100000 and 100000000")]
    BitRate,
}

impl AndroidScreenSettings {
    pub fn validate(self) -> Result<(), AndroidScreenValidationError> {
        if !(128..=2560).contains(&self.max_size) {
            return Err(AndroidScreenValidationError::MaxSize);
        }
        if !(1..=120).contains(&self.max_fps) {
            return Err(AndroidScreenValidationError::MaxFps);
        }
        if !(100_000..=100_000_000).contains(&self.bit_rate) {
            return Err(AndroidScreenValidationError::BitRate);
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

/// Window-local image transform. Coordinates are relative to the clipped viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenViewport {
    pub zoom: f32,
    pub pan: (f32, f32),
}
impl Default for ScreenViewport {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: (0.0, 0.0),
        }
    }
}
impl ScreenViewport {
    pub fn layout(self, viewport: (f32, f32), capture: (u32, u32)) -> Option<(f32, f32, f32, f32)> {
        if capture.0 == 0
            || capture.1 == 0
            || viewport.0 <= 0.0
            || viewport.1 <= 0.0
            || ![viewport.0, viewport.1, self.zoom, self.pan.0, self.pan.1]
                .iter()
                .all(|v| v.is_finite())
        {
            return None;
        }
        let scale = (viewport.0 / capture.0 as f32).min(viewport.1 / capture.1 as f32)
            * self.zoom.clamp(1.0, 4.0);
        let (w, h) = (capture.0 as f32 * scale, capture.1 as f32 * scale);
        let limit = (
            (w - viewport.0).max(0.0) / 2.0,
            (h - viewport.1).max(0.0) / 2.0,
        );
        Some((
            (viewport.0 - w) / 2.0 + self.pan.0.clamp(-limit.0, limit.0),
            (viewport.1 - h) / 2.0 + self.pan.1.clamp(-limit.1, limit.1),
            w,
            h,
        ))
    }
    pub fn pan_by(&mut self, delta: (f32, f32), viewport: (f32, f32), capture: (u32, u32)) {
        if !delta.0.is_finite() || !delta.1.is_finite() {
            return;
        }
        if let Some((x, y, w, h)) = self.layout(viewport, capture) {
            self.pan = (x - (viewport.0 - w) / 2.0, y - (viewport.1 - h) / 2.0);
        }
        self.pan.0 += delta.0;
        self.pan.1 += delta.1;
        if let Some((x, y, w, h)) = self.layout(viewport, capture) {
            self.pan = (x - (viewport.0 - w) / 2.0, y - (viewport.1 - h) / 2.0);
        }
    }
    pub fn zoom_at(
        &mut self,
        factor: f32,
        anchor: (f32, f32),
        viewport: (f32, f32),
        capture: (u32, u32),
    ) {
        if !factor.is_finite() || factor <= 0.0 || !anchor.0.is_finite() || !anchor.1.is_finite() {
            return;
        }
        let Some((x, y, w, h)) = self.layout(viewport, capture) else {
            return;
        };
        let previous = self.zoom;
        self.zoom = (self.zoom * factor).clamp(1.0, 4.0);
        let ratio = self.zoom / previous;
        self.pan = (
            anchor.0 - (anchor.0 - x) * ratio - (viewport.0 - w * ratio) / 2.0,
            anchor.1 - (anchor.1 - y) * ratio - (viewport.1 - h * ratio) / 2.0,
        );
        self.pan_by((0.0, 0.0), viewport, capture);
    }
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

#[cfg(test)]
mod screen_settings_tests {
    use crate::models::SavedConnection;
    use crate::models::android::{AndroidScreenSettings, AndroidScreenValidationError};
    use serde_json::json;

    #[test]
    fn screen_settings_validate_each_boundary_and_identify_the_invalid_field() {
        for settings in [
            AndroidScreenSettings {
                max_size: 128,
                max_fps: 1,
                bit_rate: 100_000,
            },
            AndroidScreenSettings {
                max_size: 2560,
                max_fps: 120,
                bit_rate: 100_000_000,
            },
        ] {
            assert_eq!(settings.validate(), Ok(()));
        }
        for (settings, expected) in [
            (
                AndroidScreenSettings {
                    max_size: 127,
                    ..Default::default()
                },
                AndroidScreenValidationError::MaxSize,
            ),
            (
                AndroidScreenSettings {
                    max_size: 2561,
                    ..Default::default()
                },
                AndroidScreenValidationError::MaxSize,
            ),
            (
                AndroidScreenSettings {
                    max_fps: 0,
                    ..Default::default()
                },
                AndroidScreenValidationError::MaxFps,
            ),
            (
                AndroidScreenSettings {
                    max_fps: 121,
                    ..Default::default()
                },
                AndroidScreenValidationError::MaxFps,
            ),
            (
                AndroidScreenSettings {
                    bit_rate: 99_999,
                    ..Default::default()
                },
                AndroidScreenValidationError::BitRate,
            ),
            (
                AndroidScreenSettings {
                    bit_rate: 100_000_001,
                    ..Default::default()
                },
                AndroidScreenValidationError::BitRate,
            ),
        ] {
            assert_eq!(settings.validate(), Err(expected));
        }
    }

    #[test]
    fn android_connections_drop_removed_prerelease_flag_and_preserve_future_fields() {
        for device in [
            json!({"kind":"usb","serial":"fixture-usb"}),
            json!({"kind":"tcp","host":"example.test","port":5555}),
        ] {
            let connection: SavedConnection = serde_json::from_value(json!({
                "id":"android", "name":"Android", "type":"adb", "device":device,
                "screen":{"view_only":true,"future_option":"kept"}, "future_root":{"x":1}
            }))
            .unwrap();
            let serialized = serde_json::to_value(&connection).unwrap();
            assert!(serialized["screen"].get("view_only").is_none());
            assert_eq!(serialized["screen"]["max_size"], 1280);
            assert_eq!(serialized["screen"]["max_fps"], 30);
            assert_eq!(serialized["screen"]["bit_rate"], 8_000_000);
            assert_eq!(serialized["screen"]["future_option"], "kept");
            assert_eq!(serialized["future_root"]["x"], 1);
            let loaded: SavedConnection = serde_json::from_value(serialized).unwrap();
            assert_eq!(loaded, connection);
        }
    }
}

#[cfg(test)]
mod viewport_tests {
    use crate::models::android::{ScreenViewport, screen_point};
    #[test]
    fn pointer_anchored_zoom_and_clamped_pan_reach_all_edges() {
        let mut transform = ScreenViewport::default();
        transform.zoom_at(2.0, (25.0, 25.0), (100.0, 100.0), (2560, 2560));
        let (x, y, w, h) = transform.layout((100., 100.), (2560, 2560)).unwrap();
        assert_eq!(
            screen_point(25. - x, 25. - y, w, h, 2560, 2560),
            Some((640, 640))
        );
        transform.pan_by((10000., 10000.), (100., 100.), (2560, 2560));
        assert_eq!(
            transform.layout((100., 100.), (2560, 2560)),
            Some((0., 0., 200., 200.))
        );
        transform.pan_by((-10000., -10000.), (100., 100.), (2560, 2560));
        assert_eq!(
            transform.layout((100., 100.), (2560, 2560)),
            Some((-100., -100., 200., 200.))
        );
        transform.zoom_at(0.001, (50., 50.), (100., 100.), (2560, 2560));
        assert_eq!(transform, ScreenViewport::default());
    }
    #[test]
    fn letterbox_pan_is_clamped_and_invalid_geometry_is_rejected() {
        let mut transform = ScreenViewport::default();
        transform.pan_by((500., 500.), (100., 100.), (100, 200));
        assert_eq!(transform.pan, (0., 0.));
        assert!(transform.layout((f32::NAN, 100.), (100, 100)).is_none());
        assert!(transform.layout((100., 100.), (0, 100)).is_none());
    }
}
