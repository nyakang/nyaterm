import type { AIContext, SavedConnection, SessionPane } from "@/types/global";
import { invoke } from "./invoke";

export interface TerminalContextProvider {
  getRecentOutput: (lineLimit: number) => string;
  getSelectedText: () => string;
  getInputBuffer: () => string;
  insertCommand: (command: string) => Promise<void>;
  focus: () => void;
}

export interface TerminalContextSnapshot {
  recentOutput: string;
  selectedText: string;
  inputBuffer: string;
}

const providers = new Map<string, TerminalContextProvider>();

export function registerTerminalContextProvider(
  sessionId: string,
  provider: TerminalContextProvider,
): () => void {
  providers.set(sessionId, provider);
  return () => {
    if (providers.get(sessionId) === provider) {
      providers.delete(sessionId);
    }
  };
}

export function getTerminalContextProvider(sessionId: string | null | undefined) {
  return sessionId ? providers.get(sessionId) : undefined;
}

export function getTerminalContextSnapshot(
  sessionId: string | null | undefined,
  lineLimit: number,
): TerminalContextSnapshot {
  const provider = getTerminalContextProvider(sessionId);
  return {
    recentOutput: provider?.getRecentOutput(lineLimit) ?? "",
    selectedText: provider?.getSelectedText() ?? "",
    inputBuffer: provider?.getInputBuffer() ?? "",
  };
}

export async function buildAIContext({
  pane,
  connection,
  lineLimit,
  selectedText,
}: {
  pane: SessionPane | null;
  connection?: SavedConnection | null;
  lineLimit: number;
  selectedText?: string;
}): Promise<AIContext> {
  const snapshot = getTerminalContextSnapshot(pane?.sessionId, lineLimit);
  let cwd: string | null = null;
  if (pane?.sessionId) {
    try {
      cwd = await invoke<string>("get_terminal_cwd", { sessionId: pane.sessionId });
    } catch {
      cwd = null;
    }
  }

  return {
    connectionName: connection?.name ?? pane?.name ?? null,
    host: connection?.host ?? null,
    port: connection?.port ?? null,
    username: connection?.username ?? null,
    cwd,
    os: null,
    arch: null,
    recentOutput: snapshot.recentOutput,
    selectedText: selectedText ?? snapshot.selectedText,
    inputBuffer: snapshot.inputBuffer,
  };
}
