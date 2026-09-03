import type {
  IBufferCell,
  IBufferLine,
  IDecoration,
  IDisposable,
  IMarker,
  Terminal as XTerm,
} from "@xterm/xterm";
import type { ResolvedHighlightRule } from "./keywordHighlightPresets";
import { logger } from "./logger";
import { getKeywordHighlightPerformanceConfig, XTERM_PERFORMANCE_CONFIG } from "./xtermPerformance";

interface CompiledRule {
  regex: RegExp;
  color: string;
}

interface CachedDecoration {
  decoration: IDecoration;
  marker: IMarker;
  lineY: number;
}

interface HighlightSpan {
  cellStartCol: number;
  cellWidth: number;
  color: string;
}

interface LogicalLineSegment {
  line: IBufferLine;
  lineY: number;
  text: string;
  startIndex: number;
  endIndex: number;
  cellMap: number[] | null;
}

type KeywordHighlightPerformanceConfig = ReturnType<typeof getKeywordHighlightPerformanceConfig>;

interface RefreshBudget {
  createdDecorations: number;
  deadlineMs: number;
  hitLimit: boolean;
  hitTotalDecorationLimit: boolean;
}

interface SpanScanResult {
  spans: HighlightSpan[];
  complete: boolean;
}

interface WrappedSpanScanResult {
  spansByLine: Map<number, HighlightSpan[]>;
  lineYs: number[];
  complete: boolean;
  cacheable: boolean;
}

type RefreshReason = "write" | "scroll_idle" | "resume" | "continuation" | "resize";

interface RefreshStats {
  cacheHits: number;
  cacheMisses: number;
  scannedLines: number;
  decorationsCreated: number;
  decorationsDisposed: number;
}

/**
 * Manages terminal decorations for keyword highlighting.
 *
 * Optimizations over a naive implementation:
 * - Overscan buffer: keeps a small decoration zone around the viewport.
 * - Match LRU: immutable scrollback rows retain regex results independently from
 *   short-lived xterm decorations, including rows with no matches.
 * - Fast ASCII path: skips building the wide-char cell map for lines with only ASCII chars.
 * - Deduplicates scroll/render events: onRender viewport-Y check replaces the redundant onScroll.
 * - Auto-invalidation: each decoration subscribes to its own onDispose without
 *   invalidating the independent match cache.
 * - Alternate buffer guard: clears decorations immediately when TUI apps (vim, htop) take over.
 */
export class KeywordHighlighter implements IDisposable {
  private term: XTerm;
  private compiledRules: CompiledRule[] = [];
  private decorationCache = new Map<string, CachedDecoration>();
  /** Immutable absolute buffer line index → resolved highlight spans (including []). */
  private lineMatchCache = new Map<number, HighlightSpan[]>();
  private writeDebounceTimer: ReturnType<typeof setTimeout> | null = null;
  private scrollDebounceTimer: ReturnType<typeof setTimeout> | null = null;
  private resumeRefreshTimer: ReturnType<typeof setTimeout> | null = null;
  private resumeRefreshFrame: number | null = null;
  private continuationRefreshFrame: number | null = null;
  private enabled = false;
  private suspended = false;
  private highlightAcrossWrappedLines = false;
  private disposables: IDisposable[] = [];
  private lastViewportY = -1;

  /** Marker anchored at buffer line 0 to detect scrollback trimming. */
  private sentinelMarker: IMarker | null = null;
  private sentinelDisposable: IDisposable | null = null;
  private bufferTrimmed = false;

  private static readonly MAX_LOGICAL_LINE_SCAN_CHARS = 16 * 1024;

  constructor(term: XTerm, private readonly sessionId?: string) {
    this.term = term;

    this.disposables.push(
      this.term.onWriteParsed(() => this.triggerWriteRefresh()),
      this.term.onResize(() => {
        this.invalidateAll();
        this.lastViewportY = -1;
        this.triggerWriteRefresh("resize");
      }),
      this.term.onRender(() => {
        const currentViewportY = this.term.buffer.active?.viewportY ?? 0;
        if (currentViewportY !== this.lastViewportY) {
          this.lastViewportY = currentViewportY;
          this.triggerScrollRefresh();
        }
      }),
    );
  }

  public setRules(
    rules: ResolvedHighlightRule[],
    enabled: boolean,
    highlightAcrossWrappedLines = false,
  ): void {
    this.enabled = enabled;
    this.highlightAcrossWrappedLines = highlightAcrossWrappedLines;

    this.compiledRules = [];
    for (const rule of rules) {
      if (!rule.enabled || rule.patterns.length === 0) continue;
      const validAlts: string[] = [];
      for (const pattern of rule.patterns) {
        const trimmed = pattern.trim();
        if (!trimmed) continue;
        try {
          new RegExp(trimmed, "gi");
          validAlts.push(trimmed);
        } catch {
          // silently skip invalid regex
        }
      }
      if (validAlts.length === 0) continue;
      const combined =
        validAlts.length === 1 ? validAlts[0] : validAlts.map((p) => `(?:${p})`).join("|");
      try {
        this.compiledRules.push({ regex: new RegExp(combined, "gi"), color: rule.color });
      } catch {
        // fallback: compile each pattern individually
        for (const alt of validAlts) {
          try {
            this.compiledRules.push({ regex: new RegExp(alt, "gi"), color: rule.color });
          } catch {
            // skip
          }
        }
      }
    }

    this.invalidateAll();
    if (this.enabled && this.compiledRules.length > 0) {
      this.triggerWriteRefresh("write");
    }
  }

  public setSuspended(suspended: boolean): void {
    if (this.suspended === suspended) return;
    this.suspended = suspended;

    if (suspended) {
      this.clearAllTimers();
      return;
    }

    if (this.enabled && this.compiledRules.length > 0) {
      this.lastViewportY = -1;
      this.triggerResumeRefresh();
    }
  }

  public releaseCaches(): void {
    this.invalidateAll();
    this.lastViewportY = -1;
  }

  public dispose(): void {
    this.invalidateAll();
    this.disposables.forEach((d) => {
      d.dispose();
    });
    this.disposables = [];
  }

  private clearAllTimers(): void {
    if (this.writeDebounceTimer) {
      clearTimeout(this.writeDebounceTimer);
      this.writeDebounceTimer = null;
    }
    if (this.scrollDebounceTimer) {
      clearTimeout(this.scrollDebounceTimer);
      this.scrollDebounceTimer = null;
    }
    if (this.resumeRefreshTimer) {
      clearTimeout(this.resumeRefreshTimer);
      this.resumeRefreshTimer = null;
    }
    if (this.resumeRefreshFrame !== null) {
      cancelAnimationFrame(this.resumeRefreshFrame);
      this.resumeRefreshFrame = null;
    }
    if (this.continuationRefreshFrame !== null) {
      cancelAnimationFrame(this.continuationRefreshFrame);
      this.continuationRefreshFrame = null;
    }
  }

  /**
   * Place a marker at buffer line 0. When xterm trims the scrollback (buffer
   * full), this line is evicted and the marker is disposed, letting us detect
   * the shift and invalidate all index-based caches.
   */
  private installSentinel(): void {
    this.disposeSentinel();
    const buffer = this.term.buffer.active;
    if (!buffer || buffer.length === 0) return;

    const cursorAbsoluteY = buffer.baseY + buffer.cursorY;
    const marker = this.term.registerMarker(-cursorAbsoluteY);
    if (!marker || marker.line < 0) return;

    this.sentinelMarker = marker;
    this.sentinelDisposable = marker.onDispose(() => {
      this.bufferTrimmed = true;
      this.sentinelMarker = null;
      this.sentinelDisposable = null;
    });
  }

  private disposeSentinel(): void {
    this.sentinelDisposable?.dispose();
    this.sentinelMarker?.dispose();
    this.sentinelMarker = null;
    this.sentinelDisposable = null;
  }

  private canRefresh(): boolean {
    if (!this.enabled || this.suspended || this.compiledRules.length === 0) return false;
    if (this.term.buffer.active.type === "alternate") {
      this.invalidateAll();
      return false;
    }
    return true;
  }

  /** Debounced refresh for write/resize events (batches rapid output). */
  private triggerWriteRefresh(reason: "write" | "resize" = "write"): void {
    if (!this.canRefresh()) return;
    if (
      this.scrollDebounceTimer !== null ||
      this.resumeRefreshTimer !== null ||
      this.resumeRefreshFrame !== null
    ) {
      return;
    }
    if (this.writeDebounceTimer) clearTimeout(this.writeDebounceTimer);
    this.writeDebounceTimer = setTimeout(() => {
      this.writeDebounceTimer = null;
      this.refreshViewport(reason);
    }, XTERM_PERFORMANCE_CONFIG.highlighting.debounceMs);
  }

  /** Pure trailing debounce: scrolling always preempts pending highlight work. */
  private triggerScrollRefresh(): void {
    if (!this.canRefresh()) return;
    if (this.writeDebounceTimer !== null) {
      clearTimeout(this.writeDebounceTimer);
      this.writeDebounceTimer = null;
    }
    this.cancelContinuationRefresh();
    this.cancelResumeRefresh();
    if (this.scrollDebounceTimer !== null) clearTimeout(this.scrollDebounceTimer);
    this.scrollDebounceTimer = setTimeout(() => {
      this.scrollDebounceTimer = null;
      this.refreshViewport("scroll_idle");
    }, XTERM_PERFORMANCE_CONFIG.highlighting.scrollIdleDebounceMs);
  }

  /**
   * Let the visible terminal's first fit/refresh/output frame complete before
   * doing highlight work after a tab becomes visible again.
   */
  private triggerResumeRefresh(): void {
    if (!this.canRefresh()) return;
    if (this.resumeRefreshTimer !== null || this.resumeRefreshFrame !== null) return;

    this.resumeRefreshTimer = setTimeout(() => {
      this.resumeRefreshTimer = null;
      if (!this.canRefresh() || this.scrollDebounceTimer !== null) return;
      this.resumeRefreshFrame = requestAnimationFrame(() => {
        this.resumeRefreshFrame = null;
        this.refreshViewport("resume");
      });
    }, XTERM_PERFORMANCE_CONFIG.highlighting.resumeIdleDelayMs);
  }

  private triggerContinuationRefresh(): void {
    if (!this.canRefresh()) return;
    if (this.continuationRefreshFrame !== null || this.scrollDebounceTimer !== null) return;

    this.continuationRefreshFrame = requestAnimationFrame(() => {
      this.continuationRefreshFrame = null;
      this.refreshViewport("continuation");
    });
  }

  private cancelContinuationRefresh(): void {
    if (this.continuationRefreshFrame === null) return;
    cancelAnimationFrame(this.continuationRefreshFrame);
    this.continuationRefreshFrame = null;
  }

  private cancelResumeRefresh(): void {
    if (this.resumeRefreshTimer !== null) {
      clearTimeout(this.resumeRefreshTimer);
      this.resumeRefreshTimer = null;
    }
    if (this.resumeRefreshFrame !== null) {
      cancelAnimationFrame(this.resumeRefreshFrame);
      this.resumeRefreshFrame = null;
    }
  }

  /** Clear the map before disposal so onDispose callbacks are no-ops. */
  private clearDecorations(): void {
    const entries = [...this.decorationCache.values()];
    this.decorationCache.clear();
    for (const { decoration, marker } of entries) {
      decoration.dispose();
      marker.dispose();
    }
  }

  private clearMatchCache(): void {
    this.lineMatchCache.clear();
  }

  private invalidateAll(): void {
    this.clearAllTimers();
    this.clearDecorations();
    this.clearMatchCache();
    this.disposeSentinel();
    this.bufferTrimmed = false;
  }

  private getCachedMatches(lineY: number): HighlightSpan[] | undefined {
    const spans = this.lineMatchCache.get(lineY);
    if (spans === undefined) return undefined;
    this.lineMatchCache.delete(lineY);
    this.lineMatchCache.set(lineY, spans);
    return spans;
  }

  private setCachedMatches(lineY: number, spans: HighlightSpan[]): void {
    this.lineMatchCache.delete(lineY);
    this.lineMatchCache.set(lineY, spans);
    const maxLines = XTERM_PERFORMANCE_CONFIG.highlighting.maxCachedMatchLines;
    while (this.lineMatchCache.size > maxLines) {
      const oldest = this.lineMatchCache.keys().next().value;
      if (oldest === undefined) break;
      this.lineMatchCache.delete(oldest);
    }
  }

  private buildStringToCellMap(
    line: IBufferLine,
    stringLength: number,
    maxCols: number,
    scratchCell: IBufferCell,
  ): number[] {
    const map: number[] = [];
    let col = 0;
    let cellEndCol = 0;

    // Mirror translateToString's traversal exactly: advance by `width || 1` so that
    // wide-char continuation cells (width=0 placeholder after a 2-wide glyph) are
    // naturally skipped, while NUL cells (also width=0, but emitted as a space by
    // translateToString) still contribute one entry.  The old `col++` loop with an
    // explicit `width === 0 → continue` guard incorrectly skipped both kinds and
    // produced a map shorter than lineText when NUL cells appeared before wide chars.
    while (col < maxCols && map.length < stringLength) {
      const cell = line.getCell(col, scratchCell);
      if (!cell) break;

      const chars = cell.getChars();
      const width = cell.getWidth();
      // translateToString advances by `width || 1`; replicate the same stride so our
      // map index always stays in sync with the returned string.
      const stride = width || 1;

      if (chars.length === 0) {
        // NUL cell: getChars() returns '' but translateToString emits WHITESPACE_CELL_CHAR.
        map.push(col);
      } else {
        for (let i = 0; i < chars.length; i++) {
          map.push(col);
        }
      }
      cellEndCol = col + stride;
      col += stride;
    }

    map.push(cellEndCol); // sentinel: end position
    return map;
  }

  private getLogicalLineBounds(
    buffer: XTerm["buffer"]["active"],
    lineY: number,
    totalLines: number,
  ): { startY: number; endY: number } {
    let startY = lineY;
    while (startY > 0) {
      const currentLine = buffer.getLine(startY);
      if (!currentLine?.isWrapped) break;
      startY--;
    }

    let endY = lineY;
    while (endY + 1 < totalLines) {
      const nextLine = buffer.getLine(endY + 1);
      if (!nextLine?.isWrapped) break;
      endY++;
    }

    return { startY, endY };
  }

  private isBudgetExpired(budget: RefreshBudget): boolean {
    if (performance.now() <= budget.deadlineMs) return false;
    budget.hitLimit = true;
    return true;
  }

  private isBudgetExhausted(budget: RefreshBudget): boolean {
    return budget.hitLimit || this.isBudgetExpired(budget);
  }

  private canCreateDecoration(
    config: KeywordHighlightPerformanceConfig,
    budget: RefreshBudget,
  ): boolean {
    if (this.decorationCache.size >= config.maxDecorations) {
      budget.hitLimit = true;
      budget.hitTotalDecorationLimit = true;
      return false;
    }
    if (budget.createdDecorations >= config.maxDecorationsPerRefresh) {
      budget.hitLimit = true;
      return false;
    }
    return !this.isBudgetExhausted(budget);
  }

  private hasAnsiForegroundInRange(
    line: IBufferLine,
    start: number,
    end: number,
    cellMap: number[] | null,
    scratchCell: IBufferCell,
  ): boolean {
    for (let i = start; i < end; i++) {
      const cellCol = cellMap ? (cellMap[i] ?? i) : i;
      const cell = line.getCell(cellCol, scratchCell);
      if (cell && !cell.isFgDefault()) return true;
    }
    return false;
  }

  private hasWrappedAnsiForegroundInRange(
    segments: LogicalLineSegment[],
    start: number,
    end: number,
    scratchCell: IBufferCell,
  ): boolean {
    for (const segment of segments) {
      if (segment.endIndex <= start || segment.startIndex >= end) continue;

      const localStart = Math.max(start, segment.startIndex) - segment.startIndex;
      const localEnd = Math.min(end, segment.endIndex) - segment.startIndex;
      if (
        this.hasAnsiForegroundInRange(
          segment.line,
          localStart,
          localEnd,
          segment.cellMap,
          scratchCell,
        )
      ) {
        return true;
      }
    }
    return false;
  }

  private ensureDecoration(
    lineY: number,
    cellStartCol: number,
    cellWidth: number,
    color: string,
    cursorAbsoluteY: number,
    config: KeywordHighlightPerformanceConfig,
    budget: RefreshBudget,
  ): string | null {
    if (cellWidth <= 0) return null;

    const key = `${lineY}:${cellStartCol}:${cellWidth}:${color}`;
    if (this.decorationCache.has(key)) return key;
    if (!this.canCreateDecoration(config, budget)) return null;

    const offset = lineY - cursorAbsoluteY;
    const marker = this.term.registerMarker(offset);
    if (!marker) return null;

    const deco = this.term.registerDecoration({
      marker,
      x: cellStartCol,
      width: cellWidth,
      foregroundColor: color,
    });

    if (!deco) {
      marker.dispose();
      return null;
    }

    deco.onRender((element: HTMLElement) => {
      element.style.pointerEvents = "none";
    });

    // Decoration lifetime never invalidates immutable regex match results.
    deco.onDispose(() => {
      if (this.decorationCache.get(key)?.decoration === deco) {
        this.decorationCache.delete(key);
      }
    });

    this.decorationCache.set(key, { decoration: deco, marker, lineY });
    budget.createdDecorations++;
    return key;
  }

  private scanPhysicalLine(
    line: IBufferLine,
    scratchCell: IBufferCell,
    config: KeywordHighlightPerformanceConfig,
    budget: RefreshBudget,
  ): SpanScanResult {
    const maxCols = Math.min(line.length, this.term.cols);
    const lineText = line.translateToString(true, 0, maxCols);
    if (!lineText) return { spans: [], complete: true };

    // Only build the wide-char map if actually needed (non-ASCII present)
    const hasMultibyte = /[^\u0000-\u00FF]/.test(lineText);
    const cellMap = hasMultibyte
      ? this.buildStringToCellMap(line, lineText.length, maxCols, scratchCell)
      : null;

    // Track occupied characters in the string to prevent multi-rule overlapping
    const occupied = new Uint8Array(lineText.length);

    const spans: HighlightSpan[] = [];

    for (const { regex, color } of this.compiledRules) {
      if (spans.length >= config.maxMatchesPerLine) break;
      if (this.isBudgetExhausted(budget)) return { spans, complete: false };
      regex.lastIndex = 0;

      while (true) {
        if (spans.length >= config.maxMatchesPerLine) break;
        if (this.isBudgetExhausted(budget)) return { spans, complete: false };
        const match = regex.exec(lineText);
        if (match === null) break;

        // Avoid infinite loops on empty matches
        if (match[0].length === 0) {
          regex.lastIndex++;
          continue;
        }

        const strStart = match.index;
        const strEnd = strStart + match[0].length;

        // Check for collision with higher-priority matches.
        let isOverlapping = false;
        for (let k = strStart; k < strEnd; k++) {
          if (occupied[k]) {
            isOverlapping = true;
            break;
          }
        }
        if (isOverlapping) continue;

        // Avoid overriding original shell output colors (e.g. from `ls --color`).
        if (this.hasAnsiForegroundInRange(line, strStart, strEnd, cellMap, scratchCell)) continue;

        const cellStartCol = cellMap ? (cellMap[strStart] ?? strStart) : strStart;
        const cellEndCol = cellMap ? (cellMap[strEnd] ?? strEnd) : strEnd;
        const cellWidth = cellEndCol - cellStartCol;
        if (cellWidth <= 0) continue;
        spans.push({
          cellStartCol,
          cellWidth,
          color,
        });

        // Match priority is independent from whether a decoration can be created this frame.
        for (let k = strStart; k < strEnd; k++) {
          occupied[k] = 1;
        }
      }
    }

    return { spans, complete: true };
  }

  private scanWrappedLogicalLine(
    buffer: XTerm["buffer"]["active"],
    startY: number,
    endY: number,
    scratchCell: IBufferCell,
    config: KeywordHighlightPerformanceConfig,
    budget: RefreshBudget,
  ): WrappedSpanScanResult {
    const segments: LogicalLineSegment[] = [];
    let logicalLength = 0;

    for (let currentY = startY; currentY <= endY; currentY++) {
      if (this.isBudgetExhausted(budget)) {
        return { spansByLine: new Map(), lineYs: [], complete: false, cacheable: false };
      }
      const line = buffer.getLine(currentY);
      if (!line) continue;

      const maxCols = Math.min(line.length, this.term.cols);
      const text = line.translateToString(currentY === endY, 0, maxCols);
      const startIndex = logicalLength;
      logicalLength += text.length;

      segments.push({
        line,
        lineY: currentY,
        text,
        startIndex,
        endIndex: logicalLength,
        cellMap:
          /[^\u0000-\u00FF]/.test(text) && text.length > 0
            ? this.buildStringToCellMap(line, text.length, maxCols, scratchCell)
            : null,
      });
    }

    const lineYs = segments.map((segment) => segment.lineY);
    const emptyByLine = new Map(lineYs.map((lineY) => [lineY, [] as HighlightSpan[]]));
    if (logicalLength === 0) {
      return { spansByLine: emptyByLine, lineYs, complete: true, cacheable: true };
    }

    const logicalText = segments.map((segment) => segment.text).join("");
    if (logicalText.length > KeywordHighlighter.MAX_LOGICAL_LINE_SCAN_CHARS) {
      return { spansByLine: emptyByLine, lineYs, complete: true, cacheable: false };
    }
    const occupied = new Uint8Array(logicalText.length);

    const spansByLine = emptyByLine;
    const acceptedMatchesByLine = new Map<number, number>();

    for (const { regex, color } of this.compiledRules) {
      if (this.isBudgetExhausted(budget)) {
        return { spansByLine, lineYs, complete: false, cacheable: false };
      }
      regex.lastIndex = 0;

      while (true) {
        if (this.isBudgetExhausted(budget)) {
          return { spansByLine, lineYs, complete: false, cacheable: false };
        }
        const match = regex.exec(logicalText);
        if (match === null) break;

        if (match[0].length === 0) {
          regex.lastIndex++;
          continue;
        }

        const strStart = match.index;
        const strEnd = strStart + match[0].length;

        let isOverlapping = false;
        for (let k = strStart; k < strEnd; k++) {
          if (occupied[k]) {
            isOverlapping = true;
            break;
          }
        }
        if (isOverlapping) continue;
        if (this.hasWrappedAnsiForegroundInRange(segments, strStart, strEnd, scratchCell)) continue;

        const matchedSegments = segments.filter(
          (segment) => segment.endIndex > strStart && segment.startIndex < strEnd,
        );
        if (matchedSegments.length === 0) continue;

        let lineLimitReached = false;
        for (const segment of matchedSegments) {
          const acceptedCount = acceptedMatchesByLine.get(segment.lineY) ?? 0;
          if (acceptedCount >= config.maxMatchesPerLine) {
            lineLimitReached = true;
            break;
          }
        }
        if (lineLimitReached) continue;

        const acceptedLineYs: number[] = [];
        for (const segment of matchedSegments) {
          const localStart = Math.max(strStart, segment.startIndex) - segment.startIndex;
          const localEnd = Math.min(strEnd, segment.endIndex) - segment.startIndex;
          if (localEnd <= localStart) continue;

          const cellStartCol = segment.cellMap
            ? (segment.cellMap[localStart] ?? localStart)
            : localStart;
          const cellEndCol = segment.cellMap ? (segment.cellMap[localEnd] ?? localEnd) : localEnd;
          const cellWidth = cellEndCol - cellStartCol;
          if (cellWidth <= 0) continue;
          spansByLine.get(segment.lineY)?.push({
            cellStartCol,
            cellWidth,
            color,
          });
          acceptedLineYs.push(segment.lineY);
        }

        if (acceptedLineYs.length === 0) continue;

        for (let k = strStart; k < strEnd; k++) {
          occupied[k] = 1;
        }
        for (const lineY of acceptedLineYs) {
          acceptedMatchesByLine.set(lineY, (acceptedMatchesByLine.get(lineY) ?? 0) + 1);
        }
      }
    }

    return { spansByLine, lineYs, complete: true, cacheable: true };
  }

  private materializeSpans(
    lineY: number,
    spans: HighlightSpan[],
    cursorAbsoluteY: number,
    requiredKeys: Set<string>,
    config: KeywordHighlightPerformanceConfig,
    budget: RefreshBudget,
  ): void {
    for (const span of spans) {
      const key = this.ensureDecoration(
        lineY,
        span.cellStartCol,
        span.cellWidth,
        span.color,
        cursorAbsoluteY,
        config,
        budget,
      );
      if (key) requiredKeys.add(key);
      if (budget.hitLimit) return;
    }
  }

  private refreshViewport(reason: RefreshReason): void {
    if (!this.enabled || this.suspended || this.compiledRules.length === 0) return;
    if (!this.term?.buffer?.active) return;

    if (this.term.buffer.active.type === "alternate") {
      this.invalidateAll();
      return;
    }

    const refreshStartedAt = performance.now();
    const stats: RefreshStats = {
      cacheHits: 0,
      cacheMisses: 0,
      scannedLines: 0,
      decorationsCreated: 0,
      decorationsDisposed: 0,
    };

    // When xterm trims the scrollback, all buffer indices shift and our caches
    // become stale. Detect this via the sentinel marker and wipe everything.
    if (this.bufferTrimmed) {
      this.invalidateAll();
    }

    if (!this.sentinelMarker) {
      this.installSentinel();
    }

    const buffer = this.term.buffer.active;
    const viewportY = buffer.viewportY;
    const rows = this.term.rows;
    const cursorAbsoluteY = buffer.baseY + buffer.cursorY;
    const totalLines = buffer.length;
    // Lines below this index are in the scrollback and are immutable.
    // Lines on the current screen (>= screenStartY) may still change via escape sequences.
    const screenStartY = buffer.baseY;
    const config = getKeywordHighlightPerformanceConfig();
    const budget: RefreshBudget = {
      createdDecorations: 0,
      deadlineMs: performance.now() + config.maxRefreshTimeMs,
      hitLimit: false,
      hitTotalDecorationLimit: false,
    };

    // Expand the active zone with overscan so decorations survive typical scroll bursts
    // without being destroyed and recreated, eliminating highlight flicker.
    const scanStart = Math.max(0, viewportY - config.resolvedOverscanLines);
    const scanEnd = Math.min(totalLines - 1, viewportY + rows - 1 + config.resolvedOverscanLines);

    const requiredKeys = new Set<string>();
    const processedLines = new Set<number>();
    const scratchCell = buffer.getNullCell();
    const processedLogicalStarts = new Set<number>();

    for (let lineY = scanStart; lineY <= scanEnd; lineY++) {
      if (this.isBudgetExhausted(budget)) break;
      const line = buffer.getLine(lineY);
      if (!line) continue;
      processedLines.add(lineY);

      if (!this.highlightAcrossWrappedLines) {
        let spans: HighlightSpan[];
        if (lineY < screenStartY) {
          const cached = this.getCachedMatches(lineY);
          if (cached !== undefined) {
            stats.cacheHits++;
            spans = cached;
          } else {
            stats.cacheMisses++;
            stats.scannedLines++;
            const result = this.scanPhysicalLine(line, scratchCell, config, budget);
            spans = result.spans;
            if (result.complete) this.setCachedMatches(lineY, spans);
          }
        } else {
          stats.scannedLines++;
          spans = this.scanPhysicalLine(line, scratchCell, config, budget).spans;
        }
        this.materializeSpans(
          lineY,
          spans,
          cursorAbsoluteY,
          requiredKeys,
          config,
          budget,
        );
        continue;
      }

      const { startY, endY } = this.getLogicalLineBounds(buffer, lineY, totalLines);
      const canMemoize = endY < screenStartY;
      if (processedLogicalStarts.has(startY)) continue;
      processedLogicalStarts.add(startY);
      for (
        let processedY = Math.max(startY, scanStart);
        processedY <= Math.min(endY, scanEnd);
        processedY++
      ) {
        processedLines.add(processedY);
      }

      const logicalLineYs = Array.from({ length: endY - startY + 1 }, (_, index) => startY + index);
      const allRowsCached =
        canMemoize && logicalLineYs.every((cachedLineY) => this.lineMatchCache.has(cachedLineY));

      let spansByLine: Map<number, HighlightSpan[]>;
      if (allRowsCached) {
        spansByLine = new Map();
        for (const cachedLineY of logicalLineYs) {
          const cached = this.getCachedMatches(cachedLineY);
          if (cached !== undefined) {
            stats.cacheHits++;
            spansByLine.set(cachedLineY, cached);
          }
        }
      } else {
        if (canMemoize) stats.cacheMisses++;
        stats.scannedLines += logicalLineYs.length;
        const result = this.scanWrappedLogicalLine(
          buffer,
          startY,
          endY,
          scratchCell,
          config,
          budget,
        );
        spansByLine = result.spansByLine;
        if (
          canMemoize &&
          result.complete &&
          result.cacheable &&
          result.lineYs.length <= config.maxCachedMatchLines
        ) {
          for (const cachedLineY of result.lineYs) this.lineMatchCache.delete(cachedLineY);
          for (const cachedLineY of result.lineYs) {
            this.setCachedMatches(cachedLineY, result.spansByLine.get(cachedLineY) ?? []);
          }
        }
      }

      for (
        let visibleLineY = Math.max(startY, scanStart);
        visibleLineY <= Math.min(endY, scanEnd);
        visibleLineY++
      ) {
        this.materializeSpans(
          visibleLineY,
          spansByLine.get(visibleLineY) ?? [],
          cursorAbsoluteY,
          requiredKeys,
          config,
          budget,
        );
        if (budget.hitLimit) break;
      }
    }

    // Evict decorations that have drifted outside the overscan zone. If the refresh
    // hit a time/count budget, keep unprocessed in-zone lines to avoid flicker and churn.
    const staleKeys: string[] = [];
    for (const [key, entry] of this.decorationCache) {
      if (requiredKeys.has(key)) continue;
      const isOutsideScanZone = entry.lineY < scanStart || entry.lineY > scanEnd;
      if (isOutsideScanZone || !budget.hitLimit || processedLines.has(entry.lineY)) {
        staleKeys.push(key);
      }
    }
    for (const key of staleKeys) {
      const entry = this.decorationCache.get(key);
      if (entry) {
        this.decorationCache.delete(key); // remove before dispose to silence onDispose no-op
        entry.decoration.dispose();
        entry.marker.dispose();
        stats.decorationsDisposed++;
      }
    }

    stats.decorationsCreated = budget.createdDecorations;

    if (import.meta.env.DEV) {
      logger.debug({
        domain: "terminal.input",
        event: "terminal.keyword.refresh",
        message: "Refreshed terminal keyword highlights",
        ids: this.sessionId ? { session_id: this.sessionId } : undefined,
        data: {
          reason,
          duration_ms: performance.now() - refreshStartedAt,
          scanned_lines: stats.scannedLines,
          cache_hits: stats.cacheHits,
          cache_misses: stats.cacheMisses,
          decorations_created: stats.decorationsCreated,
          decorations_disposed: stats.decorationsDisposed,
          match_cache_size: this.lineMatchCache.size,
        },
      });
    }

    if (budget.hitLimit && !budget.hitTotalDecorationLimit) {
      this.triggerContinuationRefresh();
    }
  }
}
