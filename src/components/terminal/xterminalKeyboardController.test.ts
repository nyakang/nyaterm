import type { Terminal } from "@xterm/xterm";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { TerminalAppSettings } from "@/context/AppContext";
import { writeClipboardText } from "@/lib/clipboard";
import { createTerminalInputState } from "@/lib/terminalInputTracker";
import type { SessionType } from "@/types/global";
import type { XTerminalImeKeyboardRoute } from "./xterminalIme";
import { installXTerminalKeyboardController } from "./xterminalKeyboardController";

vi.mock("@/lib/clipboard", () => ({
  writeClipboardText: vi.fn(() => Promise.resolve()),
}));

function backspaceEvent(keyCode: number, isComposing = false): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    key: "Backspace",
    code: "Backspace",
    bubbles: true,
    cancelable: true,
  });
  Object.defineProperties(event, {
    isComposing: { value: isComposing },
    keyCode: { value: keyCode },
  });
  return event;
}

function createHarness(
  imeRoute: XTerminalImeKeyboardRoute,
  sessionType: SessionType = "Local",
  keybindings: Record<string, string> = {},
) {
  const keyHandlerRef: {
    current: ((event: KeyboardEvent) => boolean) | null;
  } = { current: null };
  const terminal = {
    attachCustomKeyEventHandler: vi.fn((handler: (event: KeyboardEvent) => boolean) => {
      keyHandlerRef.current = handler;
    }),
    getSelection: vi.fn(() => ""),
    hasSelection: vi.fn(() => false),
  } as unknown as Terminal;
  const routeKeyboardEvent = vi.fn(() => imeRoute);
  const pasteClipboard = vi.fn(async () => {});
  const sendRawInput = vi.fn(async () => {});
  const syncSuggestionsWithInputState = vi.fn();
  const inputStateRef = {
    current: {
      ...createTerminalInputState(),
      value: "a",
      cursor: 1,
    },
  };

  installXTerminalKeyboardController({
    terminal,
    imeTracker: { routeKeyboardEvent },
    terminalAppSettingsRef: {
      current: { keybindings } as TerminalAppSettings,
    },
    sessionTypeRef: { current: sessionType },
    inputStateRef,
    disconnectedRef: { current: false },
    onDisconnectedCloseRequestedRef: { current: undefined },
    showSuggestionsRef: { current: false },
    suggestionsRef: { current: [] },
    doFindRef: { current: vi.fn() },
    pasteClipboard,
    pasteText: vi.fn(),
    sendRawInput,
    triggerSearch: vi.fn(),
    dismissSuggestions: vi.fn(),
    moveCredentialSelection: vi.fn(() => false),
    isCredentialPanelActive: vi.fn(() => false),
    moveCommandSuggestionSelection: vi.fn(() => false),
    acceptCommandSuggestion: vi.fn(() => false),
    isCredentialPromptInputMode: vi.fn(() => false),
    clearSearchSelectionBeforeInput: vi.fn(() => false),
    getSmartCursorSelectedInputRange: vi.fn(() => null),
    deleteInputSelection: vi.fn(),
    collapseInputSelection: vi.fn(),
    replaceInputSelection: vi.fn(),
    syncSuggestionsWithInputState,
    lastSelectionRef: { current: "" },
  });

  const keyHandler = keyHandlerRef.current;
  if (!keyHandler) {
    throw new Error("keyboard handler was not installed");
  }

  return {
    inputStateRef,
    keyHandler,
    pasteClipboard,
    routeKeyboardEvent,
    sendRawInput,
    syncSuggestionsWithInputState,
    terminal,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("installXTerminalKeyboardController IME Backspace routing", () => {
  it("leaves IME Backspace native without preventing default", () => {
    const harness = createHarness("native-ime");
    const event = backspaceEvent(229, true);

    expect(harness.keyHandler(event)).toBe(false);
    expect(event.defaultPrevented).toBe(false);
    expect(harness.sendRawInput).not.toHaveBeenCalled();
    expect(harness.syncSuggestionsWithInputState).not.toHaveBeenCalled();
    expect(harness.inputStateRef.current.value).toBe("a");
  });

  it("delegates idle keyCode 229 Backspace to xterm", () => {
    const harness = createHarness("xterm");
    const event = backspaceEvent(229);

    expect(harness.keyHandler(event)).toBe(true);
    expect(event.defaultPrevented).toBe(false);
    expect(harness.sendRawInput).not.toHaveBeenCalled();
    expect(harness.inputStateRef.current.value).toBe("a");
  });

  it("preserves the existing non-IME Local Backspace behavior", () => {
    const harness = createHarness("application");
    const event = backspaceEvent(8);

    expect(harness.keyHandler(event)).toBe(false);
    expect(event.defaultPrevented).toBe(true);
    expect(harness.sendRawInput).toHaveBeenCalledOnce();
    expect(harness.sendRawInput).toHaveBeenCalledWith("\x7f", null);
    expect(harness.syncSuggestionsWithInputState).toHaveBeenCalledOnce();
    expect(harness.inputStateRef.current.value).toBe("");
    expect(harness.inputStateRef.current.cursor).toBe(0);
  });

  it("does not apply the IME guard outside Local Backspace handling", () => {
    const harness = createHarness("native-ime", "SSH");
    const event = backspaceEvent(8, true);

    expect(harness.keyHandler(event)).toBe(true);
    expect(harness.routeKeyboardEvent).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
  });

  it("honors a custom copy shortcut while terminal text is selected", () => {
    const harness = createHarness("application", "SSH", {
      "terminal.copy": "ctrl+c",
    });
    vi.mocked(harness.terminal.hasSelection).mockReturnValue(true);
    vi.mocked(harness.terminal.getSelection).mockReturnValue("selected output");
    const event = new KeyboardEvent("keydown", {
      key: "c",
      code: "KeyC",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });

    expect(harness.keyHandler(event)).toBe(false);
    expect(event.defaultPrevented).toBe(true);
    expect(writeClipboardText).toHaveBeenCalledWith("selected output");
    expect(harness.sendRawInput).not.toHaveBeenCalled();
  });

  it("passes a custom Ctrl+C through to the shell when there is no selection", () => {
    const harness = createHarness("application", "SSH", {
      "terminal.copy": "ctrl+c",
    });
    const event = new KeyboardEvent("keydown", {
      key: "c",
      code: "KeyC",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });

    expect(harness.keyHandler(event)).toBe(true);
    expect(event.defaultPrevented).toBe(false);
    expect(writeClipboardText).not.toHaveBeenCalled();
  });

  it("honors a custom paste shortcut while terminal text is selected", () => {
    const harness = createHarness("application", "SSH", {
      "terminal.paste": "ctrl+v",
    });
    vi.mocked(harness.terminal.hasSelection).mockReturnValue(true);
    const event = new KeyboardEvent("keydown", {
      key: "v",
      code: "KeyV",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });

    expect(harness.keyHandler(event)).toBe(false);
    expect(event.defaultPrevented).toBe(true);
    expect(harness.pasteClipboard).toHaveBeenCalledOnce();
    expect(harness.sendRawInput).not.toHaveBeenCalled();
  });

  it("keeps the default Ctrl+Shift+C copy shortcut available", () => {
    const harness = createHarness("application", "SSH");
    vi.mocked(harness.terminal.hasSelection).mockReturnValue(true);
    vi.mocked(harness.terminal.getSelection).mockReturnValue("selected output");
    const event = new KeyboardEvent("keydown", {
      key: "C",
      code: "KeyC",
      ctrlKey: true,
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    });

    expect(harness.keyHandler(event)).toBe(false);
    expect(event.defaultPrevented).toBe(true);
    expect(writeClipboardText).toHaveBeenCalledWith("selected output");
  });
});
