import { describe, expect, it } from "vitest";
import { XTERM_SECURE_WINDOW_OPTIONS } from "./xtermSecurity";

describe("xterm window option trust boundary", () => {
  it("keeps application-controlled title queries disabled", () => {
    expect(XTERM_SECURE_WINDOW_OPTIONS.getWinTitle).toBe(false);
    expect(XTERM_SECURE_WINDOW_OPTIONS.getIconTitle).toBe(false);
    expect(Object.isFrozen(XTERM_SECURE_WINDOW_OPTIONS)).toBe(true);
  });
});
