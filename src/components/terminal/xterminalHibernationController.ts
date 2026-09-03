import type { Terminal } from "@xterm/xterm";
import {
  pauseDynamicTitlePublication,
  resumeDynamicTitlePublication,
} from "@/lib/dynamicTabTitles";
import { invoke } from "@/lib/invoke";
import type { TerminalReconnectSnapshot } from "@/lib/terminalReconnectHistory";
import { XTERM_PERFORMANCE_CONFIG } from "@/lib/xtermPerformance";
import type { SessionType } from "@/types/global";
import type { TerminalOutputDrain } from "./terminalOutputDrain";
import type {
  HibernationLogEvent,
  HibernationPhase,
} from "./xterminalInternalTypes";

interface MutableRef<T> {
  current: T;
}

interface XTerminalHibernationTimers {
  now: () => number;
  setTimeout: (callback: () => void, delay: number) => number;
  clearTimeout: (handle: number) => void;
}

interface CreateXTerminalHibernationControllerParams {
  sessionId: string;
  terminal: Terminal;
  outputDrain: TerminalOutputDrain<{ beforeLine: number; ts: number }>;
  visibleRef: MutableRef<boolean>;
  sessionTypeRef: MutableRef<SessionType>;
  aiCapturingRef: MutableRef<boolean>;
  zmodemActiveRef: MutableRef<boolean>;
  syncPeerSessionIdsRef: MutableRef<readonly string[] | undefined>;
  outputDrainRef: MutableRef<TerminalOutputDrain<{
    beforeLine: number;
    ts: number;
  }> | null>;
  disconnectedRef: MutableRef<boolean>;
  reconnectingRef: MutableRef<boolean>;
  hibernateTimerRef: MutableRef<number | null>;
  hibernationEpochRef: MutableRef<number>;
  hibernationPendingRef: MutableRef<boolean>;
  hibernationPhaseRef: MutableRef<HibernationPhase>;
  detachedHibernateEpochRef: MutableRef<number | null>;
  hibernationSnapshotRef: MutableRef<TerminalReconnectSnapshot | null>;
  hibernationCleanupRef: MutableRef<boolean>;
  hibernatedRef: MutableRef<boolean>;
  lastOutputActivityAtRef: MutableRef<number>;
  showSearchBar: boolean;
  activeMode: "buffer" | "history";
  isTerminalAlive: () => boolean;
  logHibernation: (
    event: HibernationLogEvent,
    message: string,
    data?: Record<string, unknown>,
    error?: unknown,
  ) => void;
  clearHibernateTimer: () => void;
  enterDisconnectedStateIfAttachSessionMissing: (error: unknown) => boolean;
  updateOutputDrainMode: () => void;
  flushFrameGateAndDrain: (reason: string) => Promise<boolean>;
  captureReconnectSnapshot: () => TerminalReconnectSnapshot | null;
  beginSnapshotRestore: (snapshot: TerminalReconnectSnapshot | null) => void;
  setTerminalReady: (ready: boolean) => void;
  setHibernated: (hibernated: boolean) => void;
  setTerminalGeneration: (updater: (generation: number) => number) => void;
  maybeRecoverPerformanceMode: () => void;
  refreshOutputPressureMode: () => void;
  repaintVisibleTerminal: () => void;
  timers?: Partial<XTerminalHibernationTimers>;
}

function defaultTimers(): XTerminalHibernationTimers {
  return {
    now: () => Date.now(),
    setTimeout: (callback, delay) => window.setTimeout(callback, delay),
    clearTimeout: (handle) => window.clearTimeout(handle),
  };
}

export function createXTerminalHibernationController({
  sessionId,
  terminal,
  outputDrain,
  visibleRef,
  sessionTypeRef,
  aiCapturingRef,
  zmodemActiveRef,
  syncPeerSessionIdsRef,
  outputDrainRef,
  disconnectedRef,
  reconnectingRef,
  hibernateTimerRef,
  hibernationEpochRef,
  hibernationPendingRef,
  hibernationPhaseRef,
  detachedHibernateEpochRef,
  hibernationSnapshotRef,
  hibernationCleanupRef,
  hibernatedRef,
  lastOutputActivityAtRef,
  showSearchBar,
  activeMode,
  isTerminalAlive,
  logHibernation,
  clearHibernateTimer,
  enterDisconnectedStateIfAttachSessionMissing,
  updateOutputDrainMode,
  flushFrameGateAndDrain,
  captureReconnectSnapshot,
  beginSnapshotRestore,
  setTerminalReady,
  setHibernated,
  setTerminalGeneration,
  maybeRecoverPerformanceMode,
  refreshOutputPressureMode,
  repaintVisibleTerminal,
  timers: timerOverrides,
}: CreateXTerminalHibernationControllerParams) {
  const timers = { ...defaultTimers(), ...timerOverrides };

  const getOutputIdleMs = () =>
    Math.max(0, timers.now() - lastOutputActivityAtRef.current);

  const hasOutputIdleTimedOut = () =>
    getOutputIdleMs() >= XTERM_PERFORMANCE_CONFIG.lifecycle.deepHibernateDelayMs;

  const canHibernateRenderer = (options: { allowPending?: boolean } = {}) => {
    const phase = hibernationPhaseRef.current;
    if (
      !isTerminalAlive() ||
      visibleRef.current ||
      (!options.allowPending && hibernationPendingRef.current) ||
      (phase !== "idle" &&
        !(
          options.allowPending &&
          (phase === "preparing" || phase === "detached")
        ))
    ) {
      return false;
    }
    if (sessionTypeRef.current === "Local") return false;
    if (!["SSH", "Telnet", "Serial"].includes(sessionTypeRef.current))
      return false;
    if (terminal.buffer.active.type === "alternate") return false;
    if (showSearchBar || activeMode === "history") return false;
    if (aiCapturingRef.current || zmodemActiveRef.current) return false;
    if (syncPeerSessionIdsRef.current?.length) return false;
    if (outputDrainRef.current?.isWriteInFlight()) return false;
    if (disconnectedRef.current || reconnectingRef.current) return false;
    if (!hasOutputIdleTimedOut()) return false;
    return true;
  };

  const restoreDetachedRenderer = async (epoch: number, reason: string) => {
    if (detachedHibernateEpochRef.current !== epoch) return;
    try {
      await invoke("attach_session", { sessionId });
      detachedHibernateEpochRef.current = null;
      hibernationPhaseRef.current = "idle";
      resumeDynamicTitlePublication(sessionId);
      logHibernation("rollback", "Rolled back detached terminal renderer", {
        reason,
        epoch,
      });
    } catch (error) {
      if (enterDisconnectedStateIfAttachSessionMissing(error)) {
        resumeDynamicTitlePublication(sessionId);
        return;
      }
      hibernationPhaseRef.current = "failed";
      resumeDynamicTitlePublication(sessionId);
      logHibernation(
        "fail",
        "Failed to roll back detached terminal renderer",
        { reason, epoch },
        error,
      );
    }
  };

  const scheduleHibernate = () => {
    if (
      visibleRef.current ||
      hibernateTimerRef.current !== null ||
      hibernationPhaseRef.current !== "idle"
    ) {
      return;
    }
    const idleMs = getOutputIdleMs();
    const remainingMs =
      XTERM_PERFORMANCE_CONFIG.lifecycle.deepHibernateDelayMs - idleMs;
    const delayMs = Math.max(0, remainingMs);
    const epoch = hibernationEpochRef.current + 1;
    hibernationEpochRef.current = epoch;
    logHibernation("scheduled", "Scheduled terminal renderer hibernation", {
      epoch,
      idle_ms: idleMs,
      delay_ms: delayMs,
    });
    hibernateTimerRef.current = timers.setTimeout(() => {
      hibernateTimerRef.current = null;
      void hibernateRenderer(epoch);
    }, delayMs);
  };

  const cancelRecentOutputHibernate = (epoch: number) => {
    hibernationPhaseRef.current = "idle";
    logHibernation(
      "cancel",
      "Skipped terminal hibernation after recent output activity",
      {
        epoch,
        reason: "recent_output_activity",
        idle_ms: getOutputIdleMs(),
        required_idle_ms: XTERM_PERFORMANCE_CONFIG.lifecycle.deepHibernateDelayMs,
      },
    );
    scheduleHibernate();
  };

  const hibernateRenderer = async (epoch: number) => {
    clearHibernateTimer();
    if (epoch !== hibernationEpochRef.current) return;
    if (!hasOutputIdleTimedOut()) {
      cancelRecentOutputHibernate(epoch);
      return;
    }
    if (!canHibernateRenderer()) {
      hibernationPhaseRef.current = "idle";
      logHibernation(
        "cancel",
        "Skipped terminal hibernation after eligibility check",
        { epoch, reason: "eligibility_changed" },
      );
      return;
    }

    hibernationPhaseRef.current = "preparing";
    hibernationPendingRef.current = true;
    pauseDynamicTitlePublication(sessionId);
    logHibernation("start", "Starting terminal renderer hibernation", {
      epoch,
    });

    try {
      updateOutputDrainMode();
      logHibernation(
        "drain_start",
        "Draining terminal output before hibernation",
        { epoch },
      );
      const drainedBeforeDetach =
        await flushFrameGateAndDrain("hibernate_before_detach");
      if (epoch !== hibernationEpochRef.current) {
        hibernationPhaseRef.current = "idle";
        logHibernation(
          "cancel",
          "Cancelled terminal hibernation before backend detach",
          { epoch, reason: "epoch_changed" },
        );
        return;
      }
      if (!hasOutputIdleTimedOut()) {
        cancelRecentOutputHibernate(epoch);
        return;
      }
      if (!canHibernateRenderer({ allowPending: true })) {
        hibernationPhaseRef.current = "idle";
        logHibernation(
          "cancel",
          "Cancelled terminal hibernation before backend detach",
          { epoch, reason: "eligibility_changed" },
        );
        return;
      }
      if (!drainedBeforeDetach) {
        hibernationPhaseRef.current = "idle";
        resumeDynamicTitlePublication(sessionId);
        logHibernation(
          "drain_timeout",
          "Timed out draining terminal output before hibernation",
          {
            epoch,
            queue_bytes: outputDrain.getQueueBytes(),
            pending_bytes: outputDrain.getPendingBytes(),
          },
        );
        return;
      }
      logHibernation(
        "drain_complete",
        "Drained terminal output before backend detach",
        {
          epoch,
        },
      );

      await invoke("detach_session_renderer", { sessionId });
      detachedHibernateEpochRef.current = epoch;
      hibernationPhaseRef.current = "detached";
      updateOutputDrainMode();
      logHibernation(
        "detached",
        "Detached terminal renderer from backend output",
        { epoch },
      );

      if (
        epoch !== hibernationEpochRef.current ||
        !isTerminalAlive() ||
        !hasOutputIdleTimedOut() ||
        !canHibernateRenderer({ allowPending: true })
      ) {
        await restoreDetachedRenderer(
          epoch,
          hasOutputIdleTimedOut()
            ? "eligibility_changed"
            : "recent_output_activity",
        );
        return;
      }

      const drainedAfterDetach =
        await flushFrameGateAndDrain("hibernate_after_detach");
      if (
        epoch !== hibernationEpochRef.current ||
        !isTerminalAlive() ||
        !hasOutputIdleTimedOut() ||
        !canHibernateRenderer({ allowPending: true })
      ) {
        await restoreDetachedRenderer(
          epoch,
          hasOutputIdleTimedOut()
            ? "eligibility_changed"
            : "recent_output_activity",
        );
        return;
      }
      if (!drainedAfterDetach) {
        logHibernation(
          "drain_timeout",
          "Timed out draining terminal output after backend detach",
          {
            epoch,
            queue_bytes: outputDrain.getQueueBytes(),
            pending_bytes: outputDrain.getPendingBytes(),
          },
        );
        await restoreDetachedRenderer(epoch, "drain_timeout");
        return;
      }

      const hibernationSnapshot = captureReconnectSnapshot();
      hibernationSnapshotRef.current = hibernationSnapshot;
      beginSnapshotRestore(hibernationSnapshot);
      hibernationCleanupRef.current = true;
      hibernationPhaseRef.current = "hibernated";
      outputDrain.setMode("hibernated");
      logHibernation("success", "Terminal renderer hibernated", { epoch });
      hibernatedRef.current = true;
      setTerminalReady(false);
      setHibernated(true);
      setTerminalGeneration((generation) => generation + 1);
    } catch (error) {
      hibernationSnapshotRef.current = null;
      hibernationPhaseRef.current = "failed";
      logHibernation(
        "fail",
        "Failed to hibernate terminal renderer",
        { epoch },
        error,
      );
      await restoreDetachedRenderer(epoch, "error");
    } finally {
      hibernationPendingRef.current = false;
      if (
        hibernationPhaseRef.current === "preparing" ||
        (hibernationPhaseRef.current === "detached" &&
          detachedHibernateEpochRef.current !== epoch) ||
        (hibernationPhaseRef.current === "failed" &&
          detachedHibernateEpochRef.current === null)
      ) {
        hibernationPhaseRef.current = "idle";
      }
      if (hibernationPhaseRef.current === "idle") {
        resumeDynamicTitlePublication(sessionId);
      }
    }
  };

  const noteOutputActivity = () => {
    lastOutputActivityAtRef.current = timers.now();
    if (visibleRef.current) return;

    clearHibernateTimer();
    const phase = hibernationPhaseRef.current;
    if (phase === "idle") {
      scheduleHibernate();
      return;
    }

    if (phase !== "preparing" && phase !== "detached") return;

    hibernationEpochRef.current += 1;
    logHibernation(
      "cancel",
      "Cancelled terminal hibernation after output activity",
      {
        reason: "recent_output_activity",
        phase,
      },
    );

    if (phase === "preparing") {
      hibernationPhaseRef.current = "idle";
      hibernationPendingRef.current = false;
      resumeDynamicTitlePublication(sessionId);
      updateOutputDrainMode();
      scheduleHibernate();
      return;
    }

    const detachedEpoch = detachedHibernateEpochRef.current;
    if (detachedEpoch !== null) {
      void restoreDetachedRenderer(detachedEpoch, "recent_output_activity").then(
        () => {
          if (
            !visibleRef.current &&
            hibernationPhaseRef.current === "idle" &&
            isTerminalAlive()
          ) {
            scheduleHibernate();
          }
        },
      );
    }
  };

  const applyVisibilityPolicy = () => {
    if (!isTerminalAlive()) return;

    if (visibleRef.current) {
      clearHibernateTimer();
    }

    updateOutputDrainMode();
    maybeRecoverPerformanceMode();
    refreshOutputPressureMode();

    if (visibleRef.current) {
      repaintVisibleTerminal();
    } else {
      scheduleHibernate();
    }
  };

  return {
    restoreDetachedRenderer,
    hibernateRenderer,
    scheduleHibernate,
    noteOutputActivity,
    applyVisibilityPolicy,
  };
}
