import type { Terminal } from "@xterm/xterm";
import type { TerminalAppSettings } from "@/context/AppContext";
import { resolveShortcutKeys } from "@/hooks/useShortcutMap";
import { writeClipboardText } from "@/lib/clipboard";
import {
  isModifierOnlyKeyEvent,
  matchesKeyEvent,
  resolveIndexedKeys,
} from "@/lib/shortcutRegistry";
import {
  markTerminalUserInput,
  sendTerminalClearInput,
} from "@/lib/terminalControlInput";
import {
  applyTerminalInputData,
  type TerminalInputState,
} from "@/lib/terminalInputTracker";
import type { FuzzyResult, SessionType } from "@/types/global";
import {
  type InputSelectionRange,
  isShiftInsertPasteEvent,
} from "./terminalInputSelection";
import {
  getCtrlPrintableCsiuInput,
  isLocalBackspaceEvent,
} from "./xterminalKeyboardInput";

const BACKSPACE_INPUT = "\x7f";

interface MutableRef<T> {
  current: T;
}

interface InstallXTerminalKeyboardControllerParams {
  terminal: Terminal;
  terminalAppSettingsRef: MutableRef<TerminalAppSettings>;
  sessionTypeRef: MutableRef<SessionType>;
  inputStateRef: MutableRef<TerminalInputState>;
  disconnectedRef: MutableRef<boolean>;
  onDisconnectedCloseRequestedRef: MutableRef<(() => void) | undefined>;
  showSuggestionsRef: MutableRef<boolean>;
  suggestionsRef: MutableRef<FuzzyResult[]>;
  doFindRef: MutableRef<(selection?: string) => void>;
  pasteClipboard: () => Promise<void>;
  pasteText: (text: string) => void;
  sendRawInput: (data: string, command: string | null) => Promise<void>;
  triggerSearch: (options?: { manual?: boolean }) => void;
  dismissSuggestions: () => void;
  moveCredentialSelection: (direction: 1 | -1) => boolean;
  isCredentialPanelActive: () => boolean;
  moveCommandSuggestionSelection: (direction: 1 | -1) => boolean;
  acceptCommandSuggestion: (execute: boolean) => boolean;
  isCredentialPromptInputMode: () => boolean;
  clearSearchSelectionBeforeInput: () => boolean;
  getSmartCursorSelectedInputRange: () => InputSelectionRange | null;
  deleteInputSelection: (selectedInputRange: InputSelectionRange) => void;
  collapseInputSelection: (
    selectedInputRange: InputSelectionRange,
    edge: "start" | "end",
  ) => void;
  replaceInputSelection: (
    selectedInputRange: InputSelectionRange,
    data: string,
  ) => void;
  syncSuggestionsWithInputState: () => void;
  lastSelectionRef: MutableRef<string>;
}

export function installXTerminalKeyboardController({
  terminal,
  terminalAppSettingsRef,
  sessionTypeRef,
  inputStateRef,
  disconnectedRef,
  onDisconnectedCloseRequestedRef,
  showSuggestionsRef,
  suggestionsRef,
  doFindRef,
  pasteClipboard,
  pasteText,
  sendRawInput,
  triggerSearch,
  dismissSuggestions,
  moveCredentialSelection,
  isCredentialPanelActive,
  moveCommandSuggestionSelection,
  acceptCommandSuggestion,
  isCredentialPromptInputMode,
  clearSearchSelectionBeforeInput,
  getSmartCursorSelectedInputRange,
  deleteInputSelection,
  collapseInputSelection,
  replaceInputSelection,
  syncSuggestionsWithInputState,
  lastSelectionRef,
}: InstallXTerminalKeyboardControllerParams) {
  const inputFromKeyboardController = (data: string) => {
    markTerminalUserInput(terminal);
    terminal.input(data, true);
  };

  const getDirectInputDataFromKeyEvent = (e: KeyboardEvent) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return null;
    if (e.key === "Dead" || e.key === "Process" || e.key === "Unidentified")
      return null;
    if (Array.from(e.key).length !== 1) return null;
    if (/[\x00-\x1f\x7f]/u.test(e.key)) return null;
    return e.key;
  };

  terminal.attachCustomKeyEventHandler((e) => {
    if (e.type !== "keydown") return true;

    if (isModifierOnlyKeyEvent(e)) {
      e.preventDefault();
      return false;
    }

    const kb = terminalAppSettingsRef.current.keybindings;

    if (
      matchesKeyEvent(
        resolveShortcutKeys("terminal.showCommandSuggestions", kb),
        e,
      )
    ) {
      e.preventDefault();
      if (!disconnectedRef.current) {
        triggerSearch({ manual: true });
      }
      return false;
    }

    if (
      disconnectedRef.current &&
      e.ctrlKey &&
      !e.metaKey &&
      !e.altKey &&
      !e.shiftKey &&
      e.code === "KeyD"
    ) {
      e.preventDefault();
      onDisconnectedCloseRequestedRef.current?.();
      return false;
    }

    if (isShiftInsertPasteEvent(e)) {
      e.preventDefault();
      pasteClipboard().catch(() => {});
      return false;
    }

    if (
      e.key === "Tab" &&
      !e.ctrlKey &&
      !e.metaKey &&
      !e.altKey &&
      moveCredentialSelection(e.shiftKey ? -1 : 1)
    ) {
      e.preventDefault();
      return false;
    }

    if (
      !isCredentialPanelActive() &&
      showSuggestionsRef.current &&
      suggestionsRef.current.length > 0 &&
      !e.ctrlKey &&
      !e.metaKey &&
      !e.altKey &&
      !e.shiftKey
    ) {
      if (e.key === "ArrowUp") {
        e.preventDefault();
        moveCommandSuggestionSelection(-1);
        return false;
      }
      if (e.key === "ArrowDown") {
        e.preventDefault();
        moveCommandSuggestionSelection(1);
        return false;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        dismissSuggestions();
        return false;
      }
      if (e.key === "Enter" && acceptCommandSuggestion(true)) {
        e.preventDefault();
        return false;
      }
      if (e.key === "Tab" && acceptCommandSuggestion(false)) {
        e.preventDefault();
        return false;
      }
    }

    if (isLocalBackspaceEvent(e, sessionTypeRef.current)) {
      e.preventDefault();
      if (isCredentialPromptInputMode()) {
        sendRawInput(BACKSPACE_INPUT, null);
        return false;
      }

      clearSearchSelectionBeforeInput();
      const selectedInputRange = getSmartCursorSelectedInputRange();
      if (selectedInputRange) {
        deleteInputSelection(selectedInputRange);
        return false;
      }

      inputStateRef.current = applyTerminalInputData(
        inputStateRef.current,
        BACKSPACE_INPUT,
      );
      syncSuggestionsWithInputState();
      sendRawInput(BACKSPACE_INPUT, null);
      return false;
    }

    if (
      (e.key === "ArrowLeft" || e.key === "ArrowRight") &&
      !e.ctrlKey &&
      !e.metaKey &&
      !e.altKey &&
      !e.shiftKey
    ) {
      if (clearSearchSelectionBeforeInput()) {
        return true;
      }
      const selectedInputRange = getSmartCursorSelectedInputRange();
      if (selectedInputRange) {
        e.preventDefault();
        collapseInputSelection(
          selectedInputRange,
          e.key === "ArrowLeft" ? "start" : "end",
        );
        return false;
      }
    }

    if (
      (e.key === "Backspace" || e.key === "Delete") &&
      !e.ctrlKey &&
      !e.metaKey &&
      !e.altKey
    ) {
      if (clearSearchSelectionBeforeInput()) {
        return true;
      }
      const selectedInputRange = getSmartCursorSelectedInputRange();
      if (selectedInputRange) {
        e.preventDefault();
        deleteInputSelection(selectedInputRange);
        return false;
      }
    }

    const directInputData = getDirectInputDataFromKeyEvent(e);
    if (directInputData) {
      if (clearSearchSelectionBeforeInput()) {
        return true;
      }
      const selectedInputRange = getSmartCursorSelectedInputRange();
      if (selectedInputRange) {
        e.preventDefault();
        replaceInputSelection(selectedInputRange, directInputData);
        return false;
      }
    }

    if (terminal.hasSelection() && !getSmartCursorSelectedInputRange()) {
      // The selection is preserved by design while typing (it is only cleared
      // on mouse click), so input is sent with wasUserInput=false to skip
      // xterm's selection clearing. That also skips xterm's scrollOnUserInput,
      // so scroll back to the cursor explicitly to keep the prompt visible.
      const inputPreservingSelection = (data: string) => {
        markTerminalUserInput(terminal);
        terminal.input(data, false);
        const buffer = terminal.buffer.active;
        if (buffer.baseY !== buffer.viewportY) {
          terminal.scrollToBottom();
        }
      };
      if (directInputData) {
        e.preventDefault();
        inputPreservingSelection(directInputData);
        return false;
      }
      if (
        (e.key === "Backspace" || e.key === "Delete") &&
        !e.ctrlKey &&
        !e.metaKey &&
        !e.altKey
      ) {
        e.preventDefault();
        inputPreservingSelection("\x7f");
        return false;
      }
      if (e.key === "Enter" && !e.ctrlKey && !e.metaKey && !e.altKey) {
        e.preventDefault();
        inputPreservingSelection("\r");
        return false;
      }
      if (
        e.key === "ArrowLeft" &&
        !e.ctrlKey &&
        !e.metaKey &&
        !e.altKey &&
        !e.shiftKey
      ) {
        e.preventDefault();
        inputPreservingSelection("\x1b[D");
        return false;
      }
      if (
        e.key === "ArrowRight" &&
        !e.ctrlKey &&
        !e.metaKey &&
        !e.altKey &&
        !e.shiftKey
      ) {
        e.preventDefault();
        inputPreservingSelection("\x1b[C");
        return false;
      }
      if (
        e.key === "ArrowUp" &&
        !e.ctrlKey &&
        !e.metaKey &&
        !e.altKey &&
        !e.shiftKey
      ) {
        e.preventDefault();
        inputPreservingSelection("\x1b[A");
        return false;
      }
      if (
        e.key === "ArrowDown" &&
        !e.ctrlKey &&
        !e.metaKey &&
        !e.altKey &&
        !e.shiftKey
      ) {
        e.preventDefault();
        inputPreservingSelection("\x1b[B");
        return false;
      }
      if (e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey) {
        const ctrlCharMap: Record<string, string> = {
          a: "\x01",
          b: "\x02",
          c: "\x03",
          d: "\x04",
          e: "\x05",
          f: "\x06",
          g: "\x07",
          h: "\x08",
          i: "\x09",
          j: "\x0a",
          k: "\x0b",
          l: "\x0c",
          m: "\x0d",
          n: "\x0e",
          o: "\x0f",
          p: "\x10",
          q: "\x11",
          r: "\x12",
          s: "\x13",
          t: "\x14",
          u: "\x15",
          v: "\x16",
          w: "\x17",
          x: "\x18",
          y: "\x19",
          z: "\x1a",
        };
        const keyLower = e.key.toLowerCase();
        if (ctrlCharMap[keyLower]) {
          e.preventDefault();
          inputPreservingSelection(ctrlCharMap[keyLower]);
          return false;
        }
        if (e.key === "ArrowLeft") {
          e.preventDefault();
          inputPreservingSelection("\x1b[1;5D");
          return false;
        }
        if (e.key === "ArrowRight") {
          e.preventDefault();
          inputPreservingSelection("\x1b[1;5C");
          return false;
        }
        if (e.key === "ArrowUp") {
          e.preventDefault();
          inputPreservingSelection("\x1b[1;5A");
          return false;
        }
        if (e.key === "ArrowDown") {
          e.preventDefault();
          inputPreservingSelection("\x1b[1;5B");
          return false;
        }
      }
      if ((e.altKey || e.metaKey) && !e.ctrlKey && !e.shiftKey) {
        if (e.key === "ArrowLeft") {
          e.preventDefault();
          inputPreservingSelection("\x1b[1;3D");
          return false;
        }
        if (e.key === "ArrowRight") {
          e.preventDefault();
          inputPreservingSelection("\x1b[1;3C");
          return false;
        }
        if (e.key === "ArrowUp") {
          e.preventDefault();
          inputPreservingSelection("\x1b[1;3A");
          return false;
        }
        if (e.key === "ArrowDown") {
          e.preventDefault();
          inputPreservingSelection("\x1b[1;3B");
          return false;
        }
        const keyLower = e.key.toLowerCase();
        if (keyLower === "b") {
          e.preventDefault();
          inputPreservingSelection("\x1bb");
          return false;
        }
        if (keyLower === "f") {
          e.preventDefault();
          inputPreservingSelection("\x1bf");
          return false;
        }
        if (keyLower === "d") {
          e.preventDefault();
          inputPreservingSelection("\x1bd");
          return false;
        }
      }
    }

    if (matchesKeyEvent(resolveShortcutKeys("terminal.copy", kb), e)) {
      e.preventDefault();
      const sel = terminal.getSelection();
      if (sel) writeClipboardText(sel).catch(() => {});
      return false;
    }
    if (matchesKeyEvent(resolveShortcutKeys("terminal.paste", kb), e)) {
      e.preventDefault();
      pasteClipboard().catch(() => {});
      return false;
    }
    if (matchesKeyEvent(resolveShortcutKeys("terminal.find", kb), e)) {
      e.preventDefault();
      doFindRef.current();
      return false;
    }
    if (matchesKeyEvent(resolveShortcutKeys("terminal.clear", kb), e)) {
      e.preventDefault();
      sendTerminalClearInput(terminal);
      return false;
    }
    if (matchesKeyEvent(resolveShortcutKeys("terminal.pasteSelected", kb), e)) {
      e.preventDefault();
      const sel = terminal.getSelection() || lastSelectionRef.current;
      pasteText(sel);
      return false;
    }
    if (matchesKeyEvent(resolveShortcutKeys("terminal.selectAll", kb), e)) {
      e.preventDefault();
      terminal.selectAll();
      return false;
    }

    const swallowIds = [
      "tab.newSession",
      "tab.close",
      "tab.next",
      "tab.prev",
      "tab.newLocalTerminal",
      "tab.temporarySshLink",
      "tab.quickSwitch",
      "tab.duplicateSession",
      "tab.multiplexSsh",
      "tab.duplicateSessionWithCommand",
      "tab.multiplexSshWithCommand",
      "view.toggleLeftSidebar",
      "view.toggleRightSidebar",
      "view.zoomIn",
      "view.zoomOut",
      "view.resetZoom",
      "view.openSettings",
      "view.openChat",
      "view.showAllCommands",
      "terminal.manageSyncGroups",
      "terminal.showCommandSuggestions",
      "terminal.recording.toggle",
      "special.lockScreen",
    ];
    for (const sid of swallowIds) {
      if (matchesKeyEvent(resolveShortcutKeys(sid, kb), e)) {
        e.preventDefault();
        return false;
      }
    }
    for (let tabNumber = 1; tabNumber <= 9; tabNumber += 1) {
      if (
        matchesKeyEvent(
          resolveIndexedKeys(resolveShortcutKeys("tab.switchTo", kb), tabNumber),
          e,
        )
      ) {
        return false;
      }
    }

    const ctrlPrintableInput = getCtrlPrintableCsiuInput(e);
    if (ctrlPrintableInput) {
      e.preventDefault();
      inputFromKeyboardController(ctrlPrintableInput);
      return false;
    }

    return true;
  });
}
