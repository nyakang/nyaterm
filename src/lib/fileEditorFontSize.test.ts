import { describe, expect, it } from "vitest";
import {
  clampFileEditorFontSize,
  DEFAULT_FILE_EDITOR_FONT_SIZE,
  decreaseFileEditorFontSize,
  increaseFileEditorFontSize,
} from "./fileEditorFontSize";

describe("clampFileEditorFontSize", () => {
  it("clamps below the minimum", () => {
    expect(clampFileEditorFontSize(1)).toBe(8);
  });

  it("clamps above the maximum", () => {
    expect(clampFileEditorFontSize(100)).toBe(72);
  });

  it("rounds fractional sizes", () => {
    expect(clampFileEditorFontSize(13.6)).toBe(14);
  });

  it("falls back to the default for non-finite values", () => {
    expect(clampFileEditorFontSize(Number.NaN)).toBe(DEFAULT_FILE_EDITOR_FONT_SIZE);
    expect(clampFileEditorFontSize(Number.POSITIVE_INFINITY)).toBe(
      DEFAULT_FILE_EDITOR_FONT_SIZE,
    );
  });
});

describe("increaseFileEditorFontSize", () => {
  it("steps up by one", () => {
    expect(increaseFileEditorFontSize(13)).toBe(14);
  });

  it("stops at the maximum", () => {
    expect(increaseFileEditorFontSize(72)).toBe(72);
  });
});

describe("decreaseFileEditorFontSize", () => {
  it("steps down by one", () => {
    expect(decreaseFileEditorFontSize(13)).toBe(12);
  });

  it("stops at the minimum", () => {
    expect(decreaseFileEditorFontSize(8)).toBe(8);
  });
});
