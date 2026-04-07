import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { emit } from "@tauri-apps/api/event";
import i18n from "../i18n";
import { AppContext } from "./AppContext";
import { invoke } from "../lib/invoke";
import { logger } from "../lib/logger";
import { DEFAULT_TERMINAL_FONT_SIZE } from "../lib/terminalFontSize";
import type { AppSettings, Group, SavedConnection, UiConfig } from "@/types/global";

const DEFAULT_APP_SETTINGS: AppSettings = {
  general: {
    startup_restore: true,
    default_local_shell: navigator.userAgent.includes("Win") ? "powershell.exe" : "bash",
    minimize_to_tray: false,
    boss_key: null,
  },
  appearance: {
    theme: "github-dark",
    terminal_theme: "default",
    font_family: "JetBrains Mono, 'Noto Sans SC Variable', Consolas, monospace, Inter",
    font_size: DEFAULT_TERMINAL_FONT_SIZE,
    ligatures: false,
    background_opacity: 1.0,
    cursor_style: "block",
    cursor_blink: true,
    ui_font_size: 16,
  },
  proxy: { enabled: false, protocol: "socks5", host: "127.0.0.1", port: 1080 },
  search: { custom_engines: [] },
  translation: {
    target_language: "zh-CN",
    deepl_api_key: "",
    baidu_app_id: "",
    baidu_app_key: "",
    ali_app_id: "",
    ali_app_key: "",
    youdao_app_id: "",
    youdao_app_key: "",
  },
  security: {
    use_os_keyring: true,
    require_master_password: false,
    enable_screen_lock: false,
    idle_lock_minutes: 0,
    host_key_policy: "prompt",
  },
  terminal: {
    scrollback_lines: 10000,
    keep_alive_interval: 60,
    hardware_acceleration: false,
    keyword_highlights_enabled: true,
    keyword_highlights_across_wrapped_lines: false,
    keyword_highlights: [],
    action_links_enabled: true,
    action_links_matchers: {
      ipv4: true,
      archive: true,
      host_port: true,
    },
  },
  interaction: {
    copy_on_select: false,
    right_click_paste: false,
    word_separators: " ()[]{}\"':=,;|&<>",
    default_encoding: "UTF-8",
  },
  ui: {
    open_tabs: [],
    left_width: 256,
    right_width: 288,
    quick_cmd_height: 36,
    active_left_panel: "fileExplorer",
    active_right_panel: "savedConnections",
    show_quick_cmd_bar: true,
    zoom_level: 1.0,
    language: "en",
    show_remote_stats: false,
    remote_stats_interval: 3,
    saved_connections_sort_mode: "default",
    activity_bar_layout: {
      left_top: ["fileExplorer", "fileTransfer", "securityAuth"],
      left_bottom: ["settings"],
      right_top: ["savedConnections", "activeSessions", "commandHistory", "resourceMonitor"],
      right_bottom: ["quickCmdBar", "lock"],
      show_labels: false,
    },
  },
};

/**
 * Lightweight AppContext provider for child windows (settings, new-session, etc.).
 * Loads/saves appSettings via backend and emits cross-window Tauri events.
 * Tabs, connections, and dialog state are stubbed since child windows don't use them.
 */
export function ChildAppProvider({ children }: { children: ReactNode }) {
  const [appSettings, setAppSettings] = useState<AppSettings>(DEFAULT_APP_SETTINGS);
  const loaded = useRef(false);
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [settingsLoaded, setSettingsLoaded] = useState(false);

  useEffect(() => {
    invoke<AppSettings>("get_app_settings")
      .then((cfg) => {
        setAppSettings(cfg);
        loaded.current = true;
        setSettingsLoaded(true);
        if (cfg.ui?.language && cfg.ui.language !== i18n.language) {
          i18n.changeLanguage(cfg.ui.language);
        }
      })
      .catch(() => {
        loaded.current = true;
        setSettingsLoaded(true);
      });
  }, []);

  useEffect(() => {
    document.documentElement.style.fontSize = `${appSettings.appearance.ui_font_size}px`;
  }, [appSettings.appearance.ui_font_size]);

  const updateAppSettings = useCallback(
    (updates: Partial<AppSettings> | ((prev: AppSettings) => Partial<AppSettings>)) => {
      setAppSettings((prev) => {
        const nextUpdates = typeof updates === "function" ? updates(prev) : updates;
        const next = { ...prev, ...nextUpdates };
        if (loaded.current) {
          if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
          saveTimerRef.current = setTimeout(() => {
            invoke("save_app_settings", { settings: next }).catch((e) =>
              logger.error("Failed to save app settings", e),
            );
            emit("settings-changed", next).catch(() => { });
          }, 500);
        }
        return next;
      });
    },
    [],
  );

  const updateUi = useCallback(
    (updates: Partial<UiConfig> | ((prev: UiConfig) => Partial<UiConfig>)) => {
      updateAppSettings((prev) => {
        const nextUpdates = typeof updates === "function" ? updates(prev.ui) : updates;
        return { ui: { ...prev.ui, ...nextUpdates } };
      });
    },
    [updateAppSettings],
  );

  const noop = useCallback(() => { }, []);
  const noopAsync = useCallback(async () => { }, []);

  return (
    <AppContext.Provider
      value={{
        tabs: [],
        activeTabId: null,
        setActiveTabId: noop,
        addTab: noop,
        addPendingTab: () => "",
        updateTabSession: noop,
        closeTab: noop,
        appSettings,
        updateAppSettings,
        updateUi,
        savedConnections: [] as SavedConnection[],
        savedGroups: [] as Group[],
        refreshConnections: noopAsync,
        showNewSession: false,
        setShowNewSession: noop,
        editingConnection: undefined,
        setEditingConnection: noop,
        showSettingsDialog: false,
        setShowSettingsDialog: noop,
        isLocked: false,
        setIsLocked: noop,
        settingsLoaded,
      }}
    >
      {children}
    </AppContext.Provider>
  );
}
