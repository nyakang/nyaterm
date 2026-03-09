import { useHotkeys } from "react-hotkeys-hook";

const HOTKEY_OPTIONS = { enableOnFormTags: true, preventDefault: true } as const;

export interface ShortcutCallbacks {
  onNewSession: () => void;
  onNewLocalTerminal: () => void;
  onCloseTab: () => void;
  onNextTab: () => void;
  onPrevTab: () => void;
  onSwitchTab: (index: number) => void;
  onToggleLeftSidebar: () => void;
  onToggleRightSidebar: () => void;
  onZoomIn: () => void;
  onZoomOut: () => void;
  onResetZoom: () => void;
  onToggleFullscreen: () => void;
  onOpenSettings: () => void;
  onLockScreen: () => void;
}

export function useGlobalShortcuts(cb: ShortcutCallbacks) {
  // --- Tab / Session ---
  useHotkeys("ctrl+shift+n, meta+shift+n", cb.onNewSession, HOTKEY_OPTIONS);
  useHotkeys("ctrl+`, meta+`", cb.onNewLocalTerminal, HOTKEY_OPTIONS);
  useHotkeys("ctrl+shift+w, meta+shift+w", cb.onCloseTab, HOTKEY_OPTIONS);
  useHotkeys("ctrl+tab", cb.onNextTab, HOTKEY_OPTIONS);
  useHotkeys("ctrl+shift+tab", cb.onPrevTab, HOTKEY_OPTIONS);

  useHotkeys("ctrl+1, meta+1", () => cb.onSwitchTab(0), HOTKEY_OPTIONS);
  useHotkeys("ctrl+2, meta+2", () => cb.onSwitchTab(1), HOTKEY_OPTIONS);
  useHotkeys("ctrl+3, meta+3", () => cb.onSwitchTab(2), HOTKEY_OPTIONS);
  useHotkeys("ctrl+4, meta+4", () => cb.onSwitchTab(3), HOTKEY_OPTIONS);
  useHotkeys("ctrl+5, meta+5", () => cb.onSwitchTab(4), HOTKEY_OPTIONS);
  useHotkeys("ctrl+6, meta+6", () => cb.onSwitchTab(5), HOTKEY_OPTIONS);
  useHotkeys("ctrl+7, meta+7", () => cb.onSwitchTab(6), HOTKEY_OPTIONS);
  useHotkeys("ctrl+8, meta+8", () => cb.onSwitchTab(7), HOTKEY_OPTIONS);
  useHotkeys("ctrl+9, meta+9", () => cb.onSwitchTab(-1), HOTKEY_OPTIONS);

  // --- View / Layout ---
  useHotkeys("ctrl+shift+e, meta+shift+e", cb.onToggleLeftSidebar, HOTKEY_OPTIONS);
  useHotkeys("ctrl+shift+b, meta+shift+b", cb.onToggleRightSidebar, HOTKEY_OPTIONS);
  useHotkeys("ctrl+=, meta+=, ctrl+shift+=, meta+shift+=", cb.onZoomIn, HOTKEY_OPTIONS);
  useHotkeys("ctrl+-, meta+-", cb.onZoomOut, HOTKEY_OPTIONS);
  useHotkeys("ctrl+0, meta+0", cb.onResetZoom, HOTKEY_OPTIONS);
  useHotkeys("f11", cb.onToggleFullscreen, HOTKEY_OPTIONS);
  useHotkeys("ctrl+comma, meta+comma", cb.onOpenSettings, HOTKEY_OPTIONS);

  // --- Special ---
  useHotkeys("ctrl+shift+l, meta+shift+l", cb.onLockScreen, HOTKEY_OPTIONS);
}

/** Platform-aware modifier label for shortcut display. */
export const MOD = navigator.userAgent.includes("Mac") ? "\u2318" : "Ctrl";
