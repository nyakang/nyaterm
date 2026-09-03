import { invoke } from "./invoke";

const SESSION_INPUT_PREVIEW_EVENT = "nyaterm:session-input-preview";
const SESSION_COMMAND_HISTORY_EVENT = "nyaterm:session-command-history";

const sessionCommandHistory = new Map<string, string[]>();

export type SessionInputPreview =
  | { kind: "data"; data: string }
  | { kind: "replace"; value: string }
  | { kind: "replace-and-execute"; value: string }
  | { kind: "reset" };

interface SessionInputPreviewDetail {
  sessionId: string;
  preview: SessionInputPreview;
}

interface SessionCommandHistoryDetail {
  sessionId: string;
  commands: string[];
}

export interface SendSessionInputOptions {
  preview?: SessionInputPreview | null;
  registerSubmission?: string | null;
  origin?: InputOrigin;
  sensitivity?: InputSensitivity;
}

export type InputOrigin =
  | "keyboard"
  | "quick_command"
  | "startup_command"
  | "post_login"
  | "ai_agent"
  | "credential_autofill"
  | "otp_autofill"
  | "sync_input"
  | "terminal_response";

export type InputSensitivity = "normal" | "secret";

function inferPreview(data: string): SessionInputPreview {
  return { kind: "data", data };
}

export function normalizeTerminalCommandInput(command: string): string {
  return command.replace(/\r\n|\r|\n/gu, "\r");
}

export function buildTerminalCommandInput(command: string, execute: boolean = true): string {
  const input = normalizeTerminalCommandInput(command);
  return execute ? `${input}\r` : input;
}

export function emitSessionInputPreview(sessionId: string, preview: SessionInputPreview): void {
  if (typeof window === "undefined") {
    return;
  }

  window.dispatchEvent(
    new CustomEvent<SessionInputPreviewDetail>(SESSION_INPUT_PREVIEW_EVENT, {
      detail: { sessionId, preview },
    }),
  );
}

export function listenSessionInputPreview(
  sessionId: string,
  handler: (preview: SessionInputPreview) => void,
): () => void {
  if (typeof window === "undefined") {
    return () => {};
  }

  const listener = (event: Event) => {
    const customEvent = event as CustomEvent<SessionInputPreviewDetail>;
    if (customEvent.detail?.sessionId !== sessionId) {
      return;
    }
    handler(customEvent.detail.preview);
  };

  window.addEventListener(SESSION_INPUT_PREVIEW_EVENT, listener);
  return () => {
    window.removeEventListener(SESSION_INPUT_PREVIEW_EVENT, listener);
  };
}

function emitSessionCommandHistory(sessionId: string): void {
  if (typeof window === "undefined") {
    return;
  }

  window.dispatchEvent(
    new CustomEvent<SessionCommandHistoryDetail>(SESSION_COMMAND_HISTORY_EVENT, {
      detail: {
        sessionId,
        commands: [...(sessionCommandHistory.get(sessionId) ?? [])],
      },
    }),
  );
}

export function getSessionCommandHistory(sessionId: string): string[] {
  return [...(sessionCommandHistory.get(sessionId) ?? [])];
}

export function listenSessionCommandHistory(
  sessionId: string,
  handler: (commands: string[]) => void,
): () => void {
  if (typeof window === "undefined") {
    return () => {};
  }

  const listener = (event: Event) => {
    const customEvent = event as CustomEvent<SessionCommandHistoryDetail>;
    if (customEvent.detail?.sessionId !== sessionId) {
      return;
    }
    handler(customEvent.detail.commands);
  };

  window.addEventListener(SESSION_COMMAND_HISTORY_EVENT, listener);
  return () => {
    window.removeEventListener(SESSION_COMMAND_HISTORY_EVENT, listener);
  };
}

export function registerSessionCommandSubmission(sessionId: string, command: string): void {
  const normalizedCommand = command.trim();
  if (!normalizedCommand) {
    return;
  }

  const current = sessionCommandHistory.get(sessionId) ?? [];
  sessionCommandHistory.set(sessionId, [normalizedCommand, ...current]);
  emitSessionCommandHistory(sessionId);
}

export function clearSessionCommandHistory(sessionId: string): void {
  if (!sessionCommandHistory.has(sessionId)) {
    return;
  }

  sessionCommandHistory.delete(sessionId);
  emitSessionCommandHistory(sessionId);
}

export async function sendSessionInput(
  sessionId: string,
  data: string,
  options: SendSessionInputOptions = {},
): Promise<void> {
  const preview = options.preview === undefined ? inferPreview(data) : options.preview;
  if (preview) {
    emitSessionInputPreview(sessionId, preview);
  }

  if (options.registerSubmission) {
    registerSessionCommandSubmission(sessionId, options.registerSubmission);
    await invoke("register_command_submission", {
      sessionId,
      command: options.registerSubmission,
    });
  }

  await invoke("write_to_session", {
    sessionId,
    data,
    origin: options.origin,
    sensitivity: options.sensitivity,
  });
}

/**
 * Send input to a session and broadcast to all sync-group peers.
 * Peers do not emit frontend preview/history UI, but an executable command
 * candidate is registered with the backend before write so session-bound shell
 * confirmation remains exact-match and synchronized SSH history is not lost.
 */
export async function sendSessionInputWithSync(
  sessionId: string,
  data: string,
  peerSessionIds: string[],
  options: SendSessionInputOptions = {},
): Promise<void> {
  await sendSessionInput(sessionId, data, options);

  if (peerSessionIds.length > 0) {
    await Promise.allSettled(
      peerSessionIds.map(async (sid) => {
        if (options.registerSubmission) {
          await invoke("register_command_confirmation_candidate", {
            sessionId: sid,
            command: options.registerSubmission,
          }).catch(() => {
            // History confirmation is auxiliary; never block synchronized
            // input to an otherwise live peer when registration is unavailable.
          });
        }
        await invoke("write_to_session", {
          sessionId: sid,
          data,
          origin: "sync_input",
          sensitivity: options.sensitivity,
        });
        }),
    );
  }
}
