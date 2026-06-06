import type { TerminalWindowNode } from "@/lib/tabWindows";
import { collectSessionPanes } from "@/lib/workspaceTabs";
import type { ActivityBarLayout, SessionPane, SessionType, Tab } from "@/types/global";

export const NON_PANEL_IDS = new Set(["settings", "lock", "quickCmdBar", "serialSend"]);

export type TrayAction =
  | { type: "open_new_session"; targetWindowLabel?: string | null }
  | { type: "focus_session"; sessionId: string; targetWindowLabel?: string | null }
  | {
      type: "open_panel";
      panelId: "activeSessions" | "syncBackupHistory";
      targetWindowLabel?: string | null;
    }
  | { type: "open_settings"; targetWindowLabel?: string | null }
  | { type: "lock_screen"; targetWindowLabel?: string | null }
  | { type: "check_updates"; targetWindowLabel?: string | null }
  | { type: "request_quit"; targetWindowLabel?: string | null };

export function canCreateSessionFromPane(
  pane: Pick<SessionPane, "type" | "connectionId"> | null | undefined,
): pane is Pick<SessionPane, "type" | "connectionId"> {
  return !!pane && (pane.type === "Local" || !!pane.connectionId);
}

export function hasLiveSession<T extends Pick<SessionPane, "connecting" | "connectError">>(
  pane: T | null | undefined,
): pane is T {
  return !!pane && !pane.connecting && !pane.connectError;
}

export function isNonSerialSessionType(type: SessionType): boolean {
  return type === "SSH" || type === "Local" || type === "Telnet";
}

export function getItemSide(id: string, layout: ActivityBarLayout): "left" | "right" | null {
  if (layout.left_top.includes(id) || layout.left_bottom.includes(id)) return "left";
  if (layout.right_top.includes(id) || layout.right_bottom.includes(id)) return "right";
  return null;
}

export function collectActiveNonSerialSessionIds(
  layout: TerminalWindowNode | null,
  tabsById: Map<string, Tab>,
) {
  if (!layout) return [];

  const sessionIds = new Set<string>();

  const visit = (node: TerminalWindowNode) => {
    if (node.kind === "split") {
      visit(node.first);
      visit(node.second);
      return;
    }

    for (const tabId of node.tabIds) {
      const tab = tabsById.get(tabId);
      if (!tab) continue;

      for (const pane of collectSessionPanes(tab.root)) {
        if (hasLiveSession(pane) && isNonSerialSessionType(pane.type)) {
          sessionIds.add(pane.sessionId);
        }
      }
    }
  };

  visit(layout);
  return [...sessionIds];
}
