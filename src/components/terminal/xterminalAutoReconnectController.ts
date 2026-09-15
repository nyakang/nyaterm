interface MutableBooleanRef {
  current: boolean;
}

interface CreateXTerminalAutoReconnectControllerOptions {
  reconnectingRef: MutableBooleanRef;
  attemptReconnect: () => Promise<boolean>;
  getRetryDelayMs: () => number;
}

export function createXTerminalAutoReconnectController({
  reconnectingRef,
  attemptReconnect,
  getRetryDelayMs,
}: CreateXTerminalAutoReconnectControllerOptions) {
  let autoReconnectActive = false;
  let retryTimer: ReturnType<typeof setTimeout> | null = null;

  const clearRetryTimer = () => {
    if (retryTimer === null) return;
    clearTimeout(retryTimer);
    retryTimer = null;
  };

  const scheduleRetry = () => {
    if (!autoReconnectActive || retryTimer !== null) return;
    retryTimer = setTimeout(
      () => {
        retryTimer = null;
        void attemptNow();
      },
      Math.max(0, getRetryDelayMs()),
    );
  };

  const attemptNow = async (): Promise<boolean> => {
    clearRetryTimer();
    if (reconnectingRef.current) return false;

    reconnectingRef.current = true;
    let succeeded = false;
    try {
      succeeded = await attemptReconnect();
      return succeeded;
    } catch {
      return false;
    } finally {
      reconnectingRef.current = false;
      if (succeeded) {
        autoReconnectActive = false;
        clearRetryTimer();
      } else {
        scheduleRetry();
      }
    }
  };

  return {
    startAutoReconnect() {
      if (autoReconnectActive) return Promise.resolve(false);
      autoReconnectActive = true;
      return attemptNow();
    },
    attemptNow,
    cancel() {
      autoReconnectActive = false;
      clearRetryTimer();
    },
  };
}
