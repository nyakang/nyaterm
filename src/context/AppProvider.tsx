import { listen } from "@tauri-apps/api/event";
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useAppLockState } from "@/hooks/useAppLockState";
import { DEFAULT_AI_SETTINGS } from "@/lib/aiSettings";
import { DEFAULT_CLOUD_SYNC_SETTINGS } from "@/lib/cloudSync";
import { updateConnectionAutoIconAfterSessionStart } from "@/lib/connectionAutoIcon";
import { DEFAULT_TERMINAL_FONT_FAMILY, getDefaultUiFontFamily } from "@/lib/defaultFonts";
import { getErrorMessage } from "@/lib/errors";
import { cloneDefaultActivityBarLayout } from "@/lib/appWorkspace";
import {
  DEFAULT_COMMAND_SUGGESTION_MAX_CHARS,
  DEFAULT_COMMAND_SUGGESTION_MIN_CHARS,
  DEFAULT_TAB_DOUBLE_CLICK_ACTION,
  DEFAULT_TAB_MIDDLE_CLICK_ACTION,
  DEFAULT_TAB_RIGHT_CLICK_ACTION,
} from "@/lib/interactionSettings";
import {
  normalizeQuickCommandAppSettings,
  normalizeQuickCommandUiConfig,
} from "@/lib/quickCommandSettings";
import {
  collectSessionPanes,
  createFileDocumentPane,
  createSessionPane,
  createWorkspaceTab,
  ensureActivePane,
  findOpenFileDocument,
  findSessionPaneById,
  getFirstSessionPane,
  getNextPersistOrder,
  insertTabAfter,
  moveTab,
  removeSessionPane,
  replaceSessionReferences as replacePaneSessionReferences,
  restoreTabFromPersistence,
  serializeTabsForPersistence,
  splitSessionPane,
  updateSessionPane,
  updateSplitRatio as updateWorkspaceSplitRatio,
} from "@/lib/workspaceTabs";
import type {
  AppRuntimeInfo,
  AppSettings,
  FileDocumentBackend,
  FileDocumentSnapshot,
  Group,
  PaneSplitDirection,
  SavedConnection,
  SessionPane,
  SessionType,
  SyncGroup,
  Tab,
  UiConfig,
  WorkspaceSessionType,
} from "@/types/global";
import { invoke } from "../lib/invoke";
import { logger, setLoggerLevel } from "../lib/logger";
import { DEFAULT_TERMINAL_FONT_SIZE } from "../lib/terminalFontSize";
import { getOwnerMainWindowLabel, isPrimaryMainWindow } from "../lib/windowManager";
import {
  AppContext,
  type PaneConnectingUpdates,
  type PendingTabCreation,
  TerminalAppSettingsContext,
} from "./AppContext";

function createSessionRequestId() {
  return crypto.randomUUID();
}

const DEFAULT_APP_SETTINGS: AppSettings = {
  general: {
    startup_restore: true,
    startup_restore_window_layout: true,
    minimize_to_tray: false,
    boss_key: null,
    confirm_on_close: true,
  },
  appearance: {
    theme: "github-dark",
    custom_themes: [],
    font_family: DEFAULT_TERMINAL_FONT_FAMILY,
    ui_font_family: getDefaultUiFontFamily(),
    font_size: DEFAULT_TERMINAL_FONT_SIZE,
    font_weight: 400,
    font_weight_bold: 700,
    background_opacity: 1.0,
    background_image_path: null,
    background_image_fit: "cover",
    background_image_opacity: 0.45,
    cursor_style: "block",
    cursor_blink: true,
    ui_font_size: 16,
    terminal_theme: null,
    minimum_contrast_ratio: 1,
    panel_multi_open: false,
    window_transparency: "none",
    window_transparency_tint: 1,
    window_transparency_blur: false,
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
    enable_startup_lock: false,
    enable_idle_lock: false,
    idle_lock_minutes: 0,
    host_key_policy: "prompt",
  },
  terminal: {
    scrollback_lines: 10000,
    keep_alive_mode: "compatible",
    keep_alive_interval: 60,
    font_size_delta: 0,
    x11_display: "",
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
    show_workspace_padding: false,
    show_line_numbers: false,
    show_timestamps: false,
    timestamp_format: "[HH:mm:ss]",
    show_multi_line_paste_dialog: true,
    paste_image_as_path: true,
    reconnect_restore_cwd: false,
  },
  interaction: {
    copy_on_select: false,
    allow_osc52_clipboard_write: false,
    terminal_right_click_action: "menu",
    terminal_zoom_enabled: true,
    command_suggestions_enabled: true,
    command_suggestion_min_chars: DEFAULT_COMMAND_SUGGESTION_MIN_CHARS,
    command_suggestion_max_chars: DEFAULT_COMMAND_SUGGESTION_MAX_CHARS,
    duplicate_session_command_delay_ms: 1000,
    word_separators: " ()[]{}\"':=,;|&<>",
    alt_as_meta: false,
    ime_compatibility: false,
    default_encoding: "UTF-8",
    tab_double_click_action: DEFAULT_TAB_DOUBLE_CLICK_ACTION,
    tab_middle_click_action: DEFAULT_TAB_MIDDLE_CLICK_ACTION,
    tab_right_click_action: DEFAULT_TAB_RIGHT_CLICK_ACTION,
  },
  recording: {
    auto_start: false,
    default_mode: "transcript",
    base_path: "",
    path_template: "{group}/{session}/{yyyy}-{MM}-{dd}/{HH}-{mm}-{ss}-{SSS}-{session_short_id}.log",
    include_timestamps: true,
    include_io_labels: true,
    include_session_metadata: true,
    rotation: { type: "session" },
    existing_file_behavior: "unique",
    memory_limit_bytes: 5 * 1024 * 1024,
    include_binary_transfer_payloads: false,
  },
  transfer: {
    editor_type: "external",
    internal_editor_display: "workspace",
    internal_editor_font_size: 13,
    download_threads: 3,
    upload_threads: 3,
    duplicate_strategy: "ask",
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
    recording_auto_start: false,
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
    terminal_window_layout: null,
    start_workspace_mode: "workbench",
    panel_open_mode: "docked",
    left_width: 256,
    right_width: 288,
    quick_cmd_height: 180,
    quick_cmd_category_width: 176,
    quick_cmd_view_mode: "tile",
    quick_cmd_sort_mode: "created",
    quick_cmd_selected_category: "all",
    active_left_panel: "fileExplorer",
    active_right_panel: "savedConnections",
    left_open_panels: [],
    right_open_panels: [],
    panel_stack_sizes: {},
    network_panel_active_tab: "tunnel",
    security_auth_panel_active_tab: "keys",
    show_quick_cmd_bar: true,
    show_serial_send_panel: false,
    serial_send_height: 180,
    serial_send_clear_after_send: false,
    zoom_level: 1.0,
    language: "en",
    header_status_mode: "session",
    header_status_visible: true,
    show_notes_panel: true,
    show_remote_stats: true,
    remote_stats_interval: 3,
    show_gpu_monitor: false,
    gpu_monitor_interval: 3,
    show_ascend_npu_monitor: false,
    ascend_npu_monitor_interval: 3,
    show_process_manager: false,
    process_manager_interval: 5,
    show_docker_manager: false,
    docker_manager_interval: 10,
    saved_connections_sort_mode: "default",
    saved_connections_expanded_group_ids: [],
    asset_sort_key: null,
    asset_sort_direction: null,
    recent_connection_ids: [],
    transfer_height: 180,
    file_explorer_show_hidden_files: true,
    file_explorer_auto_sync_cwd_connection_ids: [],
    file_explorer_favorite_dirs_by_connection_id: {},
    notes_expanded_folder_ids: [],
    notes_last_selected_node_id: null,
    activity_bar_layout: cloneDefaultActivityBarLayout(),
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
  const { isLocked, setIsLocked, lockStateLoaded } = useAppLockState();

  // Loading State
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const [startupRestoreComplete, setStartupRestoreComplete] = useState(false);
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
        const normalized = normalizeQuickCommandAppSettings(cfg);
        appSettingsRef.current = normalized;
        setAppSettings(normalized);
        setLoggerLevel(normalized.diagnostics.level);
        appSettingsLoaded.current = true;
        setSettingsLoaded(true);
        if (isPrimaryMainWindow() && normalized.security?.enable_startup_lock) {
          setIsLocked(true);
        }
      })
      .catch(() => {
        appSettingsRef.current = DEFAULT_APP_SETTINGS;
        appSettingsLoaded.current = true;
        setAppSettings(DEFAULT_APP_SETTINGS);
        setSettingsLoaded(true);
      });
  }, [setIsLocked]);

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
        const next = normalizeQuickCommandAppSettings({
          ...prev,
          ...nextUpdates,
        });
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
      const normalized = preserveAppSettingsReferences(
        current,
        normalizeQuickCommandAppSettings(next),
      );
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
        const nextUi = normalizeQuickCommandUiConfig({
          ...prev.ui,
          ...nextUpdates,
        });
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
      type: WorkspaceSessionType,
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
      type: WorkspaceSessionType,
      connectionId?: string,
      extra?: Partial<Pick<Tab, "customName" | "tabColor">>,
      options?: { afterTabId?: string },
      paneOverrides?: Partial<SessionPane>,
    ): PendingTabCreation => {
      const createRequestId = createSessionRequestId();
      const pane = createSessionPane(name, type, connectionId, {
        ...paneOverrides,
        connecting: true,
        createRequestId,
      });
      const newTab = createWorkspaceTab(pane, getNextPersistOrder(tabsRef.current), extra);
      const nextTabs = options?.afterTabId
        ? insertTabAfter(tabsRef.current, options.afterTabId, newTab)
        : [...tabsRef.current, newTab];
      void commitTabs(nextTabs);
      setActiveTabId(newTab.id);
      return { tabId: newTab.id, createRequestId };
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
                createRequestId: undefined,
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
                createRequestId: undefined,
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
                createRequestId: undefined,
              }),
            }
          : tab,
      );
      void commitTabs(nextTabs);
    },
    [commitTabs],
  );

  const replaceSessionReferences = useCallback(
    (oldSessionId: string, newSessionId: string) => {
      const nextTabs = tabsRef.current.map((tab) => ({
        ...tab,
        root: replacePaneSessionReferences(tab.root, oldSessionId, newSessionId),
      }));
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
                createRequestId: undefined,
              }),
            }
          : tab,
      );
      void commitTabs(nextTabs);
    },
    [commitTabs],
  );

  const markPaneConnecting = useCallback(
    (tabId: string, paneId: string, updates?: PaneConnectingUpdates) => {
      const createRequestId = createSessionRequestId();
      const nextTabs = tabsRef.current.map((tab) =>
        tab.id === tabId
          ? {
              ...tab,
              root: updateSessionPane(tab.root, paneId, {
                ...updates,
                connecting: true,
                connectError: undefined,
                createRequestId,
              }),
            }
          : tab,
      );
      void commitTabs(nextTabs);
      return tabsRef.current.some((tab) => tab.id === tabId) ? createRequestId : null;
    },
    [commitTabs],
  );

  const hasTab = useCallback((tabId: string) => {
    return tabsRef.current.some((tab) => tab.id === tabId);
  }, []);

  const hasPane = useCallback((tabId: string, paneId: string) => {
    const tab = tabsRef.current.find((item) => item.id === tabId);
    return !!tab && !!findSessionPaneById(tab.root, paneId);
  }, []);

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

  const openFileDocument = useCallback(
    (input: {
      sessionId: string;
      name: string;
      type: SessionType;
      connectionId?: string;
      backend: FileDocumentBackend;
      path: string;
      file: FileDocumentSnapshot;
    }) => {
      const existing = findOpenFileDocument(tabsRef.current, input);
      if (existing) {
        setActivePane(existing.tabId, existing.paneId);
        return { ...existing, created: false };
      }

      const pane = createFileDocumentPane(input);
      const tab = createWorkspaceTab(pane, getNextPersistOrder(tabsRef.current));
      void commitTabs([...tabsRef.current, tab]);
      setActiveTabId(tab.id);
      return { tabId: tab.id, paneId: pane.id, created: true };
    },
    [commitTabs, setActivePane, setActiveTabId],
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
      updates: Partial<Pick<Tab, "customName" | "tabColor" | "locked">>,
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

  const persistTabsNow = useCallback(async (extraUi?: Partial<UiConfig>) => {
    if (!hasRestored.current || !appSettingsRef.current.general.startup_restore) return;
    const nextUi = {
      ...appSettingsRef.current.ui,
      open_tabs: serializeTabsForPersistence(tabsRef.current),
      ...extraUi,
    };
    appSettingsRef.current = { ...appSettingsRef.current, ui: nextUi };
    await invoke("save_app_ui_settings", { ui: nextUi });
  }, []);

  const closeStaleCreatedSession = useCallback(async (sessionId: string) => {
    try {
      await invoke("close_session", { sessionId });
    } catch (error) {
      logger.error({
        domain: "session.lifecycle",
        event: "session.stale_close_failed",
        message: "Failed to close stale restored session",
        ids: { session_id: sessionId },
        error,
      });
    }
  }, []);

  const handleRestoredSessionCreated = useCallback(
    async (tabId: string, paneId: string, sessionId: string, connectionId?: string) => {
      if (!hasPane(tabId, paneId)) {
        await closeStaleCreatedSession(sessionId);
        return;
      }
      updatePaneSession(tabId, paneId, sessionId);
      if (connectionId) {
        void updateConnectionAutoIconAfterSessionStart({
          connectionId,
          sessionId,
          remoteStatsEnabled: appSettingsRef.current.ui.show_remote_stats ?? true,
        });
      }
    },
    [closeStaleCreatedSession, hasPane, updatePaneSession],
  );

  const handleRestoredSessionFailed = useCallback(
    (
      tabId: string,
      paneId: string,
      sessionType: WorkspaceSessionType,
      connectionId: string | undefined,
      error: unknown,
    ) => {
      const errorMessage = getErrorMessage(error);
      if (
        errorMessage.toLowerCase().includes("session creation cancelled") ||
        !hasPane(tabId, paneId)
      ) {
        return;
      }
      logger.error({
        domain: "session.lifecycle",
        event: "session.restore_failed",
        message: `Restore ${sessionType} failed`,
        ids: connectionId ? { connection_id: connectionId } : undefined,
        data: {
          session_type: sessionType,
          pane_id: paneId,
        },
        error,
      });
      markPaneConnectionFailed(tabId, paneId, errorMessage);
    },
    [hasPane, markPaneConnectionFailed],
  );

  // 5. Startup Restore Logic
  const hasRestored = useRef(false);
  const pendingLockedStartupRestoreTabsRef = useRef<Tab[] | null>(null);

  const restoreSessionsForTabs = useCallback(
    (tabsToRestore: Tab[]) => {
      const tasks: Promise<unknown>[] = [];
      tabsToRestore.forEach((tab) => {
        const panes = collectSessionPanes(tab.root);

        panes.forEach((pane) => {
          if (!hasPane(tab.id, pane.id)) return;

          const cid = pane.connectionId;
          switch (pane.type) {
            case "SSH":
              if (!cid) {
                markPaneConnectionFailed(tab.id, pane.id, "Missing SSH connection id");
                return;
              }
              tasks.push(invoke<string>("create_ssh_session", {
                connectionId: cid,
                createRequestId: pane.createRequestId,
              })
                .then((sessionId) => handleRestoredSessionCreated(tab.id, pane.id, sessionId, cid))
                .catch((e) =>
                  handleRestoredSessionFailed(tab.id, pane.id, "SSH", pane.connectionId, e),
                ));
              break;
            case "Local":
              tasks.push(invoke<string>("create_local_session", {
                connectionId: cid || null,
                createRequestId: pane.createRequestId,
              })
                .then((sessionId) => handleRestoredSessionCreated(tab.id, pane.id, sessionId))
                .catch((e) =>
                  handleRestoredSessionFailed(tab.id, pane.id, "Local", pane.connectionId, e),
                ));
              break;
            case "Telnet":
              if (!cid) {
                markPaneConnectionFailed(tab.id, pane.id, "Missing Telnet connection id");
                return;
              }
              tasks.push(invoke<string>("create_telnet_session", {
                connectionId: cid,
                createRequestId: pane.createRequestId,
              })
                .then((sessionId) => handleRestoredSessionCreated(tab.id, pane.id, sessionId))
                .catch((e) =>
                  handleRestoredSessionFailed(tab.id, pane.id, "Telnet", pane.connectionId, e),
                ));
              break;
            case "Serial":
              if (!cid) {
                markPaneConnectionFailed(tab.id, pane.id, "Missing Serial connection id");
                return;
              }
              tasks.push(invoke<string>("create_serial_session", {
                connectionId: cid,
                createRequestId: pane.createRequestId,
              })
                .then((sessionId) => handleRestoredSessionCreated(tab.id, pane.id, sessionId))
                .catch((e) =>
                  handleRestoredSessionFailed(tab.id, pane.id, "Serial", pane.connectionId, e),
                ));
              break;
            case "VNC":
              if (!cid) {
                markPaneConnectionFailed(tab.id, pane.id, "Missing VNC connection id");
                return;
              }
              tasks.push(invoke<string>("create_vnc_session", {
                connectionId: cid,
                createRequestId: pane.createRequestId,
              })
                .then((sessionId) => handleRestoredSessionCreated(tab.id, pane.id, sessionId, cid))
                .catch((e) =>
                  handleRestoredSessionFailed(tab.id, pane.id, "VNC", pane.connectionId, e),
                ));
              break;
            case "RDP":
              if (!cid) {
                markPaneConnectionFailed(tab.id, pane.id, "Missing RDP connection id");
                return;
              }
              tasks.push(invoke<string>("create_rdp_session", {
                connectionId: cid,
                createRequestId: pane.createRequestId,
              })
                .then((sessionId) => handleRestoredSessionCreated(tab.id, pane.id, sessionId, cid))
                .catch((e) =>
                  handleRestoredSessionFailed(tab.id, pane.id, "RDP", pane.connectionId, e),
                ));
              break;
          }
        });
      });
      return Promise.allSettled(tasks).then(() => undefined);
    },
    [handleRestoredSessionCreated, handleRestoredSessionFailed, hasPane, markPaneConnectionFailed],
  );

  useEffect(() => {
    if (hasRestored.current || !appSettingsLoaded.current || !lockStateLoaded) return;

    hasRestored.current = true;
    const primaryWindow = isPrimaryMainWindow();
    let restoreCompletionScheduled = false;
    if (
      primaryWindow &&
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

      if (appSettings.security.enable_startup_lock && isLocked) {
        pendingLockedStartupRestoreTabsRef.current = restoredTabs;
      } else {
        restoreCompletionScheduled = true;
        void restoreSessionsForTabs(restoredTabs).then(() =>
          invoke("notify_mcp_session_restore_complete", {
            ownerWindowLabel: getOwnerMainWindowLabel(),
          }),
        );
      }
    }

    if (
      primaryWindow &&
      !pendingLockedStartupRestoreTabsRef.current &&
      !restoreCompletionScheduled
    ) {
      void invoke("notify_mcp_session_restore_complete", {
        ownerWindowLabel: getOwnerMainWindowLabel(),
      });
    }
    setStartupRestoreComplete(true);
  }, [appSettings, isLocked, lockStateLoaded, restoreSessionsForTabs, setActiveTabId]);

  useEffect(() => {
    if (isLocked) return;

    const pendingTabs = pendingLockedStartupRestoreTabsRef.current;
    if (!pendingTabs) return;

    pendingLockedStartupRestoreTabsRef.current = null;
    void restoreSessionsForTabs(pendingTabs).then(() =>
      invoke("notify_mcp_session_restore_complete", {
        ownerWindowLabel: getOwnerMainWindowLabel(),
      }),
    );
  }, [isLocked, restoreSessionsForTabs]);

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
      replaceSessionReferences,
      markPaneConnectionFailed,
      markPaneConnecting,
      hasTab,
      hasPane,
      setActivePane,
      updateSplitRatio,
      splitPane,
      openFileDocument,
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
      startupRestoreComplete,
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
      replaceSessionReferences,
      markPaneConnectionFailed,
      markPaneConnecting,
      hasTab,
      hasPane,
      setActivePane,
      updateSplitRatio,
      splitPane,
      openFileDocument,
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
      setIsLocked,
      settingsLoaded,
      startupRestoreComplete,
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
      transfer: appSettings.transfer,
    }),
    [
      appSettings.appearance,
      appSettings.interaction,
      appSettings.terminal,
      appSettings.translation,
      appSettings.search,
      appSettings.ai,
      appSettings.keybindings,
      appSettings.transfer,
    ],
  );

  return (
    <AppContext.Provider value={contextValue}>
      <TerminalAppSettingsContext.Provider value={terminalAppSettingsValue}>
        {lockStateLoaded && settingsLoaded ? children : null}
      </TerminalAppSettingsContext.Provider>
    </AppContext.Provider>
  );
}
