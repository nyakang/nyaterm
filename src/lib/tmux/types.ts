/** Types mirroring the backend `tmux-session-state` payload. */

import type { PaneSplitDirection } from "@/types/global";

export interface TmuxWindowInfo {
  id: string;
  name: string;
  active: boolean;
}

export type TmuxLayoutNode =
  | {
      kind: "pane";
      paneId: string;
      sessionId: string;
      name: string;
    }
  | {
      kind: "split";
      direction: PaneSplitDirection;
      ratio: number;
      first: TmuxLayoutNode;
      second: TmuxLayoutNode;
    };

export interface TmuxSessionState {
  controlSessionId: string;
  exited: boolean;
  windows: TmuxWindowInfo[];
  activeWindowId?: string | null;
  activePaneSessionId?: string | null;
  tree?: TmuxLayoutNode | null;
}
