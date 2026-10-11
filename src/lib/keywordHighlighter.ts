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
  scannedChars: number;
}

interface WrappedSpanScanResult {
  spansByLine: Map<number, HighlightSpan[]>;
  lineYs: number[];
  complete: boolean;
  cacheable: boolean;
  suppressedByHardLimit: boolean;
}

type LogicalLineBoundsStatus = "complete" | "budget" | "row_limit";

interface LogicalLineBoundsResult {
  startY: number;
  endY: number;
  status: LogicalLineBoundsStatus;
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
 * - Alternate buffer: paints visible keyword glyphs on a transparent canvas, since
 *   xterm's marker/decoration API is only available in the normal buffer.
 */
export class KeywordHighlighter implements IDisposable {
  private term: XTerm;
  private compiledRules: CompiledRule[] = [];
  private decorationCache = new Map<string, CachedDecoration>();
  /** Immutable absolute buffer line index → resolved highlight spans (including []). */
  private lineMatchCache = new Map<number, HighlightSpan[]>();
  /** Immutable rows belonging to a logical line suppressed by a deterministic hard limit. */
  private suppressedLineCache = new Map<number, true>();
  private writeRefreshTimer: ReturnType<typeof setTimeout> | null = null;
  private scrollDebounceTimer: ReturnType<typeof setTimeout> | null = null;
  private resumeRefreshTimer: ReturnType<typeof setTimeout> | null = null;
  private resumeRefreshFrame: number | null = null;
  private continuationRefreshFrame: number | null = null;
  /** Rows skipped after exhausting one refresh budget, until the continuation chain ends. */
  private continuationSkippedRows = new Set<number>();
  private enabled = false;
  private suspended = false;
  private highlightAcrossWrappedLines = false;
  private disposables: IDisposable[] = [];
  private lastViewportY = -1;
  private activeBufferType: "normal" | "alternate";
  private alternateCanvas: HTMLCanvasElement | null = null;
  private alternateContext: CanvasRenderingContext2D | null = null;

  /** Marker anchored at buffer line 0 to detect scrollback trimming. */
  private sentinelMarker: IMarker | null = null;
  private sentinelDisposable: IDisposable | null = null;
  private bufferTrimmed = false;

  private static readonly MAX_LOGICAL_LINE_SCAN_CHARS = 16 * 1024;

  constructor(
    term: XTerm,
    private readonly sessionId?: string,
  ) {
    this.term = term;
    this.activeBufferType = term.buffer.active.type;

    this.disposables.push(
      this.term.buffer.onBufferChange(() => this.syncActiveBuffer()),
      this.term.onWriteParsed(() => {
        this.syncActiveBuffer();
        this.triggerWriteRefresh();
      }),
      this.term.onResize(() => {
        this.invalidateAll();
        this.lastViewportY = -1;
        this.triggerWriteRefresh("resize");
      }),
      this.term.onRender(() => {
        this.syncActiveBuffer();
        if (this.term.buffer.active.type === "alternate") {
          // TUI redraws can replace cells in place without changing viewportY.
          this.triggerWriteRefresh();
          return;
        }
        const currentViewportY = this.term.buffer.active?.viewportY ?? 0;
        if (currentViewportY !== this.lastViewportY) {
          this.lastViewportY = currentViewportY;
          this.triggerScrollRefresh();
        }
      }),
      this.term.onSelectionChange(() => {
        if (this.term.buffer.active.type !== "alternate" || !this.canRefresh()) return;
        // xterm redraws its selection without necessarily emitting onRender.
        // Clear our higher-layer glyphs immediately, then repaint unselected cells.
        const screen = this.term.dimensions?.css.canvas;
        if (screen) this.alternateContext?.clearRect(0, 0, screen.width, screen.height);
        this.triggerWriteRefresh();
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
      this.removeAlternateCanvas();
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
    if (this.writeRefreshTimer !== null) {
      clearTimeout(this.writeRefreshTimer);
      this.writeRefreshTimer = null;
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

  /** A buffer switch invalidates normal scrollback markers even if viewportY is unchanged. */
  private syncActiveBuffer(): void {
    const type = this.term.buffer.active.type;
    if (type === this.activeBufferType) return;
    this.activeBufferType = type;
    this.invalidateAll();
    this.lastViewportY = -1;
    this.triggerWriteRefresh();
  }

  private removeAlternateCanvas(): void {
    this.alternateCanvas?.remove();
    this.alternateCanvas = null;
    this.alternateContext = null;
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
    return this.enabled && !this.suspended && this.compiledRules.length > 0;
  }

  /** Delayed refresh that batches writes without postponing work during continuous output. */
  private triggerWriteRefresh(reason: "write" | "resize" = "write"): void {
    if (!this.canRefresh()) return;
    if (
      this.scrollDebounceTimer !== null ||
      this.resumeRefreshTimer !== null ||
      this.resumeRefreshFrame !== null
    ) {
      return;
    }
    if (this.writeRefreshTimer !== null) return;
    this.writeRefreshTimer = setTimeout(() => {
      this.writeRefreshTimer = null;
      this.refreshViewport(reason);
    }, XTERM_PERFORMANCE_CONFIG.highlighting.writeRefreshIntervalMs);
  }

  /** Pure trailing debounce: scrolling always preempts pending highlight work. */
  private triggerScrollRefresh(): void {
    if (!this.canRefresh()) return;
    if (this.writeRefreshTimer !== null) {
      clearTimeout(this.writeRefreshTimer);
      this.writeRefreshTimer = null;
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
    this.suppressedLineCache.clear();
  }

  private invalidateAll(): void {
    this.clearAllTimers();
    this.removeAlternateCanvas();
    this.clearDecorations();
    this.clearMatchCache();
    this.continuationSkippedRows.clear();
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
    this.suppressedLineCache.delete(lineY);
    this.lineMatchCache.delete(lineY);
    this.lineMatchCache.set(lineY, spans);
    const maxLines = XTERM_PERFORMANCE_CONFIG.highlighting.maxCachedMatchLines;
    while (this.lineMatchCache.size > maxLines) {
      const oldest = this.lineMatchCache.keys().next().value;
      if (oldest === undefined) break;
      this.lineMatchCache.delete(oldest);
    }
  }

  private hasCachedSuppression(lineY: number): boolean {
    if (!this.suppressedLineCache.has(lineY)) return false;
    this.suppressedLineCache.delete(lineY);
    this.suppressedLineCache.set(lineY, true);
    return true;
  }

  private setCachedSuppression(lineY: number): void {
    this.lineMatchCache.delete(lineY);
    this.suppressedLineCache.delete(lineY);
    this.suppressedLineCache.set(lineY, true);
    const maxLines = XTERM_PERFORMANCE_CONFIG.highlighting.maxCachedMatchLines;
    while (this.suppressedLineCache.size > maxLines) {
      const oldest = this.suppressedLineCache.keys().next().value;
      if (oldest === undefined) break;
      this.suppressedLineCache.delete(oldest);
    }
  }

  private cacheSuppressedRows(lineYs: Iterable<number>): void {
    for (const lineY of lineYs) this.setCachedSuppression(lineY);
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
    maxRows: number,
    budget: RefreshBudget,
  ): LogicalLineBoundsResult {
    if (lineY + 1 < totalLines && this.suppressedLineCache.has(lineY + 1)) {
      if (this.isBudgetExhausted(budget)) return { startY: lineY, endY: lineY, status: "budget" };
      const nextLine = buffer.getLine(lineY + 1);
      if (this.isBudgetExhausted(budget)) return { startY: lineY, endY: lineY, status: "budget" };
      if (nextLine?.isWrapped) return { startY: lineY, endY: lineY, status: "row_limit" };
    }

    let startY = lineY;
    let visitedRows = 1;
    while (startY > 0) {
      if (this.isBudgetExhausted(budget)) return { startY, endY: lineY, status: "budget" };
      const currentLine = buffer.getLine(startY);
      if (this.isBudgetExhausted(budget)) return { startY, endY: lineY, status: "budget" };
      if (!currentLine?.isWrapped) break;
      if (this.suppressedLineCache.has(startY - 1)) {
        return { startY, endY: lineY, status: "row_limit" };
      }
      if (visitedRows >= maxRows) return { startY, endY: lineY, status: "row_limit" };
      startY--;
      visitedRows++;
    }

    let endY = lineY;
    while (endY + 1 < totalLines) {
      if (this.isBudgetExhausted(budget)) return { startY, endY, status: "budget" };
      const nextLine = buffer.getLine(endY + 1);
      if (this.isBudgetExhausted(budget)) return { startY, endY, status: "budget" };
      if (!nextLine?.isWrapped) break;
      if (this.suppressedLineCache.has(endY + 1)) {
        return { startY, endY, status: "row_limit" };
      }
      if (visitedRows >= maxRows) return { startY, endY, status: "row_limit" };
      endY++;
      visitedRows++;
    }

    return { startY, endY, status: "complete" };
  }

  private getLogicalLineBoundsInRange(
    buffer: XTerm["buffer"]["active"],
    lineY: number,
    minY: number,
    maxY: number,
  ): { startY: number; endY: number } {
    let startY = lineY;
    while (startY > minY && buffer.getLine(startY)?.isWrapped) startY--;

    let endY = lineY;
    while (endY < maxY && buffer.getLine(endY + 1)?.isWrapped) endY++;

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
    if (!lineText) return { spans: [], complete: true, scannedChars: 0 };

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
      if (this.isBudgetExhausted(budget)) {
        return { spans, complete: false, scannedChars: lineText.length };
      }
      regex.lastIndex = 0;

      while (true) {
        if (spans.length >= config.maxMatchesPerLine) break;
        if (this.isBudgetExhausted(budget)) {
          return { spans, complete: false, scannedChars: lineText.length };
        }
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

    return { spans, complete: true, scannedChars: lineText.length };
  }

  private scanPhysicalLogicalLine(
    buffer: XTerm["buffer"]["active"],
    startY: number,
    endY: number,
    scratchCell: IBufferCell,
    config: KeywordHighlightPerformanceConfig,
    budget: RefreshBudget,
  ): WrappedSpanScanResult {
    const lineYs = Array.from({ length: endY - startY + 1 }, (_, index) => startY + index);
    const emptyByLine = () =>
      new Map(lineYs.map((lineY) => [lineY, [] as HighlightSpan[]] as const));
    const spansByLine = emptyByLine();
    let logicalLength = 0;
    let decorationCandidates = 0;

    for (const lineY of lineYs) {
      if (this.isBudgetExhausted(budget)) {
        return {
          spansByLine,
          lineYs,
          complete: false,
          cacheable: false,
          suppressedByHardLimit: false,
        };
      }
      const line = buffer.getLine(lineY);
      if (!line) continue;

      const result = this.scanPhysicalLine(line, scratchCell, config, budget);
      if (!result.complete) {
        return {
          spansByLine,
          lineYs,
          complete: false,
          cacheable: false,
          suppressedByHardLimit: false,
        };
      }

      logicalLength += result.scannedChars;
      decorationCandidates += result.spans.length;
      if (
        logicalLength > KeywordHighlighter.MAX_LOGICAL_LINE_SCAN_CHARS ||
        decorationCandidates > config.maxDecorationsPerLogicalLine
      ) {
        return {
          spansByLine: emptyByLine(),
          lineYs,
          complete: true,
          cacheable: false,
          suppressedByHardLimit: true,
        };
      }
      spansByLine.set(lineY, result.spans);
    }

    return {
      spansByLine,
      lineYs,
      complete: true,
      cacheable: true,
      suppressedByHardLimit: false,
    };
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
        return {
          spansByLine: new Map(),
          lineYs: [],
          complete: false,
          cacheable: false,
          suppressedByHardLimit: false,
        };
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
      return {
        spansByLine: emptyByLine,
        lineYs,
        complete: true,
        cacheable: true,
        suppressedByHardLimit: false,
      };
    }

    const logicalText = segments.map((segment) => segment.text).join("");
    if (logicalText.length > KeywordHighlighter.MAX_LOGICAL_LINE_SCAN_CHARS) {
      return {
        spansByLine: emptyByLine,
        lineYs,
        complete: true,
        cacheable: false,
        suppressedByHardLimit: true,
      };
    }
    const occupied = new Uint8Array(logicalText.length);

    const spansByLine = emptyByLine;
    const acceptedMatchesByLine = new Map<number, number>();
    let decorationCandidates = 0;

    for (const { regex, color } of this.compiledRules) {
      if (this.isBudgetExhausted(budget)) {
        return {
          spansByLine,
          lineYs,
          complete: false,
          cacheable: false,
          suppressedByHardLimit: false,
        };
      }
      regex.lastIndex = 0;

      while (true) {
        if (this.isBudgetExhausted(budget)) {
          return {
            spansByLine,
            lineYs,
            complete: false,
            cacheable: false,
            suppressedByHardLimit: false,
          };
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

        const candidateSpans: Array<{ lineY: number; span: HighlightSpan }> = [];
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
          candidateSpans.push({
            lineY: segment.lineY,
            span: { cellStartCol, cellWidth, color },
          });
        }

        if (candidateSpans.length === 0) continue;
        if (decorationCandidates + candidateSpans.length > config.maxDecorationsPerLogicalLine) {
          return {
            spansByLine: new Map(lineYs.map((lineY) => [lineY, [] as HighlightSpan[]])),
            lineYs,
            complete: true,
            cacheable: false,
            suppressedByHardLimit: true,
          };
        }

        for (const { lineY, span } of candidateSpans) {
          spansByLine.get(lineY)?.push(span);
        }
        decorationCandidates += candidateSpans.length;

        for (let k = strStart; k < strEnd; k++) {
          occupied[k] = 1;
        }
        for (const { lineY } of candidateSpans) {
          acceptedMatchesByLine.set(lineY, (acceptedMatchesByLine.get(lineY) ?? 0) + 1);
        }
      }
    }

    return {
      spansByLine,
      lineYs,
      complete: true,
      cacheable: true,
      suppressedByHardLimit: false,
    };
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

  /** Create one pointer-transparent overlay, without using xterm's normal-buffer markers. */
  private ensureAlternateCanvas(): CanvasRenderingContext2D | null {
    const screen = this.term.element?.querySelector<HTMLElement>(".xterm-screen");
    if (!screen || !this.term.dimensions) return null;
    if (this.alternateCanvas?.parentElement === screen && this.alternateContext) {
      return this.alternateContext;
    }
    this.removeAlternateCanvas();
    const canvas = document.createElement("canvas");
    canvas.className = "nyaterm-keyword-alternate-overlay";
    canvas.setAttribute("aria-hidden", "true");
    canvas.style.pointerEvents = "none";
    canvas.style.position = "absolute";
    canvas.style.left = "0";
    canvas.style.top = "0";
    canvas.style.zIndex = "2";
    const context = canvas.getContext("2d");
    if (!context) return null;
    screen.appendChild(canvas);
    this.alternateCanvas = canvas;
    this.alternateContext = context;
    return context;
  }

  /** Repaint only the current alt viewport, without an immutable scrollback cache. */
  private refreshAlternateViewport(): void {
    const dimensions = this.term.dimensions;
    const context = this.ensureAlternateCanvas();
    if (!dimensions || !context || !this.alternateCanvas) return;
    const { cell, canvas: screen } = dimensions.css;
    if (!(cell.width > 0 && cell.height > 0 && screen.width > 0 && screen.height > 0)) {
      this.removeAlternateCanvas();
      return;
    }

    const canvas = this.alternateCanvas;
    const dpr = dimensions.device.canvas.width / screen.width || window.devicePixelRatio || 1;
    const width = Math.round(screen.width * dpr);
    const height = Math.round(screen.height * dpr);
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    canvas.style.width = `${screen.width}px`;
    canvas.style.height = `${screen.height}px`;
    context.setTransform(dpr, 0, 0, dpr, 0, 0);
    context.clearRect(0, 0, screen.width, screen.height);
    context.textBaseline = "alphabetic";
    context.font = `${this.term.options.fontWeight ?? "normal"} ${this.term.options.fontSize ?? 14}px ${this.term.options.fontFamily ?? "monospace"}`;

    const buffer = this.term.buffer.active;
    const startY = buffer.viewportY;
    const endY = Math.min(buffer.length - 1, startY + this.term.rows - 1);
    const config = getKeywordHighlightPerformanceConfig();
    const budget: RefreshBudget = {
      createdDecorations: 0,
      deadlineMs: performance.now() + config.maxRefreshTimeMs,
      hitLimit: false,
      hitTotalDecorationLimit: false,
    };
    const scratchCell = buffer.getNullCell();
    const processed = new Set<number>();
    const selection = this.term.getSelectionPosition?.();
    const charOffsetX = dimensions.device.char.left / dpr;
    const baselineY = (dimensions.device.char.top + dimensions.device.char.height) / dpr;

    for (let lineY = startY; lineY <= endY && !this.isBudgetExhausted(budget); lineY++) {
      if (processed.has(lineY)) continue;
      const bounds = this.getLogicalLineBounds(
        buffer,
        lineY,
        buffer.length,
        config.maxLogicalLineRows,
        budget,
      );
      if (bounds.status !== "complete") {
        if (bounds.status === "budget") break;
        continue;
      }
      const result = this.highlightAcrossWrappedLines
        ? this.scanWrappedLogicalLine(
            buffer,
            bounds.startY,
            bounds.endY,
            scratchCell,
            config,
            budget,
          )
        : this.scanPhysicalLogicalLine(
            buffer,
            bounds.startY,
            bounds.endY,
            scratchCell,
            config,
            budget,
          );
      if (!result.complete || this.isBudgetExhausted(budget)) break;

      for (let y = Math.max(startY, bounds.startY); y <= Math.min(endY, bounds.endY); y++) {
        processed.add(y);
        const line = buffer.getLine(y);
        if (!line || result.suppressedByHardLimit) continue;
        for (const span of result.spansByLine.get(y) ?? []) {
          if (budget.createdDecorations >= config.maxDecorationsPerRefresh) {
            budget.hitLimit = true;
            break;
          }
          budget.createdDecorations++;
          context.fillStyle = span.color;
          const row = y - startY;
          const right = Math.min(this.term.cols, span.cellStartCol + span.cellWidth);
          for (let col = span.cellStartCol; col < right; col++) {
            if (this.isBudgetExhausted(budget)) break;
            const glyph = line.getCell(col, scratchCell);
            if (!glyph || glyph.getWidth() <= 0 || !glyph.getChars() || !glyph.isFgDefault())
              continue;
            // Do not paint over Vim's inverse-video cells, cursor or xterm selection.
            if (glyph.isInverse?.() || glyph.isInvisible?.()) continue;
            if (y === buffer.baseY + buffer.cursorY && col === buffer.cursorX) continue;
            // xterm 6.1 returns zero-based buffer coordinates, with an exclusive end.
            if (
              selection &&
              (y > selection.start.y ||
                (y === selection.start.y && col >= selection.start.x)) &&
              (y < selection.end.y || (y === selection.end.y && col < selection.end.x))
            ) {
              continue;
            }
            const x = col * cell.width;
            const top = row * cell.height;
            context.save();
            context.beginPath();
            context.rect(x, top, glyph.getWidth() * cell.width, cell.height);
            context.clip();
            context.fillText(
              glyph.getChars(),
              x + charOffsetX,
              top + baselineY,
              glyph.getWidth() * cell.width,
            );
            context.restore();
          }
        }
        if (budget.hitLimit) break;
      }
      lineY = bounds.endY;
    }
  }

  private refreshViewport(reason: RefreshReason): void {
    if (!this.enabled || this.suspended || this.compiledRules.length === 0) return;
    if (!this.term?.buffer?.active) return;

    if (reason !== "continuation") {
      this.continuationSkippedRows.clear();
    }

    if (this.term.buffer.active.type === "alternate") {
      this.refreshAlternateViewport();
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
      if (this.continuationSkippedRows.has(lineY)) {
        while (lineY <= scanEnd && this.continuationSkippedRows.has(lineY)) {
          processedLines.add(lineY);
          lineY++;
        }
        lineY--;
        continue;
      }
      if (lineY < screenStartY && this.hasCachedSuppression(lineY)) {
        do {
          stats.cacheHits++;
          processedLines.add(lineY);
          lineY++;
        } while (lineY <= scanEnd && lineY < screenStartY && this.hasCachedSuppression(lineY));
        lineY--;
        continue;
      }

      if (this.isBudgetExhausted(budget)) break;
      const line = buffer.getLine(lineY);
      if (!line) continue;

      const bounds = this.getLogicalLineBounds(
        buffer,
        lineY,
        totalLines,
        config.maxLogicalLineRows,
        budget,
      );
      if (bounds.status !== "complete") {
        const localBounds = this.getLogicalLineBoundsInRange(buffer, lineY, scanStart, scanEnd);
        const localLineYs = Array.from(
          { length: localBounds.endY - localBounds.startY + 1 },
          (_, index) => localBounds.startY + index,
        );
        for (const localLineY of localLineYs) processedLines.add(localLineY);

        if (bounds.status === "budget") {
          for (const localLineY of localLineYs) this.continuationSkippedRows.add(localLineY);
        } else {
          this.cacheSuppressedRows(localLineYs.filter((localLineY) => localLineY < screenStartY));
        }

        lineY = localBounds.endY;
        continue;
      }

      const { startY, endY } = bounds;
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
        const result = this.highlightAcrossWrappedLines
          ? this.scanWrappedLogicalLine(buffer, startY, endY, scratchCell, config, budget)
          : this.scanPhysicalLogicalLine(buffer, startY, endY, scratchCell, config, budget);
        spansByLine = result.spansByLine;
        const scanInterruptedByBudget =
          (!result.complete && budget.hitLimit) ||
          (result.complete && this.isBudgetExpired(budget));
        if (scanInterruptedByBudget) {
          for (
            let skippedLineY = Math.max(startY, scanStart);
            skippedLineY <= Math.min(endY, scanEnd);
            skippedLineY++
          ) {
            this.continuationSkippedRows.add(skippedLineY);
          }
          spansByLine = new Map(
            logicalLineYs.map((skippedLineY) => [skippedLineY, [] as HighlightSpan[]]),
          );
        }
        if (canMemoize && !scanInterruptedByBudget && result.suppressedByHardLimit) {
          this.cacheSuppressedRows(result.lineYs);
        } else if (
          canMemoize &&
          !scanInterruptedByBudget &&
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
      lineY = endY;
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
          suppressed_line_cache_size: this.suppressedLineCache.size,
        },
      });
    }

    if (budget.hitLimit && !budget.hitTotalDecorationLimit) {
      this.triggerContinuationRefresh();
    }
  }
}
