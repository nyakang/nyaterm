/** Map backend tmux layout trees onto the workspace `PaneNode` model. */

import {
  collectSessionPanes,
  createWorkspaceId,
} from "@/lib/workspaceTabs";
import type { PaneNode, SessionPane, Tab } from "@/types/global";
import type { TmuxLayoutNode, TmuxSessionState } from "./types";

/** The control session owning this tab's tmux panes, if the tab is in tmux mode. */
export function findTmuxControlSessionId(tab: Tab): string | null {
  for (const pane of collectSessionPanes(tab.root)) {
    if (pane.tmux) return pane.tmux.controlSessionId;
  }
  return null;
}

/**
 * Locate the tab that owns `controlSessionId` — either the session is still a
 * plain leaf (control mode was just detected) or one of its virtual panes.
 */
export function findTmuxTab(tabs: Tab[], controlSessionId: string): Tab | null {
  for (const tab of tabs) {
    for (const pane of collectSessionPanes(tab.root)) {
      if (
        pane.sessionId === controlSessionId ||
        pane.tmux?.controlSessionId === controlSessionId
      ) {
        return tab;
      }
    }
  }
  return null;
}

/**
 * Build the workspace pane tree for a tmux window. Existing leaf ids are
 * preserved per tmux pane id so React keys and xterm instances stay stable
 * across layout changes.
 */
export function buildTmuxPaneTree(
  state: TmuxSessionState,
  existing: Tab,
): { root: PaneNode; activePaneId: string } | null {
  if (!state.tree) return null;

  const paneIdByTmuxPane = new Map<string, string>();
  for (const pane of collectSessionPanes(existing.root)) {
    if (pane.tmux) {
      paneIdByTmuxPane.set(pane.tmux.paneId, pane.id);
    }
  }

  const basePane = collectSessionPanes(existing.root).find(
    (pane) =>
      pane.sessionId === state.controlSessionId || pane.tmux != null,
  );

  const convert = (node: TmuxLayoutNode): PaneNode => {
    if (node.kind === "pane") {
      const pane: SessionPane = {
        id: paneIdByTmuxPane.get(node.paneId) ?? createWorkspaceId("pane"),
        kind: "leaf",
        paneKind: "terminal",
        sessionId: node.sessionId,
        // Keep the connection's session name so the tab title stays the
        // connection label instead of the pane's current command.
        name: basePane?.name ?? node.name,
        type: "SSH",
        connectionId: basePane?.connectionId,
        tmux: {
          controlSessionId: state.controlSessionId,
          paneId: node.paneId,
        },
      };
      return pane;
    }
    return {
      id: createWorkspaceId("pane"),
      kind: "split",
      direction: node.direction,
      // tmux-owned layout — the divider is fixed at 50/50 and not draggable.
      // Feeding tmux's real ratio back here closes a feedback loop: renderer
      // reports drive resize-pane, whose layout echo would shift the
      // containers, which report new sizes — a ratchet that runs away on any
      // px-level asymmetry. Fixed ratios keep renderer sizes stable so the
      // backend sync converges instead.
      ratio: 0.5,
      first: convert(node.first),
      second: convert(node.second),
    };
  };

  const root = convert(state.tree);
  const paneIds = new Set(collectSessionPanes(root).map((pane) => pane.id));
  const activePaneId =
    (paneIds.has(existing.activePaneId) ? existing.activePaneId : null) ??
    (state.activePaneSessionId
      ? collectSessionPanes(root).find(
          (pane) => pane.sessionId === state.activePaneSessionId,
        )?.id
      : undefined) ??
    collectSessionPanes(root)[0]?.id ??
    existing.activePaneId;

  return { root, activePaneId };
}

/** Leaf shown after the tmux control client detached: a plain shell pane. */
export function tmuxFallbackLeaf(
  state: TmuxSessionState,
  existing: Tab,
): SessionPane {
  const basePane = collectSessionPanes(existing.root)[0];
  const pane: SessionPane = {
    id: createWorkspaceId("pane"),
    kind: "leaf",
    paneKind: "terminal",
    sessionId: state.controlSessionId,
    name: basePane?.name ?? "shell",
    type: "SSH",
    connectionId: basePane?.connectionId,
    temporaryConfig: basePane?.temporaryConfig,
    sshRuntimeMode: basePane?.sshRuntimeMode,
  };
  return pane;
}
