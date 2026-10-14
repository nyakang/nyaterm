import { collectSessionPanes } from "@/lib/workspaceTabs";
import type { PaneSplitDirection, RestorableTerminalWindowNode, Tab } from "@/types/global";

export interface TerminalWindowLeaf {
  id: string;
  kind: "leaf";
  tabIds: string[];
  activeTabId: string | null;
}

export interface TerminalWindowSplit {
  id: string;
  kind: "split";
  direction: PaneSplitDirection;
  ratio: number;
  first: TerminalWindowNode;
  second: TerminalWindowNode;
}

export type TerminalWindowNode = TerminalWindowLeaf | TerminalWindowSplit;
export type SplitEdgeDirection = "left" | "right" | "top" | "bottom";

let terminalWindowIdCounter = 0;

export function createTerminalWindowId(prefix: string) {
  terminalWindowIdCounter += 1;
  return `${prefix}-${Date.now()}-${terminalWindowIdCounter}`;
}

function normalizeLeaf(leaf: TerminalWindowLeaf): TerminalWindowLeaf {
  const tabIds = Array.from(new Set(leaf.tabIds));
  const activeTabId =
    leaf.activeTabId && tabIds.includes(leaf.activeTabId) ? leaf.activeTabId : (tabIds[0] ?? null);

  return {
    ...leaf,
    tabIds,
    activeTabId,
  };
}

export function isTerminalWindowSplit(node: TerminalWindowNode): node is TerminalWindowSplit {
  return node.kind === "split";
}

export function createTerminalWindowLeaf(
  tabIds: string[],
  activeTabId: string | null,
): TerminalWindowLeaf {
  return normalizeLeaf({
    id: createTerminalWindowId("window-leaf"),
    kind: "leaf",
    tabIds,
    activeTabId,
  });
}

export function createTerminalWindowRoot(
  tabIds: string[],
  activeTabId: string | null,
): TerminalWindowNode | null {
  if (tabIds.length === 0) return null;
  return createTerminalWindowLeaf(tabIds, activeTabId);
}

export function collectWindowTabIds(node: TerminalWindowNode): string[] {
  if (!isTerminalWindowSplit(node)) {
    return [...node.tabIds];
  }

  return [...collectWindowTabIds(node.first), ...collectWindowTabIds(node.second)];
}

export function getFirstTerminalWindowLeaf(node: TerminalWindowNode): TerminalWindowLeaf {
  if (!isTerminalWindowSplit(node)) {
    return normalizeLeaf(node);
  }

  return getFirstTerminalWindowLeaf(node.first);
}

export function findTerminalWindowLeafById(
  node: TerminalWindowNode,
  leafId: string,
): TerminalWindowLeaf | null {
  if (!isTerminalWindowSplit(node)) {
    return node.id === leafId ? normalizeLeaf(node) : null;
  }

  return (
    findTerminalWindowLeafById(node.first, leafId) ??
    findTerminalWindowLeafById(node.second, leafId)
  );
}

export function findTerminalWindowLeafByTabId(
  node: TerminalWindowNode,
  tabId: string,
): TerminalWindowLeaf | null {
  if (!isTerminalWindowSplit(node)) {
    return node.tabIds.includes(tabId) ? normalizeLeaf(node) : null;
  }

  return (
    findTerminalWindowLeafByTabId(node.first, tabId) ??
    findTerminalWindowLeafByTabId(node.second, tabId)
  );
}

function updateLeafById(
  node: TerminalWindowNode,
  leafId: string,
  updater: (leaf: TerminalWindowLeaf) => TerminalWindowNode,
): TerminalWindowNode {
  if (!isTerminalWindowSplit(node)) {
    return node.id === leafId ? updater(normalizeLeaf(node)) : normalizeLeaf(node);
  }

  const nextFirst = updateLeafById(node.first, leafId, updater);
  const nextSecond = updateLeafById(node.second, leafId, updater);

  if (nextFirst === node.first && nextSecond === node.second) {
    return node;
  }

  return {
    ...node,
    first: nextFirst,
    second: nextSecond,
  };
}

function collapseWindowChildren(
  first: TerminalWindowNode | null,
  second: TerminalWindowNode | null,
  source: TerminalWindowSplit,
): TerminalWindowNode | null {
  if (!first && !second) return null;
  if (!first) return second;
  if (!second) return first;
  if (first === source.first && second === source.second) return source;

  return {
    ...source,
    first,
    second,
  };
}

function removeTabFromLeaf(leaf: TerminalWindowLeaf, tabId: string): TerminalWindowLeaf | null {
  if (!leaf.tabIds.includes(tabId)) {
    return normalizeLeaf(leaf);
  }

  const tabIds = leaf.tabIds.filter((id) => id !== tabId);
  if (tabIds.length === 0) return null;

  return normalizeLeaf({
    ...leaf,
    tabIds,
    activeTabId: leaf.activeTabId === tabId ? (tabIds[0] ?? null) : leaf.activeTabId,
  });
}

export function removeTabFromTerminalWindows(
  node: TerminalWindowNode | null,
  tabId: string,
): TerminalWindowNode | null {
  if (!node) return null;

  if (!isTerminalWindowSplit(node)) {
    return removeTabFromLeaf(node, tabId);
  }

  const nextFirst = removeTabFromTerminalWindows(node.first, tabId);
  const nextSecond = removeTabFromTerminalWindows(node.second, tabId);

  return collapseWindowChildren(nextFirst, nextSecond, node);
}

export function setLeafActiveTab(
  node: TerminalWindowNode,
  leafId: string,
  tabId: string,
): TerminalWindowNode {
  return updateLeafById(node, leafId, (leaf) =>
    leaf.tabIds.includes(tabId) ? normalizeLeaf({ ...leaf, activeTabId: tabId }) : leaf,
  );
}

export function setLeafActiveTabByTabId(
  node: TerminalWindowNode,
  tabId: string,
): TerminalWindowNode {
  const leaf = findTerminalWindowLeafByTabId(node, tabId);
  if (!leaf) return node;
  return setLeafActiveTab(node, leaf.id, tabId);
}

export function insertTabAfterInLeaf(
  node: TerminalWindowNode,
  anchorTabId: string,
  newTabId: string,
  activeTabId?: string | null,
): TerminalWindowNode {
  const leaf = findTerminalWindowLeafByTabId(node, anchorTabId);
  if (!leaf) return node;

  return updateLeafById(node, leaf.id, (currentLeaf) => {
    const index = currentLeaf.tabIds.indexOf(anchorTabId);
    const nextTabIds = [...currentLeaf.tabIds];

    if (!nextTabIds.includes(newTabId)) {
      nextTabIds.splice(index + 1, 0, newTabId);
    }

    return normalizeLeaf({
      ...currentLeaf,
      tabIds: nextTabIds,
      activeTabId: activeTabId === undefined ? currentLeaf.activeTabId : activeTabId,
    });
  });
}

export function insertTabIntoLeaf(
  node: TerminalWindowNode,
  leafId: string,
  newTabId: string,
  options?: {
    afterTabId?: string | null;
    activeTabId?: string | null;
  },
): TerminalWindowNode {
  return updateLeafById(node, leafId, (currentLeaf) => {
    const nextTabIds = [...currentLeaf.tabIds];
    if (!nextTabIds.includes(newTabId)) {
      const anchorTabId =
        options?.afterTabId && nextTabIds.includes(options.afterTabId)
          ? options.afterTabId
          : currentLeaf.activeTabId && nextTabIds.includes(currentLeaf.activeTabId)
            ? currentLeaf.activeTabId
            : (nextTabIds[nextTabIds.length - 1] ?? null);
      const insertIndex = anchorTabId ? nextTabIds.indexOf(anchorTabId) + 1 : nextTabIds.length;
      nextTabIds.splice(insertIndex, 0, newTabId);
    }

    return normalizeLeaf({
      ...currentLeaf,
      tabIds: nextTabIds,
      activeTabId:
        options?.activeTabId === undefined ? currentLeaf.activeTabId : options.activeTabId,
    });
  });
}

export function reorderTabsInLeaf(
  node: TerminalWindowNode,
  fromTabId: string,
  toIndex: number,
): TerminalWindowNode {
  const leaf = findTerminalWindowLeafByTabId(node, fromTabId);
  if (!leaf) return node;

  return updateLeafById(node, leaf.id, (currentLeaf) => {
    const fromIndex = currentLeaf.tabIds.indexOf(fromTabId);
    if (fromIndex === -1) return currentLeaf;

    const boundedIndex = Math.max(0, Math.min(currentLeaf.tabIds.length - 1, toIndex));
    if (fromIndex === boundedIndex) return currentLeaf;

    const nextTabIds = [...currentLeaf.tabIds];
    const [tabId] = nextTabIds.splice(fromIndex, 1);
    nextTabIds.splice(boundedIndex, 0, tabId);

    return normalizeLeaf({
      ...currentLeaf,
      tabIds: nextTabIds,
    });
  });
}

export function updateTerminalWindowSplitRatio(
  node: TerminalWindowNode,
  splitId: string,
  ratio: number,
): TerminalWindowNode {
  if (!isTerminalWindowSplit(node)) {
    return node;
  }

  if (node.id === splitId) {
    return {
      ...node,
      ratio: Math.max(0.2, Math.min(0.8, ratio)),
    };
  }

  const nextFirst = updateTerminalWindowSplitRatio(node.first, splitId, ratio);
  const nextSecond = updateTerminalWindowSplitRatio(node.second, splitId, ratio);

  if (nextFirst === node.first && nextSecond === node.second) {
    return node;
  }

  return {
    ...node,
    first: nextFirst,
    second: nextSecond,
  };
}

export function splitTerminalWindowForTab(
  node: TerminalWindowNode,
  tabId: string,
  direction: PaneSplitDirection,
  newTabId?: string,
): TerminalWindowNode {
  const leaf = findTerminalWindowLeafByTabId(node, tabId);
  if (!leaf) return node;

  return updateLeafById(node, leaf.id, (currentLeaf) => {
    if (newTabId) {
      return {
        id: createTerminalWindowId("window-split"),
        kind: "split",
        direction,
        ratio: 0.5,
        first: normalizeLeaf(currentLeaf),
        second: createTerminalWindowLeaf([newTabId], newTabId),
      };
    }

    if (currentLeaf.tabIds.length <= 1) {
      return currentLeaf;
    }

    const remainingTabIds = currentLeaf.tabIds.filter((id) => id !== tabId);
    const remainingLeaf = normalizeLeaf({
      ...currentLeaf,
      tabIds: remainingTabIds,
      activeTabId:
        currentLeaf.activeTabId === tabId ? (remainingTabIds[0] ?? null) : currentLeaf.activeTabId,
    });

    return {
      id: createTerminalWindowId("window-split"),
      kind: "split",
      direction,
      ratio: 0.5,
      first: remainingLeaf,
      second: createTerminalWindowLeaf([tabId], tabId),
    };
  });
}

export function moveTabBetweenLeaves(
  node: TerminalWindowNode,
  tabId: string,
  targetLeafId: string,
  insertIndex: number,
): TerminalWindowNode | null {
  const sourceLeaf = findTerminalWindowLeafByTabId(node, tabId);
  if (!sourceLeaf) return node;
  if (sourceLeaf.id === targetLeafId) {
    return reorderTabsInLeaf(node, tabId, insertIndex);
  }

  // Do not use removeTabFromTerminalWindows here. When the source split leaf
  // contains only the dragged tab, that helper intentionally collapses empty
  // leaves. During a cross-pane drag the target leaf may be the sibling of
  // that empty leaf, so collapsing first loses the target and the drop fails.
  const next = updateLeafById(node, sourceLeaf.id, (leaf) =>
    normalizeLeaf({
      ...leaf,
      tabIds: leaf.tabIds.filter((id) => id !== tabId),
      activeTabId: leaf.activeTabId === tabId ? null : leaf.activeTabId,
    }),
  );

  return updateLeafById(next, targetLeafId, (leaf) => {
    const nextTabIds = [...leaf.tabIds];
    const bounded = Math.max(0, Math.min(nextTabIds.length, insertIndex));
    nextTabIds.splice(bounded, 0, tabId);
    return normalizeLeaf({ ...leaf, tabIds: nextTabIds, activeTabId: tabId });
  });
}

export function splitLeafWithTab(
  node: TerminalWindowNode,
  tabId: string,
  targetLeafId: string,
  edge: SplitEdgeDirection,
): TerminalWindowNode | null {
  const sourceLeaf = findTerminalWindowLeafByTabId(node, tabId);
  const targetLeaf = findTerminalWindowLeafById(node, targetLeafId);
  if (!sourceLeaf || !targetLeaf) return node;

  if (sourceLeaf.id === targetLeafId && sourceLeaf.tabIds.length <= 1) {
    return node;
  }

  const next = removeTabFromTerminalWindows(node, tabId);
  if (!next) return null;

  const direction: PaneSplitDirection =
    edge === "left" || edge === "right" ? "vertical" : "horizontal";
  const tabLeaf = createTerminalWindowLeaf([tabId], tabId);

  return updateLeafById(next, targetLeafId, (leaf) => {
    const target = normalizeLeaf(leaf);
    const tabFirst = edge === "left" || edge === "top";

    return {
      id: createTerminalWindowId("window-split"),
      kind: "split",
      direction,
      ratio: 0.5,
      first: tabFirst ? tabLeaf : target,
      second: tabFirst ? target : tabLeaf,
    };
  });
}

export function flattenTerminalWindows(
  node: TerminalWindowNode,
  activeTabId: string | null,
): TerminalWindowNode {
  const tabIds = collectWindowTabIds(node);
  if (tabIds.length === 0) return node;
  return createTerminalWindowLeaf(tabIds, activeTabId ?? tabIds[0] ?? null);
}

export function serializeTerminalWindowLayout(
  node: TerminalWindowNode | null,
  tabs: Tab[],
): RestorableTerminalWindowNode | null {
  if (!node || tabs.length === 0) return null;

  // Layout indexes must match the open_tabs order in serializeTabsForPersistence.
  const restorableTabs = tabs
    .filter((tab) => collectSessionPanes(tab.root).some((pane) => pane.paneKind !== "file"))
    .sort((a, b) => a.persistOrder - b.persistOrder);
  const tabIndexById = new Map(restorableTabs.map((tab, index) => [tab.id, index]));

  const serialize = (current: TerminalWindowNode): RestorableTerminalWindowNode | null => {
    if (!isTerminalWindowSplit(current)) {
      const tabIndexes = current.tabIds
        .map((tabId) => tabIndexById.get(tabId))
        .filter((index): index is number => index !== undefined);

      if (tabIndexes.length === 0) return null;

      const activeTabIndex =
        current.activeTabId && tabIndexById.has(current.activeTabId)
          ? (tabIndexById.get(current.activeTabId) ?? null)
          : (tabIndexes[0] ?? null);

      return {
        kind: "leaf",
        tab_indexes: Array.from(new Set(tabIndexes)),
        active_tab_index: activeTabIndex,
      };
    }

    const first = serialize(current.first);
    const second = serialize(current.second);
    if (!first && !second) return null;
    if (!first) return second;
    if (!second) return first;

    return {
      kind: "split",
      direction: current.direction,
      ratio: Math.max(0.2, Math.min(0.8, current.ratio)),
      first,
      second,
    };
  };

  return serialize(node);
}

export function restoreTerminalWindowLayout(
  layout: RestorableTerminalWindowNode | null | undefined,
  tabs: Tab[],
): TerminalWindowNode | null {
  if (!layout || tabs.length === 0) return null;

  const tabIds = tabs.map((tab) => tab.id);
  const usedTabIds = new Set<string>();

  const restore = (current: RestorableTerminalWindowNode): TerminalWindowNode | null => {
    if (current.kind === "leaf") {
      const leafTabIds: string[] = [];

      for (const index of current.tab_indexes) {
        const tabId = tabIds[index];
        if (!tabId || usedTabIds.has(tabId)) continue;
        usedTabIds.add(tabId);
        leafTabIds.push(tabId);
      }

      if (leafTabIds.length === 0) return null;

      const activeTabId =
        current.active_tab_index !== null && current.active_tab_index !== undefined
          ? (tabIds[current.active_tab_index] ?? null)
          : null;

      return createTerminalWindowLeaf(
        leafTabIds,
        activeTabId && leafTabIds.includes(activeTabId) ? activeTabId : leafTabIds[0],
      );
    }

    const first = restore(current.first);
    const second = restore(current.second);
    if (!first && !second) return null;
    if (!first) return second;
    if (!second) return first;

    return {
      id: createTerminalWindowId("window-split"),
      kind: "split",
      direction: current.direction,
      ratio: Math.max(0.2, Math.min(0.8, current.ratio)),
      first,
      second,
    };
  };

  const restored = restore(layout);
  if (!restored) return null;

  const restoredTabIds = new Set(collectWindowTabIds(restored));
  if (tabIds.some((tabId) => !restoredTabIds.has(tabId))) {
    return null;
  }

  return restored;
}

export function reconcileTerminalWindows(
  current: TerminalWindowNode | null,
  tabs: Tab[],
  activeTabId: string | null,
  anchorTabId: string | null,
): TerminalWindowNode | null {
  const tabIds = tabs.map((tab) => tab.id);
  if (tabIds.length === 0) {
    return null;
  }

  if (!current) {
    return createTerminalWindowRoot(tabIds, activeTabId ?? tabIds[0] ?? null);
  }

  let next: TerminalWindowNode | null = current;

  for (const existingTabId of collectWindowTabIds(current)) {
    if (!tabIds.includes(existingTabId)) {
      next = removeTabFromTerminalWindows(next, existingTabId);
    }
  }

  if (!next) {
    return createTerminalWindowRoot(tabIds, activeTabId ?? tabIds[0] ?? null);
  }

  const layoutTabIds = new Set(collectWindowTabIds(next));
  const missingTabIds = tabIds.filter((tabId) => !layoutTabIds.has(tabId));

  if (missingTabIds.length > 0) {
    const insertAnchorTabId =
      (anchorTabId && layoutTabIds.has(anchorTabId) ? anchorTabId : null) ??
      (activeTabId && layoutTabIds.has(activeTabId) ? activeTabId : null) ??
      getFirstTerminalWindowLeaf(next).tabIds[
        Math.max(0, getFirstTerminalWindowLeaf(next).tabIds.length - 1)
      ] ??
      tabIds[0];

    let currentAnchorTabId = insertAnchorTabId;

    for (const missingTabId of missingTabIds) {
      next = insertTabAfterInLeaf(next, currentAnchorTabId, missingTabId, missingTabId);
      currentAnchorTabId = missingTabId;
    }
  }

  if (activeTabId) {
    next = setLeafActiveTabByTabId(next, activeTabId);
  }

  return next;
}
