import { describe, expect, it } from "vitest";
import { renderAiCommandStart } from "./aiTerminalRenderer";

describe("AI command sources", () => {
  it("supports legacy events and labels external MCP commands", () => {
    expect(
      renderAiCommandStart({
        type: "commandStart",
        command: "echo ok",
        stepIndex: 0,
      }),
    ).toContain("AI #1");
    expect(
      renderAiCommandStart({
        type: "commandStart",
        command: "echo ok",
        stepIndex: 1,
        source: "MCP",
      }),
    ).toContain("MCP #2");
  });
});
