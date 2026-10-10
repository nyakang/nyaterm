import type { AiCaptureEvent } from "@/types/global";

const RESET = "\x1b[0m";
const BOLD = "\x1b[1m";
const DIM = "\x1b[2m";
const ACCENT = "\x1b[38;5;141m"; // purple/violet
const GREEN = "\x1b[38;5;78m";
const RED = "\x1b[38;5;203m";

function sanitizeCommandForTerminal(command: string): string {
  return command
    .replace(/\r\n|\r|\n/gu, " ; ")
    .replace(/\x1b/gu, "^[")
    .replace(/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/gu, " ");
}

export function renderAiCommandStart(
  event: Extract<AiCaptureEvent, { type: "commandStart" }>,
): string {
  const step = event.stepIndex + 1;
  const command = sanitizeCommandForTerminal(event.command);
  const source = event.source === "MCP" ? "MCP" : "AI";
  const header = `${DIM}${ACCENT}┌ ${source} #${step}${RESET}${DIM} ${"─".repeat(30)}${RESET}`;
  return `${BOLD}${command}${RESET}\r\n${header}\r\n`;
}

export function renderAiCommandEnd(event: Extract<AiCaptureEvent, { type: "commandEnd" }>): string {
  const parts: string[] = [];

  if (event.output) {
    parts.push(`${DIM}${ACCENT}├${"─".repeat(36)}${RESET}`);
    const lines = event.output.split("\n");
    for (const line of lines) {
      parts.push(`${DIM}${ACCENT}│${RESET} ${DIM}${line}${RESET}`);
    }
    if (event.truncated) {
      parts.push(`${DIM}${ACCENT}│${RESET} ${DIM}...${RESET}`);
    }
  }

  const exitCode = event.exitCode;
  const ok = exitCode === 0;
  const statusColor = ok ? GREEN : exitCode != null ? RED : DIM;
  const statusIcon = ok ? "✓" : exitCode != null ? "✗" : "?";
  const exitStr = exitCode != null ? `exit ${exitCode}` : "no exit code";
  const duration = `${event.durationMs}ms`;

  const footer = `${DIM}${ACCENT}└${RESET} ${statusColor}${statusIcon} ${exitStr}${RESET} ${DIM}· ${duration}${RESET}`;
  parts.push(footer);

  return `${parts.map((l) => `\r\n${l}`).join("")}\r\n`;
}
