use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::Manager;
use tauri_winrt_notification::{Duration, Toast};

use crate::cmd::app::AppLockState;

pub(super) fn show(window: &tauri::WebviewWindow, on_click: impl Fn() + Send + 'static) {
    let window = window.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let app = window.app_handle();
        if app
            .try_state::<AppLockState>()
            .is_some_and(|state| state.is_locked())
        {
            tracing::debug!(
                window = window.label(),
                reason = "locked",
                "MCP approval notification skipped"
            );
            return;
        }
        let focused = window.is_focused();
        let minimized = window.is_minimized();
        let visible = window.is_visible();
        tracing::debug!(
            window = window.label(),
            focused = ?focused,
            minimized = ?minimized,
            visible = ?visible,
            "MCP approval notification window state"
        );
        if !should_notify(
            focused.unwrap_or(true),
            minimized.unwrap_or(false),
            visible.unwrap_or(true),
        ) {
            tracing::debug!(
                window = window.label(),
                reason = "not_background",
                "MCP approval notification skipped"
            );
            return;
        }
        let executable = match std::env::current_exe() {
            Ok(path) => path,
            Err(error) => {
                tracing::warn!(
                    window = window.label(),
                    %error,
                    "MCP approval notification executable path unavailable"
                );
                return;
            }
        };
        let app_id = notification_app_id(&executable, &app.config().identifier);
        let settings = crate::config::load_app_settings(app).unwrap_or_default();
        let (title, body) = notification_text(settings.ui.language.as_deref().unwrap_or("en"));
        let active = Arc::new(AtomicBool::new(true));
        let click_active = active.clone();
        // shortcut: navigation covers live banners only; add native activation for Action Center support.
        let result = Toast::new(app_id)
            .title(title)
            .text1(body)
            .duration(Duration::Short)
            .on_activated(move |action| {
                if consume_body_click(&click_active, action.as_deref()) {
                    on_click();
                }
                Ok(())
            })
            .on_dismissed(move |_| {
                active.store(false, Ordering::SeqCst);
                Ok(())
            })
            .show();
        match result {
            Ok(()) => tracing::debug!(
                window = window.label(),
                app_id,
                "MCP approval notification submitted to Windows"
            ),
            Err(error) => tracing::warn!(
                window = window.label(),
                app_id,
                %error,
                "MCP approval notification submission failed"
            ),
        }
    });
}

fn notification_app_id<'a>(executable: &Path, identifier: &'a str) -> &'a str {
    // Match Tauri's Windows notification identity selection for uninstalled target binaries.
    let is_development_binary = executable.parent().is_some_and(|directory| {
        directory.ends_with("target/debug") || directory.ends_with("target/release")
    });
    if is_development_binary {
        Toast::POWERSHELL_APP_ID
    } else {
        identifier
    }
}

fn should_notify(focused: bool, minimized: bool, visible: bool) -> bool {
    !focused || minimized || !visible
}

fn consume_body_click(active: &AtomicBool, action: Option<&str>) -> bool {
    action.is_none() && active.swap(false, Ordering::SeqCst)
}

fn notification_text(language: &str) -> (&'static str, &'static str) {
    let normalized = language.trim().replace('_', "-").to_ascii_lowercase();
    // Keep native strings small; the tests below check them against the UI locales.
    match normalized.as_str() {
        code if matches!(code, "zh-tw" | "zh-hk" | "zh-mo" | "zh-hant")
            || code.starts_with("zh-hant-") =>
        {
            ("NyaTerm：MCP 核准待處理", "返回 NyaTerm 檢視並處理請求。")
        }
        code if matches!(code, "zh" | "zh-cn" | "zh-sg" | "zh-hans")
            || code.starts_with("zh-hans-") =>
        {
            ("NyaTerm：MCP 审批待处理", "返回 NyaTerm 查看并处理请求。")
        }
        "ko" | "ko-kr" => (
            "NyaTerm: MCP 승인 필요",
            "NyaTerm으로 돌아가 요청을 검토하세요.",
        ),
        "ru" | "ru-ru" => (
            "NyaTerm: требуется подтверждение MCP",
            "Вернитесь в NyaTerm, чтобы просмотреть запрос.",
        ),
        _ => (
            "NyaTerm: MCP approval required",
            "Return to NyaTerm to review the request.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_notification_development_binaries_use_powershell_identity() {
        for profile in ["debug", "release"] {
            let executable = Path::new("C:/work/nyaterm/target")
                .join(profile)
                .join("nyaterm.exe");
            assert_eq!(
                notification_app_id(&executable, "com.kang.nyaterm"),
                Toast::POWERSHELL_APP_ID
            );
        }
    }

    #[test]
    fn mcp_notification_installed_binaries_keep_app_identity() {
        for executable in [
            "C:/Program Files/NyaTerm/nyaterm.exe",
            "C:/Apps/debug/nyaterm.exe",
            "C:/Apps/release/nyaterm.exe",
            "C:/Apps/not-target/debug/nyaterm.exe",
        ] {
            assert_eq!(
                notification_app_id(Path::new(executable), "com.kang.nyaterm"),
                "com.kang.nyaterm"
            );
        }
    }

    #[test]
    fn mcp_notification_only_reminds_for_background_windows() {
        assert!(!should_notify(true, false, true));
        assert!(should_notify(false, false, true));
        assert!(should_notify(true, true, true));
        assert!(should_notify(true, false, false));
    }

    #[test]
    fn mcp_notification_body_click_is_consumed_once() {
        let active = AtomicBool::new(true);
        assert!(consume_body_click(&active, None));
        assert!(!consume_body_click(&active, None));
    }

    #[test]
    fn mcp_notification_ignores_actions_and_dismissed_banners() {
        let active = AtomicBool::new(true);
        assert!(!consume_body_click(&active, Some("allow_once")));
        assert!(active.load(Ordering::SeqCst));
        active.store(false, Ordering::SeqCst);
        assert!(!consume_body_click(&active, None));
    }

    #[test]
    fn mcp_notification_native_strings_match_all_ui_locales() {
        for (language, source) in [
            ("en", include_str!("../../../../src/i18n/locales/en.json")),
            (
                "zh-CN",
                include_str!("../../../../src/i18n/locales/zh-CN.json"),
            ),
            (
                "zh-TW",
                include_str!("../../../../src/i18n/locales/zh-TW.json"),
            ),
            ("ko", include_str!("../../../../src/i18n/locales/ko.json")),
            ("ru", include_str!("../../../../src/i18n/locales/ru.json")),
        ] {
            let locale: serde_json::Value = serde_json::from_str(source).unwrap();
            let (title, body) = notification_text(language);
            assert_eq!(locale["ai"]["externalMcpNotificationTitle"], title);
            assert_eq!(locale["ai"]["externalMcpNotificationBody"], body);
        }
    }

    #[test]
    fn mcp_notification_supports_language_aliases() {
        for language in ["zh", "zh-SG", "zh_Hans", "zh-Hans-CN"] {
            assert_eq!(notification_text(language), notification_text("zh-CN"));
        }
        for language in ["zh-HK", "zh_MO", "zh-Hant", "zh-Hant-TW"] {
            assert_eq!(notification_text(language), notification_text("zh-TW"));
        }
        assert_eq!(notification_text("KO_kr"), notification_text("ko"));
        assert_eq!(notification_text("ru_RU"), notification_text("ru"));
        assert_eq!(notification_text("fr"), notification_text("en"));
    }
}
