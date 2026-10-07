export type NyaScriptPendingSessionOpen = {
  tabId: string;
  createRequestId: string;
};

type NyaScriptSessionOpenState =
  | { phase: "awaiting-pending" }
  | { phase: "connecting"; pending: NyaScriptPendingSessionOpen }
  | { phase: "opened" }
  | { phase: "cancelled" };

export class NyaScriptSessionOpenLifecycle {
  private readonly requests = new Map<string, NyaScriptSessionOpenState>();

  begin(requestId: string) {
    if (this.requests.get(requestId)?.phase === "cancelled") return false;
    this.requests.set(requestId, { phase: "awaiting-pending" });
    return true;
  }

  registerPending(
    requestId: string,
    pending: NyaScriptPendingSessionOpen,
    cancelPending: (pending: NyaScriptPendingSessionOpen) => void,
  ) {
    const current = this.requests.get(requestId);
    if (!current || current.phase === "cancelled") {
      this.requests.set(requestId, { phase: "cancelled" });
      cancelPending(pending);
      return false;
    }
    if (current.phase !== "awaiting-pending") return false;

    this.requests.set(requestId, { phase: "connecting", pending });
    return true;
  }

  markOpened(requestId: string) {
    const current = this.requests.get(requestId);
    if (!current || current.phase === "cancelled") return false;
    this.requests.set(requestId, { phase: "opened" });
    return true;
  }

  cancel(
    requestId: string,
    cancelPending: (pending: NyaScriptPendingSessionOpen) => void,
  ) {
    const current = this.requests.get(requestId);
    if (!current) {
      this.requests.set(requestId, { phase: "cancelled" });
      return "cancelled" as const;
    }
    if (current.phase === "opened") return "opened" as const;
    if (current.phase === "cancelled") return "cancelled" as const;

    this.requests.set(requestId, { phase: "cancelled" });
    if (current.phase === "connecting") cancelPending(current.pending);
    return "cancelled" as const;
  }

  finish(requestId: string) {
    this.requests.delete(requestId);
  }
}
