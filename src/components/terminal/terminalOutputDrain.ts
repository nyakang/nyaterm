import { XTERM_PERFORMANCE_CONFIG } from "@/lib/xtermPerformance";
import {
  createOutputQueue,
  hasOutputQueueItems,
  type OutputQueue,
  peekOutputQueue,
  pushOutputQueue,
  type QueuedOutputChunk,
  replaceOutputQueueHead,
  shiftOutputQueue,
  splitOutputChunk,
} from "./xterminalOutputQueue";

export type TerminalOutputDrainMode = "foreground" | "background" | "hibernating" | "hibernated";

interface TerminalLike {
  write(data: string, callback?: () => void): void;
}

export interface TerminalOutputBackgroundDrainStats {
  queueBytes: number;
  writingBytes: number;
  unackedBytes: number;
  backgroundCatchUp: boolean;
  drainChunkBytes: number;
  nextDelayMs: number;
}

export interface TerminalOutputForegroundFrameFallbackStats {
  queueBytes: number;
  writingBytes: number;
  unackedBytes: number;
  pendingBytes: number;
  fallbackDelayMs: number;
}

interface TerminalOutputDrainTimers {
  requestAnimationFrame: (callback: FrameRequestCallback) => number;
  cancelAnimationFrame: (handle: number) => void;
  setTimeout: (callback: () => void, delay: number) => number;
  clearTimeout: (handle: number) => void;
  queueMicrotask: (callback: () => void) => void;
  now: () => number;
}

interface TerminalOutputDrainOptions<TWriteContext = unknown> {
  sessionId: string;
  getTerminal: () => TerminalLike | null;
  getWriteChunkBytes: () => number;
  getForegroundDelayMs?: () => number;
  shouldUseLowLatencyFlush?: () => boolean;
  onAck: (bytes: number) => void;
  onWriteStart?: (payload: QueuedOutputChunk) => TWriteContext;
  onWriteComplete?: (payload: QueuedOutputChunk, context: TWriteContext | undefined) => void;
  onWriteError?: (payload: QueuedOutputChunk, error: unknown) => void;
  onPressureChange?: (pendingBytes: number) => void;
  onModeChange?: (mode: TerminalOutputDrainMode) => void;
  onBackgroundDrain?: (stats: TerminalOutputBackgroundDrainStats) => void;
  onForegroundFrameFallback?: (stats: TerminalOutputForegroundFrameFallbackStats) => void;
  timers?: Partial<TerminalOutputDrainTimers>;
}

const DEFAULT_ACK_DEBOUNCE_MS = 40;
const DEFAULT_ACK_BATCH_BYTES = 64 * 1024;
const IDLE_POLL_MS = 8;

function defaultTimers(): TerminalOutputDrainTimers {
  return {
    requestAnimationFrame: (callback) => window.requestAnimationFrame(callback),
    cancelAnimationFrame: (handle) => window.cancelAnimationFrame(handle),
    setTimeout: (callback, delay) => window.setTimeout(callback, delay),
    clearTimeout: (handle) => window.clearTimeout(handle),
    queueMicrotask: (callback) => queueMicrotask(callback),
    now: () => Date.now(),
  };
}

function dequeueOutputChunk(queue: OutputQueue, maxBytes: number): QueuedOutputChunk | null {
  if (maxBytes <= 0 || !hasOutputQueueItems(queue)) {
    return null;
  }

  let remaining = maxBytes;
  const parts: string[] = [];
  let bytes = 0;

  while (remaining > 0 && hasOutputQueueItems(queue)) {
    const chunk = peekOutputQueue(queue);
    if (!chunk) break;

    if (chunk.bytes <= remaining) {
      parts.push(chunk.data);
      shiftOutputQueue(queue);
      remaining -= chunk.bytes;
      bytes += chunk.bytes;
      continue;
    }

    const [head, tail] = splitOutputChunk(chunk, remaining);
    parts.push(head.data);
    replaceOutputQueueHead(queue, tail);
    queue.bytes = Math.max(0, queue.bytes - head.bytes);
    bytes += head.bytes;
    remaining -= head.bytes;
  }

  return parts.length > 0 ? { data: parts.join(""), bytes } : null;
}

export class TerminalOutputDrain<TWriteContext = unknown> {
  private readonly timers: TerminalOutputDrainTimers;
  private queue = createOutputQueue();
  private mode: TerminalOutputDrainMode = "foreground";
  private writingBytes = 0;
  private backendUnackedBytes = 0;
  private pendingAckBytes = 0;
  private writeInFlight = false;
  private writeQueue = Promise.resolve();
  private foregroundFrame: number | null = null;
  private foregroundTimer: number | null = null;
  private backgroundTimer: number | null = null;
  private backgroundTimerCatchUp: boolean | null = null;
  private backgroundCatchUp = false;
  private ackTimer: number | null = null;
  private microtaskPending = false;
  private foregroundTurnStartedAt: number | null = null;
  private foregroundScheduleGeneration = 0;
  private disposed = false;

  constructor(private readonly options: TerminalOutputDrainOptions<TWriteContext>) {
    this.timers = { ...defaultTimers(), ...options.timers };
  }

  setMode(mode: TerminalOutputDrainMode) {
    if (this.disposed) return;
    if (this.mode === mode) {
      this.schedule();
      return;
    }

    this.mode = mode;
    this.options.onModeChange?.(mode);

    if (mode !== "foreground") {
      this.cancelForeground();
    }
    if (mode !== "background") {
      this.cancelBackground();
      this.backgroundCatchUp = false;
    }

    this.schedule();
  }

  enqueue(chunk: QueuedOutputChunk) {
    if (this.disposed || chunk.bytes <= 0 || !chunk.data) return;
    pushOutputQueue(this.queue, chunk);
    this.backendUnackedBytes += chunk.bytes;
    this.notifyPressure();
    this.schedule();
  }

  getPendingBytes() {
    return this.queue.bytes + this.writingBytes + this.pendingAckBytes;
  }

  getQueueBytes() {
    return this.queue.bytes;
  }

  hasQueuedOutput() {
    return hasOutputQueueItems(this.queue);
  }

  isWriteInFlight() {
    return this.writeInFlight;
  }

  waitForIdle(timeoutMs: number) {
    const deadline = this.timers.now() + timeoutMs;
    return new Promise<boolean>((resolve) => {
      let settled = false;
      const finish = (value: boolean) => {
        if (settled) return;
        settled = true;
        this.timers.clearTimeout(hardTimeout);
        resolve(value);
      };
      const hardTimeout = this.timers.setTimeout(
        () => finish(false),
        Math.max(0, timeoutMs),
      );
      const poll = () => {
        if (settled) return;
        if (this.disposed) {
          finish(false);
          return;
        }

        if (this.timers.now() >= deadline) {
          finish(false);
          return;
        }

        if (hasOutputQueueItems(this.queue)) {
          const payload = dequeueOutputChunk(this.queue, this.options.getWriteChunkBytes());
          if (payload) {
            void this.writePayload(payload).then(() => {
              if (settled) return;
              this.flushPendingAck(true);
              poll();
            });
            return;
          }
        }

        if (this.writeInFlight || this.writingBytes > 0) {
          this.timers.setTimeout(poll, IDLE_POLL_MS);
          return;
        }

        this.flushPendingAck(true);
        finish(this.queue.bytes === 0 && this.writingBytes === 0);
      };

      this.cancelForeground();
      this.cancelBackground();
      poll();
    });
  }

  writeExternal(write: () => Promise<void> | void) {
    this.writeQueue = this.writeQueue.catch(() => {}).then(() => write());
    return this.writeQueue;
  }

  dispose(options: { ackRemaining?: boolean } = {}) {
    if (this.disposed) return;
    this.disposed = true;
    this.cancelForeground();
    this.cancelBackground();
    this.clearAckTimer();
    this.microtaskPending = false;

    this.flushPendingAck(true);
    if (options.ackRemaining) {
      this.sendAck(this.backendUnackedBytes);
    }

    this.queue = createOutputQueue();
    this.writingBytes = 0;
    this.pendingAckBytes = 0;
    this.backendUnackedBytes = 0;
    this.writeInFlight = false;
    this.writeQueue = Promise.resolve();
    this.notifyPressure();
  }

  private schedule() {
    if (this.disposed) return;
    if (!hasOutputQueueItems(this.queue)) {
      this.foregroundTurnStartedAt = null;
      this.flushPendingAck(true);
      this.notifyPressure();
      return;
    }
    if (this.writeInFlight || this.mode === "hibernating" || this.mode === "hibernated") {
      this.notifyPressure();
      return;
    }

    if (this.mode === "background") {
      this.scheduleBackground();
    } else {
      this.scheduleForeground();
    }
  }

  private scheduleForeground() {
    if (
      this.foregroundFrame !== null ||
      this.foregroundTimer !== null ||
      this.microtaskPending ||
      this.disposed
    ) {
      return;
    }

    const delayMs = Math.max(0, this.options.getForegroundDelayMs?.() ?? 0);
    if (delayMs > 0) {
      this.foregroundTimer = this.timers.setTimeout(() => {
        this.foregroundTimer = null;
        this.foregroundTurnStartedAt = null;
        this.flushForeground();
      }, delayMs);
      return;
    }

    if (this.options.shouldUseLowLatencyFlush?.() && this.hasForegroundTurnBudgetRemaining()) {
      this.microtaskPending = true;
      this.timers.queueMicrotask(() => {
        this.microtaskPending = false;
        this.flushForeground();
      });
      return;
    }

    const generation = this.beginForegroundScheduleCycle();
    const fallbackDelayMs = XTERM_PERFORMANCE_CONFIG.output.foregroundFrameFallbackMs;
    this.foregroundFrame = this.timers.requestAnimationFrame(() => {
      if (!this.claimForegroundScheduleCycle(generation)) return;
      this.flushForeground();
    });
    this.foregroundTimer = this.timers.setTimeout(() => {
      if (!this.claimForegroundScheduleCycle(generation)) return;
      this.options.onForegroundFrameFallback?.({
        queueBytes: this.queue.bytes,
        writingBytes: this.writingBytes,
        unackedBytes: this.backendUnackedBytes,
        pendingBytes: this.getPendingBytes(),
        fallbackDelayMs,
      });
      this.flushForeground();
    }, fallbackDelayMs);
  }

  private scheduleBackground() {
    const schedule = this.resolveBackgroundSchedule();
    if (this.disposed) return;
    if (this.backgroundTimer !== null) {
      if (schedule.backgroundCatchUp && this.backgroundTimerCatchUp === false) {
        this.cancelBackground();
      } else {
        return;
      }
    }
    this.backgroundTimerCatchUp = schedule.backgroundCatchUp;
    this.backgroundTimer = this.timers.setTimeout(() => {
      this.backgroundTimer = null;
      this.backgroundTimerCatchUp = null;
      this.flushBackground();
    }, schedule.intervalMs);
  }

  private flushForeground() {
    if (this.mode !== "foreground" || this.disposed) {
      this.schedule();
      return;
    }
    this.beginForegroundTurn();
    this.flushOne(this.options.getWriteChunkBytes());
  }

  private flushBackground() {
    if (this.mode !== "background" || this.disposed) {
      this.schedule();
      return;
    }
    const schedule = this.resolveBackgroundSchedule();
    this.options.onBackgroundDrain?.({
      queueBytes: this.queue.bytes,
      writingBytes: this.writingBytes,
      unackedBytes: this.backendUnackedBytes,
      backgroundCatchUp: schedule.backgroundCatchUp,
      drainChunkBytes: schedule.writeChunkBytes,
      nextDelayMs: schedule.intervalMs,
    });
    this.flushOne(schedule.writeChunkBytes);
  }

  private resolveBackgroundSchedule() {
    const { output } = XTERM_PERFORMANCE_CONFIG;
    if (this.backgroundCatchUp) {
      this.backgroundCatchUp = this.queue.bytes > output.lowLatencyFlushBacklogBytes;
    } else {
      this.backgroundCatchUp = this.queue.bytes >= output.strainedBacklogBytes;
    }

    return this.backgroundCatchUp
      ? {
          backgroundCatchUp: true,
          writeChunkBytes: output.backgroundCatchUpWriteChunkBytes,
          intervalMs: output.backgroundCatchUpIntervalMs,
        }
      : {
          backgroundCatchUp: false,
          writeChunkBytes: output.backgroundWriteChunkBytes,
          intervalMs: output.backgroundDrainIntervalMs,
        };
  }

  private flushOne(maxBytes: number) {
    if (this.writeInFlight || this.disposed) {
      this.schedule();
      return;
    }

    const payload = dequeueOutputChunk(this.queue, maxBytes);
    if (!payload) {
      this.flushPendingAck(true);
      this.notifyPressure();
      return;
    }

    void this.writePayload(payload).then(() => this.schedule());
  }

  private writePayload(payload: QueuedOutputChunk) {
    this.writingBytes += payload.bytes;
    this.writeInFlight = true;
    this.notifyPressure();

    this.writeQueue = this.writeQueue
      .catch(() => {})
      .then(
        () =>
          new Promise<void>((resolve) => {
            const terminal = this.options.getTerminal();
            if (!terminal || this.disposed) {
              this.writingBytes = Math.max(0, this.writingBytes - payload.bytes);
              this.writeInFlight = false;
              this.notifyPressure();
              resolve();
              return;
            }

            let context: TWriteContext | undefined;
            try {
              context = this.options.onWriteStart?.(payload);
              terminal.write(payload.data, () => {
                this.writingBytes = Math.max(0, this.writingBytes - payload.bytes);
                this.pendingAckBytes += payload.bytes;
                this.writeInFlight = false;
                this.options.onWriteComplete?.(payload, context);
                this.flushPendingAck(this.queue.bytes === 0);
                this.notifyPressure();
                resolve();
              });
            } catch (error) {
              this.writingBytes = Math.max(0, this.writingBytes - payload.bytes);
              this.writeInFlight = false;
              this.options.onWriteError?.(payload, error);
              this.notifyPressure();
              resolve();
            }
          }),
      );

    return this.writeQueue;
  }

  private scheduleAckFlush() {
    if (this.ackTimer !== null || this.disposed) return;
    this.ackTimer = this.timers.setTimeout(() => {
      this.ackTimer = null;
      this.flushPendingAck(true);
    }, DEFAULT_ACK_DEBOUNCE_MS);
  }

  private flushPendingAck(force = false) {
    const bytes = this.pendingAckBytes;
    if (bytes <= 0) {
      if (force) this.clearAckTimer();
      return;
    }

    if (!force && bytes < DEFAULT_ACK_BATCH_BYTES) {
      this.scheduleAckFlush();
      return;
    }

    this.clearAckTimer();
    this.pendingAckBytes = 0;
    this.sendAck(bytes);
    this.notifyPressure();
  }

  private sendAck(bytes: number) {
    if (bytes <= 0) return;
    const ackBytes = Math.min(bytes, this.backendUnackedBytes);
    if (ackBytes <= 0) return;
    this.backendUnackedBytes = Math.max(0, this.backendUnackedBytes - ackBytes);
    this.options.onAck(ackBytes);
  }

  private beginForegroundScheduleCycle() {
    this.foregroundScheduleGeneration += 1;
    return this.foregroundScheduleGeneration;
  }

  private claimForegroundScheduleCycle(generation: number) {
    if (generation !== this.foregroundScheduleGeneration || this.disposed) {
      return false;
    }
    this.foregroundScheduleGeneration += 1;
    this.clearForegroundSchedulers();
    this.foregroundTurnStartedAt = null;
    return true;
  }

  private clearForegroundSchedulers() {
    if (this.foregroundFrame !== null) {
      this.timers.cancelAnimationFrame(this.foregroundFrame);
      this.foregroundFrame = null;
    }
    if (this.foregroundTimer !== null) {
      this.timers.clearTimeout(this.foregroundTimer);
      this.foregroundTimer = null;
    }
  }

  private cancelForeground() {
    this.foregroundScheduleGeneration += 1;
    this.clearForegroundSchedulers();
    this.microtaskPending = false;
    this.foregroundTurnStartedAt = null;
  }

  private cancelBackground() {
    if (this.backgroundTimer !== null) {
      this.timers.clearTimeout(this.backgroundTimer);
      this.backgroundTimer = null;
    }
    this.backgroundTimerCatchUp = null;
  }

  private clearAckTimer() {
    if (this.ackTimer !== null) {
      this.timers.clearTimeout(this.ackTimer);
      this.ackTimer = null;
    }
  }

  private notifyPressure() {
    this.options.onPressureChange?.(this.getPendingBytes());
  }

  private beginForegroundTurn() {
    if (this.foregroundTurnStartedAt === null) {
      this.foregroundTurnStartedAt = this.timers.now();
    }
  }

  private hasForegroundTurnBudgetRemaining() {
    if (this.foregroundTurnStartedAt === null) return true;
    return (
      this.timers.now() - this.foregroundTurnStartedAt <
      XTERM_PERFORMANCE_CONFIG.output.maxForegroundDrainTurnMs
    );
  }
}
