import type { PerformanceMode } from "./xterminalTypes";

interface KeywordHighlightSuspensionState {
  visible: boolean;
  hibernated: boolean;
  terminalReady: boolean;
  performanceMode: PerformanceMode;
}

/**
 * Highlighting follows presentation readiness and output pressure. Focus/active
 * state is deliberately absent so every visible split pane can stay highlighted.
 */
export function shouldSuspendKeywordHighlighter({
  visible,
  hibernated,
  terminalReady,
  performanceMode,
}: KeywordHighlightSuspensionState): boolean {
  return !visible || hibernated || !terminalReady || performanceMode !== "normal";
}
