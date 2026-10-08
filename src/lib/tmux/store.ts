/** Latest tmux control-mode state per control session, for the tmux bar. */

import { useSyncExternalStore } from "react";
import type { TmuxSessionState } from "./types";

const states = new Map<string, TmuxSessionState>();
const listeners = new Set<() => void>();

export function updateTmuxState(state: TmuxSessionState) {
  if (state.exited) {
    states.delete(state.controlSessionId);
  } else {
    states.set(state.controlSessionId, state);
  }
  for (const listener of listeners) listener();
}

export function removeTmuxState(controlSessionId: string) {
  states.delete(controlSessionId);
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Live state for a tmux control session; `null` while/after it is gone. */
export function useTmuxSessionState(
  controlSessionId: string | null | undefined,
): TmuxSessionState | null {
  return useSyncExternalStore(subscribe, () =>
    controlSessionId ? (states.get(controlSessionId) ?? null) : null,
  );
}
