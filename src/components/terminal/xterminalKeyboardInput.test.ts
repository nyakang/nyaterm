import { afterEach, describe, expect, it, vi } from "vitest";
import {
  createXTerminalDataOriginTracker,
  resolveXTerminalDataOrigin,
} from "./xterminalKeyboardInput";

describe("xterm data origin tracking", () => {
  afterEach(() => {
    vi.useRealTimers();
  });
  it("distinguishes xterm terminal responses from observed user input", () => {
    const tracker = createXTerminalDataOriginTracker();

    expect(tracker.consume()).toBe("terminal_response");
    tracker.markUserInput();
    expect(tracker.consume()).toBe("keyboard");
    expect(tracker.consume()).toBe("terminal_response");
  });

  it("clears a cancelled deferred IME marker before a later response", async () => {
    vi.useFakeTimers();
    const tracker = createXTerminalDataOriginTracker();

    tracker.markDeferredUserInput();
    await vi.runAllTimersAsync();
    expect(tracker.consume()).toBe("terminal_response");

    tracker.markDeferredUserInput();
    expect(tracker.consume()).toBe("keyboard");
    await vi.runAllTimersAsync();
    expect(tracker.consume()).toBe("terminal_response");
  });

  it("does not override explicitly marked paste bytes that resemble focus reports", () => {
    expect(resolveXTerminalDataOrigin("keyboard", "\x1b[I", false, null)).toBe(
      "keyboard",
    );
    expect(
      resolveXTerminalDataOrigin("keyboard", "\x1b[I", false, "\x1b[I"),
    ).toBe("terminal_response");
  });

  it("uses a DOM-event scope as the compatibility fallback", () => {
    const tracker = createXTerminalDataOriginTracker();

    expect(tracker.consume()).toBe("terminal_response");
    tracker.beginUserInputEvent();
    expect(tracker.consume()).toBe("keyboard");
    expect(tracker.consume()).toBe("keyboard");
    tracker.endUserInputEvent();
    expect(tracker.consume()).toBe("terminal_response");
  });
});
