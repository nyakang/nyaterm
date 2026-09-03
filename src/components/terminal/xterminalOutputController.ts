import type { Terminal } from "@xterm/xterm";
import { logger } from "@/lib/logger";
import { XTERM_PERFORMANCE_CONFIG } from "@/lib/xtermPerformance";
import {
  Dec2026FrameGate,
  resolveDec2026FrameGateMode,
} from "./dec2026FrameGate";
import { outputAckCoordinator } from "./outputAckCoordinator";
import {
  TerminalOutputDrain,
  type TerminalOutputDrainMode,
} from "./terminalOutputDrain";
import { TerminalOutputScheduler } from "./terminalOutputScheduling";
import type { HibernationPhase } from "./xterminalInternalTypes";
import type { PerformanceMode } from "./xterminalTypes";

interface MutableRef<T> {
  current: T;
}

interface CreateXTerminalOutputControllerParams {
  sessionId: string;
  terminalGeneration: number;
  terminal: Terminal;
  outputDrainRef: MutableRef<TerminalOutputDrain<{
    beforeLine: number;
    ts: number;
  }> | null>;
  frameGateRef: MutableRef<Dec2026FrameGate | null>;
  visibleRef: MutableRef<boolean>;
  hibernatedRef: MutableRef<boolean>;
  hibernationPhaseRef: MutableRef<HibernationPhase>;
  performanceModeRef: MutableRef<PerformanceMode>;
  alternateScreenTrackerRef: MutableRef<{
    isAlternateScreenActive: () => boolean;
  }>;
  isTerminalAlive: () => boolean;
  getCurrentAbsoluteLine: () => number;
  stampWrittenLines: (from: number, to: number, ts: number) => void;
  clearHibernateTimer: () => void;
  enterOverloadedMode: () => void;
  setOutputPressureMode: (mode: PerformanceMode) => void;
  exitOverloadedMode: (nextMode?: PerformanceMode) => void;
}

export function createXTerminalOutputController({
  sessionId,
  terminalGeneration,
  terminal,
  outputDrainRef,
  frameGateRef,
  visibleRef,
  hibernatedRef,
  hibernationPhaseRef,
  performanceModeRef,
  alternateScreenTrackerRef,
  isTerminalAlive,
  getCurrentAbsoluteLine,
  stampWrittenLines,
  clearHibernateTimer,
  enterOverloadedMode,
  setOutputPressureMode,
  exitOverloadedMode,
}: CreateXTerminalOutputControllerParams) {
  const outputAckLease = outputAckCoordinator.acquire(
    sessionId,
    terminalGeneration,
  );
  const isAlternateScreenActive = () =>
    terminal.buffer.active.type === "alternate" ||
    alternateScreenTrackerRef.current.isAlternateScreenActive();

  const outputScheduler = new TerminalOutputScheduler({
    getQueueBytes: () =>
      (outputDrainRef.current?.getQueueBytes() ?? 0) +
      (frameGateRef.current?.getHeldBytes() ?? 0),
    isAlternateScreenActive,
  });

  const getWriteChunkBytes = () => outputScheduler.getWriteChunkBytes();

  const getRecoveryThresholdBytes = () =>
    visibleRef.current
      ? XTERM_PERFORMANCE_CONFIG.output.visibleRecoveryThresholdBytes
      : XTERM_PERFORMANCE_CONFIG.output.hiddenRecoveryThresholdBytes;

  const getPendingOutputBytes = () =>
    (outputDrainRef.current?.getPendingBytes() ?? 0) +
    (frameGateRef.current?.getHeldBytes() ?? 0);

  const getNonOverloadedPressureMode = (): PerformanceMode =>
    getPendingOutputBytes() >=
    XTERM_PERFORMANCE_CONFIG.output.strainedBacklogBytes
      ? "strained"
      : "normal";

  const refreshOutputPressureMode = () => {
    if (performanceModeRef.current === "overloaded") return;
    setOutputPressureMode(getNonOverloadedPressureMode());
  };

  const noteSkippedOutput = (count: number) => {
    if (count <= 0) return;
    enterOverloadedMode();
  };

  const sendOutputAck = (bytes: number) => {
    outputAckLease.ack(bytes);
  };

  const maybeRecoverPerformanceMode = () => {
    if (!isTerminalAlive()) return;
    if (performanceModeRef.current !== "overloaded") return;
    if (getPendingOutputBytes() > getRecoveryThresholdBytes()) return;
    exitOverloadedMode(getNonOverloadedPressureMode());
  };

  const shouldUseLowLatencyFlush = () =>
    visibleRef.current &&
    !isAlternateScreenActive() &&
    performanceModeRef.current !== "overloaded" &&
    getPendingOutputBytes() <=
      XTERM_PERFORMANCE_CONFIG.output.lowLatencyFlushBacklogBytes;

  const resolveOutputDrainMode = (): TerminalOutputDrainMode => {
    if (hibernatedRef.current) return "hibernated";
    if (
      hibernationPhaseRef.current === "preparing" ||
      hibernationPhaseRef.current === "detached"
    ) {
      return "hibernating";
    }
    return visibleRef.current ? "foreground" : "background";
  };

  const getForegroundDelayMs = () => {
    return outputScheduler.getForegroundDelayMs();
  };

  const updateOutputDrainMode = () => {
    outputDrainRef.current?.setMode(resolveOutputDrainMode());
  };

  const outputDrain = new TerminalOutputDrain<{
    beforeLine: number;
    ts: number;
  }>({
    sessionId,
    getTerminal: () => (isTerminalAlive() ? terminal : null),
    getWriteChunkBytes,
    getForegroundDelayMs,
    shouldUseLowLatencyFlush,
    onAck: sendOutputAck,
    onWriteStart: () => {
      outputScheduler.noteWriteStart();
      return { beforeLine: getCurrentAbsoluteLine(), ts: Date.now() };
    },
    onWriteComplete: (_payload, context) => {
      if (!isTerminalAlive() || !context) return;
      stampWrittenLines(
        context.beforeLine,
        getCurrentAbsoluteLine(),
        context.ts,
      );
      maybeRecoverPerformanceMode();
      refreshOutputPressureMode();
    },
    onWriteError: (payload, error) => {
      logger.warn({
        domain: "terminal.input",
        event: "terminal.output.write_failed",
        message: "Failed to write terminal output to xterm",
        ids: { session_id: sessionId },
        data: { bytes: payload.bytes },
        error,
      });
      noteSkippedOutput(payload.bytes);
    },
    onPressureChange: () => {
      maybeRecoverPerformanceMode();
      refreshOutputPressureMode();
    },
    onModeChange: (mode) => {
      logger.debug({
        domain: "terminal.input",
        event: "terminal.output.mode_changed",
        message: "Terminal output drain mode changed",
        ids: { session_id: sessionId },
        data: {
          mode,
          visible: visibleRef.current,
          queue_bytes: outputDrainRef.current?.getQueueBytes() ?? 0,
          pending_bytes: outputDrainRef.current?.getPendingBytes() ?? 0,
          frame_gate: frameGateRef.current?.snapshot(),
          performance_mode: performanceModeRef.current,
        },
      });
    },
    onBackgroundDrain: (stats) => {
      logger.debug({
        domain: "terminal.input",
        event: "terminal.output.background_drain",
        message: "Background terminal output drain cycle",
        ids: { session_id: sessionId },
        data: {
          queue_bytes: stats.queueBytes,
          writing_bytes: stats.writingBytes,
          unacked_bytes: stats.unackedBytes,
          background_catch_up: stats.backgroundCatchUp,
          drain_chunk_bytes: stats.drainChunkBytes,
          next_delay_ms: stats.nextDelayMs,
          frame_gate: frameGateRef.current?.snapshot(),
          performance_mode: performanceModeRef.current,
          buffer_type: terminal.buffer.active.type,
        },
      });
    },
    onForegroundFrameFallback: (stats) => {
      logger.debug({
        domain: "terminal.input",
        event: "terminal.output.foreground_frame_fallback",
        message: "Foreground terminal output drain fallback fired",
        ids: { session_id: sessionId },
        data: {
          queue_bytes: stats.queueBytes,
          writing_bytes: stats.writingBytes,
          unacked_bytes: stats.unackedBytes,
          pending_bytes: stats.pendingBytes,
          fallback_delay_ms: stats.fallbackDelayMs,
          frame_gate: frameGateRef.current?.snapshot(),
          performance_mode: performanceModeRef.current,
          visible: visibleRef.current,
          buffer_type: terminal.buffer.active.type,
        },
      });
    },
  });
  outputDrainRef.current = outputDrain;
  const frameGateMode = resolveDec2026FrameGateMode();
  const frameGate = new Dec2026FrameGate({
    mode: frameGateMode,
    forward: (chunk) => outputDrain.enqueue(chunk),
    ackDropped: sendOutputAck,
    getPressureSnapshot: () => ({
      alternateScreen: isAlternateScreenActive(),
      outputDrainQueueBytes: outputDrain.getQueueBytes(),
      outputDrainPendingBytes: outputDrain.getPendingBytes(),
      frameGateHeldBytes: frameGateRef.current?.getHeldBytes() ?? 0,
      performanceMode: performanceModeRef.current,
    }),
    onPressureChange: () => {
      maybeRecoverPerformanceMode();
      refreshOutputPressureMode();
    },
    logDebug: (event, message, data) => {
      logger.debug({
        domain: "terminal.input",
        event,
        message,
        ids: { session_id: sessionId },
        data: {
          mode: frameGateMode,
          ...(data ?? {}),
        },
      });
    },
  });
  frameGateRef.current = frameGate;
  logger.debug({
    domain: "terminal.input",
    event: "terminal.dec2026_frame_gate.mode",
    message: "Initialized DEC 2026 frame gate",
    ids: { session_id: sessionId },
    data: { mode: frameGateMode },
  });
  updateOutputDrainMode();

  const flushFrameGateAndDrain = async (reason: string) => {
    clearHibernateTimer();
    frameGateRef.current?.flush(reason);
    const drained = await outputDrain.waitForIdle(
      XTERM_PERFORMANCE_CONFIG.output.hibernateDrainTimeoutMs,
    );
    maybeRecoverPerformanceMode();
    refreshOutputPressureMode();
    return drained;
  };

  const writeTerminalTextAfterOutputQueue = async (data: string) => {
    await flushFrameGateAndDrain("terminal_status_write");
    return outputDrain.writeExternal(
      () =>
        new Promise<void>((resolve) => {
          if (!isTerminalAlive()) {
            resolve();
            return;
          }

          try {
            const ts = Date.now();
            const beforeLine = getCurrentAbsoluteLine();
            terminal.write(data, () => {
              if (isTerminalAlive()) {
                stampWrittenLines(beforeLine, getCurrentAbsoluteLine(), ts);
              }
              resolve();
            });
          } catch {
            resolve();
          }
        }),
    );
  };

  const flushQueuedOutputBeforeStatusNotice = async () =>
    flushFrameGateAndDrain("status_notice");

  return {
    outputAckLease,
    outputScheduler,
    outputDrain,
    frameGate,
    isAlternateScreenActive,
    noteSkippedOutput,
    maybeRecoverPerformanceMode,
    refreshOutputPressureMode,
    updateOutputDrainMode,
    flushFrameGateAndDrain,
    writeTerminalTextAfterOutputQueue,
    flushQueuedOutputBeforeStatusNotice,
  };
}
