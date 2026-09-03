import { describe, expect, it } from "vitest";
import { shouldSuspendKeywordHighlighter } from "./xterminalKeywordHighlighting";

describe("shouldSuspendKeywordHighlighter", () => {
  const ready = {
    visible: true,
    hibernated: false,
    terminalReady: true,
    performanceMode: "normal" as const,
  };

  it("only resumes for a visible, ready terminal under normal pressure", () => {
    expect(shouldSuspendKeywordHighlighter(ready)).toBe(false);
    expect(shouldSuspendKeywordHighlighter({ ...ready, visible: false })).toBe(true);
    expect(shouldSuspendKeywordHighlighter({ ...ready, hibernated: true })).toBe(true);
    expect(shouldSuspendKeywordHighlighter({ ...ready, terminalReady: false })).toBe(true);
    expect(
      shouldSuspendKeywordHighlighter({ ...ready, performanceMode: "strained" }),
    ).toBe(true);
    expect(
      shouldSuspendKeywordHighlighter({ ...ready, performanceMode: "overloaded" }),
    ).toBe(true);
  });

  it("does not accept focus or active state as an input", () => {
    expect(Object.keys(ready)).not.toContain("active");
    expect(shouldSuspendKeywordHighlighter(ready)).toBe(false);
  });
});
