const IS_MAC = navigator.userAgent.includes("Mac");

/** Platform-aware modifier label for shortcut display. */
export const MOD = IS_MAC ? "\u2318" : "Ctrl";

export type ShortcutCategory = "terminal" | "tab" | "view" | "special" | "fileExplorer";

export interface ShortcutDefinition {
  id: string;
  category: ShortcutCategory;
  labelKey: string;
  /** Default hotkey string for react-hotkeys-hook, e.g. "ctrl+shift+c, meta+shift+c" */
  defaultKeys: string;
  /** When true, the shortcut is scoped to a specific context and not registered globally. */
  contextual?: boolean;
}

export const SHORTCUT_CATEGORIES: { key: ShortcutCategory; labelKey: string }[] = [
  { key: "terminal", labelKey: "settings.shortcutCategories.terminal" },
  { key: "tab", labelKey: "settings.shortcutCategories.tab" },
  { key: "view", labelKey: "settings.shortcutCategories.view" },
  { key: "fileExplorer", labelKey: "settings.shortcutCategories.fileExplorer" },
  { key: "special", labelKey: "settings.shortcutCategories.special" },
];

/**
 * Central registry of all keyboard shortcuts.
 * `defaultKeys` uses react-hotkeys-hook format: comma-separated combos with `+`-joined modifiers.
 */
export const SHORTCUT_REGISTRY: ShortcutDefinition[] = [
  // --- Terminal ---
  {
    id: "terminal.copy",
    category: "terminal",
    labelKey: "terminalCtx.copy",
    defaultKeys: "ctrl+shift+c, meta+shift+c",
  },
  {
    id: "terminal.paste",
    category: "terminal",
    labelKey: "terminalCtx.paste",
    defaultKeys: "ctrl+shift+v, meta+shift+v, shift+insert",
  },
  {
    id: "terminal.pasteSelected",
    category: "terminal",
    labelKey: "terminalCtx.pasteSelectedText",
    defaultKeys: "ctrl+shift+x, meta+shift+x",
  },
  {
    id: "terminal.find",
    category: "terminal",
    labelKey: "terminalCtx.find",
    defaultKeys: "ctrl+shift+f, meta+shift+f",
  },
  {
    id: "terminal.clear",
    category: "terminal",
    labelKey: "terminalCtx.clearScreen",
    defaultKeys: "ctrl+shift+k, meta+shift+k",
  },
  {
    id: "terminal.selectAll",
    category: "terminal",
    labelKey: "terminalCtx.selectAll",
    defaultKeys: "ctrl+shift+a, meta+shift+a",
  },
  {
    id: "terminal.manageSyncGroups",
    category: "terminal",
    labelKey: "settings.shortcutLabels.manageSyncGroups",
    defaultKeys: "ctrl+shift+g, meta+shift+g",
  },

  // --- Tab / Session ---
  {
    id: "tab.newSession",
    category: "tab",
    labelKey: "settings.shortcutLabels.newSession",
    defaultKeys: "ctrl+shift+n, meta+shift+n",
  },
  {
    id: "tab.quickSwitch",
    category: "tab",
    labelKey: "settings.shortcutLabels.quickSwitch",
    defaultKeys: "ctrl+shift+s, meta+shift+s",
  },
  {
    id: "tab.newLocalTerminal",
    category: "tab",
    labelKey: "settings.shortcutLabels.newLocalTerminal",
    defaultKeys: "ctrl+`, meta+`",
  },
  {
    id: "tab.close",
    category: "tab",
    labelKey: "settings.shortcutLabels.closeTab",
    defaultKeys: "ctrl+shift+w, meta+shift+w",
  },
  {
    id: "tab.next",
    category: "tab",
    labelKey: "settings.shortcutLabels.nextTab",
    defaultKeys: "ctrl+tab",
  },
  {
    id: "tab.prev",
    category: "tab",
    labelKey: "settings.shortcutLabels.prevTab",
    defaultKeys: "ctrl+shift+tab",
  },
  {
    id: "tab.switchTo",
    category: "tab",
    labelKey: "settings.shortcutLabels.switchTab",
    defaultKeys: "ctrl+1-9, meta+1-9",
  },

  // --- View / Layout ---
  {
    id: "view.toggleLeftSidebar",
    category: "view",
    labelKey: "settings.shortcutLabels.toggleLeftSidebar",
    defaultKeys: "ctrl+shift+e, meta+shift+e",
  },
  {
    id: "view.toggleRightSidebar",
    category: "view",
    labelKey: "settings.shortcutLabels.toggleRightSidebar",
    defaultKeys: "ctrl+shift+b, meta+shift+b",
  },
  {
    id: "view.zoomIn",
    category: "view",
    labelKey: "settings.shortcutLabels.zoomIn",
    defaultKeys: "ctrl+=, meta+=, ctrl+shift+=, meta+shift+=",
  },
  {
    id: "view.zoomOut",
    category: "view",
    labelKey: "settings.shortcutLabels.zoomOut",
    defaultKeys: "ctrl+-, meta+-",
  },
  {
    id: "view.resetZoom",
    category: "view",
    labelKey: "settings.shortcutLabels.resetZoom",
    defaultKeys: "ctrl+0, meta+0",
  },
  {
    id: "view.openSettings",
    category: "view",
    labelKey: "settings.shortcutLabels.openSettings",
    defaultKeys: "ctrl+comma, meta+comma",
  },
  {
    id: "view.openChat",
    category: "view",
    labelKey: "settings.shortcutLabels.openChat",
    defaultKeys: "ctrl+alt+i, meta+alt+i",
  },
  {
    id: "view.showAllCommands",
    category: "view",
    labelKey: "settings.shortcutLabels.showAllCommands",
    defaultKeys: "ctrl+shift+p, meta+shift+p",
  },

  // --- File Explorer ---
  {
    id: "fileExplorer.rename",
    category: "fileExplorer",
    labelKey: "settings.shortcutLabels.renameFile",
    defaultKeys: "F2",
    contextual: true,
  },

  // --- Special ---
  {
    id: "special.lockScreen",
    category: "special",
    labelKey: "settings.shortcutLabels.lockScreen",
    defaultKeys: "ctrl+shift+l, meta+shift+l",
  },
];

const registryMap = new Map<string, ShortcutDefinition>();
for (const def of SHORTCUT_REGISTRY) {
  registryMap.set(def.id, def);
}

export function getShortcutDefinition(id: string): ShortcutDefinition | undefined {
  return registryMap.get(id);
}

export function getDefaultKeys(id: string): string {
  return registryMap.get(id)?.defaultKeys ?? "";
}

/** Resolve the effective hotkey string for a shortcut, considering user overrides. */
export function resolveKeys(id: string, overrides: Record<string, string>): string {
  if (id in overrides) return overrides[id];
  return getDefaultKeys(id);
}

/**
 * Parse a react-hotkeys-hook key string into a display-friendly format.
 * Takes the first combo (before comma) and formats modifier names for the current platform.
 */
export function formatKeysForDisplay(keys: string): string {
  const first = keys.split(",")[0].trim();

  return first
    .split("+")
    .map((k) => formatSingleKey(k.trim()))
    .join("+");
}

/** Format the tab index shortcut as its generated range, e.g. `Ctrl+1-9`. */
export function formatIndexedKeysForDisplay(keys: string): string {
  const first = keys.split(",")[0].trim();
  return formatKeysForDisplay(first.replace(/\+1$/i, "+1-9"));
}

/** Expand a tab index shortcut template into concrete key combinations for a numbered tab. */
export function resolveIndexedKeys(keys: string, tabNumber: number): string {
  const digit = String(tabNumber);
  return keys
    .split(",")
    .map((combo) => {
      const trimmed = combo.trim();
      if (/(?:1-9|[1-9])$/i.test(trimmed)) {
        return trimmed.replace(/(?:1-9|[1-9])$/i, digit);
      }
      if (isModifierOnlyCombo(trimmed)) {
        return `${trimmed}+${digit}`;
      }
      return tabNumber === 1 ? trimmed : "";
    })
    .filter(Boolean)
    .join(", ");
}

/** A numbered-tab template must be a chord whose captured terminal key is `1`. */
export function isValidIndexedShortcutTemplate(keys: string): boolean {
  const combos = keys
    .split(",")
    .map((combo) => combo.trim())
    .filter(Boolean);

  return (
    combos.length > 0 &&
    combos.every((combo) => {
      const parts = combo
        .split("+")
        .map((key) => key.trim().toLowerCase())
        .filter(Boolean);
      return parts.length > 1 && parts[parts.length - 1] === "1";
    })
  );
}

function isModifierOnlyCombo(combo: string): boolean {
  return combo
    .split("+")
    .map((key) => key.trim().toLowerCase())
    .every((key) => ["ctrl", "meta", "shift", "alt"].includes(key));
}

function formatSingleKey(key: string): string {
  const lower = key.toLowerCase();
  if (lower === "ctrl" || lower === "meta") return MOD;
  if (lower === "shift") return "Shift";
  if (lower === "alt") return "Alt";
  if (lower === "comma") return ",";
  if (lower === "tab") return "Tab";
  if (lower === "escape") return "Esc";
  if (lower === "backquote") return "`";
  if (lower === "space") return "Space";
  if (lower === "delete") return "Delete";
  if (lower === "insert") return "Insert";
  if (lower === "backspace") return "Backspace";
  if (lower === "enter") return "Enter";
  if (lower === "arrowup") return "\u2191";
  if (lower === "arrowdown") return "\u2193";
  if (lower === "arrowleft") return "\u2190";
  if (lower === "arrowright") return "\u2192";
  if (/^f\d+$/i.test(lower)) return key.toUpperCase();
  return key.toUpperCase();
}

/**
 * Convert a KeyboardEvent into a hotkey string suitable for react-hotkeys-hook registration.
 * Uses `e.code` (physical key) for reliable locale-independent capture, then converts
 * back to the react-hotkeys-hook key name format.
 * Returns both ctrl and meta variants for cross-platform support when a modifier is used.
 */
export function keyEventToHotkeyString(e: KeyboardEvent): string {
  const parts: string[] = [];
  const hasCtrlOrMeta = e.ctrlKey || e.metaKey;
  if (e.shiftKey) parts.push("shift");
  if (e.altKey) parts.push("alt");

  const keyName = codeToKeyName(e.code);
  if (!keyName) return "";
  parts.push(keyName);

  if (hasCtrlOrMeta) {
    const ctrlCombo = ["ctrl", ...parts].join("+");
    const metaCombo = ["meta", ...parts].join("+");
    return `${ctrlCombo}, ${metaCombo}`;
  }
  return parts.join("+");
}

/** Capture a numbered-tab shortcut prefix, including a modifier key pressed on its own. */
export function keyEventToIndexedHotkeyString(e: KeyboardEvent): string {
  const combo = keyEventToHotkeyString(e);
  if (combo) return combo;
  if (!/^(Control|Meta|Alt|Shift)(Left|Right)?$/.test(e.code)) return "";

  const parts: string[] = [];
  if (e.shiftKey) parts.push("shift");
  if (e.altKey) parts.push("alt");
  if (e.ctrlKey || e.metaKey) {
    return `${["ctrl", ...parts].join("+")}, ${["meta", ...parts].join("+")}`;
  }
  return parts.join("+");
}

/**
 * Convert a KeyboardEvent.code value to a react-hotkeys-hook key name.
 * Returns empty string for pure modifier keys.
 */
function codeToKeyName(code: string): string {
  if (CODE_TO_KEY[code]) return CODE_TO_KEY[code];
  if (/^F\d+$/.test(code)) return code;
  return "";
}

/**
 * Reverse mapping from KeyboardEvent.code to hotkey key names used by react-hotkeys-hook.
 */
const CODE_TO_KEY: Record<string, string> = {
  KeyA: "a",
  KeyB: "b",
  KeyC: "c",
  KeyD: "d",
  KeyE: "e",
  KeyF: "f",
  KeyG: "g",
  KeyH: "h",
  KeyI: "i",
  KeyJ: "j",
  KeyK: "k",
  KeyL: "l",
  KeyM: "m",
  KeyN: "n",
  KeyO: "o",
  KeyP: "p",
  KeyQ: "q",
  KeyR: "r",
  KeyS: "s",
  KeyT: "t",
  KeyU: "u",
  KeyV: "v",
  KeyW: "w",
  KeyX: "x",
  KeyY: "y",
  KeyZ: "z",
  Digit0: "0",
  Digit1: "1",
  Digit2: "2",
  Digit3: "3",
  Digit4: "4",
  Digit5: "5",
  Digit6: "6",
  Digit7: "7",
  Digit8: "8",
  Digit9: "9",
  Equal: "=",
  Minus: "-",
  Backquote: "backquote",
  Comma: "comma",
  Tab: "tab",
  Escape: "escape",
  Space: "space",
  Enter: "enter",
  Backspace: "backspace",
  Delete: "delete",
  Insert: "insert",
  BracketLeft: "[",
  BracketRight: "]",
  Semicolon: ";",
  Quote: "'",
  Backslash: "\\",
  Slash: "/",
  Period: ".",
  ArrowUp: "arrowup",
  ArrowDown: "arrowdown",
  ArrowLeft: "arrowleft",
  ArrowRight: "arrowright",
};

interface ParsedCombo {
  ctrl: boolean;
  meta: boolean;
  shift: boolean;
  alt: boolean;
  /** The KeyboardEvent.code value to match against (e.g. "KeyH", "F2"). */
  code: string;
}

/**
 * Maps react-hotkeys-hook key names (lowercase) to KeyboardEvent.code values.
 */
const KEY_TO_CODE: Record<string, string> = {
  a: "KeyA",
  b: "KeyB",
  c: "KeyC",
  d: "KeyD",
  e: "KeyE",
  f: "KeyF",
  g: "KeyG",
  h: "KeyH",
  i: "KeyI",
  j: "KeyJ",
  k: "KeyK",
  l: "KeyL",
  m: "KeyM",
  n: "KeyN",
  o: "KeyO",
  p: "KeyP",
  q: "KeyQ",
  r: "KeyR",
  s: "KeyS",
  t: "KeyT",
  u: "KeyU",
  v: "KeyV",
  w: "KeyW",
  x: "KeyX",
  y: "KeyY",
  z: "KeyZ",
  "0": "Digit0",
  "1": "Digit1",
  "2": "Digit2",
  "3": "Digit3",
  "4": "Digit4",
  "5": "Digit5",
  "6": "Digit6",
  "7": "Digit7",
  "8": "Digit8",
  "9": "Digit9",
  "=": "Equal",
  "-": "Minus",
  backquote: "Backquote",
  "`": "Backquote",
  comma: "Comma",
  ",": "Comma",
  tab: "Tab",
  escape: "Escape",
  space: "Space",
  enter: "Enter",
  backspace: "Backspace",
  delete: "Delete",
  insert: "Insert",
  "[": "BracketLeft",
  "]": "BracketRight",
  ";": "Semicolon",
  "'": "Quote",
  "\\": "Backslash",
  "/": "Slash",
  ".": "Period",
  arrowup: "ArrowUp",
  arrowdown: "ArrowDown",
  arrowleft: "ArrowLeft",
  arrowright: "ArrowRight",
};

/**
 * Parse a single hotkey combo string (e.g. "ctrl+shift+c") into a structured object
 * suitable for matching against KeyboardEvent.code values.
 */
function parseSingleCombo(combo: string): ParsedCombo {
  const parts = combo
    .trim()
    .split("+")
    .map((p) => p.trim().toLowerCase());
  let ctrl = false;
  let meta = false;
  let shift = false;
  let alt = false;
  let keyPart = "";

  for (const p of parts) {
    if (p === "ctrl") ctrl = true;
    else if (p === "meta") meta = true;
    else if (p === "shift") shift = true;
    else if (p === "alt") alt = true;
    else keyPart = p;
  }

  let code: string;
  if (KEY_TO_CODE[keyPart]) {
    code = KEY_TO_CODE[keyPart];
  } else if (/^f\d+$/.test(keyPart)) {
    code = keyPart.toUpperCase();
  } else {
    code = keyPart;
  }
  return { ctrl, meta, shift, alt, code };
}

/**
 * Parse a full hotkey string (potentially with comma-separated alternatives) into
 * an array of parsed combos for matching.
 */
export function parseHotkeyString(keys: string): ParsedCombo[] {
  if (!keys) return [];
  return keys.split(",").map(parseSingleCombo);
}

/**
 * Check if a KeyboardEvent matches any combo in the given hotkey string.
 * Used by the xterm key handler and file explorer to decide which keys to intercept.
 */
export function matchesKeyEvent(keys: string, e: KeyboardEvent): boolean {
  const combos = parseHotkeyString(keys);
  const ctrl = e.ctrlKey;
  const meta = e.metaKey;
  const shift = e.shiftKey;
  const alt = e.altKey;
  const code = e.code;

  return combos.some(
    (c) =>
      c.ctrl === ctrl && c.meta === meta && c.shift === shift && c.alt === alt && c.code === code,
  );
}
