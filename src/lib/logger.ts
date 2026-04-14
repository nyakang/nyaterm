import { invoke as tauriInvoke } from "@tauri-apps/api/core";

type LogLevel = "debug" | "info" | "warn" | "error";

const LOG_LEVELS: Record<LogLevel, number> = {
  debug: 0,
  info: 1,
  warn: 2,
  error: 3,
};

const MIN_LEVEL: LogLevel = import.meta.env.DEV ? "debug" : "warn";

function shouldLog(level: LogLevel): boolean {
  return LOG_LEVELS[level] >= LOG_LEVELS[MIN_LEVEL];
}

function formatMessage(level: LogLevel, message: string): string {
  const now = new Date();
  const pad = (n: number, w = 2) => String(n).padStart(w, "0");
  const timestamp = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}T${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}.${pad(now.getMilliseconds(), 3)}`;
  return `[${timestamp}] [${level.toUpperCase()}] ${message}`;
}

function serializeLogArg(arg: unknown): string {
  if (arg instanceof Error) {
    return arg.stack ?? `${arg.name}: ${arg.message}`;
  }

  if (typeof arg === "string") {
    return arg;
  }

  if (
    typeof arg === "number" ||
    typeof arg === "boolean" ||
    typeof arg === "bigint" ||
    typeof arg === "symbol" ||
    arg == null
  ) {
    return String(arg);
  }

  try {
    return JSON.stringify(arg);
  } catch {
    return String(arg);
  }
}

function persistLog(level: LogLevel, message: string, args: unknown[]): void {
  if (level !== "warn" && level !== "error") {
    return;
  }

  const context = args.map(serializeLogArg).join("\n");

  void tauriInvoke("write_log", {
    level,
    message,
    context: context || null,
  }).catch(() => {});
}

/** Level-filtered logger: debug/info/warn/error. Respects DEV vs prod min level. */
export const logger = {
  debug(message: string, ...args: unknown[]) {
    if (shouldLog("debug")) {
      console.debug(formatMessage("debug", message), ...args);
    }
  },

  info(message: string, ...args: unknown[]) {
    if (shouldLog("info")) {
      console.info(formatMessage("info", message), ...args);
    }
  },

  warn(message: string, ...args: unknown[]) {
    if (shouldLog("warn")) {
      console.warn(formatMessage("warn", message), ...args);
      persistLog("warn", message, args);
    }
  },

  error(message: string, ...args: unknown[]) {
    if (shouldLog("error")) {
      console.error(formatMessage("error", message), ...args);
      persistLog("error", message, args);
    }
  },
};
