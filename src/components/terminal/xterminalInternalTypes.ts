import type { AiCaptureEvent } from "@/types/global";
import type { ZmodemEventPayload } from "./zmodemTerminalEvents";

export interface XTermInternalTrimSource {
  _core?: {
    coreService?: {
      onUserInput?: (listener: () => void) => {
        dispose: () => void;
      };
    };
    _bufferService?: {
      buffers?: {
        normal?: {
          lines?: {
            onTrim?: (listener: (amount: number) => void) => {
              dispose: () => void;
            };
          };
        };
      };
    };
  };
}

export interface SessionCommandAcceptedEvent {
  sessionId: string;
  command: string;
}

export interface TerminalOutputPayload {
  data: string;
  bytes: number;
  droppedBytes?: number;
}

export type PendingWakeEvent =
  | { type: "error"; message: string }
  | { type: "closed" }
  | { type: "focus" }
  | { type: "zmodem"; payload: ZmodemEventPayload }
  | { type: "ai"; payload: AiCaptureEvent };

export type HibernationPhase =
  | "idle"
  | "preparing"
  | "detached"
  | "hibernated"
  | "waking"
  | "failed";

export type HibernationLogEvent =
  | "scheduled"
  | "start"
  | "detached"
  | "success"
  | "wake"
  | "rollback"
  | "drain_start"
  | "drain_complete"
  | "drain_timeout"
  | "fail"
  | "cancel";
