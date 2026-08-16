export type PasteConfirmAction = "upload" | "copy" | "move";

export interface PasteConfirmRequest {
  action: PasteConfirmAction;
  count: number;
  targetDir: string;
  fileNames: string[];
}

type PasteConfirmListener = (request: PasteConfirmRequest | null) => void;

let activeRequest: PasteConfirmRequest | null = null;
let localResolver: ((confirmed: boolean) => void) | null = null;
const listeners = new Set<PasteConfirmListener>();

function notifyListeners() {
  for (const listener of listeners) {
    listener(activeRequest);
  }
}

export function subscribePasteConfirm(listener: PasteConfirmListener): () => void {
  listeners.add(listener);
  listener(activeRequest);
  return () => {
    listeners.delete(listener);
  };
}

export function showPasteConfirm(request: PasteConfirmRequest): Promise<boolean> {
  if (localResolver) {
    localResolver(false);
  }

  return new Promise((resolve) => {
    localResolver = resolve;
    activeRequest = request;
    notifyListeners();
  });
}

export function resolvePasteConfirm(confirmed: boolean): void {
  const resolver = localResolver;
  localResolver = null;
  activeRequest = null;
  notifyListeners();
  resolver?.(confirmed);
}
