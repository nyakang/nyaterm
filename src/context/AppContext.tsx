import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
} from "react";
import { invoke } from "../lib/invoke";
import { logger } from "../lib/logger";
import type { Group, SavedConnection, SessionType, Tab, UiConfig } from "../types";

interface AppContextType {
  // Tabs
  tabs: Tab[];
  activeTabId: string | null;
  setActiveTabId: (id: string | null) => void;
  addTab: (sessionId: string, name: string, type: SessionType) => void;
  closeTab: (tabId: string) => void;

  // UI Config
  uiConfig: UiConfig;
  updateUiConfig: (updates: Partial<UiConfig>) => void;

  // Data
  savedConnections: SavedConnection[];
  savedGroups: Group[];
  refreshConnections: () => Promise<void>;

  // Dialogs
  showNewSession: boolean;
  setShowNewSession: (show: boolean) => void;
  editingConnection: SavedConnection | undefined;
  setEditingConnection: (conn: SavedConnection | undefined) => void;
}

/**
 * App-wide state: tabs, UI config (debounced save), saved connections (polled),
 * and dialog visibility. Updates via setState/useCallback; config persisted to backend.
 */
const AppContext = createContext<AppContextType | null>(null);

const DEFAULT_UI_CONFIG: UiConfig = {
  left_width: 256,
  right_width: 288,
  saved_conn_height: 240,
  history_height: 200,
  quick_cmd_height: 36,
  show_file_explorer: true,
  show_saved_connections: true,
  show_active_sessions: true,
  show_command_history: true,
  show_quick_commands: true,
  zoom_level: 1.0,
  theme: "github-dark",
};

/** Provides tabs, uiConfig, savedConnections, and dialog state to the app. */
export function AppProvider({ children }: { children: ReactNode }) {
  // Tabs State
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeTabId, setActiveTabId] = useState<string | null>(null);

  // UI Config State
  const [uiConfig, setUiConfig] = useState<UiConfig>(DEFAULT_UI_CONFIG);
  const uiConfigLoaded = useRef(false);
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Data State
  const [savedConnections, setSavedConnections] = useState<SavedConnection[]>([]);
  const [savedGroups, setSavedGroups] = useState<Group[]>([]);

  // Dialog State
  const [showNewSession, setShowNewSession] = useState(false);
  const [editingConnection, setEditingConnection] = useState<SavedConnection | undefined>(
    undefined,
  );

  // 1. Load UI Config
  useEffect(() => {
    invoke<UiConfig>("get_ui_config")
      .then((cfg) => {
        setUiConfig(cfg);
        uiConfigLoaded.current = true;
      })
      .catch(() => {
        uiConfigLoaded.current = true;
      });
  }, []);

  // 2. Save UI Config Debounced
  const updateUiConfig = useCallback((updates: Partial<UiConfig>) => {
    setUiConfig((prev) => {
      const next = { ...prev, ...updates };
      // Debounce save
      if (uiConfigLoaded.current) {
        if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
        saveTimerRef.current = setTimeout(() => {
          invoke("save_ui_config", { config: next }).catch((e) =>
            logger.error("Failed to save UI config", e),
          );
        }, 500);
      }
      return next;
    });
  }, []);

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
    const interval = setInterval(refreshConnections, 5000); // Poll every 5s
    return () => clearInterval(interval);
  }, [refreshConnections]);

  // 4. Tab Logic
  const addTab = useCallback((sessionId: string, name: string, type: SessionType) => {
    const tabId = `tab-${Date.now()}`;
    const newTab: Tab = { id: tabId, sessionId, name, type };
    setTabs((prev) => [...prev, newTab]);
    setActiveTabId(tabId);

    // Close dialogs when session starts
    setShowNewSession(false);
    setEditingConnection(undefined);
  }, []);

  const closeTab = useCallback(
    (tabId: string) => {
      setTabs((prev) => {
        const newTabs = prev.filter((t) => t.id !== tabId);
        if (activeTabId === tabId) {
          if (newTabs.length > 0) {
            setActiveTabId(newTabs[newTabs.length - 1].id);
          } else {
            setActiveTabId(null);
          }
        }
        return newTabs;
      });
    },
    [activeTabId],
  );

  return (
    <AppContext.Provider
      value={{
        tabs,
        activeTabId,
        setActiveTabId,
        addTab,
        closeTab,
        uiConfig,
        updateUiConfig,
        savedConnections,
        savedGroups,
        refreshConnections,
        showNewSession,
        setShowNewSession,
        editingConnection,
        setEditingConnection,
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
