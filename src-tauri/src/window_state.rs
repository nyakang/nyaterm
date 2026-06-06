use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{Manager, PhysicalPosition, PhysicalRect, PhysicalSize, Position};

use crate::error::{AppError, AppResult};
use crate::storage::{self, SettingsDocKey};

const DEFAULT_MAIN_WIDTH: f64 = 1280.0;
const DEFAULT_MAIN_HEIGHT: f64 = 800.0;
const MIN_MAIN_WIDTH: f64 = 720.0;
const MIN_MAIN_HEIGHT: f64 = 480.0;
pub const MAIN_WINDOW_LABEL: &str = "main";
pub const MAIN_WINDOW_PREFIX: &str = "main-";
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MainWindowState {
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
        self
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
    window_label: String,
    scheduled_at: Instant,
}

static PENDING_SAVE: OnceLock<Mutex<Option<PendingSave>>> = OnceLock::new();

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

fn pending_save() -> &'static Mutex<Option<PendingSave>> {
    PENDING_SAVE.get_or_init(|| Mutex::new(None))
}

pub fn load_main_window_state() -> MainWindowState {
    storage::load_settings_doc::<MainWindowState>(SettingsDocKey::WindowState)
        .unwrap_or_default()
        .normalized()
}

pub fn is_main_window_label(label: &str) -> bool {
    label == MAIN_WINDOW_LABEL || label.starts_with(MAIN_WINDOW_PREFIX)
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
    save_main_window_state_values(scale_factor, inner_size, maximized)
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
    save_main_window_state_values(scale_factor, inner_size, maximized)
}

fn save_main_window_state_values(
    scale_factor: f64,
    inner_size: PhysicalSize<u32>,
    maximized: bool,
) -> AppResult<()> {
    let mut state = load_main_window_state();
    state.maximized = maximized;

    if !maximized {
        let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };
        state.width =
            normalize_dimension(inner_size.width as f64 / scale, MIN_MAIN_WIDTH, state.width);
        state.height = normalize_dimension(
            inner_size.height as f64 / scale,
            MIN_MAIN_HEIGHT,
            state.height,
        );
    }

    storage::save_settings_doc(SettingsDocKey::WindowState, &state.normalized())
}

pub fn schedule_main_window_state_save(app: &tauri::AppHandle, window_label: &str) {
    let scheduled_at = Instant::now();
    {
        let mut pending = pending_save()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *pending = Some(PendingSave {
            app: app.clone(),
            window_label: window_label.to_string(),
            scheduled_at,
        });
    }

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SAVE_DEBOUNCE).await;
        let (app, window_label) = {
            let mut pending = pending_save()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match pending.as_ref() {
                Some(save) if save.scheduled_at == scheduled_at => {
                    let app = save.app.clone();
                    let window_label = save.window_label.clone();
                    *pending = None;
                    (app, window_label)
                }
                _ => return,
            }
        };

        if let Some(window) = app.get_webview_window(&window_label) {
            if let Err(error) = save_main_webview_window_state(&window) {
                tracing::warn!("Failed to save main window state: {}", error);
            }
        }
    });
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
            width: f64::NAN,
            height: 100.0,
            maximized: true,
        }
        .normalized();

        assert_eq!(state.width, DEFAULT_MAIN_WIDTH);
        assert_eq!(state.height, MIN_MAIN_HEIGHT);
        assert!(state.maximized);
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
