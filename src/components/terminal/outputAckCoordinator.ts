import { invoke } from "@/lib/invoke";
import { logger } from "@/lib/logger";

const DEFAULT_RETRY_BASE_MS = 250;
const DEFAULT_RETRY_MAX_MS = 5_000;
const DEFAULT_FAILURE_LOG_INTERVAL_MS = 30_000;

interface OutputAckCoordinatorTimers {
  setTimeout: (callback: () => void, delay: number) => ReturnType<typeof setTimeout>;
  clearTimeout: (handle: ReturnType<typeof setTimeout>) => void;
  now: () => number;
}

interface OutputAckCoordinatorOptions {
  invokeAck: (sessionId: string, bytes: number) => Promise<void>;
  onFailure?: (details: OutputAckFailureDetails) => void;
  timers?: Partial<OutputAckCoordinatorTimers>;
  retryBaseMs?: number;
  retryMaxMs?: number;
  failureLogIntervalMs?: number;
}

export interface OutputAckFailureDetails {
  sessionId: string;
  generation: number;
  bytes: number;
  pendingBytes: number;
  retryDelayMs: number;
  suppressedFailures: number;
  error: unknown;
}

export interface OutputAckCoordinatorSnapshot {
  leases: number;
  pendingBytes: number;
  inFlight: number;
  retryTimers: number;
}

export interface OutputAckLease {
  ack: (bytes: number) => void;
  dispose: () => void;
}

interface AckLeaseState {
  id: number;
  generation: number;
  accepting: boolean;
  disposed: boolean;
  pendingBytes: number;
  retryAttempt: number;
  finalAttemptAvailable: boolean;
}

interface AckBatch {
  leaseId: number;
  bytes: number;
}

interface RetryTimer {
  leaseId: number;
  handle: ReturnType<typeof setTimeout>;
}

interface SessionAckState {
  sessionId: string;
  activeLeaseId: number | null;
  leases: Map<number, AckLeaseState>;
  leaseOrder: number[];
  inFlight: AckBatch | null;
  retryTimer: RetryTimer | null;
  lastFailureLogAt: number | null;
  suppressedFailures: number;
}

function defaultTimers(): OutputAckCoordinatorTimers {
  return {
    setTimeout: (callback, delay) => setTimeout(callback, delay),
    clearTimeout: (handle) => clearTimeout(handle),
    now: () => Date.now(),
  };
}

export class OutputAckCoordinator {
  private readonly sessions = new Map<string, SessionAckState>();
  private readonly timers: OutputAckCoordinatorTimers;
  private readonly retryBaseMs: number;
  private readonly retryMaxMs: number;
  private readonly failureLogIntervalMs: number;
  private nextLeaseId = 1;

  constructor(private readonly options: OutputAckCoordinatorOptions) {
    this.timers = { ...defaultTimers(), ...options.timers };
    this.retryBaseMs = options.retryBaseMs ?? DEFAULT_RETRY_BASE_MS;
    this.retryMaxMs = options.retryMaxMs ?? DEFAULT_RETRY_MAX_MS;
    this.failureLogIntervalMs = options.failureLogIntervalMs ?? DEFAULT_FAILURE_LOG_INTERVAL_MS;
  }

  acquire(sessionId: string, generation: number): OutputAckLease {
    const state = this.getOrCreateSession(sessionId);
    const previousLeaseId = state.activeLeaseId;

    const leaseId = this.nextLeaseId;
    this.nextLeaseId += 1;
    state.activeLeaseId = leaseId;
    state.leases.set(leaseId, {
      id: leaseId,
      generation,
      accepting: true,
      disposed: false,
      pendingBytes: 0,
      retryAttempt: 0,
      finalAttemptAvailable: false,
    });
    state.leaseOrder.push(leaseId);
    if (previousLeaseId !== null) {
      this.closeLease(state, previousLeaseId);
    }
    this.pump(state);

    let leaseDisposed = false;
    return {
      ack: (bytes) => {
        if (leaseDisposed) return;
        this.addAck(state, leaseId, bytes);
      },
      dispose: () => {
        if (leaseDisposed) return;
        leaseDisposed = true;
        this.closeLease(state, leaseId);
      },
    };
  }

  snapshot(sessionId: string): OutputAckCoordinatorSnapshot {
    const state = this.sessions.get(sessionId);
    if (!state) {
      return { leases: 0, pendingBytes: 0, inFlight: 0, retryTimers: 0 };
    }
    return {
      leases: state.leases.size,
      pendingBytes: [...state.leases.values()].reduce(
        (total, lease) => total + lease.pendingBytes,
        0,
      ),
      inFlight: state.inFlight === null ? 0 : 1,
      retryTimers: state.retryTimer === null ? 0 : 1,
    };
  }

  private getOrCreateSession(sessionId: string): SessionAckState {
    const existing = this.sessions.get(sessionId);
    if (existing) return existing;
    const state: SessionAckState = {
      sessionId,
      activeLeaseId: null,
      leases: new Map(),
      leaseOrder: [],
      inFlight: null,
      retryTimer: null,
      lastFailureLogAt: null,
      suppressedFailures: 0,
    };
    this.sessions.set(sessionId, state);
    return state;
  }

  private addAck(state: SessionAckState, leaseId: number, bytes: number): void {
    if (!Number.isSafeInteger(bytes) || bytes <= 0) return;
    const lease = state.leases.get(leaseId);
    if (!lease?.accepting || state.activeLeaseId !== leaseId) return;
    lease.pendingBytes += bytes;
    this.pump(state);
  }

  private closeLease(state: SessionAckState, leaseId: number): void {
    const lease = state.leases.get(leaseId);
    if (!lease || lease.disposed) return;
    lease.accepting = false;
    lease.disposed = true;
    lease.finalAttemptAvailable = true;
    if (state.activeLeaseId === leaseId) {
      state.activeLeaseId = null;
    }
    if (state.retryTimer?.leaseId === leaseId) {
      this.timers.clearTimeout(state.retryTimer.handle);
      state.retryTimer = null;
    }
    this.pump(state);
  }

  private pump(state: SessionAckState): void {
    if (state.inFlight !== null || state.retryTimer !== null) return;
    this.pruneCompletedLeases(state);

    for (const leaseId of state.leaseOrder) {
      const lease = state.leases.get(leaseId);
      if (!lease || lease.pendingBytes <= 0) continue;
      if (lease.disposed && !lease.finalAttemptAvailable) {
        lease.pendingBytes = 0;
        continue;
      }

      const bytes = lease.pendingBytes;
      lease.pendingBytes = 0;
      if (lease.disposed) {
        lease.finalAttemptAvailable = false;
      }
      state.inFlight = { leaseId, bytes };
      void Promise.resolve()
        .then(() => this.options.invokeAck(state.sessionId, bytes))
        .then(
          () => this.completeBatch(state, leaseId),
          (error) => this.failBatch(state, leaseId, bytes, error),
        );
      return;
    }

    this.pruneCompletedLeases(state);
    this.deleteSessionIfIdle(state);
  }

  private completeBatch(state: SessionAckState, leaseId: number): void {
    if (state.inFlight?.leaseId !== leaseId) return;
    state.inFlight = null;
    const lease = state.leases.get(leaseId);
    if (lease) {
      lease.retryAttempt = 0;
    }
    this.pump(state);
  }

  private failBatch(state: SessionAckState, leaseId: number, bytes: number, error: unknown): void {
    if (state.inFlight?.leaseId !== leaseId) return;
    state.inFlight = null;
    const lease = state.leases.get(leaseId);
    if (!lease) {
      this.pump(state);
      return;
    }

    lease.pendingBytes += bytes;
    let retryDelayMs = 0;
    if (lease.disposed) {
      if (!lease.finalAttemptAvailable) {
        lease.pendingBytes = 0;
      }
    } else {
      lease.retryAttempt += 1;
      retryDelayMs = Math.min(
        this.retryMaxMs,
        this.retryBaseMs * 2 ** Math.min(lease.retryAttempt - 1, 30),
      );
    }
    this.logFailure(state, lease, bytes, retryDelayMs, error);

    if (!lease.disposed) {
      state.retryTimer = {
        leaseId,
        handle: this.timers.setTimeout(() => {
          if (state.retryTimer?.leaseId !== leaseId) return;
          state.retryTimer = null;
          this.pump(state);
        }, retryDelayMs),
      };
      return;
    }

    this.pump(state);
  }

  private logFailure(
    state: SessionAckState,
    lease: AckLeaseState,
    bytes: number,
    retryDelayMs: number,
    error: unknown,
  ): void {
    const now = this.timers.now();
    if (
      state.lastFailureLogAt !== null &&
      now - state.lastFailureLogAt < this.failureLogIntervalMs
    ) {
      state.suppressedFailures += 1;
      return;
    }

    const suppressedFailures = state.suppressedFailures;
    state.suppressedFailures = 0;
    state.lastFailureLogAt = now;
    this.options.onFailure?.({
      sessionId: state.sessionId,
      generation: lease.generation,
      bytes,
      pendingBytes: lease.pendingBytes,
      retryDelayMs,
      suppressedFailures,
      error,
    });
  }

  private pruneCompletedLeases(state: SessionAckState): void {
    const inFlightLeaseId = state.inFlight?.leaseId;
    const retryLeaseId = state.retryTimer?.leaseId;
    state.leaseOrder = state.leaseOrder.filter((leaseId) => {
      const lease = state.leases.get(leaseId);
      if (!lease) return false;
      const completed =
        lease.disposed &&
        lease.pendingBytes === 0 &&
        inFlightLeaseId !== leaseId &&
        retryLeaseId !== leaseId;
      if (completed) {
        state.leases.delete(leaseId);
        return false;
      }
      return true;
    });
  }

  private deleteSessionIfIdle(state: SessionAckState): void {
    if (
      state.activeLeaseId === null &&
      state.leases.size === 0 &&
      state.inFlight === null &&
      state.retryTimer === null &&
      this.sessions.get(state.sessionId) === state
    ) {
      this.sessions.delete(state.sessionId);
    }
  }
}

export const outputAckCoordinator = new OutputAckCoordinator({
  invokeAck: (sessionId, bytes) => invoke("ack_session_output", { sessionId, bytes }),
  onFailure: ({
    sessionId,
    generation,
    bytes,
    pendingBytes,
    retryDelayMs,
    suppressedFailures,
    error,
  }) => {
    logger.warn({
      domain: "terminal.input",
      event: "terminal.output.ack_failed",
      message: "Failed to acknowledge terminal output",
      ids: { session_id: sessionId },
      data: {
        generation,
        bytes,
        pending_bytes: pendingBytes,
        retry_delay_ms: retryDelayMs,
        suppressed_failures: suppressedFailures,
      },
      error,
    });
  },
});
