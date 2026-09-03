import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@/lib/invoke";
import type { RemoteGpuOverview } from "@/types/global";

const MAX_CONSECUTIVE_FAILURES = 3;

interface OwnedRemoteGpuOverviewState {
  sessionId: string;
  overview: RemoteGpuOverview | null;
  error: boolean;
  isManualRefreshing: boolean;
}

export interface RemoteGpuOverviewState {
  sessionId: string | null;
  overview: RemoteGpuOverview | null;
  error: boolean;
  isManualRefreshing: boolean;
  refresh: () => void;
}

export function useRemoteGpuOverview(
  activeSessionId: string | null,
  enabled: boolean,
  intervalSeconds: number,
): RemoteGpuOverviewState {
  const [state, setState] = useState<OwnedRemoteGpuOverviewState | null>(null);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const activeSessionRef = useRef<string | null>(null);
  const generationRef = useRef(0);
  const fetchingGenerationRef = useRef<number | null>(null);
  const failCountRef = useRef(0);
  const unavailableSessionRef = useRef<string | null>(null);
  const pollIntervalMs = Math.max(3, intervalSeconds) * 1000;
  const requestedSessionId = enabled ? activeSessionId : null;

  activeSessionRef.current = requestedSessionId;

  const isCurrentRequest = useCallback(
    (sessionId: string, generation: number) =>
      activeSessionRef.current === sessionId && generationRef.current === generation,
    [],
  );

  const fetchOverview = useCallback(
    async (sessionId: string, generation: number, manual = false) => {
      if (!isCurrentRequest(sessionId, generation)) return null;
      if (!manual && unavailableSessionRef.current === sessionId) return null;
      if (fetchingGenerationRef.current === generation) return null;
      fetchingGenerationRef.current = generation;
      if (manual) {
        setState((current) =>
          current?.sessionId === sessionId ? { ...current, isManualRefreshing: true } : current,
        );
      }

      try {
        const data = await invoke<RemoteGpuOverview>("get_remote_gpu_overview", { sessionId });
        if (!isCurrentRequest(sessionId, generation)) return null;

        setState((current) => ({
          sessionId,
          overview: data,
          error: false,
          isManualRefreshing: current?.sessionId === sessionId ? current.isManualRefreshing : false,
        }));
        failCountRef.current = 0;

        if (data.available) {
          unavailableSessionRef.current = null;
        } else {
          unavailableSessionRef.current = sessionId;
          if (pollRef.current) {
            clearInterval(pollRef.current);
            pollRef.current = null;
          }
        }

        return data;
      } catch {
        if (!isCurrentRequest(sessionId, generation)) return null;

        failCountRef.current += 1;
        const clearOverview = failCountRef.current >= MAX_CONSECUTIVE_FAILURES;
        setState((current) => ({
          sessionId,
          overview: clearOverview || current?.sessionId !== sessionId ? null : current.overview,
          error: true,
          isManualRefreshing: current?.sessionId === sessionId ? current.isManualRefreshing : false,
        }));
        return null;
      } finally {
        if (fetchingGenerationRef.current === generation) {
          fetchingGenerationRef.current = null;
        }
        if (manual && isCurrentRequest(sessionId, generation)) {
          setState((current) =>
            current?.sessionId === sessionId ? { ...current, isManualRefreshing: false } : current,
          );
        }
      }
    },
    [isCurrentRequest],
  );

  const refresh = useCallback(() => {
    if (!requestedSessionId) return;
    const generation = generationRef.current;
    void fetchOverview(requestedSessionId, generation, true).then((data) => {
      if (
        !data?.available ||
        !isCurrentRequest(requestedSessionId, generation) ||
        pollRef.current
      ) {
        return;
      }
      pollRef.current = setInterval(
        () => void fetchOverview(requestedSessionId, generation),
        pollIntervalMs,
      );
    });
  }, [fetchOverview, isCurrentRequest, pollIntervalMs, requestedSessionId]);

  useEffect(() => {
    const generation = generationRef.current + 1;
    generationRef.current = generation;
    fetchingGenerationRef.current = null;

    if (pollRef.current) {
      clearInterval(pollRef.current);
      pollRef.current = null;
    }

    failCountRef.current = 0;
    unavailableSessionRef.current = null;

    if (!requestedSessionId) {
      setState(null);
      return () => {
        if (generationRef.current === generation) generationRef.current += 1;
      };
    }

    setState({
      sessionId: requestedSessionId,
      overview: null,
      error: false,
      isManualRefreshing: false,
    });
    void fetchOverview(requestedSessionId, generation);
    pollRef.current = setInterval(
      () => void fetchOverview(requestedSessionId, generation),
      pollIntervalMs,
    );

    return () => {
      if (generationRef.current === generation) generationRef.current += 1;
      if (pollRef.current) {
        clearInterval(pollRef.current);
        pollRef.current = null;
      }
    };
  }, [fetchOverview, pollIntervalMs, requestedSessionId]);

  const visibleState = state?.sessionId === requestedSessionId ? state : null;

  return {
    sessionId: visibleState?.sessionId ?? null,
    overview: visibleState?.overview ?? null,
    error: visibleState?.error ?? false,
    isManualRefreshing: visibleState?.isManualRefreshing ?? false,
    refresh,
  };
}
