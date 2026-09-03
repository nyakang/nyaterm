import { describe, expect, it } from "vitest";
import {
  DEFAULT_TERMINAL_RIGHT_CLICK_ACTION,
  normalizeTerminalRightClickAction,
  TERMINAL_RIGHT_CLICK_ACTIONS,
} from "./interactionSettings";

describe("terminal right-click settings", () => {
  it("accepts every supported action", () => {
    for (const action of TERMINAL_RIGHT_CLICK_ACTIONS) {
      expect(normalizeTerminalRightClickAction(action)).toBe(action);
    }
  });

  it("falls back to the menu action for invalid values", () => {
    expect(DEFAULT_TERMINAL_RIGHT_CLICK_ACTION).toBe("menu");
    expect(normalizeTerminalRightClickAction(undefined)).toBe("menu");
    expect(normalizeTerminalRightClickAction("invalid")).toBe("menu");
  });
});
