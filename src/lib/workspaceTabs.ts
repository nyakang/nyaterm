import type {
  FileDocumentBackend,
  FileDocumentPane,
  FileDocumentSnapshot,
  PaneNode,
  PaneSplitDirection,
  RestorablePaneNode,
  RestorableTab,
  SessionPane,
  SessionTitleSnapshot,
  SessionType,
  SplitPane,
  Tab,
  WorkspaceSessionType,
} from "@/types/global";

let workspaceIdCounter = 0;

export function createWorkspaceId(prefix: string) {
  workspaceIdCounter += 1;
  return `${prefix}-${Date.now()}-${workspaceIdCounter}`;
}

export function isSplitPane(node: PaneNode): node is SplitPane {
  return node.kind === "split";
}

export function isSessionPane(node: PaneNode): node is SessionPane {
  return node.kind === "leaf";
}

export function isTerminalPane(
  node: PaneNode,
): node is Extract<SessionPane, { paneKind: "terminal" }> {
  return isSessionPane(node) && node.paneKind === "terminal";
}

export function isRemoteDesktopPane(
  node: PaneNode,
): node is Extract<SessionPane, { paneKind: "remote-desktop" }> {
  return isSessionPane(node) && node.paneKind === "remote-desktop";
}

export function isRdpPane(node: PaneNode): node is Extract<SessionPane, { type: "RDP" }> {
  return isRemoteDesktopPane(node) && node.type === "RDP";
}

export function isFileDocumentPane(node: PaneNode): node is FileDocumentPane {
  return isSessionPane(node) && node.paneKind === "file";
}

function isRemoteDesktopSessionType(type: WorkspaceSessionType): type is "RDP" | "VNC" {
  return type === "RDP" || type === "VNC";
}

const DEFAULT_REMOTE_DESKTOP_DISPLAY = {
  remoteWidth: 1920,
  remoteHeight: 1080,
  scaleMode: "fit",
} as const;
export function createSessionPane(
  name: string,
  type: WorkspaceSessionType,
  connectionId?: string,
  overrides?: Partial<SessionPane>,
): SessionPane {
  const remoteDesktop = isRemoteDesktopSessionType(type);
  const paneKind = remoteDesktop ? "remote-desktop" : "terminal";
  return {
    id: overrides?.id ?? createWorkspaceId("pane"),
    kind: "leaf",
    paneKind,
    sessionId: overrides?.sessionId ?? createWorkspaceId("session"),
    name,
    type,
    connectionId,
    display: remoteDesktop
      ? ((overrides && "display" in overrides ? overrides.display : undefined) ?? {
          ...DEFAULT_REMOTE_DESKTOP_DISPLAY,
        })
      : undefined,
    connecting: overrides?.connecting,
    createRequestId: overrides?.createRequestId,
    connectError: overrides?.connectError,
    temporaryConfig: overrides?.temporaryConfig,
  } as SessionPane;
}

export function createFileDocumentPane({
  sessionId,
  name,
  type,
  connectionId,
  backend,
  path,
  file,
}: {
  sessionId: string;
  name: string;
  type: SessionType;
  connectionId?: string;
  backend: FileDocumentBackend;
  path: string;
  file: FileDocumentSnapshot;
}): FileDocumentPane {
  return {
    id: createWorkspaceId("pane"),
    kind: "leaf",
    paneKind: "file",
    sessionId,
    name,
    type,
    connectionId,
    file: { backend, path, initial: file },
  };
}

export function createWorkspaceTab(
  pane: SessionPane,
  persistOrder: number,
  extra?: Partial<Pick<Tab, "customName" | "tabColor">>,
): Tab {
  return {
    id: createWorkspaceId("tab"),
    persistOrder,
    activePaneId: pane.id,
    root: pane,
    customName: extra?.customName,
    tabColor: extra?.tabColor,
  };
}

export function collectSessionPanes(node: PaneNode): SessionPane[] {
  if (isSessionPane(node)) return [node];
  return [
    ...collectSessionPanes(node.first),
    ...collectSessionPanes(node.second),
  ];
}

export function getFirstSessionPane(node: PaneNode): SessionPane | null {
  if (isSessionPane(node)) return node;
  return getFirstSessionPane(node.first) ?? getFirstSessionPane(node.second);
}

export function findPaneById(node: PaneNode, paneId: string): PaneNode | null {
  if (node.id === paneId) return node;
  if (isSessionPane(node)) return null;
  return findPaneById(node.first, paneId) ?? findPaneById(node.second, paneId);
}

export function findSessionPaneById(
  node: PaneNode,
  paneId: string,
): SessionPane | null {
  const pane = findPaneById(node, paneId);
  return pane && isSessionPane(pane) ? pane : null;
}

export function findSessionPaneBySessionId(
  node: PaneNode,
  sessionId: string,
): SessionPane | null {
  if (isSessionPane(node)) return node.sessionId === sessionId ? node : null;
  return (
    findSessionPaneBySessionId(node.first, sessionId) ??
    findSessionPaneBySessionId(node.second, sessionId)
  );
}

function updatePaneTree(
  node: PaneNode,
  paneId: string,
  updater: (current: PaneNode) => PaneNode,
): PaneNode {
  if (node.id === paneId) return updater(node);
  if (isSessionPane(node)) return node;

  const nextFirst = updatePaneTree(node.first, paneId, updater);
  const nextSecond = updatePaneTree(node.second, paneId, updater);
  if (nextFirst === node.first && nextSecond === node.second) return node;
  return { ...node, first: nextFirst, second: nextSecond };
}

export function replaceSessionReferences(
  root: PaneNode,
  oldSessionId: string,
  newSessionId: string,
): PaneNode {
  if (isSessionPane(root)) {
    return root.sessionId === oldSessionId
      ? ({ ...root, sessionId: newSessionId } as SessionPane)
      : root;
  }

  const first = replaceSessionReferences(
    root.first,
    oldSessionId,
    newSessionId,
  );
  const second = replaceSessionReferences(
    root.second,
    oldSessionId,
    newSessionId,
  );
  if (first === root.first && second === root.second) return root;
  return { ...root, first, second };
}

export function updateSessionPane(
  root: PaneNode,
  paneId: string,
  updates: Partial<
    Pick<
      SessionPane,
      | "sessionId"
      | "name"
      | "type"
      | "connectionId"
      | "connecting"
      | "connectError"
      | "createRequestId"
    >
  >,
): PaneNode {
  return updatePaneTree(root, paneId, (current) => {
    if (!isSessionPane(current) || current.paneKind === "file") return current;
    const type = updates.type ?? current.type;
    const remoteDesktop = isRemoteDesktopSessionType(type);
    const display = remoteDesktop
      ? (("display" in current ? current.display : undefined) ?? {
          ...DEFAULT_REMOTE_DESKTOP_DISPLAY,
        })
      : undefined;
    const next = {
      ...current,
      ...updates,
      type,
      paneKind: remoteDesktop ? "remote-desktop" : "terminal",
    };
    if (remoteDesktop) {
      return { ...next, display } as SessionPane;
    }
    const { display: _display, ...terminalPane } = next as typeof next & { display?: unknown };
    return terminalPane as SessionPane;
  });
}

export function splitSessionPane(
  root: PaneNode,
  paneId: string,
  direction: PaneSplitDirection,
  newPane: SessionPane,
): PaneNode {
  return updatePaneTree(root, paneId, (current) => {
    if (!isSessionPane(current)) return current;
    return {
      id: createWorkspaceId("split"),
      kind: "split",
      direction,
      ratio: 0.5,
      first: current,
      second: newPane,
    };
  });
}

export function updateSplitRatio(
  root: PaneNode,
  splitId: string,
  ratio: number,
): PaneNode {
  return updatePaneTree(root, splitId, (current) => {
    if (!isSplitPane(current)) return current;
    return { ...current, ratio: clampSplitRatio(ratio) };
  });
}

export function removeSessionPane(
  root: PaneNode,
  paneId: string,
): PaneNode | null {
  if (isSessionPane(root)) return root.id === paneId ? null : root;

  const nextFirst = removeSessionPane(root.first, paneId);
  const nextSecond = removeSessionPane(root.second, paneId);

  if (!nextFirst && !nextSecond) return null;
  if (!nextFirst) return nextSecond;
  if (!nextSecond) return nextFirst;
  if (nextFirst === root.first && nextSecond === root.second) return root;
  return { ...root, first: nextFirst, second: nextSecond };
}

export function getActivePane(tab: Tab): SessionPane | null {
  return (
    findSessionPaneById(tab.root, tab.activePaneId) ??
    getFirstSessionPane(tab.root)
  );
}

export function getTabDisplayName(
  tab: Tab,
  dynamicTitle?: string | null,
): string {
  if (tab.customName) return tab.customName;
  const activePane = getActivePane(tab);
  const dynamic = dynamicTitle?.trim();
  if (activePane?.paneKind === "terminal" && dynamic) return dynamic;
  return activePane?.name || "Session";
}

export function getTabActiveSessionId(tab: Tab) {
  return getActivePane(tab)?.sessionId ?? null;
}

/** Resolve the display name from the active pane's dynamic-title snapshot. */
export function getActiveSessionTabDisplayName(
  tab: Tab,
  getDynamicTitle: (sessionId: string | null) => string | null | undefined,
): string {
  return getTabDisplayName(tab, getDynamicTitle(getTabActiveSessionId(tab)));
}

/** Resolve a row whose identity is a session, without changing legacy rows. */
export function getSessionRowDisplayName(
  tab: Tab,
  sessionName: string,
  dynamicSnapshot: SessionTitleSnapshot | null | undefined,
): string {
  if (tab.customName) return tab.customName;
  if (dynamicSnapshot?.enabled) {
    return dynamicSnapshot.effectiveTitle || sessionName;
  }
  return getTabDisplayName(tab);
}

export function getTabActiveConnectionId(tab: Tab) {
  return getActivePane(tab)?.connectionId;
}

export function getTabActiveType(tab: Tab) {
  return getActivePane(tab)?.type ?? null;
}

export function ensureActivePane(tab: Tab): Tab {
  const activePane = getActivePane(tab);
  if (!activePane || activePane.id === tab.activePaneId) return tab;
  return { ...tab, activePaneId: activePane.id };
}

export function getNextPersistOrder(tabs: Tab[]) {
  return tabs.reduce((max, tab) => Math.max(max, tab.persistOrder), -1) + 1;
}

export function insertTabAfter(
  tabs: Tab[],
  afterTabId: string,
  newTab: Tab,
): Tab[] {
  const index = tabs.findIndex((tab) => tab.id === afterTabId);
  if (index === -1) return [...tabs, newTab];
  const next = [...tabs];
  next.splice(index + 1, 0, newTab);
  return next;
}

export function moveTab(
  tabs: Tab[],
  fromTabId: string,
  toIndex: number,
): Tab[] {
  const fromIndex = tabs.findIndex((tab) => tab.id === fromTabId);
  if (fromIndex === -1) return tabs;
  const boundedIndex = Math.max(0, Math.min(tabs.length - 1, toIndex));
  if (fromIndex === boundedIndex) return tabs;

  const next = [...tabs];
  const [tab] = next.splice(fromIndex, 1);
  next.splice(boundedIndex, 0, tab);
  return next;
}

export function findTabBySessionId(tabs: Tab[], sessionId: string) {
  return tabs.find((tab) => findSessionPaneBySessionId(tab.root, sessionId));
}

export function findPaneBySessionId(tab: Tab, sessionId: string) {
  return findSessionPaneBySessionId(tab.root, sessionId);
}

export interface FileDocumentIdentity {
  backend: FileDocumentBackend;
  sessionId: string;
  path: string;
}

export function findOpenFileDocument(
  tabs: Tab[],
  identity: FileDocumentIdentity,
): { tabId: string; paneId: string } | null {
  for (const tab of tabs) {
    const pane = collectSessionPanes(tab.root).find(
      (candidate) =>
        candidate.paneKind === "file" &&
        candidate.sessionId === identity.sessionId &&
        candidate.file.backend === identity.backend &&
        candidate.file.path === identity.path,
    );
    if (pane) return { tabId: tab.id, paneId: pane.id };
  }
  return null;
}

function collectSessionReferenceCounts(tabs: Tab[]) {
  const counts = new Map<string, number>();
  for (const tab of tabs) {
    for (const pane of collectSessionPanes(tab.root)) {
      counts.set(pane.sessionId, (counts.get(pane.sessionId) ?? 0) + 1);
    }
  }
  return counts;
}

export function getReleasedSessionIds(
  previousTabs: Tab[],
  nextTabs: Tab[],
): string[] {
  const previous = collectSessionReferenceCounts(previousTabs);
  const next = collectSessionReferenceCounts(nextTabs);
  return [...previous.keys()].filter((sessionId) => !next.has(sessionId));
}

function serializePane(node: PaneNode): RestorablePaneNode | null {
  if (isSessionPane(node)) {
    if (node.paneKind === "file") return null;
    return {
      id: node.id,
      kind: "leaf",
      pane_kind: node.paneKind,
      title: node.name,
      session_type: node.type,
      connection_id: node.connectionId,
      display: isRemoteDesktopPane(node) ? node.display : undefined,
    };
  }

  const first = serializePane(node.first);
  const second = serializePane(node.second);
  if (!first) return second;
  if (!second) return first;
  return {
    id: node.id,
    kind: "split",
    direction: node.direction,
    ratio: clampSplitRatio(node.ratio),
    first,
    second,
  };
}

function findFirstRestorableLeaf(
  node: RestorablePaneNode,
): Extract<RestorablePaneNode, { kind: "leaf" }> {
  return node.kind === "leaf" ? node : findFirstRestorableLeaf(node.first);
}

function hasRestorablePaneId(
  node: RestorablePaneNode,
  paneId: string,
): boolean {
  if (node.kind === "leaf") return node.id === paneId;
  return (
    hasRestorablePaneId(node.first, paneId) ||
    hasRestorablePaneId(node.second, paneId)
  );
}

export function serializeTabsForPersistence(tabs: Tab[]): RestorableTab[] {
  return [...tabs]
    .sort((a, b) => a.persistOrder - b.persistOrder)
    .flatMap((tab) => {
      const root = serializePane(tab.root);
      if (!root) return [];
      const fallback = findFirstRestorableLeaf(root);
      const activePaneId = hasRestorablePaneId(root, tab.activePaneId)
        ? tab.activePaneId
        : fallback.id;
      return [
        {
          title: tab.customName || fallback.title,
          session_type: fallback.session_type,
          connection_id: fallback.connection_id,
      custom_name: tab.customName,
      tab_color: tab.tabColor,
      locked: tab.locked,
          active_pane_id: activePaneId,
          root,
        },
      ];
    });
}

function createLegacyPaneNode(tab: RestorableTab): RestorablePaneNode | null {
  const type = normalizeSessionType(tab.session_type);
  if (!type) return null;

  return {
    kind: "leaf",
    title: tab.title || "Session",
    session_type: type,
    connection_id: tab.connection_id,
  };
}

export function normalizeSessionType(
  value: string,
): WorkspaceSessionType | null {
  switch (value) {
    case "SSH":
      return "SSH";
    case "Local":
    case "local":
      return "Local";
    case "Telnet":
      return "Telnet";
    case "Serial":
      return "Serial";
    case "RDP":
    case "rdp":
      return "RDP";
    case "VNC":
    case "vnc":
      return "VNC";
    default:
      return null;
  }
}

function restorePane(node: RestorablePaneNode): PaneNode | null {
  if (node.kind === "leaf") {
    const type = normalizeSessionType(node.session_type);
    if (!type || node.pane_kind === "file") return null;
    const remoteDesktop = isRemoteDesktopSessionType(type);
    const persistedPaneKind = node.pane_kind;
    const paneKind =
      persistedPaneKind === "rdp"
        ? "remote-desktop"
        : (persistedPaneKind ?? (remoteDesktop ? "remote-desktop" : "terminal"));
    if ((paneKind === "remote-desktop") !== remoteDesktop) return null;
    return {
      id: node.id || createWorkspaceId("pane"),
      kind: "leaf",
      paneKind,
      sessionId: createWorkspaceId("pending"),
      name: node.title,
      type,
      connectionId: node.connection_id,
      display: remoteDesktop
        ? {
            ...DEFAULT_REMOTE_DESKTOP_DISPLAY,
            ...node.display,
          }
        : undefined,
      connecting: true,
      createRequestId: crypto.randomUUID(),
    } as SessionPane;
  }

  const first = restorePane(node.first);
  const second = restorePane(node.second);
  if (!first && !second) return null;
  if (!first) return second;
  if (!second) return first;

  return {
    id: node.id || createWorkspaceId("split"),
    kind: "split",
    direction: node.direction,
    ratio: clampSplitRatio(node.ratio),
    first,
    second,
  };
}

export function restoreTabFromPersistence(
  tab: RestorableTab,
  persistOrder: number,
): Tab | null {
  const restorableRoot = tab.root ?? createLegacyPaneNode(tab);
  if (!restorableRoot) return null;

  const root = restorePane(restorableRoot);
  if (!root) return null;

  const restored: Tab = {
    id: createWorkspaceId("tab"),
    persistOrder,
    activePaneId: tab.active_pane_id || getFirstSessionPane(root)?.id || "",
    root,
    customName: tab.custom_name,
    tabColor: tab.tab_color,
    locked: tab.locked,
  };

  return ensureActivePane(restored);
}

export function clampSplitRatio(ratio: number) {
  return Math.max(0.2, Math.min(0.8, ratio));
}
