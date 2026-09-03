import type { Terminal } from "@xterm/xterm";
import { describe, expect, it, vi } from "vitest";
import {
  registerTerminalUserInputMarker,
  sendTerminalClearInput,
} from "./terminalControlInput";

describe("terminal control input origin", () => {
  it("marks programmatic user input before emitting terminal data", () => {
    const order: string[] = [];
    const terminal = {
      clearSelection: vi.fn(),
      input: vi.fn(() => order.push("input")),
      focus: vi.fn(),
    } as unknown as Terminal;
    const unregister = registerTerminalUserInputMarker(terminal, () => {
      order.push("mark");
    });

    sendTerminalClearInput(terminal, { focus: true });

    expect(order).toEqual(["mark", "input"]);
    expect(terminal.input).toHaveBeenCalledWith("\x0c", true);
    expect(terminal.focus).toHaveBeenCalledOnce();
    unregister();
  });
});
