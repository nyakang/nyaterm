import { logger } from "@/lib/logger";

export interface TerminalFitDimensions {
  cols: number;
  rows: number;
}

interface TerminalLike {
  cols: number;
  rows: number;
  element?: HTMLElement;
  clearTextureAtlas(): void;
  focus(): void;
  refresh(start: number, end: number): void;
}

interface FitAddonLike {
  fit(): void;
  proposeDimensions?(): TerminalFitDimensions | undefined;
}

export type TerminalFitReason =
  | "initial"
  | "observer"
  | "ready"
  | "padding"
  | "active"
  | "global-refresh"
  | "appearance"
  | "window-resized"
  | "window-moved"
  | "window-focus"
  | "scale-factor"
  | "visible"
  | "braille-font"
  | "oscillation-settle";

export interface TerminalFitResult {
  reason: TerminalFitReason;
  before: TerminalFitDimensions;
  after: TerminalFitDimensions;
  proposed?: TerminalFitDimensions;
  observedWidth?: number;
  observedHeight?: number;
  devicePixelRatio: number;
  applied: boolean;
  skippedReason?: string;
}

interface TerminalFitRequest {
  reason: TerminalFitReason;
  force?: boolean;
  refresh?: boolean;
  clearTextureAtlas?: boolean;
  focus?: boolean;
  observedWidth?: number;
  observedHeight?: number;
  onComplete?: (result: TerminalFitResult) => void;
}

interface TerminalFitSchedulerOptions {
  sessionId: string;
  getTerminal: () => TerminalLike | null;
  getFitAddon: () => FitAddonLike | null;
  getContainer: () => HTMLElement | null;
  isVisible: () => boolean;
  canFocus?: () => boolean;
  onAfterFit?: (result: TerminalFitResult) => void;
  requestAnimationFrame?: (callback: FrameRequestCallback) => number;
  cancelAnimationFrame?: (handle: number) => void;
  setTimeout?: (callback: () => void, delay: number) => number;
  clearTimeout?: (handle: number) => void;
  now?: () => number;
}

const OBSERVED_SIZE_EPSILON_PX = 0.1;
const OSCILLATION_SUPPRESS_MS = 160;
const OSCILLATION_WARN_INTERVAL_MS = 1_000;

function dimensionsEqual(
  left: TerminalFitDimensions | undefined,
  right: TerminalFitDimensions | undefined,
) {
  return !!left && !!right && left.cols === right.cols && left.rows === right.rows;
}

function dimensionKey(dimensions: TerminalFitDimensions) {
  return `${dimensions.cols}x${dimensions.rows}`;
}

function isConnected(element: HTMLElement) {
  return element.isConnected !== false;
}

function getDevicePixelRatio() {
  return typeof window === "undefined" ? 1 : window.devicePixelRatio || 1;
}

export class TerminalResizeDeduper {
  private identity: string | null = null;
  private lastSent: TerminalFitDimensions | null = null;

  reset(sessionId: string, generation: number) {
    this.identity = `${sessionId}:${generation}`;
    this.lastSent = null;
  }

  shouldSend(sessionId: string, generation: number, cols: number, rows: number) {
    const identity = `${sessionId}:${generation}`;
    if (this.identity !== identity) {
      this.reset(sessionId, generation);
    }

    if (cols <= 0 || rows <= 0) {
      return false;
    }

    if (this.lastSent?.cols === cols && this.lastSent.rows === rows) {
      return false;
    }

    this.lastSent = { cols, rows };
    return true;
  }
}

export class TerminalFitScheduler {
  private pendingFrame: number | null = null;
  private pendingRequest: TerminalFitRequest | null = null;
  private deferredVisibleRequest: TerminalFitRequest | null = null;
  private lastObservedWidth: number | null = null;
  private lastObservedHeight: number | null = null;
  private recentFitKeys: string[] = [];
  private oscillationSuppressedUntil = 0;
  private oscillationSettleTimer: number | null = null;
  private lastOscillationWarnAt = 0;
  private disposed = false;

  private readonly requestFrame: (callback: FrameRequestCallback) => number;
  private readonly cancelFrame: (handle: number) => void;
  private readonly setTimer: (callback: () => void, delay: number) => number;
  private readonly clearTimer: (handle: number) => void;
  private readonly getNow: () => number;

  constructor(private readonly options: TerminalFitSchedulerOptions) {
    this.requestFrame =
      options.requestAnimationFrame ??
      ((callback) => window.requestAnimationFrame(callback));
    this.cancelFrame =
      options.cancelAnimationFrame ??
      ((handle) => window.cancelAnimationFrame(handle));
    this.setTimer =
      options.setTimeout ??
      ((callback, delay) => window.setTimeout(callback, delay));
    this.clearTimer =
      options.clearTimeout ??
      ((handle) => window.clearTimeout(handle));
    this.getNow = options.now ?? Date.now;
  }

  schedule(request: TerminalFitRequest) {
    if (this.disposed) return;

    const canceledPending = this.pendingFrame !== null;
    if (this.pendingFrame !== null) {
      this.cancelFrame(this.pendingFrame);
      this.pendingFrame = null;
    }

    this.pendingRequest = this.mergeRequests(this.pendingRequest, request);
    logger.debug({
      domain: "terminal.resize",
      event: "terminal.resize.fit_scheduled",
      message: "Scheduled terminal fit",
      ids: { session_id: this.options.sessionId },
      data: {
        reason: request.reason,
        canceled_pending: canceledPending,
        observed_width: request.observedWidth,
        observed_height: request.observedHeight,
        device_pixel_ratio: getDevicePixelRatio(),
      },
    });

    this.pendingFrame = this.requestFrame(() => {
      this.pendingFrame = null;
      const pending = this.pendingRequest;
      this.pendingRequest = null;
      if (pending) {
        this.run(pending);
      }
    });
  }

  observeResize(width: number, height: number) {
    if (width <= 0 || height <= 0) {
      this.logSkipped("observer", "zero_size", { observedWidth: width, observedHeight: height });
      return;
    }

    if (!this.options.isVisible()) {
      this.logSkipped("observer", "hidden", { observedWidth: width, observedHeight: height });
      return;
    }

    const widthChanged =
      this.lastObservedWidth === null ||
      Math.abs(width - this.lastObservedWidth) >= OBSERVED_SIZE_EPSILON_PX;
    const heightChanged =
      this.lastObservedHeight === null ||
      Math.abs(height - this.lastObservedHeight) >= OBSERVED_SIZE_EPSILON_PX;

    if (!widthChanged && !heightChanged) {
      this.logSkipped("observer", "same_observed_size", {
        observedWidth: width,
        observedHeight: height,
      });
      return;
    }

    this.lastObservedWidth = width;
    this.lastObservedHeight = height;
    this.schedule({ reason: "observer", observedWidth: width, observedHeight: height });
  }

  notifyVisible() {
    if (!this.options.isVisible()) return;
    const deferred = this.deferredVisibleRequest;
    this.deferredVisibleRequest = null;
    this.schedule(
      deferred
        ? { ...deferred, reason: "visible", force: true }
        : { reason: "visible", force: true, refresh: true },
    );
  }

  dispose() {
    this.disposed = true;
    if (this.pendingFrame !== null) {
      this.cancelFrame(this.pendingFrame);
      this.pendingFrame = null;
    }
    if (this.oscillationSettleTimer !== null) {
      this.clearTimer(this.oscillationSettleTimer);
      this.oscillationSettleTimer = null;
    }
    this.pendingRequest = null;
    this.deferredVisibleRequest = null;
  }

  private mergeRequests(
    current: TerminalFitRequest | null,
    next: TerminalFitRequest,
  ): TerminalFitRequest {
    if (!current) return next;
    return {
      reason: next.reason,
      force: current.force || next.force,
      refresh: current.refresh || next.refresh,
      clearTextureAtlas: current.clearTextureAtlas || next.clearTextureAtlas,
      focus: current.focus || next.focus,
      observedWidth: next.observedWidth ?? current.observedWidth,
      observedHeight: next.observedHeight ?? current.observedHeight,
      onComplete: (result) => {
        current.onComplete?.(result);
        next.onComplete?.(result);
      },
    };
  }

  private run(request: TerminalFitRequest) {
    if (this.disposed) return;

    const terminal = this.options.getTerminal();
    const fitAddon = this.options.getFitAddon();
    const container = this.options.getContainer();
    const before = { cols: terminal?.cols ?? 0, rows: terminal?.rows ?? 0 };

    const complete = (result: Omit<TerminalFitResult, "reason" | "before" | "devicePixelRatio">) => {
      const finalResult: TerminalFitResult = {
        reason: request.reason,
        before,
        devicePixelRatio: getDevicePixelRatio(),
        ...result,
      };
      request.onComplete?.(finalResult);
      return finalResult;
    };

    if (!terminal || !fitAddon || !container) {
      const result = complete({ after: before, applied: false, skippedReason: "missing_terminal" });
      this.logSkipped(request.reason, "missing_terminal", request);
      return result;
    }

    if (!isConnected(container)) {
      const result = complete({ after: before, applied: false, skippedReason: "disconnected" });
      this.logSkipped(request.reason, "disconnected", request);
      return result;
    }

    if (!this.options.isVisible()) {
      if (request.force) {
        this.deferredVisibleRequest = request;
      }
      const result = complete({ after: before, applied: false, skippedReason: "hidden" });
      this.logSkipped(request.reason, "hidden", request);
      return result;
    }

    const rect = container.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) {
      const result = complete({
        after: before,
        applied: false,
        skippedReason: "zero_size",
        observedWidth: rect.width,
        observedHeight: rect.height,
      });
      this.logSkipped(request.reason, "zero_size", {
        ...request,
        observedWidth: rect.width,
        observedHeight: rect.height,
      });
      return result;
    }

    const proposed = fitAddon.proposeDimensions?.();
    if (!request.force && dimensionsEqual(proposed, before)) {
      const result = complete({
        after: before,
        proposed,
        applied: false,
        skippedReason: "same_geometry",
        observedWidth: request.observedWidth ?? rect.width,
        observedHeight: request.observedHeight ?? rect.height,
      });
      this.logSkipped(request.reason, "same_geometry", request, proposed);
      return result;
    }

    if (!request.force && proposed && this.shouldSuppressOscillation(proposed)) {
      this.scheduleOscillationSettle(request);
      const result = complete({
        after: before,
        proposed,
        applied: false,
        skippedReason: "oscillation",
        observedWidth: request.observedWidth ?? rect.width,
        observedHeight: request.observedHeight ?? rect.height,
      });
      return result;
    }

    fitAddon.fit();

    const after = { cols: terminal.cols, rows: terminal.rows };
    if (request.clearTextureAtlas) {
      terminal.clearTextureAtlas();
    }
    if (request.refresh) {
      terminal.refresh(0, Math.max(0, terminal.rows - 1));
    }
    if (request.focus && (this.options.canFocus?.() ?? true)) {
      terminal.focus();
    }

    const result = complete({
      after,
      proposed,
      applied: true,
      observedWidth: request.observedWidth ?? rect.width,
      observedHeight: request.observedHeight ?? rect.height,
    });

    this.recordFitResult(after);
    this.options.onAfterFit?.(result);
    logger.debug({
      domain: "terminal.resize",
      event: "terminal.resize.fit_applied",
      message: "Applied terminal fit",
      ids: { session_id: this.options.sessionId },
      data: {
        reason: request.reason,
        before_cols: before.cols,
        before_rows: before.rows,
        after_cols: after.cols,
        after_rows: after.rows,
        proposed_cols: proposed?.cols,
        proposed_rows: proposed?.rows,
        observed_width: result.observedWidth,
        observed_height: result.observedHeight,
        device_pixel_ratio: result.devicePixelRatio,
        cleared_texture_atlas: !!request.clearTextureAtlas,
      },
    });

    return result;
  }

  private shouldSuppressOscillation(proposed: TerminalFitDimensions) {
    const now = this.getNow();
    if (now < this.oscillationSuppressedUntil) {
      return true;
    }

    const keys = this.recentFitKeys;
    if (keys.length < 3) {
      return false;
    }

    const next = dimensionKey(proposed);
    const [first, second, third] = keys.slice(-3);
    if (first === third && first !== second && next === second) {
      this.oscillationSuppressedUntil = now + OSCILLATION_SUPPRESS_MS;
      if (now - this.lastOscillationWarnAt >= OSCILLATION_WARN_INTERVAL_MS) {
        this.lastOscillationWarnAt = now;
        logger.warn({
          domain: "terminal.resize",
          event: "terminal.resize.oscillation_detected",
          message: "Detected terminal resize oscillation",
          ids: { session_id: this.options.sessionId },
          data: {
            pattern: [first, second, third, next],
            suppress_ms: OSCILLATION_SUPPRESS_MS,
            device_pixel_ratio: getDevicePixelRatio(),
          },
        });
      }
      return true;
    }

    return false;
  }

  private scheduleOscillationSettle(request: TerminalFitRequest) {
    if (this.oscillationSettleTimer !== null) {
      this.clearTimer(this.oscillationSettleTimer);
    }
    this.oscillationSettleTimer = this.setTimer(() => {
      this.oscillationSettleTimer = null;
      this.schedule({
        ...request,
        reason: "oscillation-settle",
        force: true,
      });
    }, OSCILLATION_SUPPRESS_MS);
  }

  private recordFitResult(dimensions: TerminalFitDimensions) {
    this.recentFitKeys.push(dimensionKey(dimensions));
    if (this.recentFitKeys.length > 6) {
      this.recentFitKeys = this.recentFitKeys.slice(-6);
    }
  }

  private logSkipped(
    reason: TerminalFitReason,
    skippedReason: string,
    request: Pick<TerminalFitRequest, "observedWidth" | "observedHeight">,
    proposed?: TerminalFitDimensions,
  ) {
    logger.debug({
      domain: "terminal.resize",
      event: "terminal.resize.fit_skipped",
      message: "Skipped terminal fit",
      ids: { session_id: this.options.sessionId },
      data: {
        reason,
        skipped_reason: skippedReason,
        proposed_cols: proposed?.cols,
        proposed_rows: proposed?.rows,
        observed_width: request.observedWidth,
        observed_height: request.observedHeight,
        device_pixel_ratio: getDevicePixelRatio(),
      },
    });
  }
}

export function createTerminalFitScheduler(options: TerminalFitSchedulerOptions) {
  return new TerminalFitScheduler(options);
}
