import type { TerminalReconnectSnapshot } from "@/lib/terminalReconnectHistory";

interface MutableRef<T> {
  current: T;
}

export type SnapshotRestorePhase =
  | "idle"
  | "replaying"
  | "awaiting-final-fit"
  | "revealed";

interface CreateXTerminalSnapshotRestoreControllerParams {
  restoringRef: MutableRef<boolean>;
  setRestoring: (restoring: boolean) => void;
  setTerminalReady: (ready: boolean) => void;
}

export function createXTerminalSnapshotRestoreController({
  restoringRef,
  setRestoring,
  setTerminalReady,
}: CreateXTerminalSnapshotRestoreControllerParams) {
  let phase: SnapshotRestorePhase = "idle";

  const begin = (snapshot: TerminalReconnectSnapshot | null | undefined) => {
    if (!snapshot?.content) {
      if (restoringRef.current) {
        phase = "idle";
        restoringRef.current = false;
        setRestoring(false);
      }
      return false;
    }

    phase = "replaying";
    restoringRef.current = true;
    setRestoring(true);
    setTerminalReady(false);
    return true;
  };

  const markReplayAndAttachComplete = () => {
    if (phase !== "replaying") return false;

    phase = "awaiting-final-fit";
    setTerminalReady(true);
    return true;
  };

  const completeAfterFinalFit = () => {
    if (phase !== "awaiting-final-fit") return false;

    phase = "revealed";
    restoringRef.current = false;
    setRestoring(false);
    return true;
  };

  return {
    begin,
    markReplayAndAttachComplete,
    completeAfterFinalFit,
    isRestoring: () => restoringRef.current,
    getPhase: () => phase,
  };
}

export type XTerminalSnapshotRestoreController = ReturnType<
  typeof createXTerminalSnapshotRestoreController
>;
