function normalizeInteger(value: number | null | undefined, fallback: number): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return fallback;
  }

  return Math.trunc(value);
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

export const DEFAULT_COMMAND_SUGGESTION_MIN_CHARS = 2;
export const MIN_COMMAND_SUGGESTION_MIN_CHARS = 1;
export const MAX_COMMAND_SUGGESTION_MIN_CHARS = 500;
export const DEFAULT_COMMAND_SUGGESTION_MAX_CHARS = 64;
export const MIN_COMMAND_SUGGESTION_MAX_CHARS = 1;
export const MAX_COMMAND_SUGGESTION_MAX_CHARS = 500;

export const TERMINAL_RIGHT_CLICK_ACTIONS = ["none", "menu", "paste"] as const;

export type TerminalRightClickAction = (typeof TERMINAL_RIGHT_CLICK_ACTIONS)[number];

export const DEFAULT_TERMINAL_RIGHT_CLICK_ACTION: TerminalRightClickAction = "menu";

export function isTerminalRightClickAction(value: unknown): value is TerminalRightClickAction {
  return (
    typeof value === "string" &&
    TERMINAL_RIGHT_CLICK_ACTIONS.includes(value as TerminalRightClickAction)
  );
}

export function normalizeTerminalRightClickAction(
  value: unknown,
  fallback: TerminalRightClickAction = DEFAULT_TERMINAL_RIGHT_CLICK_ACTION,
): TerminalRightClickAction {
  return isTerminalRightClickAction(value) ? value : fallback;
}

export const TAB_MOUSE_ACTIONS = [
  "none",
  "rename_tab",
  "copy_tab_name",
  "copy_server_ip",
  "duplicate_session",
  "multiplex_ssh",
  "reconnect_session",
  "disconnect_session",
  "close_tab",
] as const;

export type TabMouseAction = (typeof TAB_MOUSE_ACTIONS)[number];

export const DEFAULT_TAB_DOUBLE_CLICK_ACTION: TabMouseAction = "disconnect_session";
export const DEFAULT_TAB_MIDDLE_CLICK_ACTION: TabMouseAction = "rename_tab";
export const DEFAULT_TAB_RIGHT_CLICK_ACTION: TabMouseAction = "none";

export const TAB_MOUSE_ACTION_LABEL_KEYS: Record<TabMouseAction, string> = {
  none: "settings.tabMouseActionNone",
  rename_tab: "tabCtx.rename",
  copy_tab_name: "tabCtx.copyName",
  copy_server_ip: "tabCtx.copyIp",
  duplicate_session: "tabCtx.duplicate",
  multiplex_ssh: "tabCtx.multiplexSsh",
  reconnect_session: "tabCtx.reconnect",
  disconnect_session: "tabCtx.disconnect",
  close_tab: "tabCtx.close",
};

export function isTabMouseAction(value: unknown): value is TabMouseAction {
  return typeof value === "string" && TAB_MOUSE_ACTIONS.includes(value as TabMouseAction);
}

export function normalizeTabMouseAction(
  value: unknown,
  fallback: TabMouseAction = "none",
): TabMouseAction {
  return isTabMouseAction(value) ? value : fallback;
}

export function normalizeCommandSuggestionMinChars(
  value: number | null | undefined,
  maxValue: number | null | undefined = MAX_COMMAND_SUGGESTION_MIN_CHARS,
): number {
  const normalizedMax = clamp(
    normalizeInteger(maxValue, MAX_COMMAND_SUGGESTION_MIN_CHARS),
    MIN_COMMAND_SUGGESTION_MIN_CHARS,
    MAX_COMMAND_SUGGESTION_MIN_CHARS,
  );

  return clamp(
    normalizeInteger(value, DEFAULT_COMMAND_SUGGESTION_MIN_CHARS),
    MIN_COMMAND_SUGGESTION_MIN_CHARS,
    normalizedMax,
  );
}

export function normalizeCommandSuggestionMaxChars(
  value: number | null | undefined,
  minValue: number | null | undefined = MIN_COMMAND_SUGGESTION_MAX_CHARS,
): number {
  const normalizedMin = clamp(
    normalizeInteger(minValue, MIN_COMMAND_SUGGESTION_MAX_CHARS),
    MIN_COMMAND_SUGGESTION_MAX_CHARS,
    MAX_COMMAND_SUGGESTION_MAX_CHARS,
  );

  return clamp(
    normalizeInteger(value, DEFAULT_COMMAND_SUGGESTION_MAX_CHARS),
    normalizedMin,
    MAX_COMMAND_SUGGESTION_MAX_CHARS,
  );
}
