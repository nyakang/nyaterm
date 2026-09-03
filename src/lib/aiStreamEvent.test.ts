import { describe, expect, it } from "vitest";
import { classifyAIStreamControlEvent } from "./aiStreamEvent";

describe("AI stream control events", () => {
  it("keeps warning events non-terminal", () => {
    expect(classifyAIStreamControlEvent("warning")).toEqual({
      kind: "warning",
      terminatesStream: false,
    });
  });

  it.each(["done", "error"] as const)("treats %s as terminal", (type) => {
    expect(classifyAIStreamControlEvent(type).terminatesStream).toBe(true);
  });
});
