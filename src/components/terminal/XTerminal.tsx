import { listen } from "@tauri-apps/api/event";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { SerializeAddon } from "@xterm/addon-serialize";
import { UnicodeGraphemesAddon } from "@xterm/addon-unicode-graphemes";
import { Terminal } from "@xterm/xterm";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import MultiLinePasteDialog from "@/components/dialog/terminal/MultiLinePasteDialog";
import ExternalFileDropOverlay from "@/components/ExternalFileDropOverlay";
import { useTerminalAppSettings } from "@/context/AppContext";
import { useTheme } from "@/context/ThemeContext";
import { useTransfer } from "@/context/TransferContext";
import { useActionLinks } from "@/hooks/useActionLinks";
import { useCommandHistory } from "@/hooks/useCommandHistory";
import { useCredentialAutofill } from "@/hooks/useCredentialAutofill";
import { useKeywordHighlighter } from "@/hooks/useKeywordHighlighter";
import { useShellIntegration } from "@/hooks/useShellIntegration";
import { useTerminalSearch } from "@/hooks/useTerminalSearch";
import { useTerminalSettings } from "@/hooks/useTerminalSettings";
import {
  renderAiCommandEnd,
  renderAiCommandStart,
} from "@/lib/aiTerminalRenderer";
import { createAsyncUnlistenBag } from "@/lib/asyncUnlistenBag";
import {
  buildTerminalThemeColors,
  isTerminalTransparencyEnabled,
} from "@/lib/backgroundImage";
import {
  readClipboardPathPayload,
  readClipboardText,
  uploadClipboardImageToSsh,
  writeClipboardText,
} from "@/lib/clipboard";
import {
  commandStartsSuggestionSuppressingProgram,
  isPagerSearchOrCommandInput,
  isPagerSingleKeyInput,
} from "@/lib/commandSuggestionSuppression";
import { detectCredentialPromptKind } from "@/lib/credentialAutofill";
import { invoke } from "@/lib/invoke";
import { hexLuminance } from "@/lib/keywordHighlightPresets";
import { logger } from "@/lib/logger";
import { isMacOS, isWindows } from "@/lib/platform";
import { openSendCommandPanel } from "@/lib/sendCommandPanelEvents";
import {
  buildTerminalCommandInput,
  listenSessionInputPreview,
  normalizeTerminalCommandInput,
  type SendSessionInputOptions,
  type SessionInputPreview,
  sendSessionInput,
  sendSessionInputWithSync,
} from "@/lib/sessionInput";
import { registerTerminalContextProvider } from "@/lib/terminalContext";
import { resolveTerminalFontSize } from "@/lib/terminalFontSize";
import {
  applyTerminalInputData,
  applyTerminalInputPreview,
  canSuggestFromTracker,
  createTerminalInputState,
  deleteTerminalInputRange,
  getTrackedSubmissionCommand,
  resyncFromTerminalLine,
} from "@/lib/terminalInputTracker";
import {
  consumePreservedTerminalReconnectContent,
  registerTerminalReconnectCapture,
  type TerminalReconnectSnapshot,
} from "@/lib/terminalReconnectHistory";
import {
  buildReconnectCwdStartupCommand,
  carryOverSessionCwd,
  recordSessionCwd,
} from "@/lib/terminalSessionCwd";
import { buildStartupCommandPayload } from "@/lib/appSessionFactory";
import { TERMINAL_SEARCH_VISIBLE_MATCH_LIMIT } from "@/lib/terminalSearch";
import type { AiCaptureEvent } from "@/types/global";
import ActionLinkMenu from "./ActionLinkMenu";
import ActionLinkTooltip from "./ActionLinkTooltip";
import CommandSuggestions from "./CommandSuggestions";
import CredentialSuggestions from "./CredentialSuggestions";
import { installRemoteColorOscGuard } from "./remoteColorOscGuard";
import SyncActionOverlay from "./SyncActionOverlay";
import TerminalContextMenu from "./TerminalContextMenu";
import TerminalGutter from "./TerminalGutter";
import TerminalSearchBar from "./TerminalSearchBar";
import {
  createTerminalFitScheduler,
  type TerminalFitResult,
  type TerminalFitScheduler,
  TerminalResizeDeduper,
} from "./terminalFitScheduler";
import { installTerminalImageAddon } from "./terminalImageAddon";
import {
  getSelectedInputRange,
  type InputSelectionRange,
  isMultiLineText,
  readCurrentInputLine,
  readRecentOutput,
} from "./terminalInputSelection";
import { createTerminalLinkHandlers } from "./terminalLinkHandlers";
import type { TerminalOutputDrain } from "./terminalOutputDrain";
import { AlternateScreenStateTracker } from "./alternateScreenStateTracker";
import type { Dec2026FrameGate } from "./dec2026FrameGate";
import { useTerminalExternalDrop } from "./useTerminalExternalDrop";
import { useTerminalRefreshEffects } from "./useTerminalRefreshEffects";
import {
  buildClipboardPathPasteText,
  decodeOsc52ClipboardText,
  quotePosixPath,
} from "./xterminalClipboard";
import { createXTerminalHibernationController } from "./xterminalHibernationController";
import { installXTerminalKeyboardController } from "./xterminalKeyboardController";
import { createXTerminalOutputController } from "./xterminalOutputController";
import { installXTerminalSelectionController } from "./xterminalSelectionController";
import { createXTerminalSessionEvents } from "./xterminalSessionEvents";
import { createXTerminalSnapshotRestoreController } from "./xterminalSnapshotRestoreController";
import type {
  HibernationLogEvent,
  HibernationPhase,
  PendingWakeEvent,
  XTermInternalTrimSource,
} from "./xterminalInternalTypes";
import {
  createXTerminalDataOriginTracker,
  isSessionNotFoundError,
  resolveXTerminalDataOrigin,
} from "./xterminalKeyboardInput";
import {
  markTerminalUserInput,
  registerTerminalUserInputMarker,
} from "@/lib/terminalControlInput";
import { XTERM_SECURE_WINDOW_OPTIONS } from "@/lib/xtermSecurity";
import {
  publishApplicationTitle,
  resumeDynamicTitlePublication,
} from "@/lib/dynamicTabTitles";
import {
  serializeTerminalSnapshot,
  writeTextInFrames,
} from "./xterminalOutputQueue";
import type { PerformanceMode, XTerminalProps } from "./xterminalTypes";
import { shouldSuspendKeywordHighlighter } from "./xterminalKeywordHighlighting";
import {
  createZmodemEventHandler,
  type ZmodemEventPayload,
} from "./zmodemTerminalEvents";
import "@xterm/xterm/css/xterm.css";

type SearchAddonWithLifecycle = SearchAddon & {
  onBeforeSearch?: (listener: () => void) => { dispose: () => void };
  onAfterSearch?: (listener: () => void) => { dispose: () => void };
};

/**
 * xterm.js terminal for a session. Handles OSC 133 shell integration (or fallback prompt
 * detection), fuzzy command history suggestions, and resize/fit. Key props: sessionId, active.
 */
export default function XTerminal({
  sessionId,
  sessionName,
  active,
  visible = true,
  sessionType,
  connectionId,
  temporaryConfig,
  onReconnected,
  onDisconnectedCloseRequested,
  onConnectionError,
  syncPeerSessionIds,
  syncOverlay,
  recordingStatus,
  onToggleRecording,
  onSaveTranscript,
}: XTerminalProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const fitSchedulerRef = useRef<TerminalFitScheduler | null>(null);
  const resizeDeduperRef = useRef(new TerminalResizeDeduper());
  const [terminalInstance, setTerminalInstance] = useState<Terminal | null>(
    null,
  );
  const [terminalReady, setTerminalReady] = useState(false);
  const [restoringSnapshot, setRestoringSnapshot] = useState(false);
  const restoringSnapshotRef = useRef(false);
  const [snapshotRestoreController] = useState(() =>
    createXTerminalSnapshotRestoreController({
      restoringRef: restoringSnapshotRef,
      setRestoring: setRestoringSnapshot,
      setTerminalReady,
    }),
  );
  const [performanceMode, setPerformanceMode] =
    useState<PerformanceMode>("normal");
  const [terminalGeneration, setTerminalGeneration] = useState(0);
  const [hibernated, setHibernated] = useState(false);
  const [multiLinePasteText, setMultiLinePasteText] = useState<string | null>(
    null,
  );
  const aiCapturingRef = useRef(false);

  const { terminalTheme } = useTheme();
  const { t } = useTranslation();
  const {
    upsertExternalTransferProgress,
    completeExternalTransfer,
    failExternalTransfer,
  } = useTransfer();
  const terminalAppSettings = useTerminalAppSettings();
  const {
    appearance,
    interaction,
    terminal: terminalSettings,
  } = terminalAppSettings;
  const terminalThemeColors = useMemo(
    () => buildTerminalThemeColors(terminalTheme.colors.terminal, appearance),
    [appearance, terminalTheme.colors.terminal],
  );
  const terminalTransparencyEnabled = isTerminalTransparencyEnabled(appearance);
  const terminalLifecycleStateRef = useRef({
    sessionId,
    terminalTransparencyEnabled,
  });
  terminalLifecycleStateRef.current = {
    sessionId,
    terminalTransparencyEnabled,
  };
  const showLineNumbers = terminalSettings.show_line_numbers;
  const showTimestamps = terminalSettings.show_timestamps;
  const timestampFormat = terminalSettings.timestamp_format ?? "[HH:mm:ss]";
  const showWorkspacePadding = terminalSettings.show_workspace_padding ?? false;
  const showGutter = showLineNumbers || showTimestamps;
  const showContentPadding = showWorkspacePadding;
  const commandSuggestionsEnabled = interaction.command_suggestions_enabled;
  const commandSuggestionMinChars = interaction.command_suggestion_min_chars;
  const commandSuggestionMaxChars = interaction.command_suggestion_max_chars;

  const inputStateRef = useRef(createTerminalInputState());
  const terminalAppSettingsRef = useRef(terminalAppSettings);
  const tRef = useRef(t);
  const doFindRef = useRef<(selection?: string) => void>(() => {});
  const pasteTextRef = useRef<
    (text: string, options?: { skipDialog?: boolean }) => void
  >(() => {});
  const clearAllRef = useRef<() => void>(() => {});
  const pendingSearchSelectionRef = useRef(false);
  const searchSelectionTextRef = useRef<string | null>(null);
  const disconnectedRef = useRef(false);
  const disconnectedNoticeShownRef = useRef(false);
  const disconnectedCloseRequestedRef = useRef(false);
  const reconnectingRef = useRef(false);
  const preservedReconnectContentRef = useRef<TerminalReconnectSnapshot | null>(
    null,
  );
  const hibernationSnapshotRef = useRef<TerminalReconnectSnapshot | null>(null);
  const hibernateTimerRef = useRef<number | null>(null);
  const hibernationCleanupRef = useRef(false);
  const hibernationPendingRef = useRef(false);
  const hibernationPhaseRef = useRef<HibernationPhase>("idle");
  const hibernationEpochRef = useRef(0);
  const detachedHibernateEpochRef = useRef<number | null>(null);
  const lastOutputActivityAtRef = useRef(Date.now());
  const hibernatedRef = useRef(hibernated);
  const pendingWakeEventsRef = useRef<PendingWakeEvent[]>([]);
  const zmodemActiveRef = useRef(false);
  const outputDrainRef = useRef<TerminalOutputDrain<{
    beforeLine: number;
    ts: number;
  }> | null>(null);
  const frameGateRef = useRef<Dec2026FrameGate | null>(null);
  const lineTimestampsRef = useRef<Map<number, number>>(new Map());
  const gutterLineOffsetRef = useRef(0);
  const sessionTypeRef = useRef(sessionType);
  const connectionIdRef = useRef(connectionId);
  const temporaryConfigRef = useRef(temporaryConfig);
  const onReconnectedRef = useRef(onReconnected);
  const onDisconnectedCloseRequestedRef = useRef(onDisconnectedCloseRequested);
  const onConnectionErrorRef = useRef(onConnectionError);
  const sessionIdRef = useRef(sessionId);
  const syncPeerSessionIdsRef = useRef(syncPeerSessionIds);
  const visibleRef = useRef(visible);
  const activeRef = useRef(active);
  const performanceModeRef = useRef<PerformanceMode>("normal");
  const alternateScreenTrackerRef = useRef(new AlternateScreenStateTracker());
  const handleVisibilityChangeRef = useRef<(() => void) | null>(null);
  const replaceInputCommandRef = useRef<((command: string) => void) | null>(
    null,
  );
  const lastErrorNoticeAtRef = useRef(0);
  const credentialPromptBufferRef = useRef("");
  const credentialPromptInputUntilRef = useRef(0);
  const commandSuggestionSuppressedRef = useRef(false);

  const clearSearchSelectionState = useCallback(() => {
    pendingSearchSelectionRef.current = false;
    searchSelectionTextRef.current = null;
  }, []);

  const logHibernation = useCallback(
    (
      event: HibernationLogEvent,
      message: string,
      data: Record<string, unknown> = {},
      error?: unknown,
    ) => {
      logger.debug({
        domain: "terminal.input",
        event: `terminal.hibernate.${event}`,
        message,
        ids: { session_id: sessionIdRef.current },
        data: {
          session_type: sessionTypeRef.current,
          phase: hibernationPhaseRef.current,
          epoch: hibernationEpochRef.current,
          ...data,
        },
        error,
      });
    },
    [],
  );

  const clearHibernateTimer = useCallback(() => {
    if (hibernateTimerRef.current !== null) {
      window.clearTimeout(hibernateTimerRef.current);
      hibernateTimerRef.current = null;
    }
  }, []);

  const invalidateHibernation = useCallback(
    (reason: string) => {
      clearHibernateTimer();
      hibernationEpochRef.current += 1;
      if (hibernationPhaseRef.current !== "idle") {
        logHibernation("cancel", "Cancelled terminal hibernation", { reason });
      }
    },
    [clearHibernateTimer, logHibernation],
  );

  const requestWake = useCallback(
    (reason: string) => {
      clearHibernateTimer();
      hibernationEpochRef.current += 1;

      if (hibernationPhaseRef.current !== "idle") {
        hibernationPhaseRef.current = "waking";
        logHibernation("wake", "Waking hibernated terminal renderer", {
          reason,
        });
      }

      if (hibernatedRef.current) {
        hibernatedRef.current = false;
        setHibernated(false);
        setTerminalGeneration((generation) => generation + 1);
      }
    },
    [clearHibernateTimer, logHibernation],
  );

  const pasteClipboard = useCallback(async () => {
    const pasteImageAsPathEnabled =
      terminalAppSettingsRef.current?.terminal?.paste_image_as_path ?? true;
    const currentSessionType = sessionTypeRef.current;

    if (pasteImageAsPathEnabled && currentSessionType === "Local") {
      try {
        const payload = await readClipboardPathPayload();
        const pathText = buildClipboardPathPasteText(payload);
        if (pathText) {
          pasteTextRef.current(pathText, { skipDialog: true });
          return;
        }
      } catch {
        /* fall back to text clipboard */
      }
    }

    if (pasteImageAsPathEnabled && currentSessionType === "SSH") {
      try {
        const payload = await uploadClipboardImageToSsh(sessionIdRef.current);
        if (payload?.remote_path) {
          const quotedRemotePath = quotePosixPath(payload.remote_path);
          await sendSessionInput(sessionIdRef.current, quotedRemotePath, {
            preview: { kind: "data", data: quotedRemotePath },
            registerSubmission: null,
          });
          return;
        }
      } catch {
        /* fall back to text clipboard */
      }
    }

    const text = await readClipboardText();
    pasteTextRef.current(text);
  }, []);

  const getGutterLineOffset = useCallback(
    () => gutterLineOffsetRef.current,
    [],
  );

  useEffect(() => {
    sessionTypeRef.current = sessionType;
  }, [sessionType]);

  useEffect(() => {
    connectionIdRef.current = connectionId;
  }, [connectionId]);

  useEffect(() => {
    temporaryConfigRef.current = temporaryConfig;
  }, [temporaryConfig]);

  useEffect(() => {
    onReconnectedRef.current = onReconnected;
  }, [onReconnected]);

  useEffect(() => {
    onDisconnectedCloseRequestedRef.current = onDisconnectedCloseRequested;
  }, [onDisconnectedCloseRequested]);

  useEffect(() => {
    onConnectionErrorRef.current = onConnectionError;
  }, [onConnectionError]);

  useEffect(() => {
    syncPeerSessionIdsRef.current = syncPeerSessionIds;
  }, [syncPeerSessionIds]);

  useEffect(() => {
    activeRef.current = active;
  }, [active]);

  useEffect(() => {
    sessionIdRef.current = sessionId;
  }, [sessionId]);

  useEffect(() => {
    hibernatedRef.current = hibernated;
  }, [hibernated]);

  useEffect(() => {
    visibleRef.current = visible;
    if (visible) {
      requestWake("visible");
    }
    handleVisibilityChangeRef.current?.();
  }, [requestWake, visible]);

  useEffect(() => {
    if (!hibernated) return;

    let disposed = false;
    const unlistenBag = createAsyncUnlistenBag();
    const wake = (event: PendingWakeEvent) => {
      pendingWakeEventsRef.current.push(event);
      if (disposed) return;
      requestWake(event.type);
    };

    const setupWakeListeners = () => {
      unlistenBag.add(
        listen<string>(`session-error-${sessionId}`, (event) => {
          wake({
            type: "error",
            message: String(
              event.payload || tRef.current("terminal.connectionFailed"),
            ),
          });
        }),
      );
      unlistenBag.add(
        listen<void>(`session-closed-${sessionId}`, () => {
          wake({ type: "closed" });
        }),
      );
      unlistenBag.add(
        listen<ZmodemEventPayload>(`zmodem-event-${sessionId}`, (event) => {
          wake({ type: "zmodem", payload: event.payload });
        }),
      );
      unlistenBag.add(
        listen<AiCaptureEvent>(`ai-capture-${sessionId}`, (event) => {
          wake({ type: "ai", payload: event.payload });
        }),
      );
      unlistenBag.add(
        listen<void>(`focus-terminal-${sessionId}`, () => {
          wake({ type: "focus" });
        }),
      );
    };

    setupWakeListeners();

    return () => {
      disposed = true;
      unlistenBag.dispose();
    };
  }, [hibernated, requestWake, sessionId]);

  useEffect(() => {
    let disposed = false;
    let cwdEventObserved = false;
    const unlisten = listen<string>(`cwd-changed-${sessionId}`, (event) => {
      cwdEventObserved = true;
      recordSessionCwd(sessionId, event.payload);
    });
    void unlisten.then(() => {
      if (disposed) return;
      void invoke<string | null>("try_get_terminal_cwd", { sessionId }).then(
        (cwd) => {
          if (disposed || cwdEventObserved) return;
          // A successful null snapshot is authoritative and clears a carried cwd.
          recordSessionCwd(sessionId, cwd ?? "");
        },
        () => {},
      );
    });
    return () => {
      disposed = true;
      void unlisten.then((fn) => fn());
    };
  }, [sessionId]);

  useEffect(() => {
    return () => {
      clearHibernateTimer();
      hibernationEpochRef.current += 1;
      const detachedEpoch = detachedHibernateEpochRef.current;
      if (detachedEpoch === null) return;

      void invoke("attach_session", { sessionId: sessionIdRef.current })
        .then(() => {
          if (detachedHibernateEpochRef.current === detachedEpoch) {
            detachedHibernateEpochRef.current = null;
          }
          hibernationPhaseRef.current = "idle";
          resumeDynamicTitlePublication(sessionIdRef.current);
          logHibernation(
            "rollback",
            "Rolled back detached renderer on component unmount",
            {
              reason: "unmount",
              epoch: detachedEpoch,
            },
          );
        })
        .catch((error) => {
          hibernationPhaseRef.current = "failed";
          resumeDynamicTitlePublication(sessionIdRef.current);
          logHibernation(
            "fail",
            "Failed to roll back detached renderer on component unmount",
            { reason: "unmount", epoch: detachedEpoch },
            error,
          );
        });
    };
  }, [clearHibernateTimer, logHibernation]);

  useEffect(() => {
    terminalAppSettingsRef.current = terminalAppSettings;
  }, [terminalAppSettings]);

  useEffect(() => {
    tRef.current = t;
  }, [t]);

  const enterOverloadedMode = useCallback(() => {
    performanceModeRef.current = "overloaded";
    setPerformanceMode("overloaded");
  }, []);

  const setOutputPressureMode = useCallback((mode: PerformanceMode) => {
    if (performanceModeRef.current === mode) return;
    performanceModeRef.current = mode;
    setPerformanceMode(mode);
  }, []);

  const exitOverloadedMode = useCallback(
    (nextMode: PerformanceMode = "normal") => {
      performanceModeRef.current = nextMode;
      setPerformanceMode(nextMode);
    },
    [],
  );

  // Search Addon state and handlers
  const {
    registerSearchAddon,
    showSearchBar,
    setShowSearchBar,
    searchQuery,
    setSearchQuery,
    searchState,
    searchFlags,
    setSearchFlag,
    activeMode,
    setActiveMode,
    historyState,
    handleSearchNext,
    handleSearchPrev,
    handleCloseSearch,
  } = useTerminalSearch(terminalRef, {
    terminal: terminalInstance,
    sessionId,
    visible: visible && active,
    performanceMode: performanceMode === "strained" ? "busy" : performanceMode,
  });

  // Shell integration state
  const { shellIntegrationRef } = useShellIntegration();
  const canShowCommandSuggestions = useCallback(
    (options?: { allowEmpty?: boolean }) => {
      if (credentialPromptInputUntilRef.current > Date.now()) {
        return false;
      }
      credentialPromptInputUntilRef.current = 0;

      const terminal = terminalRef.current;
      if (terminal?.buffer.active.type === "alternate") {
        return false;
      }

      const shellIntegration = shellIntegrationRef.current;
      if (shellIntegration.enabled && shellIntegration.commandRunning) {
        return false;
      }

      const inputState = inputStateRef.current;
      if (commandSuggestionSuppressedRef.current) {
        return false;
      }
      if (isPagerSearchOrCommandInput(inputState.value)) {
        return false;
      }

      if (options?.allowEmpty) {
        return (
          !inputState.desynced &&
          !inputState.multiline &&
          inputState.cursor === inputState.value.length
        );
      }

      return canSuggestFromTracker(inputState);
    },
    [shellIntegrationRef],
  );

  const applySuggestion = useCallback(
    (command: string, execute: boolean) => {
      const trackedState = inputStateRef.current;
      const replaceCurrentLine = trackedState.lineRewriteRequired;
      const input = replaceCurrentLine
        ? `\u0005\u0015${command}`
        : `${"\x7f".repeat(trackedState.value.length)}${command}`;
      const data = buildTerminalCommandInput(input, execute);
      const options: SendSessionInputOptions = {
        preview: execute
          ? { kind: "replace-and-execute", value: command }
          : { kind: "replace", value: command },
        registerSubmission: execute ? command : null,
      };
      const peers = syncPeerSessionIdsRef.current ?? [];
      const sendInput =
        peers.length > 0
          ? sendSessionInputWithSync(sessionId, data, peers, options)
          : sendSessionInput(sessionId, data, options);
      void sendInput.catch(() => {});
      if (execute && commandStartsSuggestionSuppressingProgram(command)) {
        commandSuggestionSuppressedRef.current = true;
      }
    },
    [sessionId],
  );

  // Command history & fuzzy search UI
  const {
    suggestions,
    selectedIndex,
    setSelectedIndex,
    showSuggestions,
    cursorPosition,
    suggestionsRef,
    selectedIndexRef,
    showSuggestionsRef,
    searchTimerRef,
    triggerSearch,
    dismissSuggestions,
    handleSelectSuggestion,
    handleDeleteSuggestion,
  } = useCommandHistory(
    terminalRef,
    inputStateRef,
    applySuggestion,
    canShowCommandSuggestions,
    commandSuggestionsEnabled,
    commandSuggestionMinChars,
    commandSuggestionMaxChars,
  );

  const canDetectCredentialPrompt = useCallback(() => {
    const terminal = terminalRef.current;
    if (terminal?.buffer.active.type === "alternate") {
      return false;
    }

    const inputState = inputStateRef.current;
    return (
      inputState.value.length === 0 &&
      !inputState.desynced &&
      !inputState.lineRewriteRequired &&
      !inputState.multiline &&
      !inputState.pasteMode
    );
  }, []);

  const {
    panelState: credentialPanelState,
    selectedIndex: credentialSelectedIndex,
    setSelectedIndex: setCredentialSelectedIndex,
    cursorPosition: credentialCursorPosition,
    showPanelRef: credentialShowPanelRef,
    matchesRef: credentialMatchesRef,
    selectedIndexRef: credentialSelectedIndexRef,
    feedOutput: feedCredentialOutput,
    handleSelect: handleCredentialSelect,
    dismiss: dismissCredentialPanel,
    reset: resetCredentialAutofill,
  } = useCredentialAutofill(
    terminalRef,
    sessionIdRef,
    activeRef,
    visibleRef,
    performanceModeRef,
    canDetectCredentialPrompt,
  );

  // Create and setup terminal
  // biome-ignore lint/correctness/useExhaustiveDependencies: terminal lifecycle is intentionally scoped to session changes.
  useEffect(() => {
    if (hibernated) {
      fitSchedulerRef.current?.dispose();
      fitSchedulerRef.current = null;
      setTerminalReady(false);
      terminalRef.current = null;
      setTerminalInstance(null);
      fitAddonRef.current = null;
      return;
    }
    if (!containerRef.current) return;
    setTerminalReady(false);
    lineTimestampsRef.current = new Map();
    gutterLineOffsetRef.current = 0;
    frameGateRef.current?.dispose({
      ackRemaining: true,
      reason: "terminal_rebuild",
    });
    frameGateRef.current = null;
    outputDrainRef.current?.dispose({ ackRemaining: true });
    outputDrainRef.current = null;
    alternateScreenTrackerRef.current.reset();
    disconnectedRef.current = false;
    disconnectedNoticeShownRef.current = false;
    disconnectedCloseRequestedRef.current = false;
    reconnectingRef.current = false;
    performanceModeRef.current = "normal";
    resetCredentialAutofill();
    setPerformanceMode("normal");
    let disposed = false;

    const preservedReconnectSnapshot =
      hibernationSnapshotRef.current ??
      preservedReconnectContentRef.current ??
      consumePreservedTerminalReconnectContent(sessionId);
    const restoringInitialSnapshot = snapshotRestoreController.begin(
      preservedReconnectSnapshot,
    );
    hibernationSnapshotRef.current = null;
    preservedReconnectContentRef.current = null;

    const terminal = new Terminal({
      scrollback: terminalSettings.scrollback_lines,
      cursorBlink: appearance.cursor_blink,
      cursorStyle: appearance.cursor_style as "block" | "underline" | "bar",
      fontSize: resolveTerminalFontSize(
        appearance.font_size,
        terminalSettings.font_size_delta,
      ),
      fontFamily: appearance.font_family,
      fontWeight: appearance.font_weight,
      fontWeightBold: appearance.font_weight_bold,
      minimumContrastRatio: appearance.minimum_contrast_ratio,
      wordSeparator: interaction.word_separators,
      macOptionIsMeta: interaction.alt_as_meta,
      scrollOnEraseInDisplay: true,
      theme: { ...terminalThemeColors },
      allowTransparency: terminalTransparencyEnabled,
      allowProposedApi: true,
      windowOptions: XTERM_SECURE_WINDOW_OPTIONS,
      vtExtensions: { kittyKeyboard: true },
    });

    const fitAddon = new FitAddon();
    const {
      oscLinkHandler,
      webLinksAddon,
      removePopup: removeLinkPopup,
    } = createTerminalLinkHandlers(terminal, tRef);
    const searchAddon = new SearchAddon({
      highlightLimit: TERMINAL_SEARCH_VISIBLE_MATCH_LIMIT,
    }) as SearchAddonWithLifecycle;
    const searchLifecycleDisposables = [
      searchAddon.onBeforeSearch?.(() => {
        pendingSearchSelectionRef.current = true;
        searchSelectionTextRef.current = null;
      }),
      searchAddon.onAfterSearch?.(() => {
        window.setTimeout(() => {
          pendingSearchSelectionRef.current = false;
        }, 0);
      }),
    ].filter((disposable): disposable is { dispose: () => void } =>
      Boolean(disposable),
    );
    const serializeAddon = new SerializeAddon();
    const unicodeGraphemesAddon = new UnicodeGraphemesAddon();
    let writeOrderedTerminalStatus = (data: string) => {
      terminal.write(data);
    };
    const zmodemHandler = createZmodemEventHandler(
      terminal,
      sessionId,
      () => tRef.current,
      () =>
        terminalAppSettingsRef.current?.transfer.duplicate_strategy ?? "ask",
      {
        upsertProgress: upsertExternalTransferProgress,
        complete: completeExternalTransfer,
        fail: failExternalTransfer,
      },
      (data) => {
        writeOrderedTerminalStatus(data);
      },
    );

    terminal.options.linkHandler = oscLinkHandler;
    terminal.loadAddon(fitAddon);
    terminal.loadAddon(webLinksAddon);
    terminal.loadAddon(searchAddon);
    terminal.loadAddon(serializeAddon);
    terminal.loadAddon(unicodeGraphemesAddon);
    installTerminalImageAddon(terminal, { sessionId, sessionType });
    terminal.open(containerRef.current);

    const coreService = (terminal as Terminal & XTermInternalTrimSource)._core
      ?.coreService;
    const canDistinguishTerminalResponses =
      typeof coreService?.onUserInput === "function";
    const inputOriginTracker = createXTerminalDataOriginTracker();
    const userInputDisposable = coreService?.onUserInput?.(() => {
      inputOriginTracker.markUserInput();
    });
    const unregisterUserInputMarker = registerTerminalUserInputMarker(
      terminal,
      () => inputOriginTracker.markUserInput(),
    );
    let fallbackFocusReport: "\x1b[I" | "\x1b[O" | null = null;
    const fallbackUserInputEvents = [
      "keydown",
      "beforeinput",
      "paste",
      "compositionend",
      "pointerdown",
      "pointermove",
      "pointerup",
      "wheel",
      "drop",
    ] as const;
    const markFallbackUserInput = (event: Event) => {
      if (
        typeof PointerEvent !== "undefined" &&
        event instanceof PointerEvent &&
        event.type === "pointermove" &&
        event.buttons === 0
      ) {
        return;
      }
      if (
        event.type === "compositionend" ||
        (typeof KeyboardEvent !== "undefined" &&
          event instanceof KeyboardEvent &&
          event.keyCode === 229)
      ) {
        // xterm defers IME data with setTimeout(0), beyond this DOM event.
        inputOriginTracker.markDeferredUserInput();
      }
      inputOriginTracker.beginUserInputEvent();
      queueMicrotask(() => inputOriginTracker.endUserInputEvent());
    };
    const markFallbackFocusReport = (event: Event) => {
      fallbackFocusReport = event.type === "focusin" ? "\x1b[I" : "\x1b[O";
      queueMicrotask(() => {
        fallbackFocusReport = null;
      });
    };
    if (!canDistinguishTerminalResponses) {
      for (const eventName of fallbackUserInputEvents) {
        containerRef.current.addEventListener(
          eventName,
          markFallbackUserInput,
          true,
        );
      }
      containerRef.current.addEventListener(
        "focusin",
        markFallbackFocusReport,
        true,
      );
      containerRef.current.addEventListener(
        "focusout",
        markFallbackFocusReport,
        true,
      );
    }

    const flushPendingDynamicTitle = () => {
      resumeDynamicTitlePublication(sessionId);
    };
    const titleDisposable = terminal.onTitleChange((title) => {
      publishApplicationTitle(sessionId, title);
    });

    const trimDisposable = (
      terminal as Terminal & XTermInternalTrimSource
    )._core?._bufferService?.buffers?.normal?.lines?.onTrim?.((amount) => {
      if (amount <= 0) return;
      gutterLineOffsetRef.current += amount;
    });

    registerSearchAddon(searchAddon);

    terminalRef.current = terminal;
    setTerminalInstance(terminal);
    fitAddonRef.current = fitAddon;
    inputStateRef.current = createTerminalInputState();
    credentialPromptBufferRef.current = "";
    credentialPromptInputUntilRef.current = 0;
    shellIntegrationRef.current.enabled = false;
    shellIntegrationRef.current.commandRunning = false;

    const getCurrentAbsoluteLine = () =>
      gutterLineOffsetRef.current +
      terminal.buffer.active.baseY +
      terminal.buffer.active.cursorY;

    const requestGutterRefresh = () => {
      if (disposed || terminalRef.current !== terminal) return;
      if (performanceModeRef.current !== "normal") return;
      const terminalSettings = terminalAppSettingsRef.current?.terminal;
      if (
        !terminalSettings?.show_line_numbers &&
        !terminalSettings?.show_timestamps
      )
        return;
      window.dispatchEvent(
        new CustomEvent("nyaterm:refresh-gutter", { detail: { sessionId } }),
      );
    };

    const captureReconnectSnapshot = (
      contentSuffix = "",
    ): TerminalReconnectSnapshot => {
      const serialized = serializeTerminalSnapshot(terminal, serializeAddon);
      const captureStartLine =
        gutterLineOffsetRef.current + serialized.captureStartLine;
      const captureEndLine =
        gutterLineOffsetRef.current + serialized.captureEndLine;
      const lineTimestamps: Array<[number, number]> = [];

      for (const [line, timestamp] of lineTimestampsRef.current) {
        if (
          line >= captureStartLine &&
          line <= captureEndLine &&
          Number.isFinite(line) &&
          Number.isFinite(timestamp)
        ) {
          lineTimestamps.push([line, timestamp]);
        }
      }

      return {
        content: `${serialized.content}${contentSuffix}`,
        lineTimestamps,
        captureStartLine,
        captureEndLine,
      };
    };

    const restoreLineTimestampsFromSnapshot = (
      snapshot: TerminalReconnectSnapshot,
    ) => {
      const map = lineTimestampsRef.current;
      map.clear();

      if (snapshot.lineTimestamps.length === 0) return;

      const restoredEndLine = getCurrentAbsoluteLine();
      const lineDelta = restoredEndLine - snapshot.captureEndLine;
      const minLine = gutterLineOffsetRef.current;
      const maxLine = restoredEndLine;

      for (const [line, timestamp] of snapshot.lineTimestamps) {
        const restoredLine = line + lineDelta;
        if (
          Number.isFinite(restoredLine) &&
          Number.isFinite(timestamp) &&
          restoredLine >= minLine &&
          restoredLine <= maxLine
        ) {
          map.set(restoredLine, timestamp);
        }
      }
    };

    const initialReplayPromise = preservedReconnectSnapshot?.content
      ? writeTextInFrames(terminal, preservedReconnectSnapshot.content).then(
          () => {
            restoreLineTimestampsFromSnapshot(preservedReconnectSnapshot);
            requestGutterRefresh();
          },
        )
      : Promise.resolve();
    const unregisterReconnectCapture = registerTerminalReconnectCapture(
      sessionId,
      () => captureReconnectSnapshot(),
    );
    const isTerminalAlive = () => !disposed && terminalRef.current === terminal;
    const syncSuggestionsWithInputState = () => {
      if (canShowCommandSuggestions()) {
        triggerSearch();
      } else {
        dismissSuggestions();
      }
    };

    const sendRawInput = (
      data: string,
      command: string | null,
      origin: "keyboard" | "terminal_response" = "keyboard",
    ) => {
      const peers = syncPeerSessionIdsRef.current;
      if (origin === "keyboard" && peers && peers.length > 0) {
        return sendSessionInputWithSync(sessionId, data, peers, {
          preview: null,
          registerSubmission: command,
          origin,
        }).catch(() => {});
      }
        return sendSessionInput(sessionId, data, {
          preview: null,
          registerSubmission: command,
        origin,
        }).catch(() => {});
    };

    const canReconnectDisconnectedSession = () =>
      sessionTypeRef.current === "Local" ||
      !!connectionIdRef.current ||
      temporaryConfigMatchesSessionType();

    const temporaryConfigMatchesSessionType = () => {
      const temporaryConfig = temporaryConfigRef.current;
      if (!temporaryConfig) return false;
      switch (sessionTypeRef.current) {
        case "SSH":
          return temporaryConfig.protocol === "ssh";
        case "Telnet":
          return temporaryConfig.protocol === "telnet";
        case "Serial":
          return temporaryConfig.protocol === "serial";
        default:
          return false;
      }
    };

    const assertTemporaryConfigMatchesSessionType = () => {
      if (!temporaryConfigRef.current || temporaryConfigMatchesSessionType()) return;
      throw new Error("Temporary session config protocol mismatch");
    };

    const createReconnectedSession = () => {
      const connectionId = connectionIdRef.current;
      const temporaryConfig = temporaryConfigRef.current;
      const settingsSnapshot = terminalAppSettingsRef.current;
      const restoreCwdStartupCommand =
        sessionTypeRef.current === "SSH" &&
        (settingsSnapshot.terminal.reconnect_restore_cwd ?? false)
          ? buildReconnectCwdStartupCommand(
              sessionIdRef.current,
              settingsSnapshot.interaction.duplicate_session_command_delay_ms,
            )
          : undefined;

      switch (sessionTypeRef.current) {
        case "Local":
          return invoke<string>("create_local_session", {
            connectionId: connectionId || null,
          });
        case "Telnet":
          if (connectionId) {
            return invoke<string>("create_telnet_session", { connectionId });
          }
          assertTemporaryConfigMatchesSessionType();
          if (temporaryConfig?.protocol === "telnet") {
            return invoke<string>("create_telnet_session", {
              connectionId: null,
              host: temporaryConfig.host,
              port: temporaryConfig.port,
              name: temporaryConfig.name,
              startupCommand: null,
            });
          }
          return invoke<string>("create_telnet_session", { connectionId });
        case "Serial":
          if (connectionId) {
            return invoke<string>("create_serial_session", { connectionId });
          }
          assertTemporaryConfigMatchesSessionType();
          if (temporaryConfig?.protocol === "serial") {
            return invoke<string>("create_serial_session", {
              connectionId: null,
              portName: temporaryConfig.portName,
              baudRate: temporaryConfig.baudRate,
              name: temporaryConfig.name,
            });
          }
          return invoke<string>("create_serial_session", { connectionId });
        default:
          if (connectionId) {
            return invoke<string>("create_ssh_session", {
              connectionId,
              startupCommand: buildStartupCommandPayload(restoreCwdStartupCommand),
            });
          }
          assertTemporaryConfigMatchesSessionType();
          if (temporaryConfig?.protocol === "ssh") {
            const { protocol: _protocol, ...sshConfig } = temporaryConfig;
            return invoke<string>("create_temporary_ssh_session", {
              config: sshConfig,
              startupCommand: buildStartupCommandPayload(restoreCwdStartupCommand),
            });
          }
          return invoke<string>("create_ssh_session", {
            connectionId,
            startupCommand: buildStartupCommandPayload(restoreCwdStartupCommand),
          });
      }
    };

    const buildReplaceInputData = (command: string) => {
      const trackedState = inputStateRef.current;
      if (
        trackedState.value.length === 0 &&
        !trackedState.lineRewriteRequired &&
        !trackedState.desynced &&
        !trackedState.multiline
      ) {
        return command;
      }
      return `\u0005\u0015${command}`;
    };

    const replaceInputCommand = (command: string) => {
      const input = buildReplaceInputData(command);
      void sendSessionInput(sessionId, normalizeTerminalCommandInput(input), {
        preview: { kind: "replace", value: command },
        registerSubmission: null,
      }).catch(() => {});
    };

    const executeInputCommand = async (command: string) => {
      const input = buildReplaceInputData(command);
      await sendSessionInput(sessionId, normalizeTerminalCommandInput(input), {
        preview: { kind: "replace-and-execute", value: command },
        registerSubmission: null,
      });
      inputStateRef.current = applyTerminalInputData(
        inputStateRef.current,
        "\r",
      );
      syncSuggestionsWithInputState();
      await sendSessionInput(sessionId, "\r", {
        preview: null,
        registerSubmission: command,
      });
    };

    replaceInputCommandRef.current = replaceInputCommand;

    const unregisterTerminalContext = registerTerminalContextProvider(
      sessionId,
      {
        getRecentOutput: (lineLimit) => readRecentOutput(terminal, lineLimit),
        getSelectedText: () => terminal.getSelection(),
        getInputBuffer: () => inputStateRef.current.value,
        insertCommand: async (command) => {
          const input = buildReplaceInputData(command);
          await sendSessionInput(
            sessionId,
            normalizeTerminalCommandInput(input),
            {
              preview: { kind: "replace", value: command },
              registerSubmission: null,
            },
          );
        },
        executeCommand: async (command) => {
          await executeInputCommand(command);
        },
        focus: () => terminal.focus(),
      },
    );

    const handleInputPreview = (preview: SessionInputPreview) => {
      inputStateRef.current = applyTerminalInputPreview(
        inputStateRef.current,
        preview,
      );
      syncSuggestionsWithInputState();
    };

    const isCredentialPromptInputMode = () => {
      if (credentialPromptInputUntilRef.current > Date.now()) {
        return true;
      }
      credentialPromptInputUntilRef.current = 0;
      return false;
    };

    const canUseSmartCursor = (state = inputStateRef.current) => {
      if (disconnectedRef.current || aiCapturingRef.current) return false;
      if (terminal.buffer.active.type === "alternate") return false;
      if (isCredentialPromptInputMode()) return false;

      const shellIntegration = shellIntegrationRef.current;
      if (shellIntegration.enabled && shellIntegration.commandRunning)
        return false;

      if (
        state.desynced ||
        state.lineRewriteRequired ||
        state.pasteMode ||
        state.multiline
      ) {
        return false;
      }

      return !syncPeerSessionIdsRef.current?.length;
    };

    const getSmartCursorSelectedInputRange = () => {
      const state = inputStateRef.current;
      if (!canUseSmartCursor(state)) return null;
      return getSelectedInputRange(terminal, state);
    };

    const isPlainTextInputData = (data: string) => {
      if (!data || data.startsWith("\x1b")) return false;
      return !/[\x00-\x1f\x7f]/u.test(data);
    };

    const isCurrentSelectionFromSearch = () => {
      const searchSelectionText = searchSelectionTextRef.current;
      return (
        searchSelectionText !== null &&
        terminal.hasSelection() &&
        terminal.getSelection() === searchSelectionText
      );
    };

    const clearSearchSelectionBeforeInput = () => {
      if (!isCurrentSelectionFromSearch()) return false;
      terminal.clearSelection();
      clearSearchSelectionState();
      return true;
    };

    const pasteText = (
      text: string,
      options: { skipDialog?: boolean } = {},
    ) => {
      if (!text) return;
      terminal.focus();
      const showMultiLinePasteDialog =
        terminalAppSettingsRef.current?.terminal
          ?.show_multi_line_paste_dialog ?? true;
      if (
        !options.skipDialog &&
        showMultiLinePasteDialog &&
        sessionTypeRef.current !== "Serial" &&
        isMultiLineText(text)
      ) {
        setMultiLinePasteText(text);
        return;
      }

      clearSearchSelectionBeforeInput();
      const selectedInputRange = getSmartCursorSelectedInputRange();
      let pendingSelectionDelete: Promise<void> | null = null;
      if (selectedInputRange) {
        const currentCursor = inputStateRef.current.cursor;
        const moveToSelectionEnd = buildMoveInputCursorData(
          currentCursor,
          selectedInputRange.end,
        );
        const deleteSelection = "\u007f".repeat(
          selectedInputRange.end - selectedInputRange.start,
        );
        inputStateRef.current = deleteTerminalInputRange(
          inputStateRef.current,
          selectedInputRange.start,
          selectedInputRange.end,
        );
        terminal.clearSelection();
        syncSuggestionsWithInputState();
        if (moveToSelectionEnd || deleteSelection) {
          pendingSelectionDelete = sendRawInput(
            `${moveToSelectionEnd}${deleteSelection}`,
            null,
          );
        }
      }

      const runPaste = () => {
        markTerminalUserInput(terminal);
        terminal.paste(text);
        terminal.focus();
        requestAnimationFrame(() => {
          if (!isTerminalAlive()) return;
          terminal.focus();
        });
      };
      if (pendingSelectionDelete) {
        pendingSelectionDelete.then(runPaste);
      } else {
        runPaste();
      }
    };
    pasteTextRef.current = pasteText;

    const lastSelectionRef = { current: "" };

    const buildMoveInputCursorData = (
      currentCursor: number,
      targetCursor: number,
    ) => {
      if (targetCursor === currentCursor) return "";
      return targetCursor > currentCursor
        ? "\x1b[C".repeat(targetCursor - currentCursor)
        : "\x1b[D".repeat(currentCursor - targetCursor);
    };

    const moveInputCursor = (targetCursor: number) => {
      const currentState = inputStateRef.current;
      if (!canUseSmartCursor(currentState)) return;
      const nextCursor = Math.max(
        0,
        Math.min(currentState.value.length, targetCursor),
      );
      const input = buildMoveInputCursorData(currentState.cursor, nextCursor);
      if (!input) return;

      inputStateRef.current = { ...currentState, cursor: nextCursor };
      syncSuggestionsWithInputState();
      sendRawInput(input, null);
    };

    const collapseInputSelection = (
      selectedInputRange: InputSelectionRange,
      edge: "start" | "end",
    ) => {
      const currentState = inputStateRef.current;
      if (!canUseSmartCursor(currentState)) return;
      const targetCursor =
        edge === "start" ? selectedInputRange.start : selectedInputRange.end;
      const input = buildMoveInputCursorData(currentState.cursor, targetCursor);

      inputStateRef.current = { ...currentState, cursor: targetCursor };
      terminal.clearSelection();
      syncSuggestionsWithInputState();
      if (input) sendRawInput(input, null);
    };

    const deleteInputSelection = (selectedInputRange: InputSelectionRange) => {
      if (!canUseSmartCursor()) return;
      const currentCursor = inputStateRef.current.cursor;
      const moveToSelectionEnd = buildMoveInputCursorData(
        currentCursor,
        selectedInputRange.end,
      );
      const deleteSelection = "\u007f".repeat(
        selectedInputRange.end - selectedInputRange.start,
      );

      inputStateRef.current = deleteTerminalInputRange(
        inputStateRef.current,
        selectedInputRange.start,
        selectedInputRange.end,
      );
      terminal.clearSelection();
      syncSuggestionsWithInputState();
      sendRawInput(`${moveToSelectionEnd}${deleteSelection}`, null);
    };

    const replaceInputSelection = (
      selectedInputRange: InputSelectionRange,
      data: string,
    ) => {
      if (!canUseSmartCursor()) return;
      const currentCursor = inputStateRef.current.cursor;
      const moveToSelectionEnd = buildMoveInputCursorData(
        currentCursor,
        selectedInputRange.end,
      );
      const deleteSelection = "\u007f".repeat(
        selectedInputRange.end - selectedInputRange.start,
      );
      const stateAfterDelete = deleteTerminalInputRange(
        inputStateRef.current,
        selectedInputRange.start,
        selectedInputRange.end,
      );

      inputStateRef.current = applyTerminalInputData(stateAfterDelete, data);
      terminal.clearSelection();
      syncSuggestionsWithInputState();
      sendRawInput(`${moveToSelectionEnd}${deleteSelection}${data}`, null);
    };

    const moveCredentialSelection = (direction: 1 | -1) => {
      if (
        !credentialShowPanelRef.current ||
        credentialMatchesRef.current.length === 0
      ) {
        return false;
      }

      const cur = credentialSelectedIndexRef.current;
      const len = credentialMatchesRef.current.length;
      const next =
        direction > 0
          ? cur < 0 || cur >= len - 1
            ? 0
            : cur + 1
          : cur <= 0
            ? len - 1
            : cur - 1;

      credentialSelectedIndexRef.current = next;
      setCredentialSelectedIndex(next);
      return true;
    };

    const isCredentialPanelActive = () =>
      credentialShowPanelRef.current && credentialMatchesRef.current.length > 0;

    const moveCommandSuggestionSelection = (direction: 1 | -1) => {
      if (!showSuggestionsRef.current || suggestionsRef.current.length === 0) {
        return false;
      }

      const cur = selectedIndexRef.current;
      const len = suggestionsRef.current.length;
      const next =
        direction > 0
          ? cur === -1
            ? 0
            : cur >= len - 1
              ? -1
              : cur + 1
          : cur === -1
            ? len - 1
            : cur <= 0
              ? -1
              : cur - 1;

      selectedIndexRef.current = next;
      setSelectedIndex(next);
      return true;
    };

    const acceptCommandSuggestion = (execute: boolean) => {
      if (!showSuggestionsRef.current || suggestionsRef.current.length === 0) {
        return false;
      }

      const selected = suggestionsRef.current[selectedIndexRef.current];
      if (!selected) {
        return false;
      }

      applySuggestion(selected.command, execute);
      if (execute) {
        refreshCommandLineTimestamp();
      }
      dismissSuggestions();
      return true;
    };

    const moveInputCursorAfterSelection = (
      selectedInputRange: InputSelectionRange,
      targetCursor: number,
    ) => {
      const nextCursor = Math.max(
        selectedInputRange.start,
        Math.min(selectedInputRange.end, targetCursor),
      );
      moveInputCursor(nextCursor);
    };

    installXTerminalKeyboardController({
      terminal,
      terminalAppSettingsRef,
      sessionTypeRef,
      inputStateRef,
      disconnectedRef,
      onDisconnectedCloseRequestedRef,
      showSuggestionsRef,
      suggestionsRef,
      doFindRef,
      pasteClipboard,
      pasteText,
      sendRawInput,
      triggerSearch,
      dismissSuggestions,
      moveCredentialSelection,
      isCredentialPanelActive,
      moveCommandSuggestionSelection,
      acceptCommandSuggestion,
      isCredentialPromptInputMode,
      clearSearchSelectionBeforeInput,
      getSmartCursorSelectedInputRange,
      deleteInputSelection,
      collapseInputSelection,
      replaceInputSelection,
      syncSuggestionsWithInputState,
      lastSelectionRef,
    });

    const blockedColorOscIds = new Set<number>();
    const remoteColorOscGuardDisposable = installRemoteColorOscGuard(
      terminal,
      sessionTypeRef.current,
      (oscId) => {
        if (blockedColorOscIds.has(oscId)) return;
        blockedColorOscIds.add(oscId);

        logger.debug({
          domain: "terminal.input",
          event: "terminal.remote_color_osc_blocked",
          message: "Blocked remote color OSC",
          ids: { session_id: sessionId },
          data: {
            session_type: sessionTypeRef.current,
            terminal_transparency_enabled: terminalTransparencyEnabled,
            osc_id: oscId,
          },
        });
      },
      { blockDefaultBackground: terminalTransparencyEnabled },
    );

    const oscDisposable = terminal.parser.registerOscHandler(133, (data) => {
      const si = shellIntegrationRef.current;

      if (data.startsWith("A")) {
        si.enabled = true;
        si.commandRunning = false;
        return false;
      }

      if (data.startsWith("B")) {
        si.enabled = true;
        si.commandRunning = false;
        resetCommandSuggestionSuppression();
        return false;
      }

      if (data.startsWith("C")) {
        si.enabled = true;
        si.commandRunning = true;
        inputStateRef.current = createTerminalInputState();
        resetCommandSuggestionSuppression();
        dismissSuggestions();
        return false;
      }

      if (data.startsWith("D")) {
        si.enabled = true;
        si.commandRunning = false;
        resetCommandSuggestionSuppression();
        return false;
      }

      return false;
    });

    const clipboardOscDisposable = terminal.parser.registerOscHandler(
      52,
      (data) => {
        if (
          !terminalAppSettingsRef.current?.interaction
            ?.allow_osc52_clipboard_write
        ) {
          return true;
        }

        const text = decodeOsc52ClipboardText(data);
        if (text === null) return true;

        void writeClipboardText(text).catch(() => {});
        return true;
      },
    );

    const writeParsedDisposable = terminal.onWriteParsed(() => {
      alternateScreenTrackerRef.current.setXtermBufferType(
        terminal.buffer.active.type,
      );
      if (terminal.buffer.active.type === "alternate") {
        dismissSuggestions();
      }
      const terminalSettings = terminalAppSettingsRef.current?.terminal;
      if (
        performanceModeRef.current === "normal" &&
        (terminalSettings?.show_line_numbers ||
          terminalSettings?.show_timestamps)
      ) {
        window.dispatchEvent(
          new CustomEvent("nyaterm:refresh-gutter", { detail: { sessionId } }),
        );
      }
    });

    const clearCredentialPromptInputMode = () => {
      credentialPromptBufferRef.current = "";
      credentialPromptInputUntilRef.current = 0;
    };

    const updateCredentialPromptInputMode = (payload: string) => {
      credentialPromptBufferRef.current =
        `${credentialPromptBufferRef.current}${payload}`.slice(-4096);

      if (detectCredentialPromptKind(credentialPromptBufferRef.current)) {
        credentialPromptInputUntilRef.current = Date.now() + 120_000;
        dismissSuggestions();
        return;
      }

      if (/[\r\n]/u.test(payload)) {
        credentialPromptInputUntilRef.current = 0;
      }
    };

    const setCommandSuggestionSuppressed = (suppressed: boolean) => {
      if (commandSuggestionSuppressedRef.current === suppressed) return;
      commandSuggestionSuppressedRef.current = suppressed;
      if (suppressed) {
        dismissSuggestions();
      }
    };

    const noteShellCommand = (command: string) => {
      setCommandSuggestionSuppressed(
        commandStartsSuggestionSuppressingProgram(command),
      );
    };

    const resetCommandSuggestionSuppression = () => {
      setCommandSuggestionSuppressed(false);
    };

    const refreshGutter = () => {
      if (!isTerminalAlive()) return;
      requestGutterRefresh();
    };

    clearAllRef.current = () => {
      lineTimestampsRef.current = new Map();
      gutterLineOffsetRef.current = 0;
      terminal.reset();
      terminal.focus();
      requestGutterRefresh();
    };

    resizeDeduperRef.current.reset(sessionId, terminalGeneration);
    const sendBackendResize = (cols: number, rows: number, source: string) => {
      if (
        resizeDeduperRef.current.shouldSend(
          sessionId,
          terminalGeneration,
          cols,
          rows,
        )
      ) {
        logger.debug({
          domain: "terminal.resize",
          event: "terminal.resize.backend_sent",
          message: "Sent terminal resize to backend",
          ids: { session_id: sessionId },
          data: { cols, rows, source },
        });
        invoke("resize_session", { sessionId, cols, rows }).catch(() => {});
        return;
      }

      logger.debug({
        domain: "terminal.resize",
        event: "terminal.resize.backend_skipped",
        message: "Skipped duplicate terminal resize to backend",
        ids: { session_id: sessionId },
        data: { cols, rows, source },
      });
    };

    const handleFitComplete = (result: TerminalFitResult) => {
      if (!isTerminalAlive()) return;
      sendBackendResize(terminal.cols, terminal.rows, result.reason);
      refreshGutter();
      snapshotRestoreController.completeAfterFinalFit();
    };

    const fitScheduler = createTerminalFitScheduler({
      sessionId,
      getTerminal: () => (isTerminalAlive() ? terminal : null),
      getFitAddon: () => fitAddonRef.current,
      getContainer: () => containerRef.current,
      isVisible: () => visibleRef.current && !hibernatedRef.current,
      onAfterFit: handleFitComplete,
    });
    fitSchedulerRef.current = fitScheduler;
    handleVisibilityChangeRef.current = () => {
      fitScheduler.notifyVisible();
    };

    const stampWrittenLines = (from: number, to: number, ts: number) => {
      if (!terminalAppSettingsRef.current?.terminal?.show_timestamps) return;
      if (terminal.buffer.active.type === "alternate") return;

      const map = lineTimestampsRef.current;
      const start = Math.min(from, to);
      const end = Math.max(from, to);

      for (let y = start; y <= end; y += 1) {
        if (!map.has(y)) {
          map.set(y, ts);
        }
      }

      const keepFrom = Math.max(0, start - 3000);
      for (const key of Array.from(map.keys())) {
        if (key < keepFrom) {
          map.delete(key);
        }
      }

      if (performanceModeRef.current === "normal") {
        refreshGutter();
      }
    };

    const refreshCommandLineTimestamp = () => {
      if (!terminalAppSettingsRef.current?.terminal?.show_timestamps) return;
      if (terminal.buffer.active.type === "alternate") return;

      const buf = terminal.buffer.active;
      const offset = gutterLineOffsetRef.current;
      const cursorLine = buf.baseY + buf.cursorY;
      const ts = Date.now();
      const map = lineTimestampsRef.current;

      let startLine = cursorLine;
      while (startLine > 0) {
        const line = buf.getLine(startLine);
        if (line && !line.isWrapped) break;
        startLine -= 1;
      }

      for (let y = startLine; y <= cursorLine; y += 1) {
        map.set(offset + y, ts);
      }

      if (performanceModeRef.current === "normal") {
        refreshGutter();
      }
    };

    const {
      outputAckLease,
      outputScheduler,
      outputDrain,
      frameGate,
      noteSkippedOutput,
      maybeRecoverPerformanceMode,
      refreshOutputPressureMode,
      updateOutputDrainMode,
      flushFrameGateAndDrain,
      writeTerminalTextAfterOutputQueue,
      flushQueuedOutputBeforeStatusNotice,
    } = createXTerminalOutputController({
      sessionId,
      terminalGeneration,
      terminal,
      outputDrainRef,
      frameGateRef,
      visibleRef,
      hibernatedRef,
      hibernationPhaseRef,
      performanceModeRef,
      alternateScreenTrackerRef,
      isTerminalAlive,
      getCurrentAbsoluteLine,
      stampWrittenLines,
      clearHibernateTimer,
      enterOverloadedMode,
      setOutputPressureMode,
      exitOverloadedMode,
    });

    writeOrderedTerminalStatus = (data: string) => {
      void writeTerminalTextAfterOutputQueue(data);
    };

    const resetDisconnectedInputState = () => {
      inputStateRef.current = createTerminalInputState();
      clearCredentialPromptInputMode();
      resetCommandSuggestionSuppression();
      dismissSuggestions();
    };

    const enterDisconnectedState = ({
      title,
      message,
      titleColor,
      showReconnectPrompt,
    }: {
      title: string;
      message?: string;
      titleColor: "31" | "36";
      showReconnectPrompt: boolean;
    }) => {
      disconnectedRef.current = true;
      resetDisconnectedInputState();

      if (disconnectedNoticeShownRef.current) return;
      disconnectedNoticeShownRef.current = true;
      window.dispatchEvent(
        new CustomEvent("nyaterm:session-disconnected", {
          detail: { sessionId },
        }),
      );

      void (async () => {
        await flushQueuedOutputBeforeStatusNotice();
        if (!isTerminalAlive()) return;

        await writeTerminalTextAfterOutputQueue(
          `\r\n\x1b[${titleColor}m[${title}]\x1b[0m\r\n`,
        );
        if (message) {
          await writeTerminalTextAfterOutputQueue(
            `\x1b[31m${message}\x1b[0m\r\n`,
          );
        }
        if (showReconnectPrompt && canReconnectDisconnectedSession()) {
          await writeTerminalTextAfterOutputQueue(
            `\x1b[33m[${tRef.current("terminal.pressEnterToReconnect")}]\x1b[0m\r\n`,
          );
        }
      })();
    };

    const enterDisconnectedStateIfAttachSessionMissing = (error: unknown) => {
      if (!isSessionNotFoundError(error)) return false;
      enterDisconnectedState({
        title: tRef.current("terminal.sessionDisconnected"),
        titleColor: "31",
        showReconnectPrompt: true,
      });
      return true;
    };

    const repaintVisibleTerminal = () => {
      if (
        restoringSnapshotRef.current ||
        !visibleRef.current ||
        !isTerminalAlive()
      )
        return;
      requestAnimationFrame(() => {
        if (
          restoringSnapshotRef.current ||
          !visibleRef.current ||
          !isTerminalAlive()
        )
          return;
        terminal.refresh(0, Math.max(0, terminal.rows - 1));
        requestAnimationFrame(() => {
          if (
            restoringSnapshotRef.current ||
            !visibleRef.current ||
            !isTerminalAlive()
          )
            return;
          terminal.refresh(0, Math.max(0, terminal.rows - 1));
        });
      });
    };

    const replayPendingWakeEvents = () => {
      const events = pendingWakeEventsRef.current.splice(0);
      for (const event of events) {
        if (!isTerminalAlive()) return;
        switch (event.type) {
          case "error": {
            enterDisconnectedState({
              title: tRef.current("terminal.connectionFailed"),
              message: event.message,
              titleColor: "31",
              showReconnectPrompt: false,
            });
            toast.error(event.message);
            onConnectionErrorRef.current?.(sessionIdRef.current, event.message);
            break;
          }
          case "closed":
            enterDisconnectedState({
              title: tRef.current("terminal.sessionDisconnected"),
              titleColor: "31",
              showReconnectPrompt: true,
            });
            break;
          case "focus":
            terminal.focus();
            break;
          case "zmodem":
            if (
              event.payload.type === "detected" ||
              event.payload.type === "progress"
            ) {
              zmodemActiveRef.current = true;
            } else if (
              event.payload.type === "complete" ||
              event.payload.type === "failed"
            ) {
              zmodemActiveRef.current = false;
            }
            zmodemHandler.handle(event.payload);
            break;
          case "ai":
            if (event.payload.type === "commandStart") {
              aiCapturingRef.current = true;
              inputStateRef.current = createTerminalInputState();
              clearCredentialPromptInputMode();
              dismissSuggestions();
              void writeTerminalTextAfterOutputQueue(
                renderAiCommandStart(event.payload),
              );
            } else if (event.payload.type === "commandEnd") {
              aiCapturingRef.current = false;
              void writeTerminalTextAfterOutputQueue(
                renderAiCommandEnd(event.payload),
              );
            }
            break;
        }
      }
    };

    const { applyVisibilityPolicy, noteOutputActivity } =
      createXTerminalHibernationController({
        sessionId,
        terminal,
        outputDrain,
        visibleRef,
        sessionTypeRef,
        aiCapturingRef,
        zmodemActiveRef,
        syncPeerSessionIdsRef,
        outputDrainRef,
        disconnectedRef,
        reconnectingRef,
        hibernateTimerRef,
        hibernationEpochRef,
        hibernationPendingRef,
        hibernationPhaseRef,
        detachedHibernateEpochRef,
        hibernationSnapshotRef,
        hibernationCleanupRef,
        hibernatedRef,
        lastOutputActivityAtRef,
        showSearchBar,
        activeMode,
        isTerminalAlive,
        logHibernation,
        clearHibernateTimer,
        enterDisconnectedStateIfAttachSessionMissing,
        updateOutputDrainMode,
        flushFrameGateAndDrain,
        captureReconnectSnapshot,
        beginSnapshotRestore: (snapshot) => {
          snapshotRestoreController.begin(snapshot);
        },
        setTerminalReady,
        setHibernated,
        setTerminalGeneration,
        maybeRecoverPerformanceMode,
        refreshOutputPressureMode,
        repaintVisibleTerminal,
      });

    handleVisibilityChangeRef.current = applyVisibilityPolicy;
    applyVisibilityPolicy();

    const sessionEvents = createXTerminalSessionEvents({
      sessionId,
      terminal,
      frameGate,
      sessionIdRef,
      visibleRef,
      lastErrorNoticeAtRef,
      aiCapturingRef,
      zmodemActiveRef,
      inputStateRef,
      alternateScreenTrackerRef,
      hibernationPhaseRef,
      detachedHibernateEpochRef,
      onConnectionErrorRef,
      tRef,
      isTerminalAlive,
      requestWake,
      enterDisconnectedState,
      enterDisconnectedStateIfAttachSessionMissing,
      noteSkippedOutput,
      noteOutputActivity,
      updateCredentialPromptInputMode,
      feedCredentialOutput,
      maybeRecoverPerformanceMode,
      refreshOutputPressureMode,
      noteShellCommand,
      clearCredentialPromptInputMode,
      dismissSuggestions,
      writeTerminalTextAfterOutputQueue,
      initialReplayPromise,
      updateOutputDrainMode,
      logHibernation,
      zmodemHandler,
      replayPendingWakeEvents,
      settleOutputAfterAttach: () =>
        flushFrameGateAndDrain("dynamic_title_attach"),
      flushPendingDynamicTitle,
    });
    const sessionSetupPromise = sessionEvents.setup().catch((error) => {
      resumeDynamicTitlePublication(sessionId);
      logger.error({
        domain: "session.lifecycle",
        event: "session_listener_setup_failed",
        message: "Failed to initialize terminal session listeners",
        ids: { session_id: sessionId },
        error,
      });
    });

    const removePreviewListener = listenSessionInputPreview(
      sessionId,
      handleInputPreview,
    );

    const dataDisposable = terminal.onData((data) => {
      const trackedOrigin = inputOriginTracker.consume();
      const origin = resolveXTerminalDataOrigin(
        trackedOrigin,
        data,
        canDistinguishTerminalResponses,
        fallbackFocusReport,
      );
      if (data === fallbackFocusReport) fallbackFocusReport = null;
      if (origin === "terminal_response") {
        void sendRawInput(data, null, origin);
        return;
      }
      if (aiCapturingRef.current) return;
      if (hibernationPhaseRef.current !== "idle") {
        requestWake("input");
      }

      if (disconnectedRef.current) {
        if (
          data === "\r" &&
          canReconnectDisconnectedSession() &&
          !reconnectingRef.current
        ) {
          reconnectingRef.current = true;
          void (async () => {
            try {
              await writeTerminalTextAfterOutputQueue(
                `\r\n\x1b[36m[${tRef.current("terminal.reconnecting")}]\x1b[0m\r\n`,
              );
              const newSessionId = await createReconnectedSession();
              const reconnectSnapshot = captureReconnectSnapshot();
              preservedReconnectContentRef.current = reconnectSnapshot;
              snapshotRestoreController.begin(reconnectSnapshot);
              const oldSessionId = sessionIdRef.current;
              carryOverSessionCwd(oldSessionId, newSessionId);
              disconnectedRef.current = false;
              disconnectedNoticeShownRef.current = false;
              disconnectedCloseRequestedRef.current = false;
              reconnectingRef.current = false;
              window.dispatchEvent(
                new CustomEvent("nyaterm:session-reconnected", {
                  detail: { oldSessionId, newSessionId },
                }),
              );
              onReconnectedRef.current?.(oldSessionId, newSessionId);
            } catch (err) {
              reconnectingRef.current = false;
              await writeTerminalTextAfterOutputQueue(
                `\r\n\x1b[31m[${tRef.current("terminal.reconnectFailed")}: ${err}]\x1b[0m\r\n`,
              );
              await writeTerminalTextAfterOutputQueue(
                `\x1b[33m[${tRef.current("terminal.pressEnterToReconnect")}]\x1b[0m\r\n`,
              );
            }
          })();
        }
        if (
          data === "\x04" &&
          sessionTypeRef.current === "Local" &&
          !reconnectingRef.current &&
          !disconnectedCloseRequestedRef.current
        ) {
          disconnectedCloseRequestedRef.current = true;
          onDisconnectedCloseRequestedRef.current?.();
        }
        return;
      }

      if (
        credentialShowPanelRef.current &&
        credentialMatchesRef.current.length > 0
      ) {
        if (data === "\x1b[A") {
          moveCredentialSelection(-1);
          return;
        }
        if (data === "\x1b[B") {
          moveCredentialSelection(1);
          return;
        }
        if (data === "\t") {
          moveCredentialSelection(1);
          return;
        }
        if (data === "\x1b[Z") {
          moveCredentialSelection(-1);
          return;
        }
        if (data === "\r" && credentialSelectedIndexRef.current >= 0) {
          const selected =
            credentialMatchesRef.current[credentialSelectedIndexRef.current];
          if (selected) void handleCredentialSelect(selected);
          return;
        }
        if (data === "\x1b") {
          dismissCredentialPanel();
          return;
        }
        dismissCredentialPanel();
      }

      if (isCredentialPromptInputMode()) {
        if (data === "\r" || data === "\u0003") {
          clearCredentialPromptInputMode();
          inputStateRef.current = createTerminalInputState();
        }
        dismissSuggestions();
        sendRawInput(data, null);
        return;
      }

      if (
        canShowCommandSuggestions() &&
        showSuggestionsRef.current &&
        suggestionsRef.current.length > 0
      ) {
        if (data === "\t" && acceptCommandSuggestion(false)) {
          return;
        }

        if (data === "\x1b[A" || data === "\x1bOA") {
          moveCommandSuggestionSelection(-1);
          return;
        }

        if (data === "\x1b[B" || data === "\x1bOB") {
          moveCommandSuggestionSelection(1);
          return;
        }

        if (data === "\x1b") {
          dismissSuggestions();
          return;
        }

        if (data === "\r" && acceptCommandSuggestion(true)) {
          return;
        }
      }

      if (
        data !== "\t" &&
        inputStateRef.current.desynced &&
        inputStateRef.current.desyncReason === "tab"
      ) {
        const recovered = resyncFromTerminalLine(
          inputStateRef.current,
          readCurrentInputLine(terminal),
        );
        if (recovered) {
          inputStateRef.current = recovered;
        }
      }

      if (commandSuggestionSuppressedRef.current) {
        dismissSuggestions();
        if (data === "\u0003" || data === "q") {
          resetCommandSuggestionSuppression();
          inputStateRef.current = createTerminalInputState();
        }
        sendRawInput(data, null);
        return;
      }

      if (isPlainTextInputData(data)) {
        clearSearchSelectionBeforeInput();
        const selectedInputRange = getSmartCursorSelectedInputRange();
        if (selectedInputRange) {
          replaceInputSelection(selectedInputRange, data);
          return;
        }
      }

      let command = "";
      if (data === "\r") {
        const currentInputState = inputStateRef.current;
        const recovered = resyncFromTerminalLine(
          currentInputState,
          readCurrentInputLine(terminal),
        );
        command = getTrackedSubmissionCommand(recovered ?? currentInputState);
      }
      if (data === "\r") {
        refreshCommandLineTimestamp();
      }
      inputStateRef.current = applyTerminalInputData(
        inputStateRef.current,
        data,
      );
      if (data === "\r" || data === "\u0003") {
        clearCredentialPromptInputMode();
      }
      if (data === "\r" && command) {
        noteShellCommand(command);
        dismissSuggestions();
      } else if (
        isPagerSingleKeyInput(data) ||
        isPagerSearchOrCommandInput(inputStateRef.current.value)
      ) {
        dismissSuggestions();
      } else {
        syncSuggestionsWithInputState();
      }
      sendRawInput(data, data === "\r" && command ? command : null);
    });

    const resizeDisposable = terminal.onResize(({ cols, rows }) => {
      sendBackendResize(cols, rows, "xterm.onResize");
      refreshGutter();
    });

    const scrollDisposable = terminal.onScroll(() => {
      removeLinkPopup();
      refreshGutter();
    });

    const observer = new ResizeObserver((entries) => {
      if (restoringSnapshotRef.current) return;
      const entry = entries[0];
      if (!entry) return;
      fitScheduler.observeResize(
        entry.contentRect.width,
        entry.contentRect.height,
      );
    });
    observer.observe(containerRef.current);

    const containerEl = containerRef.current;
    const selectionController = installXTerminalSelectionController({
      terminal,
      containerEl,
      isMacOS,
      isWindows,
      activeRef,
      visibleRef,
      terminalAppSettingsRef,
      pendingSearchSelectionRef,
      searchSelectionTextRef,
      lastSelectionRef,
      disconnectedRef,
      aiCapturingRef,
      inputStateRef,
      isTerminalAlive,
      removeLinkPopup,
      clearSearchSelectionState,
      getSmartCursorSelectedInputRange,
      moveInputCursorAfterSelection,
      canUseSmartCursor,
      moveInputCursor,
      pasteText,
      pasteClipboard,
    });

    if (restoringInitialSnapshot) {
      void sessionSetupPromise.then(() => {
        if (!isTerminalAlive()) return;
        snapshotRestoreController.markReplayAndAttachComplete();
      });
    } else {
      void sessionSetupPromise;
      fitScheduler.schedule({
        reason: "initial",
        force: true,
        refresh: true,
        onComplete: () => {
          if (!isTerminalAlive()) return;
          setTerminalReady(true);
          refreshGutter();
        },
      });
    }

    return () => {
      disposed = true;
      handleVisibilityChangeRef.current = null;
      const cleanupDetachedEpoch = detachedHibernateEpochRef.current;
      const isHibernateRendererCleanup =
        hibernationCleanupRef.current &&
        hibernationPhaseRef.current === "hibernated";
      if (isHibernateRendererCleanup) {
        clearHibernateTimer();
      } else {
        invalidateHibernation("cleanup");
      }
      if (!isHibernateRendererCleanup && cleanupDetachedEpoch !== null) {
        void invoke("attach_session", { sessionId })
          .then(() => {
            if (detachedHibernateEpochRef.current === cleanupDetachedEpoch) {
              detachedHibernateEpochRef.current = null;
            }
            hibernationPhaseRef.current = "idle";
            resumeDynamicTitlePublication(sessionId);
            logHibernation(
              "rollback",
              "Rolled back detached renderer during cleanup",
              {
                reason: "cleanup",
                epoch: cleanupDetachedEpoch,
              },
            );
          })
          .catch((error) => {
            hibernationPhaseRef.current = "failed";
            resumeDynamicTitlePublication(sessionId);
            logHibernation(
              "fail",
              "Failed to roll back detached renderer during cleanup",
              { reason: "cleanup", epoch: cleanupDetachedEpoch },
              error,
            );
          });
      }
      setTerminalReady(false);
      selectionController.dispose();
      if (searchTimerRef.current) clearTimeout(searchTimerRef.current);
      inputStateRef.current = createTerminalInputState();
      clearCredentialPromptInputMode();
      shellIntegrationRef.current.enabled = false;
      shellIntegrationRef.current.commandRunning = false;
      replaceInputCommandRef.current = null;
      pasteTextRef.current = () => {};
      resetCredentialAutofill();

      if (!canDistinguishTerminalResponses) {
        for (const eventName of fallbackUserInputEvents) {
          containerEl.removeEventListener(
            eventName,
            markFallbackUserInput,
            true,
          );
        }
        containerEl.removeEventListener(
          "focusin",
          markFallbackFocusReport,
          true,
        );
        containerEl.removeEventListener(
          "focusout",
          markFallbackFocusReport,
          true,
        );
      }
      unregisterUserInputMarker();
      userInputDisposable?.dispose();
      titleDisposable.dispose();
      oscDisposable.dispose();
      remoteColorOscGuardDisposable.dispose();
      clipboardOscDisposable.dispose();
      writeParsedDisposable.dispose();
      dataDisposable.dispose();
      resizeDisposable.dispose();
      scrollDisposable.dispose();
      for (const disposable of searchLifecycleDisposables) {
        disposable.dispose();
      }
      trimDisposable?.dispose();
      removeLinkPopup();
      removePreviewListener();
      unregisterTerminalContext();
      unregisterReconnectCapture();

      observer.disconnect();
      fitScheduler.dispose();
      if (fitSchedulerRef.current === fitScheduler) {
        fitSchedulerRef.current = null;
      }
      sessionEvents.dispose();
      zmodemHandler.dispose();
      frameGate.dispose({ ackRemaining: true, reason: "terminal_cleanup" });
      if (frameGateRef.current === frameGate) {
        frameGateRef.current = null;
      }
      outputDrain.dispose();
      outputAckLease.dispose();
      if (outputDrainRef.current === outputDrain) {
        outputDrainRef.current = null;
      }
      outputScheduler.reset();
      const latestLifecycleState = terminalLifecycleStateRef.current;
      if (
        !hibernationCleanupRef.current &&
        latestLifecycleState.sessionId === sessionId &&
        latestLifecycleState.terminalTransparencyEnabled !==
          terminalTransparencyEnabled
      ) {
        const reconnectSnapshot = captureReconnectSnapshot();
        preservedReconnectContentRef.current = reconnectSnapshot;
        snapshotRestoreController.begin(reconnectSnapshot);
      }
      if (!isHibernateRendererCleanup) {
        resumeDynamicTitlePublication(sessionId);
      }
      terminal.dispose();
      terminalRef.current = null;
      setTerminalInstance(null);
      fitAddonRef.current = null;
      registerSearchAddon(null);
      hibernationCleanupRef.current = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hibernated, sessionId, terminalGeneration, terminalTransparencyEnabled]);

  // Appearance, theme, and interaction settings sync.
  // Declared AFTER the terminal creation effect so effects from these hooks
  // run after terminalRef.current is already set on initial mount.
  useTerminalSettings(
    terminalRef,
    fitSchedulerRef,
    terminalThemeColors,
    appearance,
    terminalSettings,
    interaction,
    visible && active,
    terminalInstance,
    sessionId,
    restoringSnapshotRef,
  );

  // isDark is derived from the terminal theme background so built-in rule colors
  // switch automatically when the user changes themes.
  const isDark = hexLuminance(terminalTheme.colors.terminal.background) < 0.5;
  const keywordHighlighterSuspended = shouldSuspendKeywordHighlighter({
    visible,
    hibernated,
    terminalReady,
    performanceMode,
  });
  useKeywordHighlighter(
    terminalInstance,
    terminalSettings,
    sessionId,
    isDark,
    {
      suspended: keywordHighlighterSuspended,
      releaseCachesAfterDelay: !visible || hibernated,
    },
  );

  const { tooltipState, menuState, closeMenu } = useActionLinks(
    terminalInstance,
    terminalSettings,
    sessionId,
    replaceInputCommandRef,
    performanceMode !== "normal" || !visible,
  );

  useTerminalRefreshEffects({
    terminalRef,
    fitSchedulerRef,
    active,
    visible,
    terminalReady,
    performanceMode,
    sessionId,
    showGutter,
    showContentPadding,
    workspacePaddingSetting: terminalSettings.show_workspace_padding,
    snapshotRestoringRef: restoringSnapshotRef,
  });

  const searchInputRef = useRef<HTMLInputElement | null>(null);

  const doFind = useCallback(
    (selection?: string) => {
      setShowSearchBar(true);
      // When the bar is already open the focus effect won't rerun, so focus directly.
      searchInputRef.current?.focus();
      searchInputRef.current?.select();
      if (selection) {
        setSearchQuery(selection);
        handleSearchNext(selection);
      }
    },
    [handleSearchNext, setShowSearchBar, setSearchQuery],
  );

  const handleTerminalSearchQueryChange = useCallback(
    (query: string) => {
      if (!query) {
        clearSearchSelectionState();
      }
      setSearchQuery(query);
    },
    [clearSearchSelectionState, setSearchQuery],
  );

  const handleTerminalSearchModeChange = useCallback(
    (mode: "buffer" | "history") => {
      if (mode === "history") {
        clearSearchSelectionState();
      }
      setActiveMode(mode);
    },
    [clearSearchSelectionState, setActiveMode],
  );

  const handleTerminalSearchFlagChange = useCallback(
    (flag: keyof typeof searchFlags, value: boolean) => {
      clearSearchSelectionState();
      setSearchFlag(flag, value);
    },
    [clearSearchSelectionState, setSearchFlag],
  );

  const handleTerminalSearchClose = useCallback(() => {
    clearSearchSelectionState();
    handleCloseSearch();
  }, [clearSearchSelectionState, handleCloseSearch]);

  const handlePasteText = useCallback((text: string) => {
    pasteTextRef.current(text);
  }, []);

  const handleClearAll = useCallback(() => {
    clearAllRef.current();
  }, []);

  const handleDirectMultiLinePaste = useCallback((text: string) => {
    if (!text) return;
    setMultiLinePasteText(null);
    requestAnimationFrame(() => {
      pasteTextRef.current(text, { skipDialog: true });
    });
  }, []);

  const handleSendMultiLinePasteByLine = useCallback(
    (text: string) => {
      if (!text) return;
      openSendCommandPanel({
        text,
        sourceSessionId: sessionId,
        sourceSessionType: sessionType,
        dataType: "text",
        sendMode: "line",
        count: 1,
        intervalSeconds: 1,
        target: "current",
      });
      setMultiLinePasteText(null);
    },
    [sessionId, sessionType],
  );

  useEffect(() => {
    doFindRef.current = doFind;
  }, [doFind]);

  const { isExternalDropActive, dropOverlayCopy } = useTerminalExternalDrop({
    sessionId,
    sessionType,
    visible,
    containerRef,
    t,
    duplicateStrategy: terminalAppSettings.transfer.duplicate_strategy,
  });

  const terminalBackground = "var(--df-terminal-surface-bg)";

  return (
    <div
      className="nyaterm-wallpaper-transparent-surface nyaterm-terminal-surface h-full w-full relative flex"
      style={{
        display: visible ? "flex" : "none",
        backgroundColor: terminalBackground,
      }}
    >
      {showGutter && terminalReady && !restoringSnapshot && (
        <TerminalGutter
          terminalRef={terminalRef}
          showLineNumbers={showLineNumbers}
          showTimestamps={showTimestamps}
          timestampFormat={timestampFormat}
          lineTimestamps={lineTimestampsRef.current}
          getLineOffset={getGutterLineOffset}
          sessionId={sessionId}
          suspended={performanceMode !== "normal" || !visible}
        />
      )}
      <div
        className="nyaterm-wallpaper-transparent-surface nyaterm-terminal-surface flex-1 min-w-0 h-full relative"
        style={{
          backgroundColor: terminalBackground,
          visibility: restoringSnapshot ? "hidden" : "visible",
        }}
      >
        <TerminalContextMenu
          sessionId={sessionId}
          sessionName={sessionName}
          terminalRef={terminalRef}
          onFind={doFind}
          onPasteText={handlePasteText}
          onPasteClipboard={pasteClipboard}
          onClearAll={handleClearAll}
          recordingStatus={recordingStatus}
          onToggleRecording={onToggleRecording}
          onSaveTranscript={onSaveTranscript}
        >
          <div
            className={`nyaterm-wallpaper-transparent-surface nyaterm-terminal-surface h-full w-full ${
              showContentPadding ? "pl-2" : ""
            }`}
            style={{ backgroundColor: terminalBackground }}
          >
            <div
              ref={containerRef}
              data-terminal-root="true"
              className="nyaterm-wallpaper-transparent-surface nyaterm-terminal-surface h-full w-full"
              style={{ backgroundColor: terminalBackground }}
            />
          </div>
        </TerminalContextMenu>

        {isExternalDropActive && (
          <ExternalFileDropOverlay
            title={dropOverlayCopy.title}
            hint={dropOverlayCopy.hint}
          />
        )}

        {syncOverlay && <SyncActionOverlay overlay={syncOverlay} />}

        <TerminalSearchBar
          show={showSearchBar}
          inputRef={searchInputRef}
          searchQuery={searchQuery}
          searchState={searchState}
          searchFlags={searchFlags}
          activeMode={activeMode}
          historyState={historyState}
          setSearchQuery={handleTerminalSearchQueryChange}
          onModeChange={handleTerminalSearchModeChange}
          onSearchFlagChange={handleTerminalSearchFlagChange}
          onNext={handleSearchNext}
          onPrev={handleSearchPrev}
          onClose={handleTerminalSearchClose}
        />

        <CommandSuggestions
          suggestions={suggestions}
          visible={
            commandSuggestionsEnabled &&
            showSuggestions &&
            canShowCommandSuggestions({ allowEmpty: suggestions.length > 0 })
          }
          selectedIndex={selectedIndex}
          cursorPosition={cursorPosition}
          onSelect={handleSelectSuggestion}
          onDismiss={dismissSuggestions}
          onDeleteHistory={handleDeleteSuggestion}
        />

        <CredentialSuggestions
          panelState={credentialPanelState}
          selectedIndex={credentialSelectedIndex}
          cursorPosition={credentialCursorPosition}
          onSelect={(credential) => void handleCredentialSelect(credential)}
          onDismiss={dismissCredentialPanel}
        />

        <ActionLinkTooltip state={tooltipState} />
        <ActionLinkMenu state={menuState} onClose={closeMenu} />

        <MultiLinePasteDialog
          open={multiLinePasteText !== null}
          text={multiLinePasteText}
          onClose={() => setMultiLinePasteText(null)}
          onDirectPaste={handleDirectMultiLinePaste}
          onSendLineByLine={handleSendMultiLinePasteByLine}
        />
      </div>
    </div>
  );
}
