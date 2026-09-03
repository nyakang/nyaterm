import { WebglAddon } from "@xterm/addon-webgl";
import type { Terminal } from "@xterm/xterm";
import { type RefObject, useCallback, useEffect, useRef } from "react";
import { isTerminalTransparencyEnabled } from "@/lib/backgroundImage";
import { resolveTerminalFontSize } from "@/lib/terminalFontSize";
import type { TerminalColors } from "@/lib/themes";
import { installImeCompatibilityPatch } from "@/lib/xtermImeCompatibility";
import { XTERM_PERFORMANCE_CONFIG } from "@/lib/xtermPerformance";
import type { TerminalFitScheduler } from "@/components/terminal/terminalFitScheduler";
import type { AppSettings } from "@/types/global";

type TerminalRendererPreference = "dom" | "webgl" | "auto";
type ResolvedTerminalRendererMode = "dom" | "webgl";

function isSnapshotRestoreActive(ref?: RefObject<boolean>) {
  return ref?.current === true;
}

function resolveTerminalRendererMode(options: {
  preference: TerminalRendererPreference;
  transparencyEnabled: boolean;
  webglCircuitBroken: boolean;
}): ResolvedTerminalRendererMode {
  if (options.preference === "dom") return "dom";
  if (options.transparencyEnabled || options.webglCircuitBroken) return "dom";
  if (options.preference === "webgl") return "webgl";

  return "dom";
}

export function useTerminalSettings(
  terminalRef: RefObject<Terminal | null>,
  fitSchedulerRef: RefObject<TerminalFitScheduler | null>,
  terminalThemeColors: TerminalColors,
  appearance: AppSettings["appearance"],
  terminalSettings: AppSettings["terminal"],
  interaction: AppSettings["interaction"],
  rendererVisible = true,
  terminalInstance: Terminal | null = null,
  sessionId?: string,
  snapshotRestoringRef?: RefObject<boolean>,
) {
  const webglAddonRef = useRef<WebglAddon | null>(null);
  const webglTerminalRef = useRef<Terminal | null>(null);
  const webglCircuitBrokenRef = useRef(false);
  const webglContextLossesRef = useRef<number[]>([]);
  const textureRefreshFrameRef = useRef<number | null>(null);
  const revealRefreshFrameRef = useRef<number | null>(null);
  const hiddenWebglDisposeTimerRef = useRef<number | null>(null);
  const contextLossRetryTimerRef = useRef<number | null>(null);
  const terminalTransparencyEnabled = isTerminalTransparencyEnabled(appearance);

  const cancelTextureRefresh = useCallback(() => {
    if (textureRefreshFrameRef.current !== null) {
      cancelAnimationFrame(textureRefreshFrameRef.current);
      textureRefreshFrameRef.current = null;
    }
  }, []);

  const disposeWebgl = useCallback(() => {
    if (webglAddonRef.current) {
      webglAddonRef.current.dispose();
      webglAddonRef.current = null;
    }
    webglTerminalRef.current = null;
  }, []);

  const clearHiddenWebglDisposeTimer = useCallback(() => {
    if (hiddenWebglDisposeTimerRef.current !== null) {
      window.clearTimeout(hiddenWebglDisposeTimerRef.current);
      hiddenWebglDisposeTimerRef.current = null;
    }
  }, []);

  const clearContextLossRetryTimer = useCallback(() => {
    if (contextLossRetryTimerRef.current !== null) {
      window.clearTimeout(contextLossRetryTimerRef.current);
      contextLossRetryTimerRef.current = null;
    }
  }, []);

  const cancelRevealRefresh = useCallback(() => {
    if (revealRefreshFrameRef.current !== null) {
      cancelAnimationFrame(revealRefreshFrameRef.current);
      revealRefreshFrameRef.current = null;
    }
  }, []);

  const scheduleTextureRefresh = useCallback(() => {
    if (isSnapshotRestoreActive(snapshotRestoringRef)) return;
    if (textureRefreshFrameRef.current !== null) return;
    textureRefreshFrameRef.current = requestAnimationFrame(() => {
      textureRefreshFrameRef.current = null;
      if (isSnapshotRestoreActive(snapshotRestoringRef)) return;
      const terminal = terminalRef.current;
      if (!terminal) return;
      terminal.clearTextureAtlas();
      terminal.refresh(0, Math.max(0, terminal.rows - 1));
    });
  }, [snapshotRestoringRef, terminalRef]);

  const scheduleRevealRefresh = useCallback(() => {
    cancelRevealRefresh();
    if (isSnapshotRestoreActive(snapshotRestoringRef)) return;
    let remainingFrames = XTERM_PERFORMANCE_CONFIG.webgl.revealRefreshFrames;
    const refreshNextFrame = () => {
      revealRefreshFrameRef.current = requestAnimationFrame(() => {
        if (isSnapshotRestoreActive(snapshotRestoringRef)) {
          revealRefreshFrameRef.current = null;
          return;
        }
        const terminal = terminalRef.current;
        if (terminal) {
          terminal.refresh(0, Math.max(0, terminal.rows - 1));
        }

        remainingFrames -= 1;
        if (remainingFrames > 0) {
          refreshNextFrame();
        } else {
          revealRefreshFrameRef.current = null;
        }
      });
    };
    refreshNextFrame();
  }, [cancelRevealRefresh, snapshotRestoringRef, terminalRef]);

  useEffect(() => {
    return () => {
      cancelTextureRefresh();
      cancelRevealRefresh();
      clearHiddenWebglDisposeTimer();
      clearContextLossRetryTimer();
      disposeWebgl();
    };
  }, [
    cancelRevealRefresh,
    cancelTextureRefresh,
    clearContextLossRetryTimer,
    clearHiddenWebglDisposeTimer,
    disposeWebgl,
  ]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: WebGL circuit breaker intentionally resets when the terminal instance or hardware acceleration setting changes.
  useEffect(() => {
    webglCircuitBrokenRef.current = false;
    webglContextLossesRef.current = [];
    clearContextLossRetryTimer();
  }, [clearContextLossRetryTimer, terminalInstance, terminalSettings.hardware_acceleration]);

  // React to hardware acceleration settings changes.
  useEffect(() => {
    const terminal = terminalInstance ?? terminalRef.current;
    if (!terminal) return;

    if (webglAddonRef.current && webglTerminalRef.current !== terminal) {
      disposeWebgl();
    }

    const rendererMode = resolveTerminalRendererMode({
      preference: terminalSettings.hardware_acceleration ? "webgl" : "dom",
      transparencyEnabled: terminalTransparencyEnabled,
      webglCircuitBroken: webglCircuitBrokenRef.current,
    });
    const shouldUseWebgl = rendererMode === "webgl";

    if (!shouldUseWebgl) {
      clearHiddenWebglDisposeTimer();
      clearContextLossRetryTimer();
      disposeWebgl();
      return;
    }

    if (!rendererVisible) {
      clearContextLossRetryTimer();
      if (hiddenWebglDisposeTimerRef.current === null && webglAddonRef.current) {
        hiddenWebglDisposeTimerRef.current = window.setTimeout(() => {
          hiddenWebglDisposeTimerRef.current = null;
          disposeWebgl();
        }, XTERM_PERFORMANCE_CONFIG.webgl.hiddenDisposeDelayMs);
      }
      return;
    }

    clearHiddenWebglDisposeTimer();

    const installWebgl = (targetTerminal: Terminal) => {
      try {
        const webgl = new WebglAddon();
        webgl.onContextLoss(() => {
          webgl.dispose();
          webglAddonRef.current = null;
          webglTerminalRef.current = null;
          const now = Date.now();
          const lossWindowMs = XTERM_PERFORMANCE_CONFIG.webgl.contextLossWindowMs;
          webglContextLossesRef.current = [...webglContextLossesRef.current, now].filter(
            (lossAt) => now - lossAt <= lossWindowMs,
          );

          if (
            webglContextLossesRef.current.length >
            XTERM_PERFORMANCE_CONFIG.webgl.contextLossCircuitBreakerLimit
          ) {
            webglCircuitBrokenRef.current = true;
            clearContextLossRetryTimer();
            return;
          }

          clearContextLossRetryTimer();
          contextLossRetryTimerRef.current = window.setTimeout(() => {
            contextLossRetryTimerRef.current = null;
            const retryTerminal = terminalInstance ?? terminalRef.current;
            if (!retryTerminal || !rendererVisible || webglCircuitBrokenRef.current) return;
            installWebgl(retryTerminal);
          }, XTERM_PERFORMANCE_CONFIG.webgl.contextLossRetryDelayMs);
        });
        targetTerminal.loadAddon(webgl);
        webglAddonRef.current = webgl;
        webglTerminalRef.current = targetTerminal;
        scheduleRevealRefresh();
      } catch {
        // Fallback to DOM renderer if WebGL initialization fails
        webglCircuitBrokenRef.current = true;
      }
    };

    if (webglAddonRef.current) {
      scheduleRevealRefresh();
    } else {
      installWebgl(terminal);
    }
  }, [
    terminalSettings.hardware_acceleration,
    terminalTransparencyEnabled,
    rendererVisible,
    terminalRef,
    terminalInstance,
    clearContextLossRetryTimer,
    clearHiddenWebglDisposeTimer,
    disposeWebgl,
    scheduleRevealRefresh,
  ]);
  // React to terminal theme changes: update terminal colors dynamically
  useEffect(() => {
    if (terminalRef.current) {
      terminalRef.current.options.theme = { ...terminalThemeColors };
      scheduleTextureRefresh();
    }
  }, [terminalThemeColors, terminalRef, scheduleTextureRefresh]);

  // React to appearance settings changes: font family, size, cursor etc
  useEffect(() => {
    if (terminalRef.current) {
      const options = terminalRef.current.options;
      options.fontFamily = appearance.font_family;
      options.fontSize = resolveTerminalFontSize(
        appearance.font_size,
        terminalSettings.font_size_delta,
      );
      options.fontWeight = appearance.font_weight;
      options.fontWeightBold = appearance.font_weight_bold;
      options.cursorBlink = appearance.cursor_blink;
      options.cursorStyle = appearance.cursor_style as "block" | "underline" | "bar";
      options.minimumContrastRatio = appearance.minimum_contrast_ratio;
      scheduleTextureRefresh();

      // Auto-fit on font size change
      if (!isSnapshotRestoreActive(snapshotRestoringRef)) {
        fitSchedulerRef.current?.schedule({
          reason: "appearance",
          force: true,
          refresh: true,
          clearTextureAtlas: true,
        });
      }
    }
  }, [
    appearance,
    terminalSettings.font_size_delta,
    terminalRef,
    fitSchedulerRef,
    scheduleTextureRefresh,
    snapshotRestoringRef,
  ]);

  // React to terminal core settings changes: scrollback
  useEffect(() => {
    if (terminalRef.current) {
      terminalRef.current.options.scrollback = terminalSettings.scrollback_lines;
    }
  }, [terminalSettings.scrollback_lines, terminalRef]);

  // React to interaction settings changes
  useEffect(() => {
    if (terminalRef.current) {
      terminalRef.current.options.wordSeparator = interaction.word_separators;
      terminalRef.current.options.macOptionIsMeta = interaction.alt_as_meta;
    }
  }, [interaction.word_separators, interaction.alt_as_meta, terminalRef]);

  useEffect(() => {
    const terminal = terminalInstance ?? terminalRef.current;
    if (!terminal) return;

    const patch = installImeCompatibilityPatch(terminal, interaction.ime_compatibility, sessionId);
    return () => patch.dispose();
  }, [interaction.ime_compatibility, terminalInstance, terminalRef, sessionId]);
}
