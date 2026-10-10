import { useCallback, useEffect, useRef, useState } from "react";
import type { AppContextType } from "@/context/AppContext";
import {
  flattenTerminalWindows,
  moveTabBetweenLeaves,
  reconcileTerminalWindows,
  reorderTabsInLeaf,
  restoreTerminalWindowLayout,
  type SplitEdgeDirection,
  serializeTerminalWindowLayout,
  setLeafActiveTab,
  splitLeafWithTab,
  type TerminalWindowNode,
  updateTerminalWindowSplitRatio,
} from "@/lib/tabWindows";
import { invoke } from "@/lib/invoke";
import { collectSessionPanes, findSessionPaneById } from "@/lib/workspaceTabs";
import type { AppSettings, Tab, UiConfig } from "@/types/global";

interface TerminalWindowLayoutOptions
  extends Pick<
    AppContextType,
    | "tabs"
    | "activeTabId"
    | "setActiveTabId"
    | "setActivePane"
    | "updateSplitRatio"
    | "updateUi"
    | "persistTabsNow"
    | "settingsLoaded"
    | "startupRestoreComplete"
  > {
  general: AppSettings["general"];
  savedLayout: UiConfig["terminal_window_layout"];
}

export function useTerminalWindowLayout({
  tabs,
  activeTabId,
  setActiveTabId,
  setActivePane,
  updateSplitRatio,
  updateUi,
  persistTabsNow,
  settingsLoaded,
  startupRestoreComplete,
  general,
  savedLayout,
}: TerminalWindowLayoutOptions) {
  const [terminalWindows, setTerminalWindows] = useState<TerminalWindowNode | null>(null);
  const previousActiveTabIdRef = useRef<string | null>(null);
  const terminalWindowsRef = useRef<TerminalWindowNode | null>(null);
  const terminalWindowsRestoredRef = useRef(false);
  const terminalWindowsHydratedRef = useRef(false);
  const preserveRestoredLeafActiveTabsRef = useRef(false);
  const restoredGlobalActiveTabIdRef = useRef<string | null>(null);
  const lastPersistedTerminalWindowLayoutKeyRef = useRef(JSON.stringify(savedLayout ?? null));
  const tabsRef = useRef(tabs);
  useEffect(() => {
    tabsRef.current = tabs;
  }, [tabs]);

  useEffect(() => {
    terminalWindowsRef.current = terminalWindows;
  }, [terminalWindows]);

  useEffect(() => {
    if (terminalWindowsRestoredRef.current) return;
    lastPersistedTerminalWindowLayoutKeyRef.current = JSON.stringify(savedLayout ?? null);
  }, [savedLayout]);

  const persistTerminalWindowLayout = useCallback(
    (layout: TerminalWindowNode | null, nextTabs: Tab[] = tabsRef.current) => {
      if (!settingsLoaded || !startupRestoreComplete || !general.startup_restore) return;
      const terminalWindowLayout =
        general.startup_restore_window_layout === false
          ? null
          : serializeTerminalWindowLayout(layout, nextTabs);
      const layoutKey = JSON.stringify(terminalWindowLayout ?? null);
      if (layoutKey === lastPersistedTerminalWindowLayoutKeyRef.current) return;
      lastPersistedTerminalWindowLayoutKeyRef.current = layoutKey;
      updateUi({ terminal_window_layout: terminalWindowLayout });
    },
    [
      general.startup_restore,
      general.startup_restore_window_layout,
      settingsLoaded,
      startupRestoreComplete,
      updateUi,
    ],
  );

  useEffect(() => {
    if (!preserveRestoredLeafActiveTabsRef.current) return;
    if (restoredGlobalActiveTabIdRef.current === null) return;
    if (activeTabId === restoredGlobalActiveTabIdRef.current) return;
    preserveRestoredLeafActiveTabsRef.current = false;
    restoredGlobalActiveTabIdRef.current = null;
  }, [activeTabId]);

  useEffect(() => {
    if (!settingsLoaded || !startupRestoreComplete) return;

    setTerminalWindows((current) => {
      let next = current;
      let preserveRestoredLeafActiveTabs = preserveRestoredLeafActiveTabsRef.current;

      if (!terminalWindowsRestoredRef.current) {
        terminalWindowsRestoredRef.current = true;
        if (
          general.startup_restore &&
          general.startup_restore_window_layout !== false &&
          tabs.length > 0
        ) {
          const restored = restoreTerminalWindowLayout(savedLayout, tabs);
          if (restored) {
            next = restored;
            preserveRestoredLeafActiveTabs = true;
            preserveRestoredLeafActiveTabsRef.current = true;
            restoredGlobalActiveTabIdRef.current = activeTabId;
          }
        }
      }

      const reconciled = reconcileTerminalWindows(
        next,
        tabs,
        preserveRestoredLeafActiveTabs ? null : activeTabId,
        previousActiveTabIdRef.current,
      );
      terminalWindowsHydratedRef.current = true;
      terminalWindowsRef.current = reconciled;
      return reconciled;
    });
    previousActiveTabIdRef.current = activeTabId;
  }, [
    activeTabId,
    general.startup_restore,
    general.startup_restore_window_layout,
    settingsLoaded,
    startupRestoreComplete,
    tabs,
    savedLayout,
  ]);

  useEffect(() => {
    if (!settingsLoaded) return;
    if (!startupRestoreComplete) return;
    if (!terminalWindowsRestoredRef.current) return;
    if (!terminalWindowsHydratedRef.current) return;
    if (tabs.length > 0 && !terminalWindows) return;
    persistTerminalWindowLayout(terminalWindows, tabs);
  }, [persistTerminalWindowLayout, settingsLoaded, startupRestoreComplete, tabs, terminalWindows]);

  const handleSelectLeafTab = useCallback(
    (leafId: string, tabId: string) => {
      preserveRestoredLeafActiveTabsRef.current = false;
      restoredGlobalActiveTabIdRef.current = null;
      setTerminalWindows((current) =>
        current ? setLeafActiveTab(current, leafId, tabId) : current,
      );
      setActiveTabId(tabId);
    },
    [setActiveTabId],
  );

  const handleReorderTabsInLeaf = useCallback((_: string, fromTabId: string, toIndex: number) => {
    preserveRestoredLeafActiveTabsRef.current = false;
    restoredGlobalActiveTabIdRef.current = null;
    setTerminalWindows((current) =>
      current ? reorderTabsInLeaf(current, fromTabId, toIndex) : current,
    );
  }, []);

  const handleMoveTabToLeaf = useCallback(
    (fromTabId: string, targetLeafId: string, toIndex: number) => {
      preserveRestoredLeafActiveTabsRef.current = false;
      restoredGlobalActiveTabIdRef.current = null;
      setTerminalWindows((current) => {
        if (!current) return current;
        const next = moveTabBetweenLeaves(current, fromTabId, targetLeafId, toIndex);
        return next ?? current;
      });
      setActiveTabId(fromTabId);
      requestAnimationFrame(() => {
        window.dispatchEvent(new CustomEvent("nyaterm:refresh-terminals"));
      });
    },
    [setActiveTabId],
  );

  const handleSplitTabToLeaf = useCallback(
    (fromTabId: string, targetLeafId: string, direction: SplitEdgeDirection) => {
      preserveRestoredLeafActiveTabsRef.current = false;
      restoredGlobalActiveTabIdRef.current = null;
      setTerminalWindows((current) => {
        if (!current) return current;
        const next = splitLeafWithTab(current, fromTabId, targetLeafId, direction);
        return next ?? current;
      });
      setActiveTabId(fromTabId);
      requestAnimationFrame(() => {
        window.dispatchEvent(new CustomEvent("nyaterm:refresh-terminals"));
      });
    },
    [setActiveTabId],
  );

  const handleUnsplit = useCallback(() => {
    preserveRestoredLeafActiveTabsRef.current = false;
    restoredGlobalActiveTabIdRef.current = null;
    setTerminalWindows((current) => {
      if (!current) return current;
      return flattenTerminalWindows(current, activeTabId);
    });
    requestAnimationFrame(() => {
      window.dispatchEvent(new CustomEvent("nyaterm:refresh-terminals"));
    });
  }, [activeTabId]);

  const handleUpdateWindowSplitRatio = useCallback((splitId: string, ratio: number) => {
    setTerminalWindows((current) =>
      current ? updateTerminalWindowSplitRatio(current, splitId, ratio) : current,
    );
  }, []);

  const handleActivatePane = useCallback(
    (tabId: string, paneId: string) => {
      preserveRestoredLeafActiveTabsRef.current = false;
      restoredGlobalActiveTabIdRef.current = null;
      setActiveTabId(tabId);
      setActivePane(tabId, paneId);
      // tmux panes: keep the remote "current pane" in sync with local focus
      // so untargeted commands (kill-pane, respawn-pane, ...) act on it.
      const tab = tabsRef.current.find((item) => item.id === tabId);
      const pane = tab ? findSessionPaneById(tab.root, paneId) : null;
      if (pane?.kind === "leaf" && pane.tmux) {
        void invoke("tmux_send_command", {
          sessionId: pane.tmux.controlSessionId,
          command: `select-pane -t '${pane.tmux.paneId}'`,
        }).catch(() => {});
      }
    },
    [setActivePane, setActiveTabId],
  );

  const handleUpdatePaneSplitRatio = useCallback(
    (tabId: string, splitId: string, ratio: number) => {
      updateSplitRatio(tabId, splitId, ratio);
    },
    [updateSplitRatio],
  );

  const persistWorkspaceLayoutNow = useCallback(async () => {
    if (!settingsLoaded || !startupRestoreComplete || !terminalWindowsRestoredRef.current) {
      await persistTabsNow();
      return;
    }

    const restorableTabs = tabsRef.current.filter((tab) =>
      collectSessionPanes(tab.root).some((pane) => pane.paneKind !== "file"),
    );
    const terminalWindowLayout =
      general.startup_restore_window_layout === false
        ? null
        : serializeTerminalWindowLayout(terminalWindowsRef.current, restorableTabs);
    lastPersistedTerminalWindowLayoutKeyRef.current = JSON.stringify(terminalWindowLayout ?? null);
    await persistTabsNow({ terminal_window_layout: terminalWindowLayout });
  }, [
    general.startup_restore_window_layout,
    persistTabsNow,
    settingsLoaded,
    startupRestoreComplete,
  ]);

  return {
    terminalWindows,
    setTerminalWindows,
    handleSelectLeafTab,
    handleReorderTabsInLeaf,
    handleMoveTabToLeaf,
    handleSplitTabToLeaf,
    handleUnsplit,
    handleUpdateWindowSplitRatio,
    handleActivatePane,
    handleUpdatePaneSplitRatio,
    persistWorkspaceLayoutNow,
  };
}
