import { createContext, useContext } from "react";
import type { TmuxSessionState } from "@/lib/tmux/types";
import type {
  AppRuntimeInfo,
  AppSettings,
  FileDocumentBackend,
  FileDocumentSnapshot,
  Group,
  PaneSplitDirection,
  RemoteDesktopSessionPane,
  SavedConnection,
  SessionPane,
  SessionType,
  SyncGroup,
  Tab,
  UiConfig,
  WorkspaceSessionType,
} from "@/types/global";

export type PaneConnectingUpdates = Partial<Pick<SessionPane, "name" | "type" | "connectionId">> & {
  display?: RemoteDesktopSessionPane["display"];
};

export interface PendingTabCreation {
  tabId: string;
  paneId: string;
  createRequestId: string;
}

export interface AppContextType {
  tabs: Tab[];
  activeTabId: string | null;
  setActiveTabId: (id: string | null) => void;
  addTab: (
    sessionId: string,
    name: string,
    type: WorkspaceSessionType,
    connectionId?: string,
    extra?: Partial<Pick<Tab, "customName" | "tabColor">>,
    options?: { afterTabId?: string },
  ) => string;
  addPendingTab: (
    name: string,
    type: WorkspaceSessionType,
    connectionId?: string,
    extra?: Partial<Pick<Tab, "customName" | "tabColor">>,
    options?: { afterTabId?: string },
    paneOverrides?: Partial<SessionPane>,
  ) => PendingTabCreation;
  updateTabSession: (tabId: string, sessionId: string) => void;
  markTabConnectionFailed: (tabId: string, error: string) => void;
  updatePaneSession: (tabId: string, paneId: string, sessionId: string) => void;
  replaceSessionReferences: (oldSessionId: string, newSessionId: string) => void;
  markPaneConnectionFailed: (tabId: string, paneId: string, error: string) => void;
  markPaneConnecting: (
    tabId: string,
    paneId: string,
    updates?: PaneConnectingUpdates,
  ) => string | null;
  hasTab: (tabId: string) => boolean;
  hasPane: (tabId: string, paneId: string) => boolean;
  setActivePane: (tabId: string, paneId: string) => void;
  updateSplitRatio: (tabId: string, splitId: string, ratio: number) => void;
  splitPane: (
    tabId: string,
    paneId: string,
    direction: PaneSplitDirection,
    pane: SessionPane,
    options?: { immediatePersist?: boolean },
  ) => string | null;
  openFileDocument: (input: {
    sessionId: string;
    name: string;
    type: SessionType;
    connectionId?: string;
    backend: FileDocumentBackend;
    path: string;
    file: FileDocumentSnapshot;
  }) => { tabId: string; paneId: string; created: boolean };
  closePane: (tabId: string, paneId: string, options?: { immediatePersist?: boolean }) => void;
  reorderTabs: (fromTabId: string, toIndex: number) => void;
  updateTab: (
    tabId: string,
    updates: Partial<Pick<Tab, "customName" | "tabColor" | "locked">>,
    options?: { immediatePersist?: boolean },
  ) => Promise<void>;
  closeTabs: (
    tabIds: string[],
    options?: { immediatePersist?: boolean; nextActiveTabId?: string | null },
  ) => void;
  closeTab: (tabId: string) => void;
  /** Reconcile a tmux control-mode state into the owning tab's pane tree. */
  applyTmuxState: (state: TmuxSessionState) => void;
  persistTabsNow: (extraUi?: Partial<UiConfig>) => Promise<void>;
  appSettings: AppSettings;
  updateAppSettings: (
    updates: Partial<AppSettings> | ((prev: AppSettings) => Partial<AppSettings>),
  ) => void;
  replaceAppSettings: (next: AppSettings) => void;
  updateUi: (updates: Partial<UiConfig> | ((prev: UiConfig) => Partial<UiConfig>)) => void;
  savedConnections: SavedConnection[];
  savedGroups: Group[];
  refreshConnections: () => Promise<void>;
  recordRecentConnection: (connectionId: string) => void;
  showNewSession: boolean;
  setShowNewSession: (show: boolean) => void;
  editingConnection: SavedConnection | undefined;
  setEditingConnection: (conn: SavedConnection | undefined) => void;
  showSettingsDialog: boolean;
  setShowSettingsDialog: (show: boolean) => void;
  syncGroups: SyncGroup[];
  setSyncGroups: (groups: SyncGroup[] | ((prev: SyncGroup[]) => SyncGroup[])) => void;
  broadcastToAll: boolean;
  setBroadcastToAll: (value: boolean | ((prev: boolean) => boolean)) => void;
  isLocked: boolean;
  setIsLocked: (locked: boolean) => void;
  settingsLoaded: boolean;
  startupRestoreComplete: boolean;
  runtimeInfo: AppRuntimeInfo;
  runtimeInfoLoaded: boolean;
}

export type TerminalAppSettings = Pick<
  AppSettings,
  | "appearance"
  | "interaction"
  | "terminal"
  | "translation"
  | "search"
  | "ai"
  | "keybindings"
  | "transfer"
>;

export const AppContext = createContext<AppContextType | null>(null);
export const TerminalAppSettingsContext = createContext<TerminalAppSettings | null>(null);

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
