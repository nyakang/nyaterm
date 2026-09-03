import { isMacOS } from "./platform";

export const XTERM_PERFORMANCE_CONFIG = {
  highlighting: {
    /** Debounce delay in ms before re-scanning after new output is written. */
    debounceMs: 80,
    /** Idle delay before refreshing highlights after viewport scrolling stops. */
    scrollIdleDebounceMs: 120,
    /** Idle delay before rebuilding highlights after a suspended terminal resumes. */
    resumeIdleDelayMs: 150,
    /** Lines above and below the viewport to keep decorated on most platforms. */
    overscanLines: 15,
    /** macOS WebView benefits from a slightly smaller highlighted active zone. */
    macosOverscanLines: 10,
    /** Hard cap for total keyword highlight decorations held by one terminal. */
    maxDecorations: 1_000,
    /** Hard cap for new keyword highlight decorations created by one refresh. */
    maxDecorationsPerRefresh: 100,
    /** Hard cap for accepted keyword highlight matches on one physical line. */
    maxMatchesPerLine: 20,
    /** Main-thread time budget for one viewport refresh. */
    maxRefreshTimeMs: 3,
    /** Maximum immutable scrollback rows retained in the regex match LRU. */
    maxCachedMatchLines: 3_000,
  },
  output: {
    /** Backlog threshold where terminal side work should start yielding to rendering/input. */
    strainedBacklogBytes: 128 * 1024,
    /** Backlog threshold for using microtask low-latency writes on normal shell output. */
    lowLatencyFlushBacklogBytes: 64 * 1024,
    /** Main-thread time budget for one continuous foreground drain turn. */
    maxForegroundDrainTurnMs: 10,
    /** Watchdog delay that keeps visible output moving when animation frames are starved. */
    foregroundFrameFallbackMs: 100,
    /** Max UTF-8 bytes to write into xterm in a single call. */
    writeChunkBytes: 32 * 1024,
    /** Max UTF-8 bytes to write into xterm during one hidden background drain. */
    backgroundWriteChunkBytes: 16 * 1024,
    /** Delay between hidden background drain cycles. */
    backgroundDrainIntervalMs: 160,
    /** Max UTF-8 bytes to write into xterm during one hidden catch-up drain. */
    backgroundCatchUpWriteChunkBytes: 64 * 1024,
    /** Delay between hidden catch-up drain cycles. */
    backgroundCatchUpIntervalMs: 16,
    /** Lower per-frame write budget for repaint-heavy alternate-screen TUIs. */
    alternateScreenWriteChunkBytes: 16 * 1024,
    /** Max write rate while an alternate-screen TUI has queued repaint backlog. */
    alternateScreenMaxWriteFps: 20,
    /** Backlog threshold before alternate-screen foreground writes are throttled. */
    alternateScreenThrottleBacklogBytes: 32 * 1024,
    /** Recovery threshold after overload while visible. */
    visibleRecoveryThresholdBytes: 200_000,
    /** Recovery threshold after overload while hidden. */
    hiddenRecoveryThresholdBytes: 50_000,
    /** How long to keep the recovery notice visible. */
    recoveryNoticeMs: 3_000,
    /** Max time to wait for frontend writes before allowing deep hibernate. */
    hibernateDrainTimeoutMs: 2_500,
  },
  webgl: {
    /** Release WebGL resources after a terminal has been hidden for this long. */
    hiddenDisposeDelayMs: 15_000,
    /** Delay before trying to recreate WebGL after a context loss. */
    contextLossRetryDelayMs: 50,
    /** Rolling window for context-loss circuit breaking. */
    contextLossWindowMs: 10_000,
    /** More losses than this in the rolling window disables WebGL until settings/session reset. */
    contextLossCircuitBreakerLimit: 2,
    /** Number of reveal frames to repaint after a hidden terminal becomes visible. */
    revealRefreshFrames: 2,
  },
  lifecycle: {
    /** Fully dispose hidden eligible terminals after this delay. */
    deepHibernateDelayMs: 120_000,
    /** Release hidden keyword/action caches before deep hibernation. */
    hiddenCacheReleaseDelayMs: 30_000,
    /** Maximum scrollback rows to serialize for reconnect/rebuild/hibernate snapshots. */
    snapshotMaxLines: 3_000,
    /** Maximum serialized snapshot size to keep in memory. */
    snapshotMaxBytes: 2 * 1024 * 1024,
  },
} as const;

export function getKeywordHighlightPerformanceConfig() {
  const highlighting = XTERM_PERFORMANCE_CONFIG.highlighting;
  return {
    ...highlighting,
    resolvedOverscanLines: isMacOS ? highlighting.macosOverscanLines : highlighting.overscanLines,
  };
}
