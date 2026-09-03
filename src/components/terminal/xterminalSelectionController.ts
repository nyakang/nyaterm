import type { Terminal } from "@xterm/xterm";
import type { TerminalAppSettings } from "@/context/AppContext";
import { writeClipboardText } from "@/lib/clipboard";
import type { TerminalInputState } from "@/lib/terminalInputTracker";
import {
  getInputIndexAtBufferPosition,
  getMouseBufferPosition,
  type InputSelectionRange,
} from "./terminalInputSelection";

interface MutableRef<T> {
  current: T;
}

interface InstallXTerminalSelectionControllerParams {
  terminal: Terminal;
  containerEl: HTMLDivElement;
  isMacOS: boolean;
  isWindows: boolean;
  activeRef: MutableRef<boolean>;
  visibleRef: MutableRef<boolean>;
  terminalAppSettingsRef: MutableRef<TerminalAppSettings>;
  pendingSearchSelectionRef: MutableRef<boolean>;
  searchSelectionTextRef: MutableRef<string | null>;
  lastSelectionRef: MutableRef<string>;
  disconnectedRef: MutableRef<boolean>;
  aiCapturingRef: MutableRef<boolean>;
  inputStateRef: MutableRef<TerminalInputState>;
  isTerminalAlive: () => boolean;
  removeLinkPopup: () => void;
  clearSearchSelectionState: () => void;
  getSmartCursorSelectedInputRange: () => InputSelectionRange | null;
  moveInputCursorAfterSelection: (
    selectedInputRange: InputSelectionRange,
    targetCursor: number,
  ) => void;
  canUseSmartCursor: (state?: TerminalInputState) => boolean;
  moveInputCursor: (targetCursor: number) => void;
  pasteText: (text: string) => void;
  pasteClipboard: () => Promise<void>;
}

export function installXTerminalSelectionController({
  terminal,
  containerEl,
  isMacOS,
  isWindows,
  activeRef,
  visibleRef,
  terminalAppSettingsRef,
  pendingSearchSelectionRef,
  searchSelectionTextRef,
  lastSelectionRef,
  disconnectedRef,
  aiCapturingRef,
  inputStateRef,
  isTerminalAlive,
  removeLinkPopup,
  clearSearchSelectionState,
  getSmartCursorSelectedInputRange,
  moveInputCursorAfterSelection,
  canUseSmartCursor,
  moveInputCursor,
  pasteText,
  pasteClipboard,
}: InstallXTerminalSelectionControllerParams) {
  let primaryMouseDown: { x: number; y: number } | null = null;
  let terminalHasFocus = document.activeElement === terminal.textarea;

  const resetTerminalPointerState = (
    options: { clearSelection?: boolean } = {},
  ) => {
    primaryMouseDown = null;
    if (options.clearSelection && isTerminalAlive()) {
      terminal.clearSelection();
    }
    if (options.clearSelection) {
      clearSearchSelectionState();
    }
  };

  const selectionDisposable = terminal.onSelectionChange(() => {
    const text = terminal.getSelection();
    if (!text) {
      if (!pendingSearchSelectionRef.current) {
        searchSelectionTextRef.current = null;
      }
      return;
    }

    if (pendingSearchSelectionRef.current) {
      searchSelectionTextRef.current = text;
    }

    if (text) {
      lastSelectionRef.current = text;
    }
    if (terminalAppSettingsRef.current?.interaction?.copy_on_select) {
      if (searchSelectionTextRef.current !== text) {
        writeClipboardText(text).catch(() => {});
      }
    }
  });

  const handleTerminalMouseDown = (e: MouseEvent) => {
    removeLinkPopup();
    clearSearchSelectionState();

    if (
      e.button === 0 &&
      !e.ctrlKey &&
      !e.metaKey &&
      !e.altKey &&
      !e.shiftKey
    ) {
      primaryMouseDown = { x: e.clientX, y: e.clientY };
    } else {
      primaryMouseDown = null;
    }

    if (e.button === 1) e.preventDefault();
  };

  const handleTerminalMouseUp = (e: MouseEvent) => {
    if (e.button === 0) {
      const down = primaryMouseDown;
      primaryMouseDown = null;
      const isPlainPrimaryMouseUp =
        down &&
        !disconnectedRef.current &&
        !aiCapturingRef.current &&
        !e.ctrlKey &&
        !e.metaKey &&
        !e.altKey &&
        !e.shiftKey;
      const isStationaryMouseUp =
        !!down &&
        Math.abs(e.clientX - down.x) <= 4 &&
        Math.abs(e.clientY - down.y) <= 4;

      if (isPlainPrimaryMouseUp && terminal.hasSelection()) {
        const selectedInputRange = getSmartCursorSelectedInputRange();
        if (selectedInputRange) {
          if (e.detail >= 2 || isStationaryMouseUp) {
            moveInputCursorAfterSelection(
              selectedInputRange,
              selectedInputRange.end,
            );
          } else {
            const position = getMouseBufferPosition(terminal, e);
            const targetCursor = position
              ? getInputIndexAtBufferPosition(
                  terminal,
                  inputStateRef.current,
                  position,
                )
              : null;
            if (targetCursor !== null) {
              moveInputCursorAfterSelection(selectedInputRange, targetCursor);
            }
          }
        }
      } else if (isPlainPrimaryMouseUp && isStationaryMouseUp) {
        const state = inputStateRef.current;
        if (canUseSmartCursor(state)) {
          const position = getMouseBufferPosition(terminal, e);
          if (position) {
            const targetCursor = getInputIndexAtBufferPosition(
              terminal,
              state,
              position,
            );
            if (targetCursor !== null) {
              moveInputCursor(targetCursor);
            }
          }
        }
      }

      if (terminalAppSettingsRef.current?.interaction?.copy_on_select) {
        const sel = terminal.getSelection();
        if (sel && searchSelectionTextRef.current !== sel)
          writeClipboardText(sel).catch(() => {});
      }
      return;
    }

    if (e.button !== 1) return;
    e.preventDefault();
    const sel = terminal.getSelection();
    if (sel) {
      pasteText(sel);
    } else {
      pasteClipboard().catch(() => {});
    }
  };

  const handleMacReleasedMouseMove = (e: MouseEvent) => {
    if (!isMacOS || !primaryMouseDown || e.buttons !== 0) return;

    primaryMouseDown = null;
    e.stopImmediatePropagation();

    const syntheticMouseUp = new MouseEvent("mouseup", {
      bubbles: true,
      cancelable: true,
      button: 0,
      buttons: 0,
      clientX: e.clientX,
      clientY: e.clientY,
      screenX: e.screenX,
      screenY: e.screenY,
      ctrlKey: e.ctrlKey,
      metaKey: e.metaKey,
      altKey: e.altKey,
      shiftKey: e.shiftKey,
    });
    document.dispatchEvent(syntheticMouseUp);
  };

  const handleTerminalPointerCancel = () => {
    resetTerminalPointerState({ clearSelection: true });
  };

  const handleTerminalMouseLeave = () => {
    resetTerminalPointerState();
  };

  const handleTerminalDragStart = () => {
    resetTerminalPointerState();
  };

  const handleTerminalFocus = () => {
    terminalHasFocus = true;
  };

  const handleTerminalBlur = (e: FocusEvent) => {
    if (e.relatedTarget !== null && document.hasFocus()) {
      terminalHasFocus = false;
    }
  };

  const handleTerminalWindowBlur = () => {
    resetTerminalPointerState({ clearSelection: true });
  };

  const handleTerminalVisibilityChange = () => {
    if (document.visibilityState !== "visible") {
      resetTerminalPointerState({ clearSelection: true });
    }
  };

  // Windows clipboard history injects Ctrl+V with an empty KeyboardEvent.code.
  const handleSyntheticWinVPaste = (e: KeyboardEvent) => {
    if (
      !isWindows ||
      !terminalHasFocus ||
      !isTerminalAlive() ||
      !activeRef.current ||
      !visibleRef.current ||
      !e.ctrlKey ||
      e.shiftKey ||
      e.altKey ||
      e.metaKey ||
      e.code !== "" ||
      (e.key !== "v" && e.key !== "V")
    ) {
      return;
    }

    e.preventDefault();
    e.stopPropagation();
    pasteClipboard().catch(() => {});
  };

  containerEl.addEventListener("mousedown", handleTerminalMouseDown);
  containerEl.addEventListener("mouseup", handleTerminalMouseUp);
  containerEl.addEventListener("pointercancel", handleTerminalPointerCancel);
  containerEl.addEventListener("mouseleave", handleTerminalMouseLeave);
  containerEl.addEventListener("dragstart", handleTerminalDragStart);
  if (isMacOS) {
    document.addEventListener("mousemove", handleMacReleasedMouseMove, true);
  }
  if (isWindows) {
    terminal.textarea?.addEventListener("focus", handleTerminalFocus);
    terminal.textarea?.addEventListener("blur", handleTerminalBlur);
    window.addEventListener("keydown", handleSyntheticWinVPaste, true);
  }
  window.addEventListener("blur", handleTerminalWindowBlur);
  document.addEventListener("visibilitychange", handleTerminalVisibilityChange);

  return {
    dispose: () => {
      containerEl.removeEventListener("mousedown", handleTerminalMouseDown);
      containerEl.removeEventListener("mouseup", handleTerminalMouseUp);
      containerEl.removeEventListener(
        "pointercancel",
        handleTerminalPointerCancel,
      );
      containerEl.removeEventListener("mouseleave", handleTerminalMouseLeave);
      containerEl.removeEventListener("dragstart", handleTerminalDragStart);
      if (isMacOS) {
        document.removeEventListener(
          "mousemove",
          handleMacReleasedMouseMove,
          true,
        );
      }
      if (isWindows) {
        terminal.textarea?.removeEventListener("focus", handleTerminalFocus);
        terminal.textarea?.removeEventListener("blur", handleTerminalBlur);
        window.removeEventListener("keydown", handleSyntheticWinVPaste, true);
      }
      window.removeEventListener("blur", handleTerminalWindowBlur);
      document.removeEventListener(
        "visibilitychange",
        handleTerminalVisibilityChange,
      );
      selectionDisposable.dispose();
    },
  };
}
