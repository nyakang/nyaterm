import type { Terminal } from "@xterm/xterm";
import { useEffect, useMemo, useRef, useState } from "react";
import { XTERM_PERFORMANCE_CONFIG } from "@/lib/xtermPerformance";
import { KeywordHighlighter } from "../lib/keywordHighlighter";
import { getBuiltinRules } from "../lib/keywordHighlightPresets";
import type { AppSettings, KeywordHighlightRule } from "../types/global";

/**
 * Creates and manages a KeywordHighlighter tied to the terminal session lifecycle.
 *
 * Rule priority: built-in rules are compiled first, user rules last.
 * xterm registers decorations in that order, so user-rule decorations render
 * on top and visually override built-in ones when patterns overlap.
 *
 * isDark should be derived from the current theme's background luminance so
 * built-in rule colors switch automatically when the user changes themes.
 */
export function useKeywordHighlighter(
  terminal: Terminal | null,
  terminalSettings: AppSettings["terminal"],
  sessionId: string,
  isDark: boolean,
  options: {
    suspended?: boolean;
    releaseCachesAfterDelay?: boolean;
  } = {},
): void {
  const highlighterRef = useRef<KeywordHighlighter | null>(null);
  const cacheReleaseTimerRef = useRef<number | null>(null);
  const [highlighterInstance, setHighlighterInstance] = useState<KeywordHighlighter | null>(null);
  const enabled = terminalSettings.keyword_highlights_enabled ?? false;
  const suspended = options.suspended ?? false;
  const releaseCachesAfterDelay = options.releaseCachesAfterDelay ?? false;

  // Merge user rules (higher priority) + built-in rules (lower priority).
  // User rules carry two color fields; pick the right one for the current theme
  // so the highlighter engine always receives a single resolved color.
  const mergedRules = useMemo(() => {
    const builtinRuleSettings = terminalSettings.keyword_highlight_builtin_rules ?? {};
    const builtin = getBuiltinRules(isDark).map((rule) => ({
      ...rule,
      enabled: builtinRuleSettings[rule.id] ?? true,
    }));
    const user = (terminalSettings.keyword_highlights ?? []).map((r: KeywordHighlightRule) => ({
      id: r.id,
      name: r.name,
      patterns: r.patterns,
      color: isDark ? r.color_dark : r.color_light,
      enabled: r.enabled,
    }));
    // User rules go first so they match and occupy string positions before built-ins
    return [...user, ...builtin];
  }, [
    isDark,
    terminalSettings.keyword_highlight_builtin_rules,
    terminalSettings.keyword_highlights,
  ]);

  // Create the highlighter once per terminal instance.
  useEffect(() => {
    if (!enabled) {
      highlighterRef.current?.dispose();
      highlighterRef.current = null;
      setHighlighterInstance(null);
      return;
    }

    if (!terminal) return;

    const highlighter = new KeywordHighlighter(terminal, sessionId);
    highlighterRef.current = highlighter;
    setHighlighterInstance(highlighter);

    return () => {
      highlighter.dispose();
      highlighterRef.current = null;
      setHighlighterInstance((current) => (current === highlighter ? null : current));
    };
  }, [terminal, enabled, sessionId]);

  // Re-push rules whenever settings change or theme family switches.
  useEffect(() => {
    if (!highlighterInstance) return;
    highlighterInstance.setRules(
      mergedRules,
      enabled,
      terminalSettings.keyword_highlights_across_wrapped_lines ?? false,
    );
  }, [
    highlighterInstance,
    mergedRules,
    enabled,
    terminalSettings.keyword_highlights_across_wrapped_lines,
  ]);

  useEffect(() => {
    highlighterInstance?.setSuspended(suspended);
    if (!highlighterInstance) return;

    if (cacheReleaseTimerRef.current !== null) {
      window.clearTimeout(cacheReleaseTimerRef.current);
      cacheReleaseTimerRef.current = null;
    }

    if (suspended && releaseCachesAfterDelay) {
      cacheReleaseTimerRef.current = window.setTimeout(() => {
        cacheReleaseTimerRef.current = null;
        highlighterInstance.releaseCaches();
      }, XTERM_PERFORMANCE_CONFIG.lifecycle.hiddenCacheReleaseDelayMs);
    }

    return () => {
      if (cacheReleaseTimerRef.current !== null) {
        window.clearTimeout(cacheReleaseTimerRef.current);
        cacheReleaseTimerRef.current = null;
      }
    };
  }, [highlighterInstance, releaseCachesAfterDelay, suspended]);
}
