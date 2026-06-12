import { useHotkeys } from "react-hotkeys-hook";
import { resolveShortcutKeys } from "@/hooks/useShortcutMap";
import { MOD, resolveIndexedKeys } from "@/lib/shortcutRegistry";

export { MOD };

const HOTKEY_OPTIONS = { enableOnFormTags: true, preventDefault: true } as const;

export interface ShortcutCallbacks {
  onNewSession: () => void;
  onOpenSessionSwitcher: () => void;
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
  onOpenSettings: () => void;
  onOpenChat: () => void;
  onShowAllCommands: () => void;
  onLockScreen: () => void;
  onManageSyncGroups: () => void;
  onClearTerminal: () => void;
}

export function useGlobalShortcuts(
  cb: ShortcutCallbacks,
  keybindings: Record<string, string> = {},
) {
  const k = (id: string) => resolveShortcutKeys(id, keybindings);

  useHotkeys(k("tab.newSession"), cb.onNewSession, HOTKEY_OPTIONS);
  useHotkeys(k("tab.quickSwitch"), cb.onOpenSessionSwitcher, HOTKEY_OPTIONS);
  useHotkeys(k("tab.newLocalTerminal"), cb.onNewLocalTerminal, HOTKEY_OPTIONS);
  useHotkeys(k("tab.close"), cb.onCloseTab, HOTKEY_OPTIONS);
  useHotkeys(k("tab.next"), cb.onNextTab, HOTKEY_OPTIONS);
  useHotkeys(k("tab.prev"), cb.onPrevTab, HOTKEY_OPTIONS);

  const switchTabKeys = k("tab.switchTo");
  useHotkeys(resolveIndexedKeys(switchTabKeys, 1), () => cb.onSwitchTab(0), HOTKEY_OPTIONS);
  useHotkeys(resolveIndexedKeys(switchTabKeys, 2), () => cb.onSwitchTab(1), HOTKEY_OPTIONS);
  useHotkeys(resolveIndexedKeys(switchTabKeys, 3), () => cb.onSwitchTab(2), HOTKEY_OPTIONS);
  useHotkeys(resolveIndexedKeys(switchTabKeys, 4), () => cb.onSwitchTab(3), HOTKEY_OPTIONS);
  useHotkeys(resolveIndexedKeys(switchTabKeys, 5), () => cb.onSwitchTab(4), HOTKEY_OPTIONS);
  useHotkeys(resolveIndexedKeys(switchTabKeys, 6), () => cb.onSwitchTab(5), HOTKEY_OPTIONS);
  useHotkeys(resolveIndexedKeys(switchTabKeys, 7), () => cb.onSwitchTab(6), HOTKEY_OPTIONS);
  useHotkeys(resolveIndexedKeys(switchTabKeys, 8), () => cb.onSwitchTab(7), HOTKEY_OPTIONS);
  useHotkeys(resolveIndexedKeys(switchTabKeys, 9), () => cb.onSwitchTab(-1), HOTKEY_OPTIONS);

  useHotkeys(k("view.toggleLeftSidebar"), cb.onToggleLeftSidebar, HOTKEY_OPTIONS);
  useHotkeys(k("view.toggleRightSidebar"), cb.onToggleRightSidebar, HOTKEY_OPTIONS);
  useHotkeys(k("view.zoomIn"), cb.onZoomIn, HOTKEY_OPTIONS);
  useHotkeys(k("view.zoomOut"), cb.onZoomOut, HOTKEY_OPTIONS);
  useHotkeys(k("view.resetZoom"), cb.onResetZoom, HOTKEY_OPTIONS);
  useHotkeys(k("view.openSettings"), cb.onOpenSettings, HOTKEY_OPTIONS);
  useHotkeys(k("view.openChat"), cb.onOpenChat, HOTKEY_OPTIONS);
  useHotkeys(k("view.showAllCommands"), cb.onShowAllCommands, HOTKEY_OPTIONS);

  useHotkeys(k("terminal.manageSyncGroups"), cb.onManageSyncGroups, HOTKEY_OPTIONS);
  useHotkeys(k("terminal.clear"), cb.onClearTerminal, HOTKEY_OPTIONS);

  useHotkeys(k("special.lockScreen"), cb.onLockScreen, HOTKEY_OPTIONS);
}
