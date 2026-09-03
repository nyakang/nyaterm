import type { Terminal } from "@xterm/xterm";
import { renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { XTERM_PERFORMANCE_CONFIG } from "@/lib/xtermPerformance";
import type { AppSettings } from "@/types/global";
import { useKeywordHighlighter } from "./useKeywordHighlighter";

const highlighterMocks = vi.hoisted(() => ({
  instances: [] as Array<{
    dispose: ReturnType<typeof vi.fn>;
    releaseCaches: ReturnType<typeof vi.fn>;
    setRules: ReturnType<typeof vi.fn>;
    setSuspended: ReturnType<typeof vi.fn>;
  }>,
}));

vi.mock("../lib/keywordHighlighter", () => ({
  KeywordHighlighter: class {
    dispose = vi.fn();
    releaseCaches = vi.fn();
    setRules = vi.fn();
    setSuspended = vi.fn();

    constructor() {
      highlighterMocks.instances.push(this);
    }
  },
}));

vi.mock("../lib/keywordHighlightPresets", () => ({ getBuiltinRules: () => [] }));

const settings = {
  keyword_highlights_enabled: true,
  keyword_highlights: [],
  keyword_highlight_builtin_rules: {},
  keyword_highlights_across_wrapped_lines: false,
} as unknown as AppSettings["terminal"];

describe("useKeywordHighlighter cache release policy", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    highlighterMocks.instances.length = 0;
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("suspends under pressure without scheduling a cache release", () => {
    const terminal = {} as Terminal;
    renderHook(() =>
      useKeywordHighlighter(terminal, settings, "session-1", true, {
        suspended: true,
        releaseCachesAfterDelay: false,
      }),
    );
    const highlighter = highlighterMocks.instances[0];
    vi.advanceTimersByTime(XTERM_PERFORMANCE_CONFIG.lifecycle.hiddenCacheReleaseDelayMs);

    expect(highlighter.setSuspended).toHaveBeenCalledWith(true);
    expect(highlighter.releaseCaches).not.toHaveBeenCalled();
  });

  it("releases caches after a terminal stays hidden", () => {
    const terminal = {} as Terminal;
    renderHook(() =>
      useKeywordHighlighter(terminal, settings, "session-1", true, {
        suspended: true,
        releaseCachesAfterDelay: true,
      }),
    );
    const highlighter = highlighterMocks.instances[0];
    vi.advanceTimersByTime(XTERM_PERFORMANCE_CONFIG.lifecycle.hiddenCacheReleaseDelayMs);
    expect(highlighter.releaseCaches).toHaveBeenCalledTimes(1);
  });
});
