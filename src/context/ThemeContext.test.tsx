import { describe, expect, it } from "vitest";
import type { ThemeColors } from "@/lib/themes";
import { applyTerminalThemeToDOM } from "./ThemeContext";

describe("applyTerminalThemeToDOM", () => {
  it("publishes the terminal selection color for shared editor surfaces", () => {
    const selectionBackground = "#264f78";

    applyTerminalThemeToDOM({
      selectionBackground,
    } as ThemeColors["terminal"]);

    expect(document.documentElement.style.getPropertyValue("--df-terminal-selection")).toBe(
      selectionBackground,
    );
  });
});
