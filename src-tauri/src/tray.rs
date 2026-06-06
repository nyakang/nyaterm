use std::sync::Arc;

use serde::Serialize;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;

use crate::app;
use crate::config::{self, AppSettings, CloudSyncStatus};
use crate::core::{CloudSyncManager, SessionInfo, SessionManager, SessionType};

const TRAY_ID: &str = "main-tray";

const MENU_SHOW_MAIN: &str = "tray::show_main";
const MENU_HIDE_TO_TRAY: &str = "tray::hide_to_tray";
const MENU_NEW_SESSION: &str = "tray::new_session";
const MENU_OPEN_ACTIVE_SESSIONS_PANEL: &str = "tray::open_active_sessions_panel";
const MENU_SYNC_PUSH: &str = "tray::sync_push";
const MENU_SYNC_PULL: &str = "tray::sync_pull";
const MENU_SYNC_BACKUP: &str = "tray::sync_backup";
const MENU_OPEN_SYNC_HISTORY: &str = "tray::open_sync_history";
const MENU_SETTINGS: &str = "tray::settings";
const MENU_MINIMIZE_TO_TRAY: &str = "tray::minimize_to_tray";
const MENU_LOCK_SCREEN: &str = "tray::lock_screen";
const MENU_CHECK_UPDATES: &str = "tray::check_updates";
const MENU_QUIT: &str = "tray::quit";

const SUBMENU_ACTIVE_SESSIONS: &str = "tray::submenu::active_sessions";
const SUBMENU_SYNC_BACKUP: &str = "tray::submenu::sync_backup";

const MENU_DISABLED_ACTIVE_SESSIONS: &str = "tray::disabled::active_sessions";
const MENU_DISABLED_ACTIVE_SESSIONS_LIMITED: &str = "tray::disabled::active_sessions_limited";
const MENU_DISABLED_SYNC_STATUS: &str = "tray::disabled::sync_status";

const SESSION_MENU_PREFIX: &str = "tray::session::";
const ACTIVE_SESSION_LIMIT: usize = 8;

pub struct TrayMenuState {
    menu: Menu<tauri::Wry>,
    refresh_lock: Mutex<()>,
}

impl TrayMenuState {
    fn new(menu: Menu<tauri::Wry>) -> Self {
        Self {
            menu,
            refresh_lock: Mutex::new(()),
        }
    }

    fn menu(&self) -> Menu<tauri::Wry> {
        self.menu.clone()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TrayActionPayload {
    OpenNewSession,
    FocusSession {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    OpenPanel {
        #[serde(rename = "panelId")]
        panel_id: &'static str,
    },
    OpenSettings,
    LockScreen,
    CheckUpdates,
    RequestQuit,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TargetedTrayActionPayload {
    #[serde(flatten)]
    action: TrayActionPayload,
    target_window_label: Option<String>,
}

#[derive(Clone, Copy)]
enum TrayLanguage {
    En,
    ZhCn,
}

impl TrayLanguage {
    fn from_settings(settings: &AppSettings) -> Self {
        let language = settings.ui.language.as_deref().unwrap_or("en");
        if language.eq_ignore_ascii_case("zh-CN")
            || language.eq_ignore_ascii_case("zh_CN")
            || language.eq_ignore_ascii_case("zh")
            || language.to_ascii_lowercase().starts_with("zh-")
        {
            Self::ZhCn
        } else {
            Self::En
        }
    }
}

struct TrayStrings {
    show_main_window: &'static str,
    hide_to_tray: &'static str,
    new_session: &'static str,
    active_sessions: &'static str,
    no_active_sessions: &'static str,
    only_showing_first_8: &'static str,
    open_active_sessions_panel: &'static str,
    sync_backup: &'static str,
    current_status: &'static str,
    sync_push_now: &'static str,
    sync_pull_now: &'static str,
    sync_backup_now: &'static str,
    open_sync_backup_history: &'static str,
    settings: &'static str,
    minimize_to_tray: &'static str,
    lock_screen: &'static str,
    check_updates: &'static str,
    quit: &'static str,
    status_idle: &'static str,
    status_running: &'static str,
    status_success: &'static str,
    status_failed: &'static str,
    status_conflict: &'static str,
    status_disabled: &'static str,
    session_type_ssh: &'static str,
    session_type_local: &'static str,
    session_type_telnet: &'static str,
    session_type_serial: &'static str,
}

impl TrayStrings {
    fn for_language(language: TrayLanguage) -> Self {
        match language {
            TrayLanguage::ZhCn => Self {
                show_main_window: "显示主窗口",
                hide_to_tray: "隐藏到托盘",
                new_session: "新建连接…",
                active_sessions: "活动会话",
                no_active_sessions: "暂无活动会话",
                only_showing_first_8: "仅显示前 8 个",
                open_active_sessions_panel: "打开活动会话面板",
                sync_backup: "同步与备份",
                current_status: "当前状态",
                sync_push_now: "立即推送",
                sync_pull_now: "立即拉取",
                sync_backup_now: "立即备份",
                open_sync_backup_history: "打开同步与备份历史",
                settings: "设置…",
                minimize_to_tray: "关闭时最小化到托盘",
                lock_screen: "锁定界面",
                check_updates: "检查更新",
                quit: "退出 NyaTerm",
                status_idle: "空闲",
                status_running: "执行中",
                status_success: "成功",
                status_failed: "失败",
                status_conflict: "冲突",
                status_disabled: "已禁用",
                session_type_ssh: "SSH",
                session_type_local: "本地终端",
                session_type_telnet: "Telnet",
                session_type_serial: "串口",
            },
            TrayLanguage::En => Self {
                show_main_window: "Show Main Window",
                hide_to_tray: "Hide to Tray",
                new_session: "New Connection…",
                active_sessions: "Active Sessions",
                no_active_sessions: "No Active Sessions",
                only_showing_first_8: "Showing only the first 8",
                open_active_sessions_panel: "Open Active Sessions Panel",
                sync_backup: "Sync & Backup",
                current_status: "Current Status",
                sync_push_now: "Push Now",
                sync_pull_now: "Pull Now",
                sync_backup_now: "Backup Now",
                open_sync_backup_history: "Open Sync & Backup History",
                settings: "Settings…",
                minimize_to_tray: "Minimize To Tray On Close",
                lock_screen: "Lock Screen",
                check_updates: "Check for Updates",
                quit: "Quit NyaTerm",
                status_idle: "Idle",
                status_running: "Running",
                status_success: "Success",
                status_failed: "Failed",
                status_conflict: "Conflict",
                status_disabled: "Disabled",
                session_type_ssh: "SSH",
                session_type_local: "Local",
                session_type_telnet: "Telnet",
                session_type_serial: "Serial",
            },
        }
    }

    fn active_sessions_title(&self, language: TrayLanguage, count: usize) -> String {
        match language {
            TrayLanguage::ZhCn => format!("{} ({count})", self.active_sessions),
            TrayLanguage::En => format!("{} ({count})", self.active_sessions),
        }
    }

    fn sync_backup_title(&self, language: TrayLanguage, status: &str) -> String {
        match language {
            TrayLanguage::ZhCn => format!("{}（{status}）", self.sync_backup),
            TrayLanguage::En => format!("{} ({status})", self.sync_backup),
        }
    }

    fn current_status_row(&self, status: &str) -> String {
        format!("{}: {status}", self.current_status)
    }

    fn localized_status(&self, state: &str) -> String {
        match state {
            "idle" => self.status_idle.to_string(),
            "running" => self.status_running.to_string(),
            "success" => self.status_success.to_string(),
            "failed" => self.status_failed.to_string(),
            "conflict" => self.status_conflict.to_string(),
            "disabled" => self.status_disabled.to_string(),
            other if other.is_empty() => self.status_idle.to_string(),
            other => other.to_string(),
        }
    }

    fn localized_session_type(&self, session_type: &SessionType) -> &'static str {
        match session_type {
            SessionType::SSH => self.session_type_ssh,
            SessionType::Local => self.session_type_local,
            SessionType::Telnet => self.session_type_telnet,
            SessionType::Serial => self.session_type_serial,
        }
    }
}

pub fn setup(app: &mut tauri::App) -> tauri::Result<()> {
    let menu = Menu::new(app)?;
    app.manage(TrayMenuState::new(menu.clone()));

    TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("NyaTerm")
        .show_menu_on_left_click(false)
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } => {
                app::show_main_window(tray.app_handle());
            }
            _ => {}
        })
        .build(app)?;

    schedule_refresh(app.handle());
    Ok(())
}

pub fn schedule_refresh(app: &AppHandle) {
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        refresh(&app_handle).await;
    });
}

pub async fn refresh(app: &AppHandle) {
    let Some(state) = app.try_state::<TrayMenuState>() else {
        return;
    };
    let _refresh_guard = state.refresh_lock.lock().await;

    let settings = config::load_app_settings(app).unwrap_or_default();
    let language = TrayLanguage::from_settings(&settings);
    let strings = TrayStrings::for_language(language);
    let sessions = collect_sessions(app).await;
    let sync_status = collect_sync_status(app).await;
    let effective_sync_state = effective_sync_state(&settings, &sync_status);

    if let Err(error) = rebuild_root_menu(
        app,
        &state.menu(),
        &settings,
        language,
        &strings,
        &sessions,
        effective_sync_state,
    ) {
        tracing::warn!("Failed to refresh tray menu: {}", error);
    }
}

fn rebuild_root_menu(
    app: &AppHandle,
    menu: &Menu<tauri::Wry>,
    settings: &AppSettings,
    language: TrayLanguage,
    strings: &TrayStrings,
    sessions: &[SessionInfo],
    effective_sync_state: &str,
) -> tauri::Result<()> {
    clear_menu(menu)?;

    let show_main = new_menu_item(app, MENU_SHOW_MAIN, strings.show_main_window, true)?;
    let hide_to_tray = new_menu_item(app, MENU_HIDE_TO_TRAY, strings.hide_to_tray, true)?;
    let separator_1 = PredefinedMenuItem::separator(app)?;
    let new_session = new_menu_item(app, MENU_NEW_SESSION, strings.new_session, true)?;
    let active_sessions_submenu = build_active_sessions_submenu(app, language, strings, sessions)?;
    let sync_backup_submenu =
        build_sync_backup_submenu(app, language, strings, settings, effective_sync_state)?;
    let separator_2 = PredefinedMenuItem::separator(app)?;
    let settings_item = new_menu_item(app, MENU_SETTINGS, strings.settings, true)?;
    let minimize_to_tray = CheckMenuItem::with_id(
        app,
        MENU_MINIMIZE_TO_TRAY,
        strings.minimize_to_tray,
        true,
        settings.general.minimize_to_tray,
        None::<&str>,
    )?;
    let check_updates = new_menu_item(app, MENU_CHECK_UPDATES, strings.check_updates, true)?;
    let separator_3 = PredefinedMenuItem::separator(app)?;
    let quit = new_menu_item(app, MENU_QUIT, strings.quit, true)?;

    menu.append(&show_main)?;
    menu.append(&hide_to_tray)?;
    menu.append(&separator_1)?;
    menu.append(&new_session)?;
    menu.append(&active_sessions_submenu)?;
    menu.append(&sync_backup_submenu)?;
    menu.append(&separator_2)?;
    menu.append(&settings_item)?;
    menu.append(&minimize_to_tray)?;

    if settings.security.enable_screen_lock {
        let lock_screen = new_menu_item(app, MENU_LOCK_SCREEN, strings.lock_screen, true)?;
        menu.append(&lock_screen)?;
    }

    menu.append(&check_updates)?;
    menu.append(&separator_3)?;
    menu.append(&quit)?;
    Ok(())
}

fn build_active_sessions_submenu(
    app: &AppHandle,
    language: TrayLanguage,
    strings: &TrayStrings,
    sessions: &[SessionInfo],
) -> tauri::Result<Submenu<tauri::Wry>> {
    let mut sorted_sessions = sessions.to_vec();
    sorted_sessions.sort_by(|left, right| {
        let left_name = left.name.to_lowercase();
        let right_name = right.name.to_lowercase();
        left_name.cmp(&right_name).then_with(|| {
            session_type_sort_key(&left.session_type)
                .cmp(session_type_sort_key(&right.session_type))
        })
    });

    let submenu = Submenu::with_id(
        app,
        SUBMENU_ACTIVE_SESSIONS,
        escape_menu_text(&strings.active_sessions_title(language, sorted_sessions.len())),
        true,
    )?;

    if sorted_sessions.is_empty() {
        let empty_item = new_menu_item(
            app,
            MENU_DISABLED_ACTIVE_SESSIONS,
            strings.no_active_sessions,
            false,
        )?;
        submenu.append(&empty_item)?;
    } else {
        for session in sorted_sessions.iter().take(ACTIVE_SESSION_LIMIT) {
            let item = new_menu_item(
                app,
                format!("{SESSION_MENU_PREFIX}{}", session.id),
                format!(
                    "{} · {}",
                    session.name,
                    strings.localized_session_type(&session.session_type)
                ),
                true,
            )?;
            submenu.append(&item)?;
        }

        if sorted_sessions.len() > ACTIVE_SESSION_LIMIT {
            let limited_item = new_menu_item(
                app,
                MENU_DISABLED_ACTIVE_SESSIONS_LIMITED,
                strings.only_showing_first_8,
                false,
            )?;
            submenu.append(&limited_item)?;
        }
    }

    let separator = PredefinedMenuItem::separator(app)?;
    let open_panel = new_menu_item(
        app,
        MENU_OPEN_ACTIVE_SESSIONS_PANEL,
        strings.open_active_sessions_panel,
        true,
    )?;
    submenu.append(&separator)?;
    submenu.append(&open_panel)?;
    Ok(submenu)
}

fn build_sync_backup_submenu(
    app: &AppHandle,
    language: TrayLanguage,
    strings: &TrayStrings,
    settings: &AppSettings,
    effective_sync_state: &str,
) -> tauri::Result<Submenu<tauri::Wry>> {
    let localized_status = strings.localized_status(effective_sync_state);
    let submenu = Submenu::with_id(
        app,
        SUBMENU_SYNC_BACKUP,
        escape_menu_text(&strings.sync_backup_title(language, &localized_status)),
        true,
    )?;

    let status_item = new_menu_item(
        app,
        MENU_DISABLED_SYNC_STATUS,
        strings.current_status_row(&localized_status),
        false,
    )?;
    submenu.append(&status_item)?;

    let show_manual_operations = settings.cloud_sync.enabled
        && matches!(
            effective_sync_state,
            "idle" | "success" | "failed" | "running"
        );
    let enable_manual_operations = settings.cloud_sync.enabled
        && matches!(effective_sync_state, "idle" | "success" | "failed");

    if show_manual_operations {
        let separator = PredefinedMenuItem::separator(app)?;
        let push = new_menu_item(
            app,
            MENU_SYNC_PUSH,
            strings.sync_push_now,
            enable_manual_operations,
        )?;
        let pull = new_menu_item(
            app,
            MENU_SYNC_PULL,
            strings.sync_pull_now,
            enable_manual_operations,
        )?;
        let backup = new_menu_item(
            app,
            MENU_SYNC_BACKUP,
            strings.sync_backup_now,
            enable_manual_operations,
        )?;
        submenu.append(&separator)?;
        submenu.append(&push)?;
        submenu.append(&pull)?;
        submenu.append(&backup)?;
    }

    let separator = PredefinedMenuItem::separator(app)?;
    let open_history = new_menu_item(
        app,
        MENU_OPEN_SYNC_HISTORY,
        strings.open_sync_backup_history,
        true,
    )?;
    submenu.append(&separator)?;
    submenu.append(&open_history)?;

    Ok(submenu)
}

fn clear_menu(menu: &Menu<tauri::Wry>) -> tauri::Result<()> {
    while menu.remove_at(0)?.is_some() {}
    Ok(())
}

fn new_menu_item<I, S>(
    app: &AppHandle,
    id: I,
    text: S,
    enabled: bool,
) -> tauri::Result<MenuItem<tauri::Wry>>
where
    I: Into<tauri::menu::MenuId>,
    S: AsRef<str>,
{
    let text = escape_menu_text(text.as_ref());
    MenuItem::with_id(app, id, text, enabled, None::<&str>)
}

fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let event_id = event.id.as_ref();
    match event_id {
        MENU_SHOW_MAIN => app::show_main_window(app),
        MENU_HIDE_TO_TRAY => app::hide_main_window(app),
        MENU_NEW_SESSION => {
            app::show_main_window(app);
            emit_tray_action(app, TrayActionPayload::OpenNewSession);
        }
        MENU_OPEN_ACTIVE_SESSIONS_PANEL => {
            app::show_main_window(app);
            emit_tray_action(
                app,
                TrayActionPayload::OpenPanel {
                    panel_id: "activeSessions",
                },
            );
        }
        MENU_SYNC_PUSH => {
            let app_handle = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Some(manager) = app_handle.try_state::<Arc<CloudSyncManager>>() {
                    let _ = manager.inner().sync_push_now("tray_manual_push").await;
                }
            });
        }
        MENU_SYNC_PULL => {
            let app_handle = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Some(manager) = app_handle.try_state::<Arc<CloudSyncManager>>() {
                    let _ = manager.inner().sync_pull_now("tray_manual_pull").await;
                }
            });
        }
        MENU_SYNC_BACKUP => {
            let app_handle = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Some(manager) = app_handle.try_state::<Arc<CloudSyncManager>>() {
                    let _ = manager
                        .inner()
                        .run_cloud_backup_now("tray_manual_backup")
                        .await;
                }
            });
        }
        MENU_OPEN_SYNC_HISTORY => {
            app::show_main_window(app);
            emit_tray_action(
                app,
                TrayActionPayload::OpenPanel {
                    panel_id: "syncBackupHistory",
                },
            );
        }
        MENU_SETTINGS => {
            app::show_main_window(app);
            emit_tray_action(app, TrayActionPayload::OpenSettings);
        }
        MENU_MINIMIZE_TO_TRAY => toggle_minimize_to_tray(app),
        MENU_LOCK_SCREEN => {
            emit_tray_action(app, TrayActionPayload::LockScreen);
        }
        MENU_CHECK_UPDATES => {
            app::show_main_window(app);
            emit_tray_action(app, TrayActionPayload::CheckUpdates);
        }
        MENU_QUIT => {
            app::show_main_window(app);
            emit_tray_action(app, TrayActionPayload::RequestQuit);
        }
        _ if event_id.starts_with(SESSION_MENU_PREFIX) => {
            if let Some(session_id) = event_id.strip_prefix(SESSION_MENU_PREFIX) {
                app::show_main_window(app);
                emit_tray_action(
                    app,
                    TrayActionPayload::FocusSession {
                        session_id: session_id.to_string(),
                    },
                );
            }
        }
        _ => {}
    }
}

fn emit_tray_action(app: &AppHandle, payload: TrayActionPayload) {
    let target_window_label = crate::app::focused_or_first_main_window(app)
        .map(|window| window.label().to_string());
    let _ = app.emit(
        "tray-action",
        TargetedTrayActionPayload {
            action: payload,
            target_window_label,
        },
    );
}

fn toggle_minimize_to_tray(app: &AppHandle) {
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut settings = config::load_app_settings(&app_handle).unwrap_or_default();
        settings.general.minimize_to_tray = !settings.general.minimize_to_tray;

        if let Some(manager) = app_handle.try_state::<Arc<CloudSyncManager>>() {
            if let Err(error) =
                crate::cmd::settings::persist_app_settings(&app_handle, manager.inner(), settings)
                    .await
            {
                tracing::warn!("Failed to toggle minimize_to_tray from tray: {}", error);
            }
        }
    });
}

async fn collect_sessions(app: &AppHandle) -> Vec<SessionInfo> {
    let Some(manager) = app.try_state::<Arc<SessionManager>>() else {
        return Vec::new();
    };
    manager.list_sessions().await
}

async fn collect_sync_status(app: &AppHandle) -> CloudSyncStatus {
    let Some(manager) = app.try_state::<Arc<CloudSyncManager>>() else {
        return CloudSyncStatus::default();
    };
    manager.get_status().await
}

fn effective_sync_state<'a>(
    settings: &'a AppSettings,
    sync_status: &'a CloudSyncStatus,
) -> &'a str {
    if !settings.cloud_sync.enabled {
        "disabled"
    } else if sync_status.state.is_empty() {
        "idle"
    } else {
        sync_status.state.as_str()
    }
}

fn session_type_sort_key(session_type: &SessionType) -> &'static str {
    match session_type {
        SessionType::Local => "local",
        SessionType::Serial => "serial",
        SessionType::SSH => "ssh",
        SessionType::Telnet => "telnet",
    }
}

fn escape_menu_text(text: &str) -> String {
    text.replace('&', "&&")
}
