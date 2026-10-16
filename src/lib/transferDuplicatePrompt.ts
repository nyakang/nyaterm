export type TransferDuplicateChoice = "skip" | "overwrite" | "rename";
export type TransferDuplicatePromptChoice = TransferDuplicateChoice | "overwriteAllForTask";

export interface TransferDuplicateRequest {
  requestId: string;
  sessionId: string;
  remotePath: string;
  fileName: string;
  isDirectory: boolean;
  targetWindowLabel?: string | null;
  destinationKind?: "local" | "remote";
  requiresActivation?: boolean;
  respondViaBackend?: boolean;
  allowApplyToTask?: boolean;
  unverified?: boolean;
}

type TransferDuplicateListener = (request: TransferDuplicateRequest | null) => void;
type PendingPrompt = {
  request: TransferDuplicateRequest;
  resolve?: (choice: TransferDuplicatePromptChoice) => void;
};

// 并发下载可能同时询问，按到达顺序展示，不能替换仍在等待的请求。
const pendingPrompts: PendingPrompt[] = [];
const listeners = new Set<TransferDuplicateListener>();

function notifyListeners() {
  for (const listener of listeners) {
    listener(getActiveTransferDuplicatePrompt());
  }
}

export function subscribeTransferDuplicatePrompt(listener: TransferDuplicateListener) {
  listeners.add(listener);
  listener(getActiveTransferDuplicatePrompt());
  return () => {
    listeners.delete(listener);
  };
}

export function showTransferDuplicatePrompt(
  request: TransferDuplicateRequest,
): Promise<TransferDuplicatePromptChoice> {
  return new Promise((resolve) => {
    pendingPrompts.push({
      request: { ...request, respondViaBackend: false },
      resolve,
    });
    if (pendingPrompts.length === 1) notifyListeners();
  });
}

export function resolveTransferDuplicatePrompt(choice: TransferDuplicatePromptChoice) {
  if (!pendingPrompts[0]?.resolve) return;
  const prompt = pendingPrompts.shift();
  notifyListeners();
  prompt?.resolve?.(choice);
}

export function setBackendTransferDuplicatePrompt(request: TransferDuplicateRequest | null) {
  if (request) {
    if (pendingPrompts.some((prompt) => prompt.request.requestId === request.requestId)) return;
    pendingPrompts.push({
      request: { ...request, respondViaBackend: true, allowApplyToTask: false },
    });
    if (pendingPrompts.length === 1) notifyListeners();
    return;
  }

  const active = pendingPrompts[0]?.request;
  if (active?.respondViaBackend) removeBackendTransferDuplicatePrompt(active.requestId);
}

export function removeBackendTransferDuplicatePrompt(requestId: string) {
  const index = pendingPrompts.findIndex(
    (prompt) => prompt.request.respondViaBackend && prompt.request.requestId === requestId,
  );
  if (index < 0) return;
  pendingPrompts.splice(index, 1);
  if (index === 0) notifyListeners();
}

export function getActiveTransferDuplicatePrompt() {
  return pendingPrompts[0]?.request ?? null;
}
