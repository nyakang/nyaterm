import type { SessionType } from "@/types/global";

const LEGACY_CTRL_KEYS = new Set([" ", "@", "[", "\\", "]", "^", "_", "?"]);

export type XTerminalDataOrigin = "keyboard" | "terminal_response";

export function resolveXTerminalDataOrigin(
  trackedOrigin: XTerminalDataOrigin,
  data: string,
  canObserveUserInput: boolean,
  expectedFocusReport: "\x1b[I" | "\x1b[O" | null,
): XTerminalDataOrigin {
  return !canObserveUserInput && data === expectedFocusReport
    ? "terminal_response"
    : trackedOrigin;
}

export function createXTerminalDataOriginTracker() {
  let nextDataWasUserInput = false;
  let userEventDepth = 0;
  let deferredGeneration = 0;
  return {
    markUserInput() {
      deferredGeneration += 1;
      nextDataWasUserInput = true;
    },
    markDeferredUserInput() {
      const generation = ++deferredGeneration;
      nextDataWasUserInput = true;
      // xterm schedules IME extraction with setTimeout(0) from the target
      // handler. A nested timer runs after that extraction, but still clears a
      // cancelled composition before unrelated terminal responses arrive.
      setTimeout(() => {
        setTimeout(() => {
          if (deferredGeneration === generation) {
            nextDataWasUserInput = false;
          }
        }, 0);
      }, 0);
    },
    beginUserInputEvent() {
      userEventDepth += 1;
    },
    endUserInputEvent() {
      userEventDepth = Math.max(0, userEventDepth - 1);
    },
    consume(): XTerminalDataOrigin {
      const wasUserInput = nextDataWasUserInput || userEventDepth > 0;
      nextDataWasUserInput = false;
      deferredGeneration += 1;
      return wasUserInput ? "keyboard" : "terminal_response";
    },
  };
}

export function getCtrlPrintableCsiuInput(e: KeyboardEvent): string | null {
  if (!e.ctrlKey || e.metaKey || e.altKey || e.key.length !== 1) return null;

  const codePoint = e.key.codePointAt(0);
  if (!codePoint || codePoint < 0x20 || codePoint > 0x7e) return null;

  if (
    /^[a-z]$/i.test(e.key) ||
    /^[2-8]$/.test(e.key) ||
    LEGACY_CTRL_KEYS.has(e.key)
  ) {
    return null;
  }

  const modifier = 1 + 4 + (e.shiftKey ? 1 : 0);
  return `\x1b[${codePoint};${modifier}u`;
}

export function isLocalBackspaceEvent(
  event: KeyboardEvent,
  sessionType: SessionType,
): boolean {
  if (
    sessionType !== "Local" ||
    event.ctrlKey ||
    event.metaKey ||
    event.altKey
  ) {
    return false;
  }

  return (
    event.key === "Backspace" ||
    (event.key === "Delete" && event.code === "Backspace")
  );
}

export function isSessionNotFoundError(error: unknown): boolean {
  const message =
    typeof error === "string"
      ? error
      : error instanceof Error
        ? error.message
        : String(error ?? "");
  return (
    message.toLowerCase().includes("session") &&
    message.toLowerCase().includes("not found")
  );
}
