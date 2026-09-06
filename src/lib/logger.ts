import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import type { DiagnosticsLogLevel } from "@/types/global";

type LogLevel = "debug" | "info" | "warn" | "error";

type LogDomain =
  | "app.lifecycle"
  | "ui.action"
  | "ui.error"
  | "tauri.invoke"
  | "window.lifecycle"
  | "settings.persistence"
  | "terminal.input"
  | "terminal.resize"
  | "session.lifecycle"
  | "ssh.auth"
  | "transfer.lifecycle"
  | "watcher.sync"
  | "security.flow"
  | "background-image"
  | "updater.flow";

type StableLogIdKey = "session_id" | "connection_id" | "transfer_id" | "tunnel_id" | "request_id";

type LogIds = Partial<Record<StableLogIdKey, string>>;

export interface LogPayload {
  domain: LogDomain;
  event: string;
  message: string;
  ids?: LogIds;
  data?: unknown;
  error?: unknown;
}

interface FrontendLogEntry extends LogPayload {
  timestamp: string;
  level: LogLevel;
}

const LOG_LEVELS: Record<LogLevel, number> = {
  debug: 0,
  info: 1,
  warn: 2,
  error: 3,
};

const DEFAULT_LEVEL: LogLevel = import.meta.env.DEV ? "debug" : "info";
const BATCH_DELAY_MS = 250;
const MAX_BATCH_SIZE = 50;
const MAX_LOG_QUEUE_SIZE = 1000;
const LOG_QUEUE_DROP_BATCH = 250;

let minLevel: LogLevel = DEFAULT_LEVEL;
const queue: FrontendLogEntry[] = [];
let flushTimer: ReturnType<typeof setTimeout> | null = null;
let flushInFlight = false;
let lifecycleFlushRegistered = false;
let droppedLogEntryCount = 0;

export function setLoggerLevel(level: DiagnosticsLogLevel): void {
  minLevel = level;
}

function shouldLog(level: LogLevel): boolean {
  return LOG_LEVELS[level] >= LOG_LEVELS[minLevel];
}

function createRequestId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return `req-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

function formatTimestamp(date = new Date()): string {
  const pad = (value: number, width = 2) => String(value).padStart(width, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}.${pad(date.getMilliseconds(), 3)}`;
}

function formatConsoleLine(entry: FrontendLogEntry): string {
  return `[${entry.timestamp}] [${entry.level.toUpperCase()}] [${entry.domain}/${entry.event}] ${entry.message}`;
}

function normalizeError(error: unknown): unknown {
  if (error instanceof Error) {
    return {
      name: error.name,
      message_hash: hashString(error.message),
      stack_hash: error.stack ? hashString(error.stack) : undefined,
      // Local debugging aid: opt in via localStorage to see raw details in
      // logs (never set in normal use — logs may be exported/shared).
      ...(typeof window !== "undefined" &&
      window.localStorage.getItem("nyaterm_debug_raw_errors") === "1"
        ? { message: error.message, stack: error.stack?.split("\n").slice(0, 10).join("\n") }
        : {}),
    };
  }
  return error;
}

function hashString(value: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0).toString(16).padStart(8, "0");
}

function normalizeKey(key: string): string {
  return key
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "");
}

function isRedactedKey(key: string): boolean {
  const normalized = normalizeKey(key);
  return (
    normalized === "password" ||
    normalized === "secret" ||
    normalized === "token" ||
    normalized === "otp" ||
    normalized === "command" ||
    normalized === "content" ||
    normalized === "clipboard" ||
    normalized === "passphrase" ||
    normalized === "master_password" ||
    normalized.endsWith("_password") ||
    normalized.endsWith("_secret") ||
    normalized.endsWith("_token") ||
    normalized.endsWith("_otp") ||
    normalized.endsWith("_command") ||
    normalized.endsWith("_content") ||
    normalized.endsWith("_clipboard") ||
    normalized.endsWith("_passphrase") ||
    normalized.includes("private_key") ||
    normalized.includes("public_key") ||
    normalized.includes("key_data") ||
    normalized.includes("secret_key") ||
    (normalized.endsWith("_key") && !normalized.endsWith("_id"))
  );
}

function isHostKey(key: string): boolean {
  const normalized = normalizeKey(key);
  return normalized === "host" || normalized.endsWith("_host");
}

function isUsernameKey(key: string): boolean {
  const normalized = normalizeKey(key);
  return normalized === "username" || normalized === "user" || normalized.endsWith("_username");
}

function isPathKey(key: string): boolean {
  const normalized = normalizeKey(key);
  return normalized === "path" || normalized === "cwd" || normalized.endsWith("_path");
}

function sanitizeString(key: string | undefined, value: string): unknown {
  if (!key) return value;
  if (isRedactedKey(key)) return "[REDACTED]";
  if (isHostKey(key)) return { type: "host", hash: hashString(value) };
  if (isUsernameKey(key)) return { type: "username", hash: hashString(value) };
  if (isPathKey(key)) {
    const extension = (() => {
      const lastDot = value.lastIndexOf(".");
      if (lastDot <= value.lastIndexOf("/") || lastDot <= value.lastIndexOf("\\")) {
        return undefined;
      }
      return value.slice(lastDot + 1);
    })();
    return {
      type: "path",
      hash: hashString(value),
      ...(extension ? { extension } : {}),
    };
  }
  return value;
}

function sanitizeValue(value: unknown, key?: string): unknown {
  if (value instanceof Error) {
    return sanitizeValue(normalizeError(value), key);
  }

  if (Array.isArray(value)) {
    return value.map((item) => sanitizeValue(item, key));
  }

  if (typeof value === "string") {
    return sanitizeString(key, value);
  }

  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>).map(([childKey, childValue]) => [
        childKey,
        sanitizeValue(childValue, childKey),
      ]),
    );
  }

  return value;
}

function normalizePayload(level: LogLevel, payload: LogPayload): FrontendLogEntry {
  return {
    ...payload,
    timestamp: formatTimestamp(),
    level,
    ids: payload.ids ? (sanitizeValue(payload.ids, "ids") as LogIds) : undefined,
    data: payload.data === undefined ? undefined : sanitizeValue(payload.data, "data"),
    error:
      payload.error === undefined
        ? undefined
        : sanitizeValue(normalizeError(payload.error), "error"),
  };
}

function writeConsole(entry: FrontendLogEntry): void {
  const line = formatConsoleLine(entry);
  const extras = [entry.ids, entry.data, entry.error].filter((item) => item !== undefined);
  switch (entry.level) {
    case "debug":
      console.debug(line, ...extras);
      break;
    case "info":
      console.info(line, ...extras);
      break;
    case "warn":
      console.warn(line, ...extras);
      break;
    case "error":
      console.error(line, ...extras);
      break;
  }
}

function scheduleFlush(): void {
  if (flushTimer) return;
  flushTimer = setTimeout(() => {
    flushTimer = null;
    void flushQueue();
  }, BATCH_DELAY_MS);
}

function enforceQueueLimit(): void {
  if (queue.length <= MAX_LOG_QUEUE_SIZE) return;
  const dropCount = Math.min(
    queue.length - MAX_LOG_QUEUE_SIZE + LOG_QUEUE_DROP_BATCH,
    queue.length,
  );
  queue.splice(0, dropCount);
  droppedLogEntryCount += dropCount;
}

function createQueueOverflowSummary(droppedCount: number): FrontendLogEntry {
  return normalizePayload("warn", {
    domain: "ui.error",
    event: "logger.queue_overflow",
    message: "Frontend log queue overflowed; old log entries were dropped",
    data: {
      dropped_count: droppedCount,
      max_queue_size: MAX_LOG_QUEUE_SIZE,
    },
  });
}

function prependQueueOverflowSummary(): void {
  if (droppedLogEntryCount <= 0) return;
  const summary = createQueueOverflowSummary(droppedLogEntryCount);
  droppedLogEntryCount = 0;
  writeConsole(summary);
  queue.unshift(summary);
}

async function flushQueue(): Promise<void> {
  if (flushInFlight) return;
  if (flushTimer) {
    clearTimeout(flushTimer);
    flushTimer = null;
  }
  if (queue.length === 0) return;

  flushInFlight = true;
  prependQueueOverflowSummary();
  const batch = queue.splice(0, MAX_BATCH_SIZE);

  try {
    await tauriInvoke("append_frontend_logs", { entries: batch });
  } catch (error) {
    console.error(
      `[${formatTimestamp()}] [ERROR] [ui.error/logger.flush_failed] Failed to persist frontend log batch`,
      error,
    );
  } finally {
    flushInFlight = false;
    if (queue.length >= MAX_BATCH_SIZE) {
      void flushQueue();
    } else if (queue.length > 0) {
      scheduleFlush();
    }
  }
}

function registerLifecycleFlush(): void {
  if (lifecycleFlushRegistered) return;
  if (typeof window === "undefined") return;

  const flush = () => {
    void flushQueue();
  };

  window.addEventListener("pagehide", flush);
  window.addEventListener("beforeunload", flush);

  if (typeof document !== "undefined") {
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "hidden") {
        flush();
      }
    });
  }

  lifecycleFlushRegistered = true;
}

function emit(level: LogLevel, payload: LogPayload): void {
  if (!shouldLog(level)) return;

  const entry = normalizePayload(level, payload);
  writeConsole(entry);
  queue.push(entry);
  enforceQueueLimit();

  if (level === "warn" || level === "error" || queue.length >= MAX_BATCH_SIZE) {
    void flushQueue();
  } else {
    scheduleFlush();
  }
}

export const logger = {
  debug(payload: LogPayload) {
    emit("debug", payload);
  },

  info(payload: LogPayload) {
    emit("info", payload);
  },

  warn(payload: LogPayload) {
    emit("warn", payload);
  },

  error(payload: LogPayload) {
    emit("error", payload);
  },

  flush(): Promise<void> {
    return flushQueue();
  },

  createRequestId,
};

registerLifecycleFlush();
