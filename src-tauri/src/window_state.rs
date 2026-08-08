use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{Manager, PhysicalPosition, PhysicalRect, PhysicalSize, Position};

use crate::error::{AppError, AppResult};
use crate::storage::{self, SettingsDocKey};

const DEFAULT_MAIN_WIDTH: f64 = 1280.0;
const DEFAULT_MAIN_HEIGHT: f64 = 800.0;
const MIN_MAIN_WIDTH: f64 = 720.0;
const MIN_MAIN_HEIGHT: f64 = 480.0;
const MIN_CHILD_WIDTH: f64 = 360.0;
const MIN_CHILD_HEIGHT: f64 = 240.0;
pub const MAIN_WINDOW_LABEL: &str = "main";
pub const MAIN_WINDOW_PREFIX: &str = "main-";
const SETTINGS_WINDOW_KEY: &str = "settings";
const NEW_SESSION_WINDOW_KEY: &str = "new-session";
const QUICK_COMMAND_WINDOW_KEY: &str = "quick-command";
const PROXY_WINDOW_KEY: &str = "proxy";
const TUNNEL_WINDOW_KEY: &str = "tunnel";
const FILE_PREVIEW_WINDOW_KEY: &str = "file-preview";
const NOTE_EDITOR_WINDOW_KEY: &str = "note-editor";
const AUTO_UPLOAD_WINDOW_PREFIX: &str = "auto-upload-";
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MainWindowState {
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    #[serde(default = "default_main_width")]
    pub width: f64,
    #[serde(default = "default_main_height")]
    pub height: f64,
    #[serde(default)]
    pub maximized: bool,
}

impl Default for MainWindowState {
    fn default() -> Self {
        Self {
            x: None,
            y: None,
            width: DEFAULT_MAIN_WIDTH,
            height: DEFAULT_MAIN_HEIGHT,
            maximized: false,
        }
    }
}

impl MainWindowState {
    fn normalized(mut self) -> Self {
        self.width = normalize_dimension(self.width, MIN_MAIN_WIDTH, DEFAULT_MAIN_WIDTH);
        self.height = normalize_dimension(self.height, MIN_MAIN_HEIGHT, DEFAULT_MAIN_HEIGHT);
        (self.x, self.y) = normalize_position_pair(self.x, self.y);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct WindowStateDoc {
    #[serde(flatten)]
    main: MainWindowState,
    #[serde(default)]
    children: BTreeMap<String, ChildWindowState>,
}

impl WindowStateDoc {
    fn normalized(mut self) -> Self {
        self.main = self.main.normalized();
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildWindowState {
    #[serde(default)]
    pub width: f64,
    #[serde(default)]
    pub height: f64,
    #[serde(default)]
    pub maximized: bool,
}

impl Default for ChildWindowState {
    fn default() -> Self {
        Self {
            width: 0.0,
            height: 0.0,
            maximized: false,
        }
    }
}

impl ChildWindowState {
    fn normalized(mut self, fallback_width: f64, fallback_height: f64) -> Self {
        self.width = normalize_child_dimension(self.width, MIN_CHILD_WIDTH, fallback_width);
        self.height = normalize_child_dimension(self.height, MIN_CHILD_HEIGHT, fallback_height);
        self
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ChildWindowStateKey {
    Settings,
    NewSession,
    QuickCommand,
    Proxy,
    Tunnel,
    FilePreview,
    NoteEditor,
}

impl ChildWindowStateKey {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Settings => SETTINGS_WINDOW_KEY,
            Self::NewSession => NEW_SESSION_WINDOW_KEY,
            Self::QuickCommand => QUICK_COMMAND_WINDOW_KEY,
            Self::Proxy => PROXY_WINDOW_KEY,
            Self::Tunnel => TUNNEL_WINDOW_KEY,
            Self::FilePreview => FILE_PREVIEW_WINDOW_KEY,
            Self::NoteEditor => NOTE_EDITOR_WINDOW_KEY,
        }
    }

    fn default_size(self) -> (f64, f64) {
        match self {
            Self::Settings => (800.0, 560.0),
            Self::NewSession => (520.0, 620.0),
            Self::QuickCommand => (540.0, 640.0),
            Self::Proxy => (520.0, 560.0),
            Self::Tunnel => (680.0, 640.0),
            Self::FilePreview => (1080.0, 760.0),
            Self::NoteEditor => (980.0, 760.0),
        }
    }

    fn default_state(self) -> ChildWindowState {
        let (width, height) = self.default_size();
        ChildWindowState {
            width,
            height,
            maximized: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ChildWindowPlacement {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug)]
struct PendingSave {
    app: tauri::AppHandle,
    scheduled_at: Instant,
}

static PENDING_SAVES: OnceLock<Mutex<BTreeMap<String, PendingSave>>> = OnceLock::new();

fn default_main_width() -> f64 {
    DEFAULT_MAIN_WIDTH
}

fn default_main_height() -> f64 {
    DEFAULT_MAIN_HEIGHT
}

fn normalize_dimension(value: f64, min: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value.max(min)
    } else {
        fallback
    }
}

fn normalize_position_pair(x: Option<f64>, y: Option<f64>) -> (Option<f64>, Option<f64>) {
    match (x, y) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => (Some(x), Some(y)),
        _ => (None, None),
    }
}

fn normalize_child_dimension(value: f64, min: f64, fallback: f64) -> f64 {
    let fallback = if fallback.is_finite() && fallback > 0.0 {
        fallback.max(min)
    } else {
        min
    };

    if value.is_finite() && value > 0.0 {
        value.max(min)
    } else {
        fallback
    }
}

fn pending_saves() -> &'static Mutex<BTreeMap<String, PendingSave>> {
    PENDING_SAVES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn load_window_state_doc() -> WindowStateDoc {
    storage::load_settings_doc::<WindowStateDoc>(SettingsDocKey::WindowState)
        .unwrap_or_default()
        .normalized()
}

pub fn load_main_window_state() -> MainWindowState {
    load_window_state_doc().main
}

pub fn is_main_window_label(label: &str) -> bool {
    label == MAIN_WINDOW_LABEL || label.starts_with(MAIN_WINDOW_PREFIX)
}

fn is_scoped_child_label(label: &str, base: &str) -> bool {
    if label == base {
        return true;
    }

    label
        .strip_prefix(base)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(is_main_window_label)
}

pub fn child_window_state_key_for_label(label: &str) -> Option<ChildWindowStateKey> {
    if label.starts_with(AUTO_UPLOAD_WINDOW_PREFIX) || is_main_window_label(label) {
        return None;
    }

    if is_scoped_child_label(label, SETTINGS_WINDOW_KEY) {
        return Some(ChildWindowStateKey::Settings);
    }
    if is_scoped_child_label(label, NEW_SESSION_WINDOW_KEY) {
        return Some(ChildWindowStateKey::NewSession);
    }
    if is_scoped_child_label(label, QUICK_COMMAND_WINDOW_KEY) {
        return Some(ChildWindowStateKey::QuickCommand);
    }
    if is_scoped_child_label(label, PROXY_WINDOW_KEY) {
        return Some(ChildWindowStateKey::Proxy);
    }
    if is_scoped_child_label(label, TUNNEL_WINDOW_KEY) {
        return Some(ChildWindowStateKey::Tunnel);
    }
    if label.starts_with(&format!("{FILE_PREVIEW_WINDOW_KEY}-")) {
        return Some(ChildWindowStateKey::FilePreview);
    }
    if label.starts_with(&format!("{NOTE_EDITOR_WINDOW_KEY}-")) {
        return Some(ChildWindowStateKey::NoteEditor);
    }

    None
}

pub fn load_child_window_state(
    key: ChildWindowStateKey,
    fallback_width: f64,
    fallback_height: f64,
) -> ChildWindowState {
    load_window_state_doc()
        .children
        .get(key.as_str())
        .cloned()
        .unwrap_or(ChildWindowState {
            width: fallback_width,
            height: fallback_height,
            maximized: false,
        })
        .normalized(fallback_width, fallback_height)
}

pub fn save_main_window_state(window: &tauri::Window) -> AppResult<()> {
    if !is_main_window_label(window.label()) {
        return Ok(());
    }

    let maximized = window.is_maximized().unwrap_or(false);
    let scale_factor = window
        .scale_factor()
        .map_err(|error| AppError::Config(error.to_string()))?;
    let inner_size = window
        .inner_size()
        .map_err(|error| AppError::Config(error.to_string()))?;
    let outer_position = window
        .outer_position()
        .map_err(|error| AppError::Config(error.to_string()))?;
    save_main_window_state_values(scale_factor, outer_position, inner_size, maximized)
}

pub fn save_main_webview_window_state(window: &tauri::WebviewWindow) -> AppResult<()> {
    if !is_main_window_label(window.label()) {
        return Ok(());
    }

    let maximized = window.is_maximized().unwrap_or(false);
    let scale_factor = window
        .scale_factor()
        .map_err(|error| AppError::Config(error.to_string()))?;
    let inner_size = window
        .inner_size()
        .map_err(|error| AppError::Config(error.to_string()))?;
    let outer_position = window
        .outer_position()
        .map_err(|error| AppError::Config(error.to_string()))?;
    save_main_window_state_values(scale_factor, outer_position, inner_size, maximized)
}

fn save_main_window_state_values(
    scale_factor: f64,
    outer_position: PhysicalPosition<i32>,
    inner_size: PhysicalSize<u32>,
    maximized: bool,
) -> AppResult<()> {
    storage::update_settings_doc::<WindowStateDoc, _, _>(SettingsDocKey::WindowState, |doc| {
        let mut state = doc.main.clone().normalized();
        state.maximized = maximized;

        if !maximized {
            let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
                scale_factor
            } else {
                1.0
            };
            state.x = Some(f64::from(outer_position.x) / scale);
            state.y = Some(f64::from(outer_position.y) / scale);
            state.width = normalize_dimension(
                f64::from(inner_size.width) / scale,
                MIN_MAIN_WIDTH,
                state.width,
            );
            state.height = normalize_dimension(
                f64::from(inner_size.height) / scale,
                MIN_MAIN_HEIGHT,
                state.height,
            );
        }

        doc.main = state.normalized();
        Ok(())
    })
}

pub fn save_child_window_state(window: &tauri::Window) -> AppResult<()> {
    let Some(key) = child_window_state_key_for_label(window.label()) else {
        return Ok(());
    };

    let maximized = window.is_maximized().unwrap_or(false);
    let scale_factor = window
        .scale_factor()
        .map_err(|error| AppError::Config(error.to_string()))?;
    let inner_size = window
        .inner_size()
        .map_err(|error| AppError::Config(error.to_string()))?;
    save_child_window_state_values(key, scale_factor, inner_size, maximized)
}

pub fn save_child_webview_window_state(window: &tauri::WebviewWindow) -> AppResult<()> {
    let Some(key) = child_window_state_key_for_label(window.label()) else {
        return Ok(());
    };

    let maximized = window.is_maximized().unwrap_or(false);
    let scale_factor = window
        .scale_factor()
        .map_err(|error| AppError::Config(error.to_string()))?;
    let inner_size = window
        .inner_size()
        .map_err(|error| AppError::Config(error.to_string()))?;
    save_child_window_state_values(key, scale_factor, inner_size, maximized)
}

fn save_child_window_state_values(
    key: ChildWindowStateKey,
    scale_factor: f64,
    inner_size: PhysicalSize<u32>,
    maximized: bool,
) -> AppResult<()> {
    storage::update_settings_doc::<WindowStateDoc, _, _>(SettingsDocKey::WindowState, |doc| {
        let (fallback_width, fallback_height) = key.default_size();
        let mut state = doc
            .children
            .get(key.as_str())
            .cloned()
            .unwrap_or_else(|| key.default_state())
            .normalized(fallback_width, fallback_height);
        state.maximized = maximized;

        if !maximized {
            let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
                scale_factor
            } else {
                1.0
            };
            state.width = normalize_child_dimension(
                inner_size.width as f64 / scale,
                MIN_CHILD_WIDTH,
                state.width,
            );
            state.height = normalize_child_dimension(
                inner_size.height as f64 / scale,
                MIN_CHILD_HEIGHT,
                state.height,
            );
        }

        doc.children.insert(key.as_str().to_string(), state);
        Ok(())
    })
}

pub fn save_webview_window_state(window: &tauri::WebviewWindow) -> AppResult<()> {
    if is_main_window_label(window.label()) {
        return save_main_webview_window_state(window);
    }
    save_child_webview_window_state(window)
}

pub fn schedule_window_state_save(app: &tauri::AppHandle, window_label: &str) {
    let scheduled_at = Instant::now();
    let window_label = window_label.to_string();
    {
        let mut pending = pending_saves()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pending.insert(
            window_label.clone(),
            PendingSave {
                app: app.clone(),
                scheduled_at,
            },
        );
    }

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SAVE_DEBOUNCE).await;
        let (app, window_label) = {
            let mut pending = pending_saves()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match pending.get(&window_label) {
                Some(save) if save.scheduled_at == scheduled_at => {
                    let save = pending.remove(&window_label).expect("pending save exists");
                    (save.app, window_label.clone())
                }
                _ => return,
            }
        };

        if let Some(window) = app.get_webview_window(&window_label) {
            if let Err(error) = save_webview_window_state(&window) {
                tracing::warn!(
                    window_label = window.label(),
                    "Failed to save window state: {}",
                    error
                );
            }
        }
    });
}

pub fn schedule_main_window_state_save(app: &tauri::AppHandle, window_label: &str) {
    schedule_window_state_save(app, window_label);
}

pub fn center_child_in_parent_monitor(
    app: &tauri::AppHandle,
    parent_label: Option<&str>,
    child_width: f64,
    child_height: f64,
) -> Option<ChildWindowPlacement> {
    let main_window = parent_label
        .and_then(|label| app.get_webview_window(label))
        .or_else(|| crate::app::focused_or_first_main_window(app))?;
    let monitor = main_window.current_monitor().ok().flatten()?;
    let scale_factor = monitor.scale_factor();
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };

    let (child_width_physical, child_height_physical) =
        child_logical_size_to_physical(child_width, child_height, scale);

    Some(center_in_work_area(
        monitor.work_area(),
        child_width_physical,
        child_height_physical,
    ))
}

fn child_logical_size_to_physical(width: f64, height: f64, scale: f64) -> (u32, u32) {
    let effective_scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    (
        (width.max(1.0) * effective_scale).round() as u32,
        (height.max(1.0) * effective_scale).round() as u32,
    )
}

fn center_in_work_area(
    work_area: &PhysicalRect<i32, u32>,
    child_width: u32,
    child_height: u32,
) -> ChildWindowPlacement {
    let x = centered_axis(work_area.position.x, work_area.size.width, child_width);
    let y = centered_axis(work_area.position.y, work_area.size.height, child_height);
    ChildWindowPlacement { x, y }
}

fn centered_axis(area_start: i32, area_size: u32, child_size: u32) -> i32 {
    let remaining = i64::from(area_size) - i64::from(child_size);
    let offset = remaining.max(0) / 2;
    area_start.saturating_add(offset as i32)
}

pub fn placement_to_position(placement: ChildWindowPlacement) -> Position {
    Position::Physical(PhysicalPosition {
        x: placement.x,
        y: placement.y,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> PhysicalRect<i32, u32> {
        PhysicalRect {
            position: PhysicalPosition { x, y },
            size: PhysicalSize { width, height },
        }
    }

    #[test]
    fn normalizes_missing_or_invalid_main_window_state_values() {
        let state = MainWindowState {
            x: Some(f64::NAN),
            y: Some(100.0),
            width: f64::NAN,
            height: 100.0,
            maximized: true,
        }
        .normalized();

        assert_eq!(state.width, DEFAULT_MAIN_WIDTH);
        assert_eq!(state.height, MIN_MAIN_HEIGHT);
        assert_eq!(state.x, None);
        assert_eq!(state.y, None);
        assert!(state.maximized);
    }

    #[test]
    fn deserializes_legacy_main_window_state_doc() {
        let doc: WindowStateDoc =
            serde_json::from_str(r#"{ "width": 1000, "height": 700, "maximized": true }"#).unwrap();

        assert_eq!(doc.main.width, 1000.0);
        assert_eq!(doc.main.height, 700.0);
        assert_eq!(doc.main.x, None);
        assert_eq!(doc.main.y, None);
        assert!(doc.main.maximized);
        assert!(doc.children.is_empty());
    }

    #[test]
    fn deserializes_main_window_state_with_position() {
        let doc: WindowStateDoc = serde_json::from_str(
            r#"{
                "x": -1200,
                "y": 80,
                "width": 1000,
                "height": 700,
                "maximized": false,
                "children": {
                    "settings": { "width": 820, "height": 600, "maximized": false }
                }
            }"#,
        )
        .unwrap();

        assert_eq!(doc.main.x, Some(-1200.0));
        assert_eq!(doc.main.y, Some(80.0));
        assert_eq!(doc.main.width, 1000.0);
        assert_eq!(doc.main.height, 700.0);
        assert!(
            doc.children
                .contains_key(ChildWindowStateKey::Settings.as_str())
        );
    }

    #[test]
    fn deserializes_child_window_state_doc() {
        let doc: WindowStateDoc = serde_json::from_str(
            r#"{
                "width": 1280,
                "height": 800,
                "maximized": false,
                "children": {
                    "file-preview": { "width": 1100, "height": 760, "maximized": true }
                }
            }"#,
        )
        .unwrap();
        let state = doc
            .children
            .get(ChildWindowStateKey::FilePreview.as_str())
            .cloned()
            .unwrap()
            .normalized(1080.0, 760.0);

        assert_eq!(state.width, 1100.0);
        assert_eq!(state.height, 760.0);
        assert!(state.maximized);
    }

    #[test]
    fn normalizes_invalid_child_window_state_values() {
        let state = ChildWindowState {
            width: f64::NAN,
            height: 100.0,
            maximized: false,
        }
        .normalized(520.0, 620.0);

        assert_eq!(state.width, 520.0);
        assert_eq!(state.height, MIN_CHILD_HEIGHT);
        assert!(!state.maximized);
    }

    #[test]
    fn recognizes_child_window_state_keys() {
        assert_eq!(
            child_window_state_key_for_label("settings"),
            Some(ChildWindowStateKey::Settings)
        );
        assert_eq!(
            child_window_state_key_for_label("settings-main-abc"),
            Some(ChildWindowStateKey::Settings)
        );
        assert_eq!(
            child_window_state_key_for_label("new-session-main-abc"),
            Some(ChildWindowStateKey::NewSession)
        );
        assert_eq!(
            child_window_state_key_for_label("quick-command-main-abc"),
            Some(ChildWindowStateKey::QuickCommand)
        );
        assert_eq!(
            child_window_state_key_for_label("proxy-main-abc"),
            Some(ChildWindowStateKey::Proxy)
        );
        assert_eq!(
            child_window_state_key_for_label("tunnel-main-abc"),
            Some(ChildWindowStateKey::Tunnel)
        );
        assert_eq!(
            child_window_state_key_for_label("file-preview-abc"),
            Some(ChildWindowStateKey::FilePreview)
        );
        assert_eq!(
            child_window_state_key_for_label("note-editor-abc"),
            Some(ChildWindowStateKey::NoteEditor)
        );
        assert_eq!(child_window_state_key_for_label("auto-upload-abc"), None);
        assert_eq!(child_window_state_key_for_label("main"), None);
    }

    #[test]
    fn recognizes_main_window_label_family() {
        assert!(is_main_window_label("main"));
        assert!(is_main_window_label("main-1234"));
        assert!(!is_main_window_label("settings"));
        assert!(!is_main_window_label("new-session-main-1234"));
    }

    #[test]
    fn centers_child_on_primary_style_monitor() {
        let placement = center_in_work_area(&rect(0, 0, 1920, 1080), 800, 560);
        assert_eq!(placement.x, 560);
        assert_eq!(placement.y, 260);
    }

    #[test]
    fn centers_child_on_negative_coordinate_monitor() {
        let placement = center_in_work_area(&rect(-1920, 0, 1920, 1040), 520, 620);
        assert_eq!(placement.x, -1220);
        assert_eq!(placement.y, 210);
    }

    #[test]
    fn converts_child_size_with_high_dpi_scale_factor() {
        let size = child_logical_size_to_physical(800.0, 560.0, 1.5);
        assert_eq!(size, (1200, 840));
    }

    #[test]
    fn clamps_child_to_work_area_start_when_too_large() {
        let placement = center_in_work_area(&rect(100, 50, 600, 400), 800, 500);
        assert_eq!(placement.x, 100);
        assert_eq!(placement.y, 50);
    }
}
