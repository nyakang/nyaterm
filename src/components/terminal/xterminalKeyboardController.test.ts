import type { Terminal } from "@xterm/xterm";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TerminalAppSettings } from "@/context/AppContext";
import { writeClipboardText } from "@/lib/clipboard";
import { createTerminalInputState } from "@/lib/terminalInputTracker";
import type { SessionType } from "@/types/global";
import {
  createXTerminalImeTracker,
  type XTerminalImeKeyboardRoute,
  type XTerminalImeTracker,
} from "./xterminalIme";
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

function ctrlUEvent(keyCode: number, key = "Process"): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    key,
    code: "KeyU",
    ctrlKey: true,
    bubbles: true,
    cancelable: true,
  });
  Object.defineProperty(event, "keyCode", { value: keyCode });
  return event;
}

function shiftEvent(
  code: "ShiftLeft" | "ShiftRight",
  key = "Shift",
  keyCode = 16,
  isComposing = false,
  modifiers: KeyboardEventInit = {},
  type = "keydown",
): KeyboardEvent {
  const event = new KeyboardEvent(type, {
    key,
    code,
    shiftKey: true,
    bubbles: true,
    cancelable: true,
    ...modifiers,
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
  options: {
    isMacOS?: boolean;
    appLocked?: boolean;
    disconnected?: boolean;
    terminal?: Terminal;
    imeTracker?: XTerminalImeTracker;
  } = {},
) {
  const keyHandlerRef: {
    current: ((event: KeyboardEvent) => boolean) | null;
  } = { current: null };
  const terminal =
    options.terminal ??
    ({
      attachCustomKeyEventHandler: vi.fn(
        (handler: (event: KeyboardEvent) => boolean) => {
          keyHandlerRef.current = handler;
        },
      ),
      getSelection: vi.fn(() => ""),
      hasSelection: vi.fn(() => false),
      clearSelection: vi.fn(),
      input: vi.fn(),
      buffer: { active: { baseY: 0, viewportY: 0 } },
      scrollToBottom: vi.fn(),
    } as unknown as Terminal);
  if (options.terminal) {
    const attach = terminal.attachCustomKeyEventHandler.bind(terminal);
    vi.spyOn(terminal, "attachCustomKeyEventHandler").mockImplementation(
      (handler) => {
        keyHandlerRef.current = handler;
        attach(handler);
      },
    );
  }
  const routeKeyboardEvent = vi.fn(
    (event: KeyboardEvent) =>
      options.imeTracker?.routeKeyboardEvent(event) ?? imeRoute,
  );
  const pasteClipboard = vi.fn(async () => {});
  const sendRawInput = vi.fn(async () => {});
  const syncSuggestionsWithInputState = vi.fn();
  const navigateCommand = vi.fn();
  const selectCommandBlock = vi.fn();
  const clearAll = vi.fn();
  const inputStateRef = {
    current: {
      ...createTerminalInputState(),
      value: "a",
      cursor: 1,
    },
  };

  installXTerminalKeyboardController({
    terminal,
    isMacOS: options.isMacOS ?? false,
    imeTracker: { routeKeyboardEvent },
    terminalAppSettingsRef: {
      current: { keybindings } as TerminalAppSettings,
    },
    sessionTypeRef: { current: sessionType },
    inputStateRef,
    appLockedRef: { current: options.appLocked ?? false },
    disconnectedRef: { current: options.disconnected ?? false },
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
    navigateCommand,
    selectCommandBlock,
    clearAll,
    resetCommandNavigation: vi.fn(),
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
    navigateCommand,
    selectCommandBlock,
    clearAll,
  };
}

function shortcutEvent(
  key: string,
  code: string,
  options: KeyboardEventInit = {},
) {
  return new KeyboardEvent("keydown", {
    key,
    code,
    ctrlKey: true,
    bubbles: true,
    cancelable: true,
    ...options,
  });
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("installXTerminalKeyboardController Shift IME toggle", () => {
  it("delegates composing Process/ShiftLeft to the native IME", () => {
    const harness = createHarness("native-ime");
    const event = shiftEvent("ShiftLeft", "Process", 229, true);

    expect(harness.keyHandler(event)).toBe(true);
    expect(event.defaultPrevented).toBe(false);
    expect(harness.routeKeyboardEvent).toHaveBeenCalledExactlyOnceWith(event);
    expect(harness.terminal.input).not.toHaveBeenCalled();
    expect(harness.sendRawInput).not.toHaveBeenCalled();
  });

  it("delegates ending composition ShiftRight to the native IME", () => {
    const harness = createHarness("native-ime");
    const event = shiftEvent("ShiftRight");

    expect(harness.keyHandler(event)).toBe(true);
    expect(event.defaultPrevented).toBe(false);
    expect(harness.routeKeyboardEvent).toHaveBeenCalledExactlyOnceWith(event);
    expect(harness.terminal.input).not.toHaveBeenCalled();
    expect(harness.sendRawInput).not.toHaveBeenCalled();
  });

  it("delegates legacy keyCode 229 ShiftRight to xterm", () => {
    const harness = createHarness("xterm");
    const event = shiftEvent("ShiftRight", "Process", 229);

    expect(harness.keyHandler(event)).toBe(true);
    expect(event.defaultPrevented).toBe(false);
    expect(harness.routeKeyboardEvent).toHaveBeenCalledExactlyOnceWith(event);
    expect(harness.terminal.input).not.toHaveBeenCalled();
    expect(harness.sendRawInput).not.toHaveBeenCalled();
  });

  it.each(["ShiftLeft", "ShiftRight"] as const)(
    "allows the OS to toggle input language for idle %s without sending input",
    (code) => {
      const harness = createHarness("application");
      const event = shiftEvent(code);

      expect(harness.keyHandler(event)).toBe(false);
      expect(event.defaultPrevented).toBe(false);
      expect(harness.routeKeyboardEvent).toHaveBeenCalledExactlyOnceWith(event);
      expect(harness.terminal.input).not.toHaveBeenCalled();
      expect(harness.sendRawInput).not.toHaveBeenCalled();
    },
  );

  it.each([
    ["ControlLeft", "Control", { ctrlKey: true }],
    ["AltRight", "Alt", { altKey: true }],
    ["MetaLeft", "Meta", { metaKey: true }],
  ] as const)("keeps standalone %s blocked", (code, key, modifiers) => {
    const harness = createHarness("native-ime");
    const event = new KeyboardEvent("keydown", {
      key,
      code,
      ...modifiers,
      bubbles: true,
      cancelable: true,
    });

    expect(harness.keyHandler(event)).toBe(false);
    expect(event.defaultPrevented).toBe(true);
    expect(harness.routeKeyboardEvent).not.toHaveBeenCalled();
    expect(harness.terminal.input).not.toHaveBeenCalled();
    expect(harness.sendRawInput).not.toHaveBeenCalled();
  });

  it.each([
    ["Ctrl+Shift", { ctrlKey: true }],
    ["Alt+Shift", { altKey: true }],
    ["Meta+Shift", { metaKey: true }],
  ] as const)("keeps %s modifier-only events blocked", (_label, modifiers) => {
    const harness = createHarness("native-ime");
    const event = shiftEvent("ShiftLeft", "Shift", 16, false, modifiers);

    expect(harness.keyHandler(event)).toBe(false);
    expect(event.defaultPrevented).toBe(true);
    expect(harness.routeKeyboardEvent).not.toHaveBeenCalled();
    expect(harness.terminal.input).not.toHaveBeenCalled();
    expect(harness.sendRawInput).not.toHaveBeenCalled();
  });

  it("keeps Shift blocked while the app is locked", () => {
    const harness = createHarness("native-ime", "SSH", {}, { appLocked: true });
    const event = shiftEvent("ShiftLeft", "Process", 229, true);

    expect(harness.keyHandler(event)).toBe(false);
    expect(event.defaultPrevented).toBe(true);
    expect(harness.routeKeyboardEvent).not.toHaveBeenCalled();
    expect(harness.terminal.input).not.toHaveBeenCalled();
    expect(harness.sendRawInput).not.toHaveBeenCalled();
  });

  it.each(["ShiftLeft", "ShiftRight"] as const)(
    "filters %s release even after IME ownership ends or modifiers change",
    (code) => {
      const harness = createHarness("native-ime");
      expect(harness.keyHandler(shiftEvent(code, "Process", 229, true))).toBe(
        true,
      );
      harness.routeKeyboardEvent.mockReturnValue("application");
      const release = shiftEvent(
        code,
        "Shift",
        16,
        false,
        { ctrlKey: true, shiftKey: false },
        "keyup",
      );

      expect(harness.keyHandler(release)).toBe(false);
      expect(release.defaultPrevented).toBe(false);
      expect(harness.routeKeyboardEvent).toHaveBeenCalledTimes(1);
    },
  );

  it.each(["KeyA", "Insert", "ControlLeft", "AltRight", "MetaLeft"])(
    "preserves xterm's release handling for %s",
    (code) => {
      const harness = createHarness("application");
      const release = new KeyboardEvent("keyup", { code, cancelable: true });
      expect(harness.keyHandler(release)).toBe(true);
      expect(release.defaultPrevented).toBe(false);
    },
  );

  it("preserves the Shift+Insert paste shortcut", () => {
    const harness = createHarness("application", "SSH");
    const event = new KeyboardEvent("keydown", {
      key: "Insert",
      code: "Insert",
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    });

    expect(harness.keyHandler(event)).toBe(false);
    expect(event.defaultPrevented).toBe(true);
    expect(harness.pasteClipboard).toHaveBeenCalledOnce();
    expect(harness.routeKeyboardEvent).not.toHaveBeenCalled();
    expect(harness.terminal.input).not.toHaveBeenCalled();
  });
});

describe("Shift IME integration with xterm", () => {
  let terminal: Terminal;
  let imeTracker: XTerminalImeTracker;
  let container: HTMLDivElement;

  beforeEach(async () => {
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({
      measureText: () => ({ width: 10 }),
    } as unknown as CanvasRenderingContext2D);
    const { Terminal } = await import("@xterm/xterm");
    terminal = new Terminal({
      vtExtensions: { kittyKeyboard: true, win32InputMode: true },
    });
    container = document.createElement("div");
    document.body.append(container);
    terminal.open(container);
    imeTracker = createXTerminalImeTracker(terminal.textarea);
    createHarness("application", "SSH", {}, { terminal, imeTracker });
  });

  afterEach(() => {
    imeTracker?.dispose();
    terminal?.dispose();
    container?.remove();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  const modes = [
    ["legacy", ""],
    // Disambiguate + report event types + report all keys.
    ["Kitty", "\x1b[>11u"],
    ["Win32", "\x1b[?9001h"],
  ] as const;

  it.each(modes)(
    "filters idle Shift press/release in %s mode",
    async (_mode, enable) => {
      await new Promise<void>((resolve) => terminal.write(enable, resolve));
      const data = vi.fn();
      terminal.onData(data);
      const textarea = terminal.textarea!;

      for (const code of ["ShiftLeft", "ShiftRight"] as const) {
        const press = shiftEvent(code);
        const release = shiftEvent(
          code,
          "Shift",
          16,
          false,
          { shiftKey: false },
          "keyup",
        );
        textarea.dispatchEvent(press);
        textarea.dispatchEvent(release);
        expect(press.defaultPrevented).toBe(false);
        expect(release.defaultPrevented).toBe(false);
      }
      expect(data).not.toHaveBeenCalled();
    },
  );

  it.each(modes.slice(1))(
    "preserves Shift+A press/release reports in %s mode",
    async (mode, enable) => {
      await new Promise<void>((resolve) => terminal.write(enable, resolve));
      const data: string[] = [];
      terminal.onData((value) => data.push(value));
      const textarea = terminal.textarea!;
      textarea.dispatchEvent(shiftEvent("ShiftLeft"));
      for (const type of ["keydown", "keyup"]) {
        textarea.dispatchEvent(
          new KeyboardEvent(type, {
            key: "A",
            code: "KeyA",
            keyCode: 65,
            shiftKey: true,
            bubbles: true,
            cancelable: true,
          }),
        );
      }
      textarea.dispatchEvent(
        shiftEvent(
          "ShiftLeft",
          "Shift",
          16,
          false,
          { shiftKey: false },
          "keyup",
        ),
      );

      expect(data).toEqual(
        mode === "Kitty"
          ? ["\x1b[97;2u", "\x1b[97;2:3u"]
          : ["\x1b[65;30;65;1;16;1_", "\x1b[65;30;65;0;16;1_"],
      );
    },
  );

  it.each(modes)(
    "commits legacy Process/229 textarea changes once in %s mode",
    async (_mode, enable) => {
      await new Promise<void>((resolve) => terminal.write(enable, resolve));
      vi.useFakeTimers();
      const data = vi.fn();
      terminal.onData(data);
      const textarea = terminal.textarea!;

      for (const code of ["ShiftLeft", "ShiftRight"] as const) {
        data.mockClear();
        textarea.value = "";
        const press = shiftEvent(code, "Process", 229);
        textarea.dispatchEvent(press);
        textarea.value = "pinyin";
        textarea.dispatchEvent(
          new InputEvent("input", {
            data: "pinyin",
            inputType: "insertText",
            bubbles: true,
            composed: true,
          }),
        );
        await vi.advanceTimersByTimeAsync(1);
        textarea.dispatchEvent(
          shiftEvent(code, "Shift", 16, false, { shiftKey: false }, "keyup"),
        );

        expect(press.defaultPrevented).toBe(false);
        expect(data).toHaveBeenCalledExactlyOnceWith("pinyin");
      }
    },
  );

  it.each(
    modes.flatMap(([mode, enable]) => [
      { mode, enable, key: "Shift", keyCode: 16 },
      { mode, enable, key: "Process", keyCode: 229 },
    ]),
  )(
    "commits preedit once in $mode mode with $key/keyCode $keyCode",
    async ({ enable, key, keyCode }) => {
      await new Promise<void>((resolve) => terminal.write(enable, resolve));
      vi.useFakeTimers();
      const data = vi.fn();
      terminal.onData(data);
      const textarea = terminal.textarea!;

      for (const code of ["ShiftLeft", "ShiftRight"] as const) {
        data.mockClear();
        textarea.value = "";
        textarea.dispatchEvent(
          new CompositionEvent("compositionstart", { bubbles: true }),
        );
        textarea.value = "pinyin";
        textarea.dispatchEvent(
          new CompositionEvent("compositionupdate", {
            data: "pinyin",
            bubbles: true,
          }),
        );
        textarea.dispatchEvent(
          new InputEvent("input", {
            data: "pinyin",
            inputType: "insertCompositionText",
            isComposing: true,
            bubbles: true,
          }),
        );
        await vi.advanceTimersByTimeAsync(0);

        const press = shiftEvent(code, key, keyCode, true);
        textarea.dispatchEvent(press);
        expect(press.defaultPrevented).toBe(false);
        expect(data).not.toHaveBeenCalled();

        textarea.dispatchEvent(
          new CompositionEvent("compositionend", {
            data: "pinyin",
            bubbles: true,
          }),
        );
        textarea.dispatchEvent(
          new InputEvent("input", {
            data: "pinyin",
            inputType: "insertFromComposition",
            bubbles: true,
          }),
        );
        // The OS may release Shift after the tracker has returned to idle.
        await vi.advanceTimersByTimeAsync(1);
        expect(imeTracker.phase).toBe("idle");
        const release = shiftEvent(
          code,
          "Shift",
          16,
          false,
          { shiftKey: false },
          "keyup",
        );
        textarea.dispatchEvent(release);
        expect(release.defaultPrevented).toBe(false);
        expect(data).toHaveBeenCalledExactlyOnceWith("pinyin");
      }
    },
  );
});

describe("installXTerminalKeyboardController IME Backspace routing", () => {
  it("runs custom command shortcuts through the sole keyboard handler", () => {
    const harness = createHarness("application", "SSH", {
      "terminal.commandNav.prev": "ctrl+alt+j",
      "terminal.commandNav.next": "ctrl+alt+k",
      "terminal.commandNav.select": "ctrl+alt+u",
      "terminal.clearAll": "ctrl+alt+y",
    });
    for (const [key, action] of [
      ["j", harness.navigateCommand],
      ["k", harness.navigateCommand],
      ["u", harness.selectCommandBlock],
      ["y", harness.clearAll],
    ] as const) {
      const event = shortcutEvent(key, `Key${key.toUpperCase()}`, { altKey: true });
      expect(harness.keyHandler(event)).toBe(false);
      expect(event.defaultPrevented).toBe(true);
      expect(action).toHaveBeenCalled();
    }
    expect(harness.navigateCommand).toHaveBeenNthCalledWith(1, -1);
    expect(harness.navigateCommand).toHaveBeenNthCalledWith(2, 1);
  });

  it("blocks command shortcuts while locked and retains copy, paste, find and clear", () => {
    const locked = createHarness("application", "SSH", {}, { appLocked: true });
    expect(
      locked.keyHandler(
        shortcutEvent("L", "KeyL", { ctrlKey: true, altKey: true, shiftKey: true }),
      ),
    ).toBe(false);
    expect(locked.clearAll).not.toHaveBeenCalled();

    const harness = createHarness("application", "SSH");
    expect(harness.keyHandler(shortcutEvent("C", "KeyC", { shiftKey: true }))).toBe(false);
    expect(harness.keyHandler(shortcutEvent("V", "KeyV", { shiftKey: true }))).toBe(false);
    expect(harness.keyHandler(shortcutEvent("F", "KeyF", { shiftKey: true }))).toBe(false);
    expect(harness.keyHandler(shortcutEvent("l", "KeyL"))).toBe(false);
    expect(harness.navigateCommand).not.toHaveBeenCalled();
    expect(harness.clearAll).not.toHaveBeenCalled();
    expect(harness.pasteClipboard).toHaveBeenCalledOnce();
    expect(harness.terminal.input).toHaveBeenCalledWith("\x0c", true);
  });

  it("keeps local command actions available while disconnected", () => {
    const harness = createHarness("application", "SSH", {}, { disconnected: true });
    for (const [event, action] of [
      [shortcutEvent("ArrowLeft", "ArrowLeft", { shiftKey: true }), harness.navigateCommand],
      [shortcutEvent("ArrowRight", "ArrowRight", { shiftKey: true }), harness.navigateCommand],
      [shortcutEvent("/", "Slash", { shiftKey: true }), harness.selectCommandBlock],
      [shortcutEvent("L", "KeyL", { altKey: true, shiftKey: true }), harness.clearAll],
    ] as const) {
      expect(harness.keyHandler(event)).toBe(false);
      expect(event.defaultPrevented).toBe(true);
      expect(action).toHaveBeenCalled();
    }
    expect(harness.navigateCommand).toHaveBeenNthCalledWith(1, -1);
    expect(harness.navigateCommand).toHaveBeenNthCalledWith(2, 1);
  });
  it("swallows keyboard input before direct send paths while locked", () => {
    const harness = createHarness("application", "Local", {}, { appLocked: true });
    const event = backspaceEvent(8);

    expect(harness.keyHandler(event)).toBe(false);
    expect(event.defaultPrevented).toBe(true);
    expect(harness.sendRawInput).not.toHaveBeenCalled();
    expect(harness.inputStateRef.current.value).toBe("a");
  });

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

  it("reserves the new session menu shortcut, including custom bindings", () => {
    for (const [keybindings, key, code, shiftKey] of [
      [{}, "O", "KeyO", true],
      [{ "tab.openNewSessionMenu": "ctrl+alt+p" }, "p", "KeyP", false],
    ] as const) {
      const harness = createHarness("application", "SSH", keybindings);
      const event = new KeyboardEvent("keydown", {
        key,
        code,
        ctrlKey: true,
        altKey: !shiftKey,
        shiftKey,
        bubbles: true,
        cancelable: true,
      });

      expect(harness.keyHandler(event)).toBe(false);
      expect(event.defaultPrevented).toBe(true);
      expect(harness.sendRawInput).not.toHaveBeenCalled();
    }
  });

  it("copies a selection for plain Cmd+C on macOS", () => {
    const harness = createHarness("application", "SSH", {}, { isMacOS: true });
    vi.mocked(harness.terminal.hasSelection).mockReturnValue(true);
    vi.mocked(harness.terminal.getSelection).mockReturnValue("selected output");
    const event = new KeyboardEvent("keydown", {
      key: "c",
      code: "KeyC",
      metaKey: true,
      bubbles: true,
      cancelable: true,
    });

    expect(harness.keyHandler(event)).toBe(false);
    expect(event.defaultPrevented).toBe(true);
    expect(writeClipboardText).toHaveBeenCalledWith("selected output");
  });

  it("lets plain Cmd+C fall through on macOS when there is no selection", () => {
    const harness = createHarness("application", "SSH", {}, { isMacOS: true });
    const event = new KeyboardEvent("keydown", {
      key: "c",
      code: "KeyC",
      metaKey: true,
      bubbles: true,
      cancelable: true,
    });

    expect(harness.keyHandler(event)).toBe(true);
    expect(event.defaultPrevented).toBe(false);
    expect(writeClipboardText).not.toHaveBeenCalled();
  });

  it("does not intercept Meta+C outside macOS", () => {
    const harness = createHarness("application", "SSH");
    vi.mocked(harness.terminal.hasSelection).mockReturnValue(true);
    vi.mocked(harness.terminal.getSelection).mockReturnValue("selected output");
    const event = new KeyboardEvent("keydown", {
      key: "c",
      code: "KeyC",
      metaKey: true,
      bubbles: true,
      cancelable: true,
    });

    expect(harness.keyHandler(event)).toBe(true);
    expect(event.defaultPrevented).toBe(false);
    expect(writeClipboardText).not.toHaveBeenCalled();
  });
});

describe("installXTerminalKeyboardController Ctrl+U IME compatibility", () => {
  it("recovers idle keyCode 229 Ctrl+U through xterm input", () => {
    const harness = createHarness("xterm");
    const event = ctrlUEvent(229);

    expect(harness.keyHandler(event)).toBe(false);
    expect(event.defaultPrevented).toBe(true);
    expect(harness.routeKeyboardEvent).toHaveBeenCalledOnce();
    expect(harness.routeKeyboardEvent).toHaveBeenCalledWith(event);
    expect(harness.terminal.input).toHaveBeenCalledOnce();
    expect(harness.terminal.input).toHaveBeenCalledWith("\x15", true);
  });

  it("does not inject Ctrl+U while native IME owns the event", () => {
    const harness = createHarness("native-ime");
    const event = ctrlUEvent(229);

    expect(harness.keyHandler(event)).toBe(false);
    expect(event.defaultPrevented).toBe(false);
    expect(harness.routeKeyboardEvent).toHaveBeenCalledOnce();
    expect(harness.terminal.input).not.toHaveBeenCalled();
  });

  it("delegates ordinary Ctrl+U to xterm without manual injection", () => {
    const harness = createHarness("application");
    const event = ctrlUEvent(85, "u");

    expect(harness.keyHandler(event)).toBe(true);
    expect(event.defaultPrevented).toBe(false);
    expect(harness.routeKeyboardEvent).not.toHaveBeenCalled();
    expect(harness.terminal.input).not.toHaveBeenCalled();
  });

  it("keeps native IME ownership when masked Ctrl+U has a terminal selection", () => {
    const harness = createHarness("native-ime");
    vi.mocked(harness.terminal.hasSelection).mockReturnValue(true);
    const event = ctrlUEvent(229, "u");

    expect(harness.keyHandler(event)).toBe(false);
    expect(event.defaultPrevented).toBe(false);
    expect(harness.routeKeyboardEvent).toHaveBeenCalledOnce();
    expect(harness.routeKeyboardEvent).toHaveBeenCalledWith(event);
    expect(harness.terminal.input).not.toHaveBeenCalled();
  });

  it("preserves terminal selection when recovering masked Ctrl+U", () => {
    const harness = createHarness("xterm");
    vi.mocked(harness.terminal.hasSelection).mockReturnValue(true);
    const event = ctrlUEvent(229);

    expect(harness.keyHandler(event)).toBe(false);
    expect(event.defaultPrevented).toBe(true);
    expect(harness.routeKeyboardEvent).toHaveBeenCalledOnce();
    expect(harness.terminal.input).toHaveBeenCalledOnce();
    expect(harness.terminal.input).toHaveBeenCalledWith("\x15", false);
  });

  it("does not double-send masked Ctrl+U when IME exposes key u with a selection", () => {
    const harness = createHarness("xterm");
    vi.mocked(harness.terminal.hasSelection).mockReturnValue(true);
    const event = ctrlUEvent(229, "u");

    expect(harness.keyHandler(event)).toBe(false);
    expect(harness.terminal.input).toHaveBeenCalledOnce();
    expect(harness.terminal.input).toHaveBeenCalledWith("\x15", false);
  });
});
