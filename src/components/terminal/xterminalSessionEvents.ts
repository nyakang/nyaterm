import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Terminal } from "@xterm/xterm";
import { toast } from "sonner";
import { emitAIErrorDetected } from "@/lib/aiEvents";
import {
  renderAiCommandEnd,
  renderAiCommandStart,
} from "@/lib/aiTerminalRenderer";
import { invoke } from "@/lib/invoke";
import { createTerminalInputState } from "@/lib/terminalInputTracker";
import type { AiCaptureEvent } from "@/types/global";
import type { Dec2026FrameGate } from "./dec2026FrameGate";
import { hasErrorKeyword } from "./terminalInputSelection";
import type {
  HibernationLogEvent,
  HibernationPhase,
  SessionCommandAcceptedEvent,
  TerminalOutputPayload,
} from "./xterminalInternalTypes";
import type { ZmodemEventPayload } from "./zmodemTerminalEvents";

interface MutableRef<T> {
  current: T;
}

interface ZmodemHandler {
  handle: (payload: ZmodemEventPayload) => void;
}

export async function replaySnapshotBeforeAttach(options: {
  initialReplayPromise: Promise<void>;
  replayPendingWakeEvents: () => void;
  attachSession: () => Promise<void>;
}) {
  await options.initialReplayPromise.catch(() => {});
  options.replayPendingWakeEvents();
  await options.attachSession();
}

interface CreateXTerminalSessionEventsParams {
  sessionId: string;
  terminal: Terminal;
  frameGate: Dec2026FrameGate;
  sessionIdRef: MutableRef<string>;
  visibleRef: MutableRef<boolean>;
  lastErrorNoticeAtRef: MutableRef<number>;
  aiCapturingRef: MutableRef<boolean>;
  zmodemActiveRef: MutableRef<boolean>;
  inputStateRef: MutableRef<ReturnType<typeof createTerminalInputState>>;
  alternateScreenTrackerRef: MutableRef<{
    ingest: (data: string) => void;
  }>;
  hibernationPhaseRef: MutableRef<HibernationPhase>;
  detachedHibernateEpochRef: MutableRef<number | null>;
  onConnectionErrorRef: MutableRef<
    ((sessionId: string, error: string) => void) | undefined
  >;
  tRef: MutableRef<(key: string) => string>;
  isTerminalAlive: () => boolean;
  requestWake: (reason: string) => void;
  enterDisconnectedState: (options: {
    title: string;
    message?: string;
    titleColor: "31" | "36";
    showReconnectPrompt: boolean;
  }) => void;
  enterDisconnectedStateIfAttachSessionMissing: (error: unknown) => boolean;
  noteSkippedOutput: (count: number) => void;
  noteOutputActivity: () => void;
  updateCredentialPromptInputMode: (payload: string) => void;
  feedCredentialOutput: (payload: string) => void;
  maybeRecoverPerformanceMode: () => void;
  refreshOutputPressureMode: () => void;
  noteShellCommand: (command: string) => void;
  clearCredentialPromptInputMode: () => void;
  dismissSuggestions: () => void;
  writeTerminalTextAfterOutputQueue: (data: string) => Promise<void>;
  initialReplayPromise: Promise<void>;
  updateOutputDrainMode: () => void;
  logHibernation: (
    event: HibernationLogEvent,
    message: string,
    data?: Record<string, unknown>,
    error?: unknown,
  ) => void;
  zmodemHandler: ZmodemHandler;
  replayPendingWakeEvents: () => void;
  settleOutputAfterAttach: () => Promise<boolean>;
  flushPendingDynamicTitle: () => void;
}

export function createXTerminalSessionEvents({
  sessionId,
  terminal,
  frameGate,
  sessionIdRef,
  visibleRef,
  lastErrorNoticeAtRef,
  aiCapturingRef,
  zmodemActiveRef,
  inputStateRef,
  alternateScreenTrackerRef,
  hibernationPhaseRef,
  detachedHibernateEpochRef,
  onConnectionErrorRef,
  tRef,
  isTerminalAlive,
  requestWake,
  enterDisconnectedState,
  enterDisconnectedStateIfAttachSessionMissing,
  noteSkippedOutput,
  noteOutputActivity,
  updateCredentialPromptInputMode,
  feedCredentialOutput,
  maybeRecoverPerformanceMode,
  refreshOutputPressureMode,
  noteShellCommand,
  clearCredentialPromptInputMode,
  dismissSuggestions,
  writeTerminalTextAfterOutputQueue,
  initialReplayPromise,
  updateOutputDrainMode,
  logHibernation,
  zmodemHandler,
  replayPendingWakeEvents,
  settleOutputAfterAttach,
  flushPendingDynamicTitle,
}: CreateXTerminalSessionEventsParams) {
  let disposed = false;
  let retryTimer: number | null = null;
  let resolveRetryWait: (() => void) | null = null;
  const unlisteners: UnlistenFn[] = [];

  const addUnlistener = (unlisten: UnlistenFn) => {
    if (disposed) {
      unlisten();
      return false;
    }
    unlisteners.push(unlisten);
    return true;
  };

  const disposeRegisteredListeners = () => {
    for (const unlisten of unlisteners.splice(0)) {
      unlisten();
    }
  };

  const setupOnce = async () => {
    const nextOutputUnlisten = await listen<TerminalOutputPayload>(
      `terminal-output-${sessionId}`,
      (event) => {
        if (!isTerminalAlive()) return;
        const payload = event.payload;
        if (!payload.data || payload.bytes <= 0) {
          noteSkippedOutput(payload.droppedBytes ?? 0);
          return;
        }
        noteOutputActivity();

        const recentPayload =
          payload.data.length > 4096
            ? payload.data.slice(-4096)
            : payload.data;
        alternateScreenTrackerRef.current.ingest(payload.data);
        updateCredentialPromptInputMode(recentPayload);
        feedCredentialOutput(recentPayload);
        if (visibleRef.current && hasErrorKeyword(recentPayload)) {
          const now = Date.now();
          if (now - lastErrorNoticeAtRef.current > 30_000) {
            lastErrorNoticeAtRef.current = now;
            emitAIErrorDetected({
              sessionId,
              output: recentPayload.slice(-4000),
            });
          }
        }

        noteSkippedOutput(payload.droppedBytes ?? 0);
        frameGate.enqueue({
          data: payload.data,
          bytes: payload.bytes,
        });

        if (!visibleRef.current) {
          maybeRecoverPerformanceMode();
          refreshOutputPressureMode();
          window.dispatchEvent(
            new CustomEvent("nyaterm:session-output", {
              detail: { sessionId },
            }),
          );
          return;
        }

        refreshOutputPressureMode();
      },
    );
    if (!addUnlistener(nextOutputUnlisten)) return;

    const nextCommandAcceptedUnlisten =
      await listen<SessionCommandAcceptedEvent>(
        "session-command-accepted",
        (event) => {
          if (!isTerminalAlive()) return;
          if (event.payload.sessionId !== sessionIdRef.current) return;
          noteShellCommand(event.payload.command);
        },
      );
    if (!addUnlistener(nextCommandAcceptedUnlisten)) return;

    const nextErrorUnlisten = await listen<string>(
      `session-error-${sessionId}`,
      (event) => {
        if (!isTerminalAlive()) return;
        requestWake("session_error");
        const message = String(
          event.payload || tRef.current("terminal.connectionFailed"),
        );
        enterDisconnectedState({
          title: tRef.current("terminal.connectionFailed"),
          message,
          titleColor: "31",
          showReconnectPrompt: false,
        });
        toast.error(message);
        onConnectionErrorRef.current?.(sessionIdRef.current, message);
      },
    );
    if (!addUnlistener(nextErrorUnlisten)) return;

    const nextClosedUnlisten = await listen<void>(
      `session-closed-${sessionId}`,
      () => {
        if (!isTerminalAlive()) return;
        requestWake("session_closed");
        enterDisconnectedState({
          title: tRef.current("terminal.sessionDisconnected"),
          titleColor: "31",
          showReconnectPrompt: true,
        });
      },
    );
    if (!addUnlistener(nextClosedUnlisten)) return;

    const nextFocusUnlisten = await listen<void>(
      `focus-terminal-${sessionId}`,
      () => {
        if (!isTerminalAlive()) return;
        requestWake("focus");
        terminal.focus();
      },
    );
    if (!addUnlistener(nextFocusUnlisten)) return;

    const nextCaptureUnlisten = await listen<AiCaptureEvent>(
      `ai-capture-${sessionId}`,
      (event) => {
        if (!isTerminalAlive()) return;
        const payload = event.payload;
        requestWake("ai");
        if (payload.type === "commandStart") {
          aiCapturingRef.current = true;
          inputStateRef.current = createTerminalInputState();
          clearCredentialPromptInputMode();
          dismissSuggestions();
          if (isTerminalAlive()) {
            void writeTerminalTextAfterOutputQueue(
              renderAiCommandStart(payload),
            );
          }
        } else if (payload.type === "commandEnd") {
          aiCapturingRef.current = false;
          if (isTerminalAlive()) {
            void writeTerminalTextAfterOutputQueue(renderAiCommandEnd(payload));
          }
        }
      },
    );
    if (!addUnlistener(nextCaptureUnlisten)) return;

    const nextZmodemUnlisten = await listen<ZmodemEventPayload>(
      `zmodem-event-${sessionId}`,
      (event) => {
        if (!isTerminalAlive()) return;
        requestWake("zmodem");
        if (
          event.payload.type === "detected" ||
          event.payload.type === "progress"
        ) {
          zmodemActiveRef.current = true;
        } else if (
          event.payload.type === "complete" ||
          event.payload.type === "failed"
        ) {
          zmodemActiveRef.current = false;
        }
        zmodemHandler.handle(event.payload);
      },
    );
    if (!addUnlistener(nextZmodemUnlisten)) return;

    let backendAttached = false;
    try {
      await replaySnapshotBeforeAttach({
        initialReplayPromise,
        replayPendingWakeEvents,
        attachSession: async () => {
          await invoke("attach_session", { sessionId });
          backendAttached = true;
        },
      });
      // Attachment has completed even if the renderer drain below times out.
      // Clear the detached epoch now so cleanup cannot issue a duplicate attach.
      detachedHibernateEpochRef.current = null;
      // The backend acknowledgement means its pending payload was emitted,
      // not that xterm has parsed it yet. Drain the renderer queue before
      // publishing the settled title/cwd snapshot.
      const replaySettled = await settleOutputAfterAttach();
      if (!replaySettled) {
        throw new Error("Timed out draining terminal output after attach");
      }
      if (
        hibernationPhaseRef.current === "waking" ||
        hibernationPhaseRef.current === "hibernated"
      ) {
        logHibernation("wake", "Attached backend output after terminal wake", {
          reason: "terminal_ready",
        });
      }
      hibernationPhaseRef.current = "idle";
      flushPendingDynamicTitle();
      updateOutputDrainMode();
    } catch (error) {
      hibernationPhaseRef.current = "failed";
      logHibernation(
        "fail",
        "Failed to attach backend output after terminal wake",
        { reason: "terminal_ready", backend_attached: backendAttached },
        error,
      );
      const sessionMissing = enterDisconnectedStateIfAttachSessionMissing(error);
      if (!backendAttached && !sessionMissing) {
        // The backend is still detached, so let setup retry without publishing
        // a title from an unsettled renderer.
        throw error;
      }
      if (sessionMissing) {
        detachedHibernateEpochRef.current = null;
      }
      // Once the backend is attached, a renderer-drain timeout must not leave
      // dynamic titles frozen for the rest of the mounted session.
      flushPendingDynamicTitle();
      updateOutputDrainMode();
    }
  };

  const setup = async () => {
    const maxAttempts = 3;
    let recoveryDelayMs = 5_000;
    for (let attempt = 1; !disposed; attempt += 1) {
      try {
        await setupOnce();
        return;
      } catch (error) {
        disposeRegisteredListeners();
        if (disposed) return;

        const burstAttempt = ((attempt - 1) % maxAttempts) + 1;
        const exhaustedBurst = burstAttempt === maxAttempts;
        if (exhaustedBurst) {
          hibernationPhaseRef.current = "failed";
          logHibernation(
            "fail",
            "Failed to register terminal session listeners; retrying slowly while backend output remains detached",
            {
              reason: "listener_setup",
              attempts: attempt,
              retry_delay_ms: recoveryDelayMs,
            },
            error,
          );
          enterDisconnectedStateIfAttachSessionMissing(error);
        }

        const delayMs = exhaustedBurst ? recoveryDelayMs : 100 * burstAttempt;
        if (exhaustedBurst) {
          recoveryDelayMs = Math.min(recoveryDelayMs * 2, 30_000);
        }
        await new Promise<void>((resolve) => {
          resolveRetryWait = resolve;
          retryTimer = window.setTimeout(() => {
            retryTimer = null;
            resolveRetryWait = null;
            resolve();
          }, delayMs);
        });
        if (disposed) return;
      }
    }
  };

  return {
    setup,
    dispose: () => {
      disposed = true;
      if (retryTimer !== null) {
        window.clearTimeout(retryTimer);
        retryTimer = null;
      }
      resolveRetryWait?.();
      resolveRetryWait = null;
      disposeRegisteredListeners();
    },
  };
}
