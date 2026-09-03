import type { Terminal } from "@xterm/xterm";
import { type RefObject, useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { logger } from "@/lib/logger";
import { sendTerminalClearInput } from "@/lib/terminalControlInput";
import type { TerminalFitScheduler } from "./terminalFitScheduler";
import type { PerformanceMode } from "./xterminalTypes";

interface UseTerminalRefreshEffectsParams {
  terminalRef: RefObject<Terminal | null>;
  fitSchedulerRef: RefObject<TerminalFitScheduler | null>;
  active: boolean;
  visible: boolean;
  terminalReady: boolean;
  performanceMode: PerformanceMode;
  sessionId: string;
  showGutter: boolean;
  showContentPadding: boolean;
  workspacePaddingSetting?: boolean;
  snapshotRestoringRef?: RefObject<boolean>;
}

export function useTerminalRefreshEffects({
  terminalRef,
  fitSchedulerRef,
  active,
  visible,
  terminalReady,
  performanceMode,
  sessionId,
  showGutter,
  showContentPadding,
  workspacePaddingSetting,
  snapshotRestoringRef,
}: UseTerminalRefreshEffectsParams) {
  useEffect(() => {
    if (terminalReady && fitSchedulerRef.current && terminalRef.current) {
      fitSchedulerRef.current.schedule({
        reason: "ready",
        force: true,
        refresh: true,
        onComplete: () => {
          if (!terminalRef.current) return;
          if (showGutter && performanceMode === "normal") {
            window.dispatchEvent(
              new CustomEvent("nyaterm:refresh-gutter", {
                detail: { sessionId },
              }),
            );
          }
        },
      });
    }
  }, [fitSchedulerRef, performanceMode, sessionId, showGutter, terminalReady, terminalRef]);

  useEffect(() => {
    const paddingEnabled = showContentPadding;
    if (!terminalReady || !fitSchedulerRef.current || !terminalRef.current) return;

    fitSchedulerRef.current.schedule({
      reason: "padding",
      force: true,
      refresh: true,
      onComplete: () => {
        if (paddingEnabled !== (workspacePaddingSetting ?? false)) {
          return;
        }
        if (showGutter && performanceMode === "normal") {
          window.dispatchEvent(
            new CustomEvent("nyaterm:refresh-gutter", {
              detail: { sessionId },
            }),
          );
        }
      },
    });
  }, [
    fitSchedulerRef,
    performanceMode,
    sessionId,
    showContentPadding,
    showGutter,
    terminalReady,
    terminalRef,
    workspacePaddingSetting,
  ]);

  useEffect(() => {
    if (active && visible && terminalReady && fitSchedulerRef.current && terminalRef.current) {
      fitSchedulerRef.current.schedule({
        reason: "active",
        force: true,
        refresh: true,
        focus: true,
      });
    }
  }, [active, fitSchedulerRef, terminalReady, terminalRef, visible]);

  useEffect(() => {
    const handleRefresh = () => {
      if (
        snapshotRestoringRef?.current ||
        !visible ||
        !fitSchedulerRef.current ||
        !terminalRef.current
      )
        return;

      fitSchedulerRef.current.schedule({
        reason: "global-refresh",
        force: true,
        refresh: true,
        focus: active,
      });
    };

    window.addEventListener("nyaterm:refresh-terminals", handleRefresh);
    return () => {
      window.removeEventListener("nyaterm:refresh-terminals", handleRefresh);
    };
  }, [active, fitSchedulerRef, snapshotRestoringRef, terminalRef, visible]);

  useEffect(() => {
    if (!terminalReady) return;

    let disposed = false;
    let unlistenResized: (() => void) | undefined;
    let unlistenMoved: (() => void) | undefined;
    let unlistenFocused: (() => void) | undefined;
    let unlistenScale: (() => void) | undefined;
    let resolutionQuery: MediaQueryList | null = null;
    let lastDevicePixelRatio = window.devicePixelRatio || 1;

    const scheduleWindowFit = (
      reason: "window-resized" | "window-moved" | "window-focus" | "scale-factor",
      force = false,
      scaleFactor?: number,
    ) => {
      if (snapshotRestoringRef?.current) return;
      const nextDevicePixelRatio = window.devicePixelRatio || 1;
      const dprChanged = Math.abs(nextDevicePixelRatio - lastDevicePixelRatio) > 0.001;
      if (dprChanged) {
        lastDevicePixelRatio = nextDevicePixelRatio;
      }

      const isScaleChange = reason === "scale-factor" || dprChanged;
      if (isScaleChange) {
        logger.debug({
          domain: "terminal.resize",
          event: "terminal.resize.scale_factor_changed",
          message: "Terminal scale factor changed",
          ids: { session_id: sessionId },
          data: {
            reason,
            device_pixel_ratio: nextDevicePixelRatio,
            tauri_scale_factor: scaleFactor,
          },
        });
      }

      if (!visible && !isScaleChange) return;
      if (!fitSchedulerRef.current || !terminalRef.current) return;
      fitSchedulerRef.current.schedule({
        reason: isScaleChange ? "scale-factor" : reason,
        force: force || isScaleChange,
        refresh: true,
        clearTextureAtlas: isScaleChange,
        focus: active && visible,
      });
    };

    const installResolutionListener = () => {
      resolutionQuery?.removeEventListener("change", handleResolutionChange);
      resolutionQuery = window.matchMedia(`(resolution: ${lastDevicePixelRatio}dppx)`);
      resolutionQuery.addEventListener("change", handleResolutionChange);
    };

    const handleResolutionChange = () => {
      if (disposed) return;
      scheduleWindowFit("scale-factor", true);
      installResolutionListener();
    };

    installResolutionListener();

    const appWindow = getCurrentWindow();
    appWindow
      .onResized(() => {
        if (!disposed) scheduleWindowFit("window-resized");
      })
      .then((unlisten) => {
        unlistenResized = unlisten;
      })
      .catch(() => {});
    appWindow
      .onMoved(() => {
        if (!disposed) scheduleWindowFit("window-moved");
      })
      .then((unlisten) => {
        unlistenMoved = unlisten;
      })
      .catch(() => {});
    appWindow
      .onFocusChanged(({ payload }) => {
        if (!disposed && payload) scheduleWindowFit("window-focus");
      })
      .then((unlisten) => {
        unlistenFocused = unlisten;
      })
      .catch(() => {});
    appWindow
      .onScaleChanged(({ payload }) => {
        if (!disposed) scheduleWindowFit("scale-factor", true, payload.scaleFactor);
      })
      .then((unlisten) => {
        unlistenScale = unlisten;
      })
      .catch(() => {});

    return () => {
      disposed = true;
      resolutionQuery?.removeEventListener("change", handleResolutionChange);
      unlistenResized?.();
      unlistenMoved?.();
      unlistenFocused?.();
      unlistenScale?.();
    };
  }, [
    active,
    fitSchedulerRef,
    sessionId,
    snapshotRestoringRef,
    terminalReady,
    terminalRef,
    visible,
  ]);

  useEffect(() => {
    const handleClear = () => {
      const terminal = terminalRef.current;
      if (!active || !terminal) return;
      sendTerminalClearInput(terminal, { focus: active });
    };

    window.addEventListener("nyaterm:clear-terminal", handleClear);
    return () => {
      window.removeEventListener("nyaterm:clear-terminal", handleClear);
    };
  }, [active, terminalRef]);
}
