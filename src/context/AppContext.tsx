import { listen } from "@tauri-apps/api/event";
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { DEFAULT_AI_SETTINGS } from "@/lib/aiSettings";
import { DEFAULT_CLOUD_SYNC_SETTINGS } from "@/lib/cloudSync";
import { getErrorMessage } from "@/lib/errors";
import {
  DEFAULT_COMMAND_SUGGESTION_MAX_CHARS,
  DEFAULT_COMMAND_SUGGESTION_MIN_CHARS,
} from "@/lib/interactionSettings";
import {
  collectSessionPanes,
  createSessionPane,
  createWorkspaceTab,
  ensureActivePane,
  getFirstSessionPane,
  getNextPersistOrder,
  insertTabAfter,
  moveTab,
  removeSessionPane,
  restoreTabFromPersistence,
  serializeTabsForPersistence,
  splitSessionPane,
  updateSessionPane,
  updateSplitRatio as updateWorkspaceSplitRatio,
} from "@/lib/workspaceTabs";
import type {
  AppRuntimeInfo,
  AppSettings,
  Group,
  PaneSplitDirection,
  SavedConnection,
  SessionPane,
  SessionType,
  SyncGroup,
  Tab,
  UiConfig,
} from "@/types/global";
import { invoke } from "../lib/invoke";
import { logger, setLoggerLevel } from "../lib/logger";
import { DEFAULT_TERMINAL_FONT_SIZE } from "../lib/terminalFontSize";
import { isPrimaryMainWindow } from "../lib/windowManager";

interface AppContextType {
  // Tabs
  tabs: Tab[];
  activeTabId: string | null;
  setActiveTabId: (id: string | null) => void;
  addTab: (
    sessionId: string,
    name: string,
    type: SessionType,
    connectionId?: string,
    extra?: Partial<Pick<Tab, "customName" | "tabColor">>,
    options?: { afterTabId?: string },
  ) => string;
  /** Immediately add a "connecting" tab and make it active. Returns the new tabId. */
  addPendingTab: (
    name: string,
    type: SessionType,
    connectionId?: string,
    extra?: Partial<Pick<Tab, "customName" | "tabColor">>,
    options?: { afterTabId?: string },
  ) => string;
  /** Swap the active pane's temporary sessionId for the real one and clear the connecting flag. */
  updateTabSession: (tabId: string, sessionId: string) => void;
  /** Mark the active pane in a tab as failed while keeping the tab visible. */
  markTabConnectionFailed: (tabId: string, error: string) => void;
  /** Update one specific pane's session binding. */
  updatePaneSession: (tabId: string, paneId: string, sessionId: string) => void;
  /** Mark a specific pane as failed while keeping the layout intact. */
  markPaneConnectionFailed: (tabId: string, paneId: string, error: string) => void;
  /** Put a specific pane back into connecting state, optionally refreshing its metadata first. */
  markPaneConnecting: (
    tabId: string,
    paneId: string,
    updates?: Partial<Pick<SessionPane, "name" | "type" | "connectionId">>,
  ) => void;
  setActivePane: (tabId: string, paneId: string) => void;
  updateSplitRatio: (tabId: string, splitId: string, ratio: number) => void;
  splitPane: (
    tabId: string,
    paneId: string,
    direction: PaneSplitDirection,
    pane: SessionPane,
    options?: { immediatePersist?: boolean },
  ) => string | null;
  closePane: (tabId: string, paneId: string, options?: { immediatePersist?: boolean }) => void;
  reorderTabs: (fromTabId: string, toIndex: number) => void;
  /** Update user-editable tab properties (customName, tabColor). */
  updateTab: (
    tabId: string,
    updates: Partial<Pick<Tab, "customName" | "tabColor">>,
    options?: { immediatePersist?: boolean },
  ) => Promise<void>;
  closeTabs: (
    tabIds: string[],
    options?: { immediatePersist?: boolean; nextActiveTabId?: string | null },
  ) => void;
  closeTab: (tabId: string) => void;
  persistTabsNow: () => Promise<void>;

  // App Settings (includes UI config)
  appSettings: AppSettings;
  updateAppSettings: (
    updates: Partial<AppSettings> | ((prev: AppSettings) => Partial<AppSettings>),
  ) => void;
  replaceAppSettings: (next: AppSettings) => void;
  updateUi: (updates: Partial<UiConfig> | ((prev: UiConfig) => Partial<UiConfig>)) => void;

  // Data
  savedConnections: SavedConnection[];
  savedGroups: Group[];
  refreshConnections: () => Promise<void>;
  recordRecentConnection: (connectionId: string) => void;

  // Dialogs
  showNewSession: boolean;
  setShowNewSession: (show: boolean) => void;
  editingConnection: SavedConnection | undefined;
  setEditingConnection: (conn: SavedConnection | undefined) => void;
  showSettingsDialog: boolean;
  setShowSettingsDialog: (show: boolean) => void;

  // Sync Input Groups
  syncGroups: SyncGroup[];
  setSyncGroups: (groups: SyncGroup[] | ((prev: SyncGroup[]) => SyncGroup[])) => void;
  broadcastToAll: boolean;
  setBroadcastToAll: (value: boolean | ((prev: boolean) => boolean)) => void;

  // Idle Lock
  isLocked: boolean;
  setIsLocked: (locked: boolean) => void;

  // Loading
  settingsLoaded: boolean;
  runtimeInfo: AppRuntimeInfo;
  runtimeInfoLoaded: boolean;
}

export type TerminalAppSettings = Pick<
  AppSettings,
  "appearance" | "interaction" | "terminal" | "translation" | "search" | "ai" | "keybindings"
>;

/**
 * App-wide state: tabs, settings (debounced save), saved connections (polled),
 * and dialog visibility. Updates via setState/useCallback; config persisted to backend.
 */
export const AppContext = createContext<AppContextType | null>(null);
const TerminalAppSettingsContext = createContext<TerminalAppSettings | null>(null);

const DEFAULT_APP_SETTINGS: AppSettings = {
  general: {
    startup_restore: true,
    minimize_to_tray: false,
    boss_key: null,
    confirm_on_close: true,
  },
  appearance: {
    theme: "github-dark",
    font_family: 'JetBrains Mono, Consolas, "Cascadia Mono", monospace',
    ui_font_family: 'JetBrains Mono, "Noto Sans SC Variable", Inter, sans-serif',
    font_size: DEFAULT_TERMINAL_FONT_SIZE,
    ligatures: false,
    background_opacity: 1.0,
    background_image_path: null,
    background_image_fit: "cover",
    background_image_opacity: 0.45,
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
      { name: "Google", url_template: "https://google.com/search?q=%s", show_in_menu: true },
      { name: "Bing", url_template: "https://bing.com/search?q=%s", show_in_menu: true },
      { name: "GitHub", url_template: "https://github.com/search?q=%s", show_in_menu: true },
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
    enable_screen_lock: false,
    idle_lock_minutes: 0,
    host_key_policy: "prompt",
  },
  terminal: {
    scrollback_lines: 10000,
    keep_alive_interval: 60,
    hardware_acceleration: false,
    keyword_highlights_enabled: false,
    keyword_highlights_across_wrapped_lines: false,
    keyword_highlight_builtin_rules: {},
    keyword_highlights: [],
    action_links_enabled: false,
    action_links_matchers: {
      ipv4: true,
      archive: true,
      host_port: true,
    },
    show_line_numbers: false,
    show_timestamps: false,
    show_multi_line_paste_dialog: true,
  },
  interaction: {
    copy_on_select: false,
    right_click_paste: false,
    command_suggestions_enabled: true,
    command_suggestion_min_chars: DEFAULT_COMMAND_SUGGESTION_MIN_CHARS,
    command_suggestion_max_chars: DEFAULT_COMMAND_SUGGESTION_MAX_CHARS,
    word_separators: " ()[]{}\"':=,;|&<>",
    default_encoding: "UTF-8",
  },
  transfer: {
    download_threads: 3,
    upload_threads: 3,
    duplicate_strategy: "overwrite",
    preserve_timestamps: true,
    resume_broken_transfer: true,
    default_file_permissions: "644",
    max_transfer_retries: 2,
    transfer_buffer_size: 32,
    download_path: "",
    ask_save_location: false,
    default_editor: "",
    recording_path: "",
    recording_include_io_labels: true,
    recording_include_timestamps: true,
    recording_memory_limit_bytes: 5 * 1024 * 1024,
  },
  diagnostics: {
    level: "info",
    retention_days: 7,
  },
  ai: {
    ...DEFAULT_AI_SETTINGS,
  },
  cloud_sync: DEFAULT_CLOUD_SYNC_SETTINGS,
  ui: {
    open_tabs: [],
    left_width: 256,
    right_width: 288,
    quick_cmd_height: 180,
    quick_cmd_view_mode: "list",
    active_left_panel: "fileExplorer",
    active_right_panel: "savedConnections",
    show_quick_cmd_bar: true,
    show_serial_send_panel: false,
    serial_send_height: 180,
    zoom_level: 1.0,
    language: "en",
    show_remote_stats: true,
    remote_stats_interval: 3,
    saved_connections_sort_mode: "default",
    recent_connection_ids: [],
    transfer_height: 180,
    file_explorer_auto_sync_cwd_connection_ids: [],
    file_explorer_favorite_dirs_by_connection_id: {},
    activity_bar_layout: {
      left_top: ["fileExplorer", "network", "securityAuth"],
      left_bottom: ["syncBackupHistory", "settings"],
      right_top: [
        "savedConnections",
        "aiAssistant",
        "activeSessions",
        "commandHistory",
        "resourceMonitor",
      ],
      right_bottom: ["quickCmdBar", "serialSend", "recording", "lock"],
      show_labels: false,
    },
  },
  keybindings: {},
};

const RECENT_CONNECTION_LIMIT = 10;

const DEFAULT_RUNTIME_INFO: AppRuntimeInfo = {
  portable: false,
  mode: "installed",
  executableDir: "",
  dataDir: "",
  configDir: "",
  logDir: "",
  webviewDataDir: "",
  portableMarkerPath: null,
};

function areSettingsValuesEqual(left: unknown, right: unknown): boolean {
  if (Object.is(left, right)) return true;
  if (typeof left !== typeof right) return false;
  if (left === null || right === null) return left === right;

  if (Array.isArray(left) || Array.isArray(right)) {
    if (!Array.isArray(left) || !Array.isArray(right) || left.length !== right.length) return false;
    for (let index = 0; index < left.length; index += 1) {
      if (!areSettingsValuesEqual(left[index], right[index])) {
        return false;
      }
    }
    return true;
  }

  if (typeof left !== "object" || typeof right !== "object") {
    return false;
  }

  const leftRecord = left as Record<string, unknown>;
  const rightRecord = right as Record<string, unknown>;
  const leftKeys = Object.keys(leftRecord);
  const rightKeys = Object.keys(rightRecord);
  if (leftKeys.length !== rightKeys.length) return false;

  for (const key of leftKeys) {
    if (!(key in rightRecord)) return false;
    if (!areSettingsValuesEqual(leftRecord[key], rightRecord[key])) {
      return false;
    }
  }

  return true;
}

function preserveAppSettingsReferences(prev: AppSettings, next: AppSettings): AppSettings {
  const general = areSettingsValuesEqual(prev.general, next.general) ? prev.general : next.general;
  const appearance = areSettingsValuesEqual(prev.appearance, next.appearance)
    ? prev.appearance
    : next.appearance;
  const proxy = areSettingsValuesEqual(prev.proxy, next.proxy) ? prev.proxy : next.proxy;
  const search = areSettingsValuesEqual(prev.search, next.search) ? prev.search : next.search;
  const translation = areSettingsValuesEqual(prev.translation, next.translation)
    ? prev.translation
    : next.translation;
  const security = areSettingsValuesEqual(prev.security, next.security)
    ? prev.security
    : next.security;
  const terminal = areSettingsValuesEqual(prev.terminal, next.terminal)
    ? prev.terminal
    : next.terminal;
  const interaction = areSettingsValuesEqual(prev.interaction, next.interaction)
    ? prev.interaction
    : next.interaction;
  const transfer = areSettingsValuesEqual(prev.transfer, next.transfer)
    ? prev.transfer
    : next.transfer;
  const diagnostics = areSettingsValuesEqual(prev.diagnostics, next.diagnostics)
    ? prev.diagnostics
    : next.diagnostics;
  const ai = areSettingsValuesEqual(prev.ai, next.ai) ? prev.ai : next.ai;
  const cloudSync = areSettingsValuesEqual(prev.cloud_sync, next.cloud_sync)
    ? prev.cloud_sync
    : next.cloud_sync;
  const ui = areSettingsValuesEqual(prev.ui, next.ui) ? prev.ui : next.ui;
  const keybindings = areSettingsValuesEqual(prev.keybindings, next.keybindings)
    ? prev.keybindings
    : next.keybindings;

  if (
    general === prev.general &&
    appearance === prev.appearance &&
    proxy === prev.proxy &&
    search === prev.search &&
    translation === prev.translation &&
    security === prev.security &&
    terminal === prev.terminal &&
    interaction === prev.interaction &&
    transfer === prev.transfer &&
    diagnostics === prev.diagnostics &&
    ai === prev.ai &&
    cloudSync === prev.cloud_sync &&
    ui === prev.ui &&
    keybindings === prev.keybindings
  ) {
    return prev;
  }

  return {
    ...next,
    general,
    appearance,
    proxy,
    search,
    translation,
    security,
    terminal,
    interaction,
    transfer,
    diagnostics,
    ai,
    cloud_sync: cloudSync,
    ui,
    keybindings,
  };
}

/** Provides tabs, appSettings, savedConnections, and dialog state to the app. */
export function AppProvider({ children }: { children: ReactNode }) {
  // Tabs State
  const [tabs, setTabs] = useState<Tab[]>([]);
  const tabsRef = useRef<Tab[]>([]);
  const [activeTabIdState, setActiveTabIdState] = useState<string | null>(null);
  const activeTabIdRef = useRef<string | null>(null);

  // App Settings State (includes UI config)
  const [appSettings, setAppSettings] = useState<AppSettings>(DEFAULT_APP_SETTINGS);
  const appSettingsRef = useRef<AppSettings>(DEFAULT_APP_SETTINGS);
  const appSettingsLoaded = useRef(false);
  const appSettingsSaveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const uiSaveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Data State
  const [savedConnections, setSavedConnections] = useState<SavedConnection[]>([]);
  const [savedGroups, setSavedGroups] = useState<Group[]>([]);

  // Dialog State
  const [showNewSession, setShowNewSession] = useState(false);
  const [editingConnection, setEditingConnection] = useState<SavedConnection | undefined>(
    undefined,
  );
  const [showSettingsDialog, setShowSettingsDialog] = useState(false);

  // Sync Input Groups
  const [syncGroups, setSyncGroups] = useState<SyncGroup[]>([]);
  const [broadcastToAll, setBroadcastToAll] = useState(false);

  // Idle Lock State
  const [isLocked, setIsLocked] = useState(false);

  // Loading State
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const [runtimeInfo, setRuntimeInfo] = useState<AppRuntimeInfo>(DEFAULT_RUNTIME_INFO);
  const [runtimeInfoLoaded, setRuntimeInfoLoaded] = useState(false);

  const setActiveTabId = useCallback((id: string | null) => {
    activeTabIdRef.current = id;
    setActiveTabIdState(id);
  }, []);

  // 1. Load App Settings
  useEffect(() => {
    invoke<AppRuntimeInfo>("get_app_runtime_info")
      .then((info) => {
        setRuntimeInfo(info);
      })
      .catch((error) => {
        logger.error({
          domain: "app.lifecycle",
          event: "runtime_info.load_failed",
          message: "Failed to load app runtime info",
          error,
        });
      })
      .finally(() => {
        setRuntimeInfoLoaded(true);
      });

    invoke<AppSettings>("get_app_settings")
      .then((cfg) => {
        appSettingsRef.current = cfg;
        setAppSettings(cfg);
        setLoggerLevel(cfg.diagnostics.level);
        appSettingsLoaded.current = true;
        setSettingsLoaded(true);
        if (cfg.security?.enable_screen_lock) {
          setIsLocked(true);
        }
      })
      .catch(() => {
        appSettingsRef.current = DEFAULT_APP_SETTINGS;
        appSettingsLoaded.current = true;
        setAppSettings(DEFAULT_APP_SETTINGS);
        setSettingsLoaded(true);
      });
  }, []);

  // Apply UI font size to root element
  useEffect(() => {
    document.documentElement.style.fontSize = `${appSettings.appearance.ui_font_size}px`;
  }, [appSettings.appearance.ui_font_size]);

  useEffect(() => {
    const fontFamily = appSettings.appearance.ui_font_family;
    document.documentElement.style.setProperty("--font-sans", fontFamily);
    document.documentElement.style.setProperty("--font-display", fontFamily);
  }, [appSettings.appearance.ui_font_family]);

  // 2. Save App Settings Debounced
  const updateAppSettings = useCallback(
    (updates: Partial<AppSettings> | ((prev: AppSettings) => Partial<AppSettings>)) => {
      setAppSettings((prev) => {
        const nextUpdates = typeof updates === "function" ? updates(prev) : updates;
        const next = { ...prev, ...nextUpdates };
        appSettingsRef.current = next;
        setLoggerLevel(next.diagnostics.level);
        if (appSettingsLoaded.current) {
          if (appSettingsSaveTimerRef.current) clearTimeout(appSettingsSaveTimerRef.current);
          appSettingsSaveTimerRef.current = setTimeout(() => {
            invoke("save_app_settings", { settings: next }).catch((e) =>
              logger.error({
                domain: "settings.persistence",
                event: "settings.save_failed",
                message: "Failed to save app settings",
                error: e,
              }),
            );
          }, 500);
        }
        return next;
      });
    },
    [],
  );

  const replaceAppSettings = useCallback((next: AppSettings) => {
    if (appSettingsSaveTimerRef.current) {
      clearTimeout(appSettingsSaveTimerRef.current);
      appSettingsSaveTimerRef.current = null;
    }
    setAppSettings((current) => {
      const normalized = preserveAppSettingsReferences(current, next);
      appSettingsRef.current = normalized;
      setLoggerLevel(normalized.diagnostics.level);
      return normalized;
    });
  }, []);

  // Convenience helper to update just the UI config portion via lightweight path
  const updateUi = useCallback(
    (updates: Partial<UiConfig> | ((prev: UiConfig) => Partial<UiConfig>)) => {
      setAppSettings((prev) => {
        const nextUpdates = typeof updates === "function" ? updates(prev.ui) : updates;
        const nextUi = { ...prev.ui, ...nextUpdates };
        const next = { ...prev, ui: nextUi };
        appSettingsRef.current = next;
        if (appSettingsLoaded.current) {
          if (uiSaveTimerRef.current) clearTimeout(uiSaveTimerRef.current);
          uiSaveTimerRef.current = setTimeout(() => {
            invoke("save_app_ui_settings", { ui: nextUi }).catch((e) =>
              logger.error({
                domain: "settings.persistence",
                event: "ui_settings.save_failed",
                message: "Failed to save UI settings",
                error: e,
              }),
            );
          }, 500);
        }
        return next;
      });
    },
    [],
  );

  const recordRecentConnection = useCallback(
    (connectionId: string) => {
      if (!connectionId) return;
      updateUi((prev) => ({
        recent_connection_ids: [
          connectionId,
          ...(prev.recent_connection_ids ?? []).filter((id) => id !== connectionId),
        ].slice(0, RECENT_CONNECTION_LIMIT),
      }));
    },
    [updateUi],
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
      logger.error({
        domain: "ui.error",
        event: "connections.fetch_failed",
        message: "Failed to fetch connections",
        error: e,
      });
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

  const syncOpenTabs = useCallback(
    async (nextTabs: Tab[], options?: { immediatePersist?: boolean }) => {
      if (!hasRestored.current || !appSettingsRef.current.general.startup_restore) return;

      const openTabs = serializeTabsForPersistence(nextTabs);
      updateUi({ open_tabs: openTabs });

      if (!options?.immediatePersist) return;

      const nextUi = { ...appSettingsRef.current.ui, open_tabs: openTabs };
      appSettingsRef.current = { ...appSettingsRef.current, ui: nextUi };
      await invoke("save_app_ui_settings", { ui: nextUi });
    },
    [updateUi],
  );

  const commitTabs = useCallback(
    async (
      nextTabs: Tab[],
      options?: {
        syncPersisted?: boolean;
        immediatePersist?: boolean;
      },
    ) => {
      const normalizedTabs = nextTabs.map(ensureActivePane);
      tabsRef.current = normalizedTabs;
      setTabs(normalizedTabs);

      if (options?.syncPersisted === false) return;
      await syncOpenTabs(normalizedTabs, { immediatePersist: options?.immediatePersist });
    },
    [syncOpenTabs],
  );

  // 4. Tab Logic
  const addTab = useCallback(
    (
      sessionId: string,
      name: string,
      type: SessionType,
      connectionId?: string,
      extra?: Partial<Pick<Tab, "customName" | "tabColor">>,
      options?: { afterTabId?: string },
    ) => {
      const pane = createSessionPane(name, type, connectionId, { sessionId });
      const newTab = createWorkspaceTab(pane, getNextPersistOrder(tabsRef.current), extra);
      const nextTabs = options?.afterTabId
        ? insertTabAfter(tabsRef.current, options.afterTabId, newTab)
        : [...tabsRef.current, newTab];
      void commitTabs(nextTabs);
      setActiveTabId(newTab.id);

      // Close dialogs when session starts
      setShowNewSession(false);
      setEditingConnection(undefined);
      return newTab.id;
    },
    [commitTabs, setActiveTabId],
  );

  const addPendingTab = useCallback(
    (
      name: string,
      type: SessionType,
      connectionId?: string,
      extra?: Partial<Pick<Tab, "customName" | "tabColor">>,
      options?: { afterTabId?: string },
    ): string => {
      const pane = createSessionPane(name, type, connectionId, { connecting: true });
      const newTab = createWorkspaceTab(pane, getNextPersistOrder(tabsRef.current), extra);
      const nextTabs = options?.afterTabId
        ? insertTabAfter(tabsRef.current, options.afterTabId, newTab)
        : [...tabsRef.current, newTab];
      void commitTabs(nextTabs);
      setActiveTabId(newTab.id);
      return newTab.id;
    },
    [commitTabs, setActiveTabId],
  );

  const updateTabSession = useCallback(
    (tabId: string, sessionId: string) => {
      const tab = tabsRef.current.find((item) => item.id === tabId);
      if (!tab) return;
      const paneId = tab.activePaneId;
      const nextTabs = tabsRef.current.map((item) =>
        item.id === tabId
          ? {
              ...item,
              root: updateSessionPane(item.root, paneId, {
                sessionId,
                connecting: false,
                connectError: undefined,
              }),
            }
          : item,
      );
      void commitTabs(nextTabs);
    },
    [commitTabs],
  );

  const markTabConnectionFailed = useCallback(
    (tabId: string, error: string) => {
      const tab = tabsRef.current.find((item) => item.id === tabId);
      if (!tab) return;
      const paneId = tab.activePaneId;
      const nextTabs = tabsRef.current.map((item) =>
        item.id === tabId
          ? {
              ...item,
              root: updateSessionPane(item.root, paneId, {
                connecting: false,
                connectError: error,
              }),
            }
          : item,
      );
      void commitTabs(nextTabs);
    },
    [commitTabs],
  );

  const updatePaneSession = useCallback(
    (tabId: string, paneId: string, sessionId: string) => {
      const nextTabs = tabsRef.current.map((tab) =>
        tab.id === tabId
          ? {
              ...tab,
              root: updateSessionPane(tab.root, paneId, {
                sessionId,
                connecting: false,
                connectError: undefined,
              }),
            }
          : tab,
      );
      void commitTabs(nextTabs);
    },
    [commitTabs],
  );

  const markPaneConnectionFailed = useCallback(
    (tabId: string, paneId: string, error: string) => {
      const nextTabs = tabsRef.current.map((tab) =>
        tab.id === tabId
          ? {
              ...tab,
              root: updateSessionPane(tab.root, paneId, {
                connecting: false,
                connectError: error,
              }),
            }
          : tab,
      );
      void commitTabs(nextTabs);
    },
    [commitTabs],
  );

  const markPaneConnecting = useCallback(
    (
      tabId: string,
      paneId: string,
      updates?: Partial<Pick<SessionPane, "name" | "type" | "connectionId">>,
    ) => {
      const nextTabs = tabsRef.current.map((tab) =>
        tab.id === tabId
          ? {
              ...tab,
              root: updateSessionPane(tab.root, paneId, {
                ...updates,
                connecting: true,
                connectError: undefined,
              }),
            }
          : tab,
      );
      void commitTabs(nextTabs);
    },
    [commitTabs],
  );

  const setActivePane = useCallback(
    (tabId: string, paneId: string) => {
      const nextTabs = tabsRef.current.map((tab) =>
        tab.id === tabId ? ensureActivePane({ ...tab, activePaneId: paneId }) : tab,
      );
      void commitTabs(nextTabs);
      setActiveTabId(tabId);
    },
    [commitTabs, setActiveTabId],
  );

  const splitPane = useCallback(
    (
      tabId: string,
      paneId: string,
      direction: PaneSplitDirection,
      pane: SessionPane,
      options?: { immediatePersist?: boolean },
    ) => {
      const tab = tabsRef.current.find((item) => item.id === tabId);
      if (!tab) return null;

      const nextTabs = tabsRef.current.map((item) =>
        item.id === tabId
          ? ensureActivePane({
              ...item,
              activePaneId: pane.id,
              root: splitSessionPane(item.root, paneId, direction, pane),
            })
          : item,
      );
      void commitTabs(nextTabs, { immediatePersist: options?.immediatePersist });
      setActiveTabId(tabId);
      return pane.id;
    },
    [commitTabs, setActiveTabId],
  );

  const updateSplitRatio = useCallback(
    (tabId: string, splitId: string, ratio: number) => {
      const nextTabs = tabsRef.current.map((tab) =>
        tab.id === tabId
          ? {
              ...tab,
              root: updateWorkspaceSplitRatio(tab.root, splitId, ratio),
            }
          : tab,
      );
      void commitTabs(nextTabs);
    },
    [commitTabs],
  );

  const closePane = useCallback(
    (tabId: string, paneId: string, options?: { immediatePersist?: boolean }) => {
      const currentTabs = tabsRef.current;
      const index = currentTabs.findIndex((item) => item.id === tabId);
      if (index === -1) return;

      const tab = currentTabs[index];
      const nextRoot = removeSessionPane(tab.root, paneId);

      if (!nextRoot) {
        const nextTabs = currentTabs.filter((item) => item.id !== tabId);
        if (activeTabIdRef.current === tabId) {
          const fallback = nextTabs[Math.max(0, index - 1)] ?? nextTabs[0] ?? null;
          setActiveTabId(fallback?.id ?? null);
        }
        void commitTabs(nextTabs, { immediatePersist: options?.immediatePersist });
        return;
      }

      const nextActivePaneId =
        tab.activePaneId === paneId
          ? (getFirstSessionPane(nextRoot)?.id ?? tab.activePaneId)
          : tab.activePaneId;

      const nextTabs = currentTabs.map((item) =>
        item.id === tabId
          ? ensureActivePane({
              ...item,
              activePaneId: nextActivePaneId,
              root: nextRoot,
            })
          : item,
      );
      void commitTabs(nextTabs, { immediatePersist: options?.immediatePersist });
    },
    [commitTabs, setActiveTabId],
  );

  const updateTab = useCallback(
    async (
      tabId: string,
      updates: Partial<Pick<Tab, "customName" | "tabColor">>,
      options?: { immediatePersist?: boolean },
    ) => {
      const nextTabs = tabsRef.current.map((tab) =>
        tab.id === tabId ? { ...tab, ...updates } : tab,
      );
      await commitTabs(nextTabs, { immediatePersist: options?.immediatePersist });
    },
    [commitTabs],
  );

  const closeTabs = useCallback(
    (
      tabIds: string[],
      options?: { immediatePersist?: boolean; nextActiveTabId?: string | null },
    ) => {
      if (tabIds.length === 0) return;

      const idsToClose = new Set(tabIds);
      const currentTabs = tabsRef.current;
      const nextTabs = currentTabs.filter((tab) => !idsToClose.has(tab.id));
      const currentActiveTabId = activeTabIdRef.current;

      let nextActiveTabId =
        options?.nextActiveTabId !== undefined ? options.nextActiveTabId : currentActiveTabId;

      if (nextActiveTabId && !nextTabs.some((tab) => tab.id === nextActiveTabId)) {
        nextActiveTabId = null;
      }

      if (!nextActiveTabId && currentActiveTabId && idsToClose.has(currentActiveTabId)) {
        const activeIndex = currentTabs.findIndex((tab) => tab.id === currentActiveTabId);
        const fallbackTab = nextTabs[Math.max(0, activeIndex - 1)] ?? nextTabs[0] ?? null;
        nextActiveTabId = fallbackTab?.id ?? null;
      }

      if (!nextActiveTabId && nextTabs.length > 0) {
        nextActiveTabId = nextTabs[0].id;
      }

      if (nextActiveTabId !== currentActiveTabId) {
        setActiveTabId(nextActiveTabId);
      }

      void commitTabs(nextTabs, { immediatePersist: options?.immediatePersist });
    },
    [commitTabs, setActiveTabId],
  );

  const closeTab = useCallback(
    (tabId: string) => {
      closeTabs([tabId]);
    },
    [closeTabs],
  );

  const reorderTabs = useCallback(
    (fromTabId: string, toIndex: number) => {
      const nextTabs = moveTab(tabsRef.current, fromTabId, toIndex);
      void commitTabs(nextTabs, { syncPersisted: false });
    },
    [commitTabs],
  );

  const persistTabsNow = useCallback(async () => {
    if (!hasRestored.current || !appSettingsRef.current.general.startup_restore) return;
    const nextUi = {
      ...appSettingsRef.current.ui,
      open_tabs: serializeTabsForPersistence(tabsRef.current),
    };
    appSettingsRef.current = { ...appSettingsRef.current, ui: nextUi };
    await invoke("save_app_ui_settings", { ui: nextUi });
  }, []);

  // 5. Startup Restore Logic
  const hasRestored = useRef(false);

  useEffect(() => {
    if (!hasRestored.current && appSettingsLoaded.current) {
      hasRestored.current = true;
      if (
        isPrimaryMainWindow() &&
        appSettings.general.startup_restore &&
        appSettings.ui.open_tabs &&
        appSettings.ui.open_tabs.length > 0
      ) {
        const restoredTabs = appSettings.ui.open_tabs
          .map((tab, index) => restoreTabFromPersistence(tab, index))
          .filter((tab): tab is Tab => tab !== null);

        tabsRef.current = restoredTabs;
        setTabs(restoredTabs);
        if (restoredTabs.length > 0) {
          setActiveTabId(restoredTabs[restoredTabs.length - 1].id);
        }

        restoredTabs.forEach((tab) => {
          const panes = collectSessionPanes(tab.root);

          panes.forEach((pane) => {
            const cid = pane.connectionId;
            switch (pane.type) {
              case "SSH":
                if (!cid) {
                  markPaneConnectionFailed(tab.id, pane.id, "Missing SSH connection id");
                  return;
                }
                invoke<string>("create_ssh_session", { connectionId: cid })
                  .then((sessionId) => updatePaneSession(tab.id, pane.id, sessionId))
                  .catch((e) => {
                    logger.error({
                      domain: "session.lifecycle",
                      event: "session.restore_failed",
                      message: "Restore SSH failed",
                      ids: pane.connectionId ? { connection_id: pane.connectionId } : undefined,
                      data: {
                        session_type: "SSH",
                        pane_id: pane.id,
                      },
                      error: e,
                    });
                    markPaneConnectionFailed(tab.id, pane.id, getErrorMessage(e));
                  });
                break;
              case "Local":
                invoke<string>("create_local_session", { connectionId: cid || null })
                  .then((sessionId) => updatePaneSession(tab.id, pane.id, sessionId))
                  .catch((e) => {
                    logger.error({
                      domain: "session.lifecycle",
                      event: "session.restore_failed",
                      message: "Restore Local failed",
                      data: {
                        session_type: "Local",
                        pane_id: pane.id,
                      },
                      error: e,
                    });
                    markPaneConnectionFailed(tab.id, pane.id, getErrorMessage(e));
                  });
                break;
              case "Telnet":
                if (!cid) {
                  markPaneConnectionFailed(tab.id, pane.id, "Missing Telnet connection id");
                  return;
                }
                invoke<string>("create_telnet_session", { connectionId: cid })
                  .then((sessionId) => updatePaneSession(tab.id, pane.id, sessionId))
                  .catch((e) => {
                    logger.error({
                      domain: "session.lifecycle",
                      event: "session.restore_failed",
                      message: "Restore Telnet failed",
                      ids: pane.connectionId ? { connection_id: pane.connectionId } : undefined,
                      data: {
                        session_type: "Telnet",
                        pane_id: pane.id,
                      },
                      error: e,
                    });
                    markPaneConnectionFailed(tab.id, pane.id, getErrorMessage(e));
                  });
                break;
              case "Serial":
                if (!cid) {
                  markPaneConnectionFailed(tab.id, pane.id, "Missing Serial connection id");
                  return;
                }
                invoke<string>("create_serial_session", { connectionId: cid })
                  .then((sessionId) => updatePaneSession(tab.id, pane.id, sessionId))
                  .catch((e) => {
                    logger.error({
                      domain: "session.lifecycle",
                      event: "session.restore_failed",
                      message: "Restore Serial failed",
                      ids: pane.connectionId ? { connection_id: pane.connectionId } : undefined,
                      data: {
                        session_type: "Serial",
                        pane_id: pane.id,
                      },
                      error: e,
                    });
                    markPaneConnectionFailed(tab.id, pane.id, getErrorMessage(e));
                  });
                break;
            }
          });
        });
      }
    }
  }, [appSettings, markPaneConnectionFailed, setActiveTabId, updatePaneSession]);

  const contextValue = useMemo(
    () => ({
      tabs,
      activeTabId: activeTabIdState,
      setActiveTabId,
      addTab,
      addPendingTab,
      updateTabSession,
      markTabConnectionFailed,
      updatePaneSession,
      markPaneConnectionFailed,
      markPaneConnecting,
      setActivePane,
      updateSplitRatio,
      splitPane,
      closePane,
      reorderTabs,
      updateTab,
      closeTabs,
      closeTab,
      persistTabsNow,
      appSettings,
      updateAppSettings,
      replaceAppSettings,
      updateUi,
      savedConnections,
      savedGroups,
      refreshConnections,
      recordRecentConnection,
      showNewSession,
      setShowNewSession,
      editingConnection,
      setEditingConnection,
      showSettingsDialog,
      setShowSettingsDialog,
      syncGroups,
      setSyncGroups,
      broadcastToAll,
      setBroadcastToAll,
      isLocked,
      setIsLocked,
      settingsLoaded,
      runtimeInfo,
      runtimeInfoLoaded,
    }),
    [
      tabs,
      activeTabIdState,
      setActiveTabId,
      addTab,
      addPendingTab,
      updateTabSession,
      markTabConnectionFailed,
      updatePaneSession,
      markPaneConnectionFailed,
      markPaneConnecting,
      setActivePane,
      updateSplitRatio,
      splitPane,
      closePane,
      reorderTabs,
      updateTab,
      closeTabs,
      closeTab,
      persistTabsNow,
      appSettings,
      updateAppSettings,
      replaceAppSettings,
      updateUi,
      savedConnections,
      savedGroups,
      refreshConnections,
      recordRecentConnection,
      showNewSession,
      editingConnection,
      showSettingsDialog,
      syncGroups,
      broadcastToAll,
      isLocked,
      settingsLoaded,
      runtimeInfo,
      runtimeInfoLoaded,
    ],
  );

  const terminalAppSettingsValue = useMemo(
    () => ({
      appearance: appSettings.appearance,
      interaction: appSettings.interaction,
      terminal: appSettings.terminal,
      translation: appSettings.translation,
      search: appSettings.search,
      ai: appSettings.ai,
      keybindings: appSettings.keybindings,
    }),
    [
      appSettings.appearance,
      appSettings.interaction,
      appSettings.terminal,
      appSettings.translation,
      appSettings.search,
      appSettings.ai,
      appSettings.keybindings,
    ],
  );

  return (
    <AppContext.Provider value={contextValue}>
      <TerminalAppSettingsContext.Provider value={terminalAppSettingsValue}>
        {children}
      </TerminalAppSettingsContext.Provider>
    </AppContext.Provider>
  );
}

/** Hook to access AppContext. Throws if used outside AppProvider. */
export function useApp() {
  const context = useContext(AppContext);
  if (!context) throw new Error("useApp must be used within AppProvider");
  return context;
}

export function useTerminalAppSettings(): TerminalAppSettings {
  const context = useContext(TerminalAppSettingsContext);
  if (!context) {
    throw new Error("useTerminalAppSettings must be used within AppProvider");
  }
  return context;
}
