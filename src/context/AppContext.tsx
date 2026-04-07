import { listen } from "@tauri-apps/api/event";
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
} from "react";
import type {
  AppSettings,
  Group,
  SavedConnection,
  SessionType,
  Tab,
  UiConfig,
} from "@/types/global";
import { invoke } from "../lib/invoke";
import { logger } from "../lib/logger";
import { DEFAULT_TERMINAL_FONT_SIZE } from "../lib/terminalFontSize";

interface AppContextType {
  // Tabs
  tabs: Tab[];
  activeTabId: string | null;
  setActiveTabId: (id: string | null) => void;
  addTab: (sessionId: string, name: string, type: SessionType, connectionId?: string) => void;
  /** Immediately add a "connecting" tab and make it active. Returns the new tabId. */
  addPendingTab: (name: string, type: SessionType, connectionId?: string) => string;
  /** Swap the temporary sessionId for the real one and clear the connecting flag. */
  updateTabSession: (tabId: string, sessionId: string) => void;
  closeTab: (tabId: string) => void;

  // App Settings (includes UI config)
  appSettings: AppSettings;
  updateAppSettings: (
    updates: Partial<AppSettings> | ((prev: AppSettings) => Partial<AppSettings>),
  ) => void;
  updateUi: (updates: Partial<UiConfig> | ((prev: UiConfig) => Partial<UiConfig>)) => void;

  // Data
  savedConnections: SavedConnection[];
  savedGroups: Group[];
  refreshConnections: () => Promise<void>;

  // Dialogs
  showNewSession: boolean;
  setShowNewSession: (show: boolean) => void;
  editingConnection: SavedConnection | undefined;
  setEditingConnection: (conn: SavedConnection | undefined) => void;
  showSettingsDialog: boolean;
  setShowSettingsDialog: (show: boolean) => void;

  // Idle Lock
  isLocked: boolean;
  setIsLocked: (locked: boolean) => void;

  // Loading
  settingsLoaded: boolean;
}

/**
 * App-wide state: tabs, settings (debounced save), saved connections (polled),
 * and dialog visibility. Updates via setState/useCallback; config persisted to backend.
 */
export const AppContext = createContext<AppContextType | null>(null);

const DEFAULT_APP_SETTINGS: AppSettings = {
  general: {
    startup_restore: true,
    default_local_shell: navigator.userAgent.includes("Win") ? "powershell.exe" : "bash",
    minimize_to_tray: false,
    boss_key: null,
  },
  appearance: {
    theme: "github-dark",
    font_family: "JetBrains Mono, 'Noto Sans SC Variable', Consolas, monospace, Inter",
    font_size: DEFAULT_TERMINAL_FONT_SIZE,
    ligatures: false,
    background_opacity: 1.0,
    cursor_style: "block",
    cursor_blink: true,
    ui_font_size: 16,
    terminal_theme: null,
  },
  proxy: {
    enabled: false,
    protocol: "socks5",
    host: "127.0.0.1",
    port: 1080,
  },
  search: {
    custom_engines: [
      { name: "Google", url_template: "https://google.com/search?q=%s" },
      { name: "Bing", url_template: "https://bing.com/search?q=%s" },
      { name: "GitHub", url_template: "https://github.com/search?q=%s" },
    ],
  },
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

/** Provides tabs, appSettings, savedConnections, and dialog state to the app. */
export function AppProvider({ children }: { children: ReactNode }) {
  // Tabs State
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeTabIdState, setActiveTabIdState] = useState<string | null>(null);
  const activeTabIdRef = useRef<string | null>(null);

  // App Settings State (includes UI config)
  const [appSettings, setAppSettings] = useState<AppSettings>(DEFAULT_APP_SETTINGS);
  const appSettingsLoaded = useRef(false);
  const appSettingsSaveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Data State
  const [savedConnections, setSavedConnections] = useState<SavedConnection[]>([]);
  const [savedGroups, setSavedGroups] = useState<Group[]>([]);

  // Dialog State
  const [showNewSession, setShowNewSession] = useState(false);
  const [editingConnection, setEditingConnection] = useState<SavedConnection | undefined>(
    undefined,
  );
  const [showSettingsDialog, setShowSettingsDialog] = useState(false);

  // Idle Lock State
  const [isLocked, setIsLocked] = useState(false);

  // Loading State
  const [settingsLoaded, setSettingsLoaded] = useState(false);

  const setActiveTabId = useCallback((id: string | null) => {
    activeTabIdRef.current = id;
    setActiveTabIdState(id);
  }, []);

  // 1. Load App Settings
  useEffect(() => {
    invoke<AppSettings>("get_app_settings")
      .then((cfg) => {
        setAppSettings(cfg);
        appSettingsLoaded.current = true;
        setSettingsLoaded(true);
        if (cfg.security?.enable_screen_lock) {
          setIsLocked(true);
        }
      })
      .catch(() => {
        appSettingsLoaded.current = true;
        setSettingsLoaded(true);
      });
  }, []);

  // Apply UI font size to root element
  useEffect(() => {
    document.documentElement.style.fontSize = `${appSettings.appearance.ui_font_size}px`;
  }, [appSettings.appearance.ui_font_size]);

  // 2. Save App Settings Debounced
  const updateAppSettings = useCallback(
    (updates: Partial<AppSettings> | ((prev: AppSettings) => Partial<AppSettings>)) => {
      setAppSettings((prev) => {
        const nextUpdates = typeof updates === "function" ? updates(prev) : updates;
        const next = { ...prev, ...nextUpdates };
        if (appSettingsLoaded.current) {
          if (appSettingsSaveTimerRef.current) clearTimeout(appSettingsSaveTimerRef.current);
          appSettingsSaveTimerRef.current = setTimeout(() => {
            invoke("save_app_settings", { settings: next }).catch((e) =>
              logger.error("Failed to save app settings", e),
            );
          }, 500);
        }
        return next;
      });
    },
    [],
  );

  // Convenience helper to update just the UI config portion
  const updateUi = useCallback(
    (updates: Partial<UiConfig> | ((prev: UiConfig) => Partial<UiConfig>)) => {
      updateAppSettings((prev) => {
        const nextUpdates = typeof updates === "function" ? updates(prev.ui) : updates;
        return { ui: { ...prev.ui, ...nextUpdates } };
      });
    },
    [updateAppSettings],
  );

  // 3. Load Connections
  const refreshConnections = useCallback(async () => {
    try {
      const [saved, groups] = await Promise.all([
        invoke<SavedConnection[]>("get_saved_connections"),
        invoke<Group[]>("get_groups"),
      ]);
      setSavedConnections(saved);
      setSavedGroups(groups);
    } catch (e) {
      logger.error("Failed to fetch connections", e);
    }
  }, []);

  useEffect(() => {
    refreshConnections();
    const unlisten = listen("connections-changed", () => {
      refreshConnections();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [refreshConnections]);

  // 4. Tab Logic
  const addTab = useCallback(
    (sessionId: string, name: string, type: SessionType, connectionId?: string) => {
      const tabId = `tab-${Date.now()}`;
      const newTab: Tab = {
        id: tabId,
        sessionId,
        name,
        type,
        connectionId,
      };
      setTabs((prev) => [...prev, newTab]);
      setActiveTabId(tabId);

      // Close dialogs when session starts
      setShowNewSession(false);
      setEditingConnection(undefined);
    },
    [setActiveTabId],
  );

  const addPendingTab = useCallback(
    (name: string, type: SessionType, connectionId?: string): string => {
      const tabId = `tab-${Date.now()}`;
      const newTab: Tab = {
        id: tabId,
        sessionId: tabId,
        name,
        type,
        connectionId,
        connecting: true,
      };
      setTabs((prev) => [...prev, newTab]);
      setActiveTabId(tabId);
      return tabId;
    },
    [setActiveTabId],
  );

  const updateTabSession = useCallback((tabId: string, sessionId: string) => {
    setTabs((prev) =>
      prev.map((tab) =>
        tab.id === tabId
          ? {
              ...tab,
              sessionId,
              connecting: false,
            }
          : tab,
      ),
    );
  }, []);

  const closeTab = useCallback(
    (tabId: string) => {
      setTabs((prev) => {
        const newTabs = prev.filter((t) => t.id !== tabId);
        if (activeTabIdRef.current === tabId) {
          if (newTabs.length > 0) {
            setActiveTabId(newTabs[newTabs.length - 1].id);
          } else {
            setActiveTabId(null);
          }
        }
        return newTabs;
      });
    },
    [setActiveTabId],
  );

  // 5. Startup Restore Logic
  const hasRestored = useRef(false);

  useEffect(() => {
    if (!hasRestored.current && appSettingsLoaded.current) {
      hasRestored.current = true;
      if (
        appSettings.general.startup_restore &&
        appSettings.ui.open_tabs &&
        appSettings.ui.open_tabs.length > 0
      ) {
        appSettings.ui.open_tabs.forEach((tab) => {
          if (tab.session_type === "SSH" && tab.connection_id) {
            invoke<string>("create_ssh_session", { connectionId: tab.connection_id })
              .then((sessionId) => {
                addTab(sessionId, tab.title, "SSH", tab.connection_id);
              })
              .catch((e) => logger.error(`Restore SSH failed for ${tab.title}`, e));
          } else if (tab.session_type === "Local" || tab.session_type === "local") {
            invoke<string>("create_local_session")
              .then((sessionId) => {
                addTab(sessionId, tab.title, "Local");
              })
              .catch((e) => logger.error(`Restore Local failed`, e));
          }
        });
      }
    }
  }, [appSettings, addTab]);

  // 6. Sync opened tabs
  useEffect(() => {
    if (hasRestored.current && appSettings.general.startup_restore) {
      updateUi({
        open_tabs: tabs.map((t) => ({
          title: t.name,
          session_type: t.type,
          connection_id: t.connectionId,
        })),
      });
    }
  }, [tabs, appSettings.general.startup_restore, updateUi]);

  return (
    <AppContext.Provider
      value={{
        tabs,
        activeTabId: activeTabIdState,
        setActiveTabId,
        addTab,
        addPendingTab,
        updateTabSession,
        closeTab,
        appSettings,
        updateAppSettings,
        updateUi,
        savedConnections,
        savedGroups,
        refreshConnections,
        showNewSession,
        setShowNewSession,
        editingConnection,
        setEditingConnection,
        showSettingsDialog,
        setShowSettingsDialog,
        isLocked,
        setIsLocked,
        settingsLoaded,
      }}
    >
      {children}
    </AppContext.Provider>
  );
}

/** Hook to access AppContext. Throws if used outside AppProvider. */
export function useApp() {
  const context = useContext(AppContext);
  if (!context) throw new Error("useApp must be used within AppProvider");
  return context;
}
