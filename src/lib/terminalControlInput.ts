import type { Terminal } from "@xterm/xterm";

const CTRL_L_INPUT = "\x0c";
const terminalUserInputMarkers = new WeakMap<Terminal, () => void>();

export function registerTerminalUserInputMarker(
  terminal: Terminal,
  marker: () => void,
): () => void {
  terminalUserInputMarkers.set(terminal, marker);
  return () => {
    terminalUserInputMarkers.delete(terminal);
  };
}

export function markTerminalUserInput(terminal: Terminal): void {
  terminalUserInputMarkers.get(terminal)?.();
}

export function sendTerminalClearInput(
  terminal: Terminal,
  options: { focus?: boolean } = {},
) {
  terminal.clearSelection();
  markTerminalUserInput(terminal);
  terminal.input(CTRL_L_INPUT, true);
  if (options.focus) {
    terminal.focus();
  }
}
