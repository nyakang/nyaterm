import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { logger } from "./logger";

export interface InvokeHealthSnapshot {
  inflight_total: number;
  inflight_by_command: Record<string, number>;
  oldest_inflight_ms: number;
}

interface InvokeHealthTrackerOptions {
  now?: () => number;
  setTimeout?: (callback: () => void, delay: number) => ReturnType<typeof setTimeout>;
  clearTimeout?: (handle: ReturnType<typeof setTimeout>) => void;
  onHealth?: (snapshot: InvokeHealthSnapshot) => void;
  staleAfterMs?: number;
  highInflightCount?: number;
  logIntervalMs?: number;
}

const INVOKE_STALE_AFTER_MS = 15_000;
const INVOKE_HIGH_INFLIGHT_COUNT = 32;
const INVOKE_HEALTH_LOG_INTERVAL_MS = 60_000;

export class InvokeHealthTracker {
  private readonly inflight = new Map<string, { command: string; startedAt: number }>();
  private readonly now: () => number;
  private readonly setTimer: NonNullable<InvokeHealthTrackerOptions["setTimeout"]>;
  private readonly clearTimer: NonNullable<InvokeHealthTrackerOptions["clearTimeout"]>;
  private readonly onHealth: (snapshot: InvokeHealthSnapshot) => void;
  private readonly staleAfterMs: number;
  private readonly highInflightCount: number;
  private readonly logIntervalMs: number;
  private healthTimer: ReturnType<typeof setTimeout> | null = null;
  private lastHealthLogAt: number | null = null;

  constructor(options: InvokeHealthTrackerOptions = {}) {
    this.now = options.now ?? (() => performance.now());
    this.setTimer = options.setTimeout ?? ((callback, delay) => setTimeout(callback, delay));
    this.clearTimer = options.clearTimeout ?? ((handle) => clearTimeout(handle));
    this.onHealth = options.onHealth ?? (() => {});
    this.staleAfterMs = options.staleAfterMs ?? INVOKE_STALE_AFTER_MS;
    this.highInflightCount = options.highInflightCount ?? INVOKE_HIGH_INFLIGHT_COUNT;
    this.logIntervalMs = options.logIntervalMs ?? INVOKE_HEALTH_LOG_INTERVAL_MS;
  }

  start(requestId: string, command: string): void {
    this.inflight.set(requestId, {
      command,
      startedAt: this.now(),
    });
    this.maybeEmitHealth();
    this.rescheduleHealthCheck();
  }

  finish(requestId: string): void {
    this.inflight.delete(requestId);
    this.rescheduleHealthCheck();
  }

  snapshot(now = this.now()): InvokeHealthSnapshot {
    const inflightByCommand: Record<string, number> = {};
    let oldestStartedAt: number | null = null;

    for (const request of this.inflight.values()) {
      inflightByCommand[request.command] = (inflightByCommand[request.command] ?? 0) + 1;
      oldestStartedAt =
        oldestStartedAt === null ? request.startedAt : Math.min(oldestStartedAt, request.startedAt);
    }

    return {
      inflight_total: this.inflight.size,
      inflight_by_command: inflightByCommand,
      oldest_inflight_ms:
        oldestStartedAt === null ? 0 : Math.max(0, Math.round(now - oldestStartedAt)),
    };
  }

  reset(): void {
    this.inflight.clear();
    this.lastHealthLogAt = null;
    this.clearHealthTimer();
  }

  private maybeEmitHealth(): void {
    const now = this.now();
    const snapshot = this.snapshot(now);
    if (
      snapshot.inflight_total < this.highInflightCount &&
      snapshot.oldest_inflight_ms < this.staleAfterMs
    ) {
      return;
    }
    if (this.lastHealthLogAt !== null && now - this.lastHealthLogAt < this.logIntervalMs) {
      return;
    }

    this.lastHealthLogAt = now;
    this.onHealth(snapshot);
  }

  private rescheduleHealthCheck(): void {
    this.clearHealthTimer();
    if (this.inflight.size === 0) return;

    const now = this.now();
    const snapshot = this.snapshot(now);
    const abnormal =
      snapshot.inflight_total >= this.highInflightCount ||
      snapshot.oldest_inflight_ms >= this.staleAfterMs;
    const delay = abnormal
      ? this.lastHealthLogAt === null
        ? 1
        : Math.max(1, this.logIntervalMs - (now - this.lastHealthLogAt))
      : Math.max(1, this.staleAfterMs - snapshot.oldest_inflight_ms);

    this.healthTimer = this.setTimer(() => {
      this.healthTimer = null;
      this.maybeEmitHealth();
      this.rescheduleHealthCheck();
    }, delay);
  }

  private clearHealthTimer(): void {
    if (this.healthTimer === null) return;
    this.clearTimer(this.healthTimer);
    this.healthTimer = null;
  }
}

export const invokeHealthTracker = new InvokeHealthTracker({
  onHealth: (snapshot) => {
    logger.warn({
      domain: "tauri.invoke",
      event: "invoke.health",
      message: "Tauri invoke requests remain outstanding",
      data: snapshot,
    });
  },
});

/**
 * Typed wrapper around Tauri's `invoke()` with built-in error logging.
 *
 * Usage:
 *   const result = await invoke<string>("create_ssh_session", { config });
 *   const sessions = await invoke<SessionInfo[]>("list_sessions");
 */
export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const requestId = logger.createRequestId();
  const startedAt = performance.now();
  const argsSummary = summarizeInvokeArgs(args);
  invokeHealthTracker.start(requestId, cmd);

  try {
    logger.debug({
      domain: "tauri.invoke",
      event: "command.start",
      message: `Command "${cmd}" started`,
      ids: { request_id: requestId },
      data: {
        command: cmd,
        args: argsSummary,
      },
    });
    const result = await tauriInvoke<T>(cmd, args);
    logger.debug({
      domain: "tauri.invoke",
      event: "command.success",
      message: `Command "${cmd}" succeeded`,
      ids: { request_id: requestId },
      data: {
        command: cmd,
        duration_ms: Math.round(performance.now() - startedAt),
      },
    });
    return result;
  } catch (error) {
    logger.error({
      domain: "tauri.invoke",
      event: "command.error",
      message: `Command "${cmd}" failed`,
      ids: { request_id: requestId },
      data: {
        command: cmd,
        duration_ms: Math.round(performance.now() - startedAt),
        args: argsSummary,
      },
      error,
    });
    throw error;
  } finally {
    invokeHealthTracker.finish(requestId);
  }
}

function summarizeInvokeArgs(args?: Record<string, unknown>): unknown {
  if (!args) return undefined;
  return Object.fromEntries(
    Object.entries(args).map(([key, value]) => [key, summarizeValue(value, 0)]),
  );
}

function summarizeValue(value: unknown, depth: number): unknown {
  if (value == null || typeof value === "boolean" || typeof value === "number") {
    return value;
  }

  if (typeof value === "string") {
    return { type: "string", length: value.length };
  }

  if (Array.isArray(value)) {
    return { type: "array", length: value.length };
  }

  if (typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>);
    if (depth >= 1) {
      return { type: "object", keys: entries.map(([key]) => key).slice(0, 20) };
    }
    return Object.fromEntries(entries.map(([key, item]) => [key, summarizeValue(item, depth + 1)]));
  }

  return { type: typeof value };
}
