import type { Terminal as XTermTerminal } from "@xterm/xterm";
import { describe, expect, it, vi } from "vitest";
import { createXTerminalDataOriginTracker } from "./xterminalKeyboardInput";
import type { XTermInternalTrimSource } from "./xterminalInternalTypes";

describe("xterm input-origin compatibility adapter", () => {
  it("fires onUserInput for real input but not a DA terminal response", async () => {
    const canvas = vi
      .spyOn(HTMLCanvasElement.prototype, "getContext")
      .mockReturnValue({
        measureText: () => ({ width: 10 }),
      } as unknown as CanvasRenderingContext2D);
    const { Terminal } = await import("@xterm/xterm");
    const terminal = new Terminal();
    const coreService = (terminal as XTermTerminal & XTermInternalTrimSource)
      ._core?.coreService;
    expect(typeof coreService?.onUserInput).toBe("function");

    const tracker = createXTerminalDataOriginTracker();
    const origins: string[] = [];
    const userDisposable = coreService?.onUserInput?.(() => {
      tracker.markUserInput();
    });
    const dataDisposable = terminal.onData(() => {
      origins.push(tracker.consume());
    });

    terminal.input("x", true);
    await new Promise<void>((resolve) => terminal.write("\x1b[c", resolve));

    expect(origins).toEqual(["keyboard", "terminal_response"]);

    dataDisposable.dispose();
    userDisposable?.dispose();
    terminal.dispose();
    canvas.mockRestore();
  });
});
