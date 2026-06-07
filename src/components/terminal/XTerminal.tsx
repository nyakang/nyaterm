import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Terminal } from "@xterm/xterm";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import MultiLinePasteDialog from "@/components/dialog/terminal/MultiLinePasteDialog";
import { useTerminalAppSettings } from "@/context/AppContext";
import { useTheme } from "@/context/ThemeContext";
import { useActionLinks } from "@/hooks/useActionLinks";
import { useCommandHistory } from "@/hooks/useCommandHistory";
import { useCredentialAutofill } from "@/hooks/useCredentialAutofill";
import { useKeywordHighlighter } from "@/hooks/useKeywordHighlighter";
import { useShellIntegration } from "@/hooks/useShellIntegration";
import { resolveShortcutKeys } from "@/hooks/useShortcutMap";
import { useTerminalSearch } from "@/hooks/useTerminalSearch";
import { useTerminalSettings } from "@/hooks/useTerminalSettings";
import { emitAIErrorDetected } from "@/lib/aiEvents";
import { renderAiCommandEnd, renderAiCommandStart } from "@/lib/aiTerminalRenderer";
import { buildTerminalThemeColors, isTerminalTransparencyEnabled } from "@/lib/backgroundImage";
import { readClipboardText } from "@/lib/clipboard";
import { detectCredentialPromptKind } from "@/lib/credentialAutofill";
import { invoke } from "@/lib/invoke";
import { hexLuminance } from "@/lib/keywordHighlightPresets";
import { openSendCommandPanel } from "@/lib/sendCommandPanelEvents";
import {
  buildTerminalCommandInput,
  listenSessionInputPreview,
  normalizeTerminalCommandInput,
  type SessionInputPreview,
  sendSessionInput,
  sendSessionInputWithSync,
} from "@/lib/sessionInput";
import { matchesKeyEvent, resolveIndexedKeys } from "@/lib/shortcutRegistry";
import { registerTerminalContextProvider } from "@/lib/terminalContext";
import {
  applyTerminalInputData,
  applyTerminalInputPreview,
  canSuggestFromTracker,
  createTerminalInputState,
  deleteTerminalInputRange,
  getTrackedSubmissionCommand,
  resyncFromTerminalLine,
} from "@/lib/terminalInputTracker";
import { XTERM_PERFORMANCE_CONFIG } from "@/lib/xtermPerformance";
import type { AiCaptureEvent, SessionType } from "@/types/global";
import ActionLinkMenu from "./ActionLinkMenu";
import ActionLinkTooltip from "./ActionLinkTooltip";
import CommandSuggestions from "./CommandSuggestions";
import CredentialSuggestions from "./CredentialSuggestions";
import SyncActionOverlay from "./SyncActionOverlay";
import TerminalContextMenu from "./TerminalContextMenu";
import TerminalGutter from "./TerminalGutter";
import TerminalSearchBar from "./TerminalSearchBar";
import {
  getInputIndexAtBufferPosition,
  getMouseBufferPosition,
  getSelectedInputRange,
  hasErrorKeyword,
  type InputSelectionRange,
  isMultiLineText,
  isShiftInsertPasteEvent,
  readCurrentInputLine,
  readRecentOutput,
} from "./terminalInputSelection";
import { createTerminalLinkHandlers } from "./terminalLinkHandlers";
import type { PerformanceMode, PerformanceOverlayState, XTerminalProps } from "./xterminalTypes";
import { createZmodemEventHandler, type ZmodemEventPayload } from "./zmodemTerminalEvents";
import "@xterm/xterm/css/xterm.css";

const BACKSPACE_INPUT = "\x7f";

function isLocalBackspaceEvent(event: KeyboardEvent, sessionType: SessionType): boolean {
  if (sessionType !== "Local" || event.ctrlKey || event.metaKey || event.altKey) {
    return false;
  }

  return event.key === "Backspace" || (event.key === "Delete" && event.code === "Backspace");
}

/**
 * xterm.js terminal for a session. Handles OSC 133 shell integration (or fallback prompt
 * detection), fuzzy command history suggestions, and resize/fit. Key props: sessionId, active.
 */
export default function XTerminal({
  sessionId,
  active,
  visible = true,
  sessionType,
  connectionId,
  onReconnected,
  syncPeerSessionIds,
  syncOverlay,
}: XTerminalProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const [terminalReady, setTerminalReady] = useState(false);
  const [performanceMode, setPerformanceMode] = useState<PerformanceMode>("normal");
  const [performanceOverlay, setPerformanceOverlay] = useState<PerformanceOverlayState>(null);
  const [skippedOutputChars, setSkippedOutputChars] = useState(0);
  const [multiLinePasteText, setMultiLinePasteText] = useState<string | null>(null);
  const aiCapturingRef = useRef(false);

  const { terminalTheme } = useTheme();
  const { t } = useTranslation();
  const terminalAppSettings = useTerminalAppSettings();
  const { appearance, interaction, terminal: terminalSettings } = terminalAppSettings;
  const terminalThemeColors = useMemo(
    () => buildTerminalThemeColors(terminalTheme.colors.terminal, appearance),
    [appearance, terminalTheme.colors.terminal],
  );
  const terminalTransparencyEnabled = isTerminalTransparencyEnabled(appearance);
  const showLineNumbers = terminalSettings.show_line_numbers;
  const showTimestamps = terminalSettings.show_timestamps;
  const showGutter = showLineNumbers || showTimestamps;
  const commandSuggestionsEnabled = interaction.command_suggestions_enabled;
  const commandSuggestionMinChars = interaction.command_suggestion_min_chars;
  const commandSuggestionMaxChars = interaction.command_suggestion_max_chars;

  const inputStateRef = useRef(createTerminalInputState());
  const terminalAppSettingsRef = useRef(terminalAppSettings);
  const tRef = useRef(t);
  const doFindRef = useRef<(selection?: string) => void>(() => {});
  const pasteTextRef = useRef<(text: string, options?: { skipDialog?: boolean }) => void>(() => {});
  const executeActionCommandRef = useRef<((command: string) => void) | null>(null);
  const disconnectedRef = useRef(false);
  const reconnectingRef = useRef(false);
  const outputWriteQueueRef = useRef(Promise.resolve());
  const outputWriteInFlightRef = useRef(false);
  const lineTimestampsRef = useRef<Map<number, number>>(new Map());
  const sessionTypeRef = useRef(sessionType);
  const connectionIdRef = useRef(connectionId);
  const onReconnectedRef = useRef(onReconnected);
  const sessionIdRef = useRef(sessionId);
  const syncPeerSessionIdsRef = useRef(syncPeerSessionIds);
  const visibleRef = useRef(visible);
  const activeRef = useRef(active);
  const performanceModeRef = useRef<PerformanceMode>("normal");
  const performanceOverlayTimerRef = useRef<number | null>(null);
  const skippedOutputCharsRef = useRef(0);
  const queuedOutputChunksRef = useRef<string[]>([]);
  const queuedOutputCharsRef = useRef(0);
  const pendingOutputFlushRef = useRef<number | null>(null);
  const handleVisibilityChangeRef = useRef<(() => void) | null>(null);
  const replaceInputCommandRef = useRef<((command: string) => void) | null>(null);
  const lastErrorNoticeAtRef = useRef(0);
  const credentialPromptBufferRef = useRef("");
  const credentialPromptInputUntilRef = useRef(0);

  useEffect(() => {
    sessionTypeRef.current = sessionType;
  }, [sessionType]);

  useEffect(() => {
    connectionIdRef.current = connectionId;
  }, [connectionId]);

  useEffect(() => {
    onReconnectedRef.current = onReconnected;
  }, [onReconnected]);

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
    visibleRef.current = visible;
    handleVisibilityChangeRef.current?.();
  }, [visible]);

  useEffect(() => {
    terminalAppSettingsRef.current = terminalAppSettings;
  }, [terminalAppSettings]);

  useEffect(() => {
    tRef.current = t;
  }, [t]);

  const clearPerformanceOverlayTimer = useCallback(() => {
    if (performanceOverlayTimerRef.current !== null) {
      window.clearTimeout(performanceOverlayTimerRef.current);
      performanceOverlayTimerRef.current = null;
    }
  }, []);

  const enterOverloadedMode = useCallback(() => {
    clearPerformanceOverlayTimer();
    performanceModeRef.current = "overloaded";
    setPerformanceMode("overloaded");
    setPerformanceOverlay("overloaded");
  }, [clearPerformanceOverlayTimer]);

  const exitOverloadedMode = useCallback(() => {
    clearPerformanceOverlayTimer();
    performanceModeRef.current = "normal";
    setPerformanceMode("normal");
    setPerformanceOverlay("recovered");
    performanceOverlayTimerRef.current = window.setTimeout(() => {
      setPerformanceOverlay((current) => (current === "recovered" ? null : current));
      performanceOverlayTimerRef.current = null;
    }, XTERM_PERFORMANCE_CONFIG.output.recoveryNoticeMs);
  }, [clearPerformanceOverlayTimer]);

  const formatSkippedCount = useCallback(
    (value: number) => new Intl.NumberFormat().format(value),
    [],
  );

  // Search Addon state and handlers
  const {
    searchAddonRef,
    showSearchBar,
    setShowSearchBar,
    searchQuery,
    setSearchQuery,
    handleSearchNext,
    handleSearchPrev,
    handleCloseSearch,
  } = useTerminalSearch(terminalRef);

  // Shell integration state
  const { shellIntegrationRef } = useShellIntegration();
  const canShowCommandSuggestions = useCallback(() => {
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

    return canSuggestFromTracker(inputStateRef.current);
  }, [shellIntegrationRef]);

  const applySuggestion = useCallback(
    (command: string, execute: boolean) => {
      const trackedState = inputStateRef.current;
      const replaceCurrentLine = trackedState.lineRewriteRequired;
      const input = replaceCurrentLine
        ? `\u0005\u0015${command}`
        : `${"\x7f".repeat(trackedState.value.length)}${command}`;
      void sendSessionInput(sessionId, buildTerminalCommandInput(input, execute), {
        preview: execute
          ? { kind: "replace-and-execute", value: command }
          : { kind: "replace", value: command },
        registerSubmission: execute ? command : null,
      }).catch(() => {});
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
  } = useCommandHistory(
    terminalRef,
    inputStateRef,
    applySuggestion,
    canShowCommandSuggestions,
    commandSuggestionsEnabled,
    commandSuggestionMinChars,
    commandSuggestionMaxChars,
  );

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
  } = useCredentialAutofill(terminalRef, sessionIdRef, activeRef, visibleRef, performanceModeRef);

  // Create and setup terminal
  // biome-ignore lint/correctness/useExhaustiveDependencies: terminal lifecycle is intentionally scoped to session changes.
  useEffect(() => {
    if (!containerRef.current) return;
    setTerminalReady(false);
    lineTimestampsRef.current = new Map();
    queuedOutputChunksRef.current = [];
    queuedOutputCharsRef.current = 0;
    skippedOutputCharsRef.current = 0;
    outputWriteInFlightRef.current = false;
    outputWriteQueueRef.current = Promise.resolve();
    disconnectedRef.current = false;
    reconnectingRef.current = false;
    performanceModeRef.current = "normal";
    resetCredentialAutofill();
    setPerformanceMode("normal");
    setPerformanceOverlay(null);
    setSkippedOutputChars(0);
    clearPerformanceOverlayTimer();
    let disposed = false;

    const terminal = new Terminal({
      scrollback: terminalSettings.scrollback_lines,
      cursorBlink: appearance.cursor_blink,
      cursorStyle: appearance.cursor_style as "block" | "underline" | "bar",
      fontSize: appearance.font_size,
      fontFamily: appearance.font_family,
      wordSeparator: interaction.word_separators,
      theme: { ...terminalThemeColors },
      allowTransparency: terminalTransparencyEnabled,
      allowProposedApi: true,
    });

    const fitAddon = new FitAddon();
    const {
      oscLinkHandler,
      webLinksAddon,
      removePopup: removeLinkPopup,
    } = createTerminalLinkHandlers(terminal, tRef);
    const searchAddon = new SearchAddon();
    const zmodemHandler = createZmodemEventHandler(terminal, sessionId, () => tRef.current);

    terminal.options.linkHandler = oscLinkHandler;
    terminal.loadAddon(fitAddon);
    terminal.loadAddon(webLinksAddon);
    terminal.loadAddon(searchAddon);
    terminal.open(containerRef.current);

    searchAddonRef.current = searchAddon;

    terminalRef.current = terminal;
    fitAddonRef.current = fitAddon;
    inputStateRef.current = createTerminalInputState();
    credentialPromptBufferRef.current = "";
    credentialPromptInputUntilRef.current = 0;
    shellIntegrationRef.current.enabled = false;
    shellIntegrationRef.current.commandRunning = false;
    const isTerminalAlive = () => !disposed && terminalRef.current === terminal;
    const syncSuggestionsWithInputState = () => {
      if (canShowCommandSuggestions()) {
        triggerSearch();
      } else {
        dismissSuggestions();
      }
    };

    const sendRawInput = (data: string, command: string | null) => {
      const peers = syncPeerSessionIdsRef.current;
      if (peers && peers.length > 0) {
        return sendSessionInputWithSync(sessionId, data, peers, {
          preview: null,
          registerSubmission: command,
        }).catch(() => {});
      } else {
        return sendSessionInput(sessionId, data, {
          preview: null,
          registerSubmission: command,
        }).catch(() => {});
      }
    };

    const canReconnectDisconnectedSession = () =>
      sessionTypeRef.current === "Local" || !!connectionIdRef.current;

    const createReconnectedSession = () => {
      const connectionId = connectionIdRef.current;

      switch (sessionTypeRef.current) {
        case "Local":
          return invoke<string>("create_local_session", { connectionId: connectionId || null });
        case "Telnet":
          return invoke<string>("create_telnet_session", { connectionId });
        case "Serial":
          return invoke<string>("create_serial_session", { connectionId });
        default:
          return invoke<string>("create_ssh_session", { connectionId });
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
      inputStateRef.current = applyTerminalInputData(inputStateRef.current, "\r");
      syncSuggestionsWithInputState();
      await sendSessionInput(sessionId, "\r", {
        preview: null,
        registerSubmission: command,
      });
    };

    replaceInputCommandRef.current = replaceInputCommand;

    const unregisterTerminalContext = registerTerminalContextProvider(sessionId, {
      getRecentOutput: (lineLimit) => readRecentOutput(terminal, lineLimit),
      getSelectedText: () => terminal.getSelection(),
      getInputBuffer: () => inputStateRef.current.value,
      insertCommand: async (command) => {
        const input = buildReplaceInputData(command);
        await sendSessionInput(sessionId, normalizeTerminalCommandInput(input), {
          preview: { kind: "replace", value: command },
          registerSubmission: null,
        });
      },
      executeCommand: async (command) => {
        await executeInputCommand(command);
      },
      focus: () => terminal.focus(),
    });

    const handleInputPreview = (preview: SessionInputPreview) => {
      inputStateRef.current = applyTerminalInputPreview(inputStateRef.current, preview);
      syncSuggestionsWithInputState();
    };

    executeActionCommandRef.current = (command: string) => {
      void executeInputCommand(command).catch(() => {});
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
      if (shellIntegration.enabled && shellIntegration.commandRunning) return false;

      if (state.desynced || state.lineRewriteRequired || state.pasteMode || state.multiline) {
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

    const pasteText = (text: string, options: { skipDialog?: boolean } = {}) => {
      if (!text) return;
      terminal.focus();
      const showMultiLinePasteDialog =
        terminalAppSettingsRef.current?.terminal?.show_multi_line_paste_dialog ?? true;
      if (
        !options.skipDialog &&
        showMultiLinePasteDialog &&
        sessionTypeRef.current !== "Serial" &&
        isMultiLineText(text)
      ) {
        setMultiLinePasteText(text);
        return;
      }

      const selectedInputRange = getSmartCursorSelectedInputRange();
      let pendingSelectionDelete: Promise<void> | null = null;
      if (selectedInputRange) {
        const currentCursor = inputStateRef.current.cursor;
        const moveToSelectionEnd = buildMoveInputCursorData(currentCursor, selectedInputRange.end);
        const deleteSelection = "\u007f".repeat(selectedInputRange.end - selectedInputRange.start);
        inputStateRef.current = deleteTerminalInputRange(
          inputStateRef.current,
          selectedInputRange.start,
          selectedInputRange.end,
        );
        terminal.clearSelection();
        syncSuggestionsWithInputState();
        if (moveToSelectionEnd || deleteSelection) {
          pendingSelectionDelete = sendRawInput(`${moveToSelectionEnd}${deleteSelection}`, null);
        }
      }

      const runPaste = () => {
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

    let lastSelection = "";
    let primaryMouseDown: { x: number; y: number } | null = null;

    const buildMoveInputCursorData = (currentCursor: number, targetCursor: number) => {
      if (targetCursor === currentCursor) return "";
      return targetCursor > currentCursor
        ? "\x1b[C".repeat(targetCursor - currentCursor)
        : "\x1b[D".repeat(currentCursor - targetCursor);
    };

    const moveInputCursor = (targetCursor: number) => {
      const currentState = inputStateRef.current;
      if (!canUseSmartCursor(currentState)) return;
      const nextCursor = Math.max(0, Math.min(currentState.value.length, targetCursor));
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
      const targetCursor = edge === "start" ? selectedInputRange.start : selectedInputRange.end;
      const input = buildMoveInputCursorData(currentState.cursor, targetCursor);

      inputStateRef.current = { ...currentState, cursor: targetCursor };
      terminal.clearSelection();
      syncSuggestionsWithInputState();
      if (input) sendRawInput(input, null);
    };

    const deleteInputSelection = (selectedInputRange: InputSelectionRange) => {
      if (!canUseSmartCursor()) return;
      const currentCursor = inputStateRef.current.cursor;
      const moveToSelectionEnd = buildMoveInputCursorData(currentCursor, selectedInputRange.end);
      const deleteSelection = "\u007f".repeat(selectedInputRange.end - selectedInputRange.start);

      inputStateRef.current = deleteTerminalInputRange(
        inputStateRef.current,
        selectedInputRange.start,
        selectedInputRange.end,
      );
      terminal.clearSelection();
      syncSuggestionsWithInputState();
      sendRawInput(`${moveToSelectionEnd}${deleteSelection}`, null);
    };

    const replaceInputSelection = (selectedInputRange: InputSelectionRange, data: string) => {
      if (!canUseSmartCursor()) return;
      const currentCursor = inputStateRef.current.cursor;
      const moveToSelectionEnd = buildMoveInputCursorData(currentCursor, selectedInputRange.end);
      const deleteSelection = "\u007f".repeat(selectedInputRange.end - selectedInputRange.start);
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

    const getDirectInputDataFromKeyEvent = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return null;
      if (e.key === "Dead" || e.key === "Process" || e.key === "Unidentified") return null;
      if (Array.from(e.key).length !== 1) return null;
      if (/[\x00-\x1f\x7f]/u.test(e.key)) return null;
      return e.key;
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

    terminal.attachCustomKeyEventHandler((e) => {
      if (e.type !== "keydown") return true;
      const kb = terminalAppSettingsRef.current.keybindings;

      if (isShiftInsertPasteEvent(e)) {
        e.preventDefault();
        readClipboardText()
          .then((text) => {
            pasteText(text);
          })
          .catch(() => {});
        return false;
      }

      if (isLocalBackspaceEvent(e, sessionTypeRef.current)) {
        e.preventDefault();
        if (isCredentialPromptInputMode()) {
          sendRawInput(BACKSPACE_INPUT, null);
          return false;
        }

        const selectedInputRange = getSmartCursorSelectedInputRange();
        if (selectedInputRange) {
          deleteInputSelection(selectedInputRange);
          return false;
        }

        inputStateRef.current = applyTerminalInputData(inputStateRef.current, BACKSPACE_INPUT);
        syncSuggestionsWithInputState();
        sendRawInput(BACKSPACE_INPUT, null);
        return false;
      }

      if (
        (e.key === "ArrowLeft" || e.key === "ArrowRight") &&
        !e.ctrlKey &&
        !e.metaKey &&
        !e.altKey &&
        !e.shiftKey
      ) {
        const selectedInputRange = getSmartCursorSelectedInputRange();
        if (selectedInputRange) {
          e.preventDefault();
          collapseInputSelection(selectedInputRange, e.key === "ArrowLeft" ? "start" : "end");
          return false;
        }
      }

      if ((e.key === "Backspace" || e.key === "Delete") && !e.ctrlKey && !e.metaKey && !e.altKey) {
        const selectedInputRange = getSmartCursorSelectedInputRange();
        if (selectedInputRange) {
          e.preventDefault();
          deleteInputSelection(selectedInputRange);
          return false;
        }
      }

      const directInputData = getDirectInputDataFromKeyEvent(e);
      if (directInputData) {
        const selectedInputRange = getSmartCursorSelectedInputRange();
        if (selectedInputRange) {
          e.preventDefault();
          replaceInputSelection(selectedInputRange, directInputData);
          return false;
        }
      }

      if (matchesKeyEvent(resolveShortcutKeys("terminal.copy", kb), e)) {
        e.preventDefault();
        const sel = terminal.getSelection();
        if (sel) navigator.clipboard.writeText(sel).catch(() => {});
        return false;
      }
      if (matchesKeyEvent(resolveShortcutKeys("terminal.paste", kb), e)) {
        e.preventDefault();
        readClipboardText()
          .then((text) => {
            pasteText(text);
          })
          .catch(() => {});
        return false;
      }
      if (matchesKeyEvent(resolveShortcutKeys("terminal.find", kb), e)) {
        e.preventDefault();
        doFindRef.current();
        return false;
      }
      if (matchesKeyEvent(resolveShortcutKeys("terminal.clear", kb), e)) {
        e.preventDefault();
        terminal.clear();
        return false;
      }
      if (matchesKeyEvent(resolveShortcutKeys("terminal.pasteSelected", kb), e)) {
        e.preventDefault();
        const sel = terminal.getSelection() || lastSelection;
        pasteText(sel);
        return false;
      }
      if (matchesKeyEvent(resolveShortcutKeys("terminal.selectAll", kb), e)) {
        e.preventDefault();
        terminal.selectAll();
        return false;
      }

      const swallowIds = [
        "tab.newSession",
        "tab.close",
        "tab.next",
        "tab.prev",
        "tab.newLocalTerminal",
        "view.toggleLeftSidebar",
        "view.toggleRightSidebar",
        "view.zoomIn",
        "view.zoomOut",
        "view.resetZoom",
        "view.openSettings",
        "terminal.manageSyncGroups",
        "special.lockScreen",
      ];
      for (const sid of swallowIds) {
        if (matchesKeyEvent(resolveShortcutKeys(sid, kb), e)) {
          return false;
        }
      }
      for (let tabNumber = 1; tabNumber <= 9; tabNumber += 1) {
        if (
          matchesKeyEvent(resolveIndexedKeys(resolveShortcutKeys("tab.switchTo", kb), tabNumber), e)
        ) {
          return false;
        }
      }

      return true;
    });

    const oscDisposable = terminal.parser.registerOscHandler(133, (data) => {
      const si = shellIntegrationRef.current;

      if (data.startsWith("A")) {
        si.enabled = true;
        return false;
      }

      if (data.startsWith("B")) {
        si.enabled = true;
        return false;
      }

      if (data.startsWith("C")) {
        si.enabled = true;
        si.commandRunning = true;
        inputStateRef.current = createTerminalInputState();
        dismissSuggestions();
        return false;
      }

      if (data.startsWith("D")) {
        si.enabled = true;
        si.commandRunning = false;
        return false;
      }

      return false;
    });

    const writeParsedDisposable = terminal.onWriteParsed(() => {
      if (terminal.buffer.active.type === "alternate") {
        dismissSuggestions();
      }
      const terminalSettings = terminalAppSettingsRef.current?.terminal;
      if (
        performanceModeRef.current !== "overloaded" &&
        (terminalSettings?.show_line_numbers || terminalSettings?.show_timestamps)
      ) {
        window.dispatchEvent(new CustomEvent("nyaterm:refresh-gutter", { detail: { sessionId } }));
      }
    });

    let outputUnlisten: UnlistenFn | null = null;
    let closedUnlisten: UnlistenFn | null = null;
    let focusUnlisten: UnlistenFn | null = null;
    let captureUnlisten: UnlistenFn | null = null;
    let zmodemUnlisten: UnlistenFn | null = null;

    const clearCredentialPromptInputMode = () => {
      credentialPromptBufferRef.current = "";
      credentialPromptInputUntilRef.current = 0;
    };

    const updateCredentialPromptInputMode = (payload: string) => {
      credentialPromptBufferRef.current = `${credentialPromptBufferRef.current}${payload}`.slice(
        -4096,
      );

      if (detectCredentialPromptKind(credentialPromptBufferRef.current)) {
        credentialPromptInputUntilRef.current = Date.now() + 120_000;
        dismissSuggestions();
        return;
      }

      if (/[\r\n]/u.test(payload)) {
        credentialPromptInputUntilRef.current = 0;
      }
    };

    const refreshGutter = () => {
      if (!isTerminalAlive()) return;
      if (performanceModeRef.current === "overloaded") return;
      const terminalSettings = terminalAppSettingsRef.current?.terminal;
      if (!terminalSettings?.show_line_numbers && !terminalSettings?.show_timestamps) return;
      window.dispatchEvent(new CustomEvent("nyaterm:refresh-gutter", { detail: { sessionId } }));
    };

    const stampWrittenLines = (from: number, to: number, ts: number) => {
      if (!terminalAppSettingsRef.current?.terminal?.show_timestamps) return;

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

      if (performanceModeRef.current !== "overloaded") {
        refreshGutter();
      }
    };

    const getBacklogCap = () =>
      visibleRef.current
        ? XTERM_PERFORMANCE_CONFIG.output.visibleBacklogCap
        : XTERM_PERFORMANCE_CONFIG.output.hiddenBacklogCap;

    const getRecoveryThreshold = () =>
      visibleRef.current
        ? XTERM_PERFORMANCE_CONFIG.output.visibleRecoveryThreshold
        : XTERM_PERFORMANCE_CONFIG.output.hiddenRecoveryThreshold;

    const noteSkippedOutput = (count: number) => {
      if (count <= 0) return;
      skippedOutputCharsRef.current += count;
      setSkippedOutputChars(skippedOutputCharsRef.current);
      enterOverloadedMode();
    };

    const trimQueuedOutput = (maxChars: number) => {
      let dropped = 0;

      while (queuedOutputCharsRef.current > maxChars && queuedOutputChunksRef.current.length > 0) {
        const overflow = queuedOutputCharsRef.current - maxChars;
        const chunk = queuedOutputChunksRef.current[0];
        if (!chunk) break;

        if (chunk.length <= overflow) {
          queuedOutputChunksRef.current.shift();
          queuedOutputCharsRef.current -= chunk.length;
          dropped += chunk.length;
          continue;
        }

        queuedOutputChunksRef.current[0] = chunk.slice(overflow);
        queuedOutputCharsRef.current -= overflow;
        dropped += overflow;
      }

      return dropped;
    };

    const dequeueOutputChunk = (maxChars: number) => {
      if (maxChars <= 0 || queuedOutputChunksRef.current.length === 0) {
        return "";
      }

      let remaining = maxChars;
      const parts: string[] = [];

      while (remaining > 0 && queuedOutputChunksRef.current.length > 0) {
        const chunk = queuedOutputChunksRef.current[0];
        if (!chunk) break;

        if (chunk.length <= remaining) {
          parts.push(chunk);
          queuedOutputChunksRef.current.shift();
          queuedOutputCharsRef.current -= chunk.length;
          remaining -= chunk.length;
          continue;
        }

        parts.push(chunk.slice(0, remaining));
        queuedOutputChunksRef.current[0] = chunk.slice(remaining);
        queuedOutputCharsRef.current -= remaining;
        remaining = 0;
      }

      return parts.join("");
    };

    const maybeRecoverPerformanceMode = () => {
      if (!isTerminalAlive()) return;
      if (performanceModeRef.current !== "overloaded") return;
      if (queuedOutputCharsRef.current > getRecoveryThreshold()) return;
      exitOverloadedMode();
    };

    const writeChunkToTerminal = (payload: string) => {
      outputWriteInFlightRef.current = true;
      outputWriteQueueRef.current = outputWriteQueueRef.current
        .catch(() => {})
        .then(
          () =>
            new Promise<void>((resolve) => {
              if (!isTerminalAlive()) {
                outputWriteInFlightRef.current = false;
                resolve();
                return;
              }

              const ts = Date.now();
              const beforeLine = terminal.buffer.active.baseY + terminal.buffer.active.cursorY;

              try {
                terminal.write(payload, () => {
                  outputWriteInFlightRef.current = false;

                  if (!isTerminalAlive()) {
                    resolve();
                    return;
                  }

                  const afterLine = terminal.buffer.active.baseY + terminal.buffer.active.cursorY;

                  stampWrittenLines(beforeLine, afterLine, ts);

                  maybeRecoverPerformanceMode();
                  resolve();

                  if (
                    visibleRef.current &&
                    isTerminalAlive() &&
                    queuedOutputCharsRef.current > 0 &&
                    pendingOutputFlushRef.current === null
                  ) {
                    pendingOutputFlushRef.current = requestAnimationFrame(flushPendingOutput);
                  }
                });
              } catch {
                outputWriteInFlightRef.current = false;
                maybeRecoverPerformanceMode();
                resolve();
              }
            }),
        );
    };

    const flushPendingOutput = () => {
      pendingOutputFlushRef.current = null;
      if (!visibleRef.current || !isTerminalAlive() || outputWriteInFlightRef.current) {
        return;
      }

      const deadline = performance.now() + XTERM_PERFORMANCE_CONFIG.output.frameBudgetMs;
      let payload = "";
      while (!payload && performance.now() < deadline) {
        payload = dequeueOutputChunk(XTERM_PERFORMANCE_CONFIG.output.writeChunkChars);
        if (!payload) break;
      }

      if (!payload) {
        maybeRecoverPerformanceMode();
        return;
      }

      writeChunkToTerminal(payload);
    };

    const schedulePendingOutputFlush = () => {
      if (!visibleRef.current || !isTerminalAlive()) return;
      if (outputWriteInFlightRef.current || pendingOutputFlushRef.current !== null) return;
      pendingOutputFlushRef.current = requestAnimationFrame(flushPendingOutput);
    };

    const applyVisibilityPolicy = () => {
      if (!isTerminalAlive()) return;

      if (!visibleRef.current && pendingOutputFlushRef.current !== null) {
        cancelAnimationFrame(pendingOutputFlushRef.current);
        pendingOutputFlushRef.current = null;
      }

      const dropped = trimQueuedOutput(getBacklogCap());
      noteSkippedOutput(dropped);
      maybeRecoverPerformanceMode();

      if (visibleRef.current) {
        schedulePendingOutputFlush();
      }
    };

    handleVisibilityChangeRef.current = applyVisibilityPolicy;

    const setupListeners = async () => {
      const nextOutputUnlisten = await listen<string>(`terminal-output-${sessionId}`, (event) => {
        if (!isTerminalAlive()) return;
        queuedOutputChunksRef.current.push(event.payload);
        queuedOutputCharsRef.current += event.payload.length;
        updateCredentialPromptInputMode(event.payload);
        feedCredentialOutput(event.payload);
        if (visibleRef.current && hasErrorKeyword(event.payload)) {
          const now = Date.now();
          if (now - lastErrorNoticeAtRef.current > 30_000) {
            lastErrorNoticeAtRef.current = now;
            emitAIErrorDetected({ sessionId, output: event.payload.slice(-4000) });
          }
        }

        const dropped = trimQueuedOutput(getBacklogCap());
        noteSkippedOutput(dropped);

        if (!visibleRef.current) {
          window.dispatchEvent(
            new CustomEvent("nyaterm:session-output", { detail: { sessionId } }),
          );
          return;
        }

        schedulePendingOutputFlush();
      });
      if (disposed) {
        nextOutputUnlisten();
        return;
      }
      outputUnlisten = nextOutputUnlisten;

      const nextClosedUnlisten = await listen<void>(`session-closed-${sessionId}`, () => {
        if (!isTerminalAlive()) return;
        disconnectedRef.current = true;
        terminal.write(`\r\n\x1b[31m[${tRef.current("terminal.sessionDisconnected")}]\x1b[0m\r\n`);
        if (canReconnectDisconnectedSession()) {
          terminal.write(`\x1b[33m[${tRef.current("terminal.pressEnterToReconnect")}]\x1b[0m\r\n`);
        }
        inputStateRef.current = createTerminalInputState();
        clearCredentialPromptInputMode();
        dismissSuggestions();
      });
      if (disposed) {
        nextClosedUnlisten();
        return;
      }
      closedUnlisten = nextClosedUnlisten;

      const nextFocusUnlisten = await listen<void>(`focus-terminal-${sessionId}`, () => {
        if (!isTerminalAlive()) return;
        terminal.focus();
      });
      if (disposed) {
        nextFocusUnlisten();
        return;
      }
      focusUnlisten = nextFocusUnlisten;

      const nextCaptureUnlisten = await listen<AiCaptureEvent>(
        `ai-capture-${sessionId}`,
        (event) => {
          const payload = event.payload;
          if (payload.type === "commandStart") {
            aiCapturingRef.current = true;
            if (isTerminalAlive()) {
              terminal.write(renderAiCommandStart(payload));
            }
          } else if (payload.type === "commandEnd") {
            aiCapturingRef.current = false;
            if (isTerminalAlive()) {
              terminal.write(renderAiCommandEnd(payload));
            }
          }
        },
      );
      if (disposed) {
        nextCaptureUnlisten();
        return;
      }
      captureUnlisten = nextCaptureUnlisten;

      const nextZmodemUnlisten = await listen<ZmodemEventPayload>(
        `zmodem-event-${sessionId}`,
        (event) => {
          if (!isTerminalAlive()) return;
          zmodemHandler.handle(event.payload);
        },
      );
      if (disposed) {
        nextZmodemUnlisten();
        return;
      }
      zmodemUnlisten = nextZmodemUnlisten;

      try {
        await invoke("attach_session", { sessionId });
      } catch {
        // The session may already be gone during mount/unmount races.
      }
    };
    void setupListeners();

    const removePreviewListener = listenSessionInputPreview(sessionId, handleInputPreview);

    const dataDisposable = terminal.onData((data) => {
      if (aiCapturingRef.current) return;

      if (disconnectedRef.current) {
        if (data === "\r" && canReconnectDisconnectedSession() && !reconnectingRef.current) {
          reconnectingRef.current = true;
          terminal.write(`\r\n\x1b[36m[${tRef.current("terminal.reconnecting")}]\x1b[0m\r\n`);
          createReconnectedSession()
            .then((newSessionId) => {
              disconnectedRef.current = false;
              reconnectingRef.current = false;
              onReconnectedRef.current?.(sessionIdRef.current, newSessionId);
            })
            .catch((err) => {
              reconnectingRef.current = false;
              terminal.write(
                `\r\n\x1b[31m[${tRef.current("terminal.reconnectFailed")}: ${err}]\x1b[0m\r\n`,
              );
              terminal.write(
                `\x1b[33m[${tRef.current("terminal.pressEnterToReconnect")}]\x1b[0m\r\n`,
              );
            });
        }
        return;
      }

      if (credentialShowPanelRef.current && credentialMatchesRef.current.length > 0) {
        if (data === "\x1b[A") {
          const cur = credentialSelectedIndexRef.current;
          const len = credentialMatchesRef.current.length;
          const next = cur <= 0 ? len - 1 : cur - 1;
          credentialSelectedIndexRef.current = next;
          setCredentialSelectedIndex(next);
          return;
        }
        if (data === "\x1b[B") {
          const cur = credentialSelectedIndexRef.current;
          const len = credentialMatchesRef.current.length;
          const next = cur >= len - 1 ? 0 : cur + 1;
          credentialSelectedIndexRef.current = next;
          setCredentialSelectedIndex(next);
          return;
        }
        if (data === "\r" && credentialSelectedIndexRef.current >= 0) {
          const selected = credentialMatchesRef.current[credentialSelectedIndexRef.current];
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
        if (data === "\t" && selectedIndexRef.current >= 0) {
          const selected = suggestionsRef.current[selectedIndexRef.current];
          if (selected) {
            applySuggestion(selected.command, false);
            dismissSuggestions();
          }
          return;
        }

        if (data === "\x1b[A") {
          const cur = selectedIndexRef.current;
          const newIdx = cur === -1 ? suggestionsRef.current.length - 1 : cur === 0 ? -1 : cur - 1;
          selectedIndexRef.current = newIdx;
          setSelectedIndex(newIdx);
          return;
        }

        if (data === "\x1b[B") {
          const cur = selectedIndexRef.current;
          const newIdx = cur === -1 ? 0 : cur === suggestionsRef.current.length - 1 ? -1 : cur + 1;
          selectedIndexRef.current = newIdx;
          setSelectedIndex(newIdx);
          return;
        }

        if (data === "\x1b") {
          dismissSuggestions();
          return;
        }

        if (data === "\r" && selectedIndexRef.current >= 0) {
          const selected = suggestionsRef.current[selectedIndexRef.current];
          if (selected) {
            applySuggestion(selected.command, true);
            dismissSuggestions();
          }
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

      if (isPlainTextInputData(data)) {
        const selectedInputRange = getSmartCursorSelectedInputRange();
        if (selectedInputRange) {
          replaceInputSelection(selectedInputRange, data);
          return;
        }
      }

      const command = data === "\r" ? getTrackedSubmissionCommand(inputStateRef.current) : "";
      inputStateRef.current = applyTerminalInputData(inputStateRef.current, data);
      if (data === "\r" || data === "\u0003") {
        clearCredentialPromptInputMode();
      }
      syncSuggestionsWithInputState();
      sendRawInput(data, data === "\r" && command ? command : null);
    });

    const resizeDisposable = terminal.onResize(({ cols, rows }) => {
      invoke("resize_session", { sessionId, cols, rows }).catch(() => {});
      refreshGutter();
    });

    const scrollDisposable = terminal.onScroll(() => {
      removeLinkPopup();
      refreshGutter();
    });

    const observer = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry || entry.contentRect.width === 0 || entry.contentRect.height === 0) return;
      requestAnimationFrame(() => {
        fitAddon.fit();
      });
    });
    observer.observe(containerRef.current);

    const selectionDisposable = terminal.onSelectionChange(() => {
      const text = terminal.getSelection();
      if (text) {
        lastSelection = text;
      }
      if (terminalAppSettingsRef.current?.interaction?.copy_on_select) {
        if (text) {
          navigator.clipboard.writeText(text).catch(() => {});
        }
      }
    });

    const handleTerminalMouseDown = (e: MouseEvent) => {
      removeLinkPopup();

      if (e.button === 0 && !e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey) {
        primaryMouseDown = { x: e.clientX, y: e.clientY };
      } else {
        primaryMouseDown = null;
      }

      if (e.button === 1) e.preventDefault(); // Prevent auto-scroll mechanism
    };

    const handleTerminalMouseUp = (e: MouseEvent) => {
      if (e.button === 0) {
        const down = primaryMouseDown;
        primaryMouseDown = null;
        const isPlainPrimaryMouseUp =
          down &&
          !disconnectedRef.current &&
          !aiCapturingRef.current &&
          !e.ctrlKey &&
          !e.metaKey &&
          !e.altKey &&
          !e.shiftKey;
        const isStationaryMouseUp =
          !!down && Math.abs(e.clientX - down.x) <= 4 && Math.abs(e.clientY - down.y) <= 4;

        if (isPlainPrimaryMouseUp && terminal.hasSelection()) {
          const selectedInputRange = getSmartCursorSelectedInputRange();
          if (selectedInputRange) {
            if (e.detail >= 2 || isStationaryMouseUp) {
              moveInputCursorAfterSelection(selectedInputRange, selectedInputRange.end);
            } else {
              const position = getMouseBufferPosition(terminal, e);
              const targetCursor = position
                ? getInputIndexAtBufferPosition(terminal, inputStateRef.current, position)
                : null;
              if (targetCursor !== null) {
                moveInputCursorAfterSelection(selectedInputRange, targetCursor);
              }
            }
          }
        } else if (isPlainPrimaryMouseUp && isStationaryMouseUp) {
          const state = inputStateRef.current;
          if (canUseSmartCursor(state)) {
            const position = getMouseBufferPosition(terminal, e);
            if (position) {
              const targetCursor = getInputIndexAtBufferPosition(terminal, state, position);
              if (targetCursor !== null) {
                moveInputCursor(targetCursor);
              }
            }
          }
        }
        return;
      }

      if (e.button !== 1) return;
      e.preventDefault();
      const sel = terminal.getSelection();
      if (sel) {
        pasteText(sel);
      } else {
        readClipboardText()
          .then((text) => {
            pasteText(text);
          })
          .catch(() => {});
      }
    };

    containerRef.current.addEventListener("mousedown", handleTerminalMouseDown);
    containerRef.current.addEventListener("mouseup", handleTerminalMouseUp);
    const containerEl = containerRef.current;

    requestAnimationFrame(() => {
      if (!isTerminalAlive()) return;
      fitAddon.fit();
      requestAnimationFrame(() => {
        if (!isTerminalAlive()) return;
        terminal.refresh(0, Math.max(0, terminal.rows - 1));
        setTerminalReady(true);
        refreshGutter();
      });
    });

    return () => {
      disposed = true;
      handleVisibilityChangeRef.current = null;
      setTerminalReady(false);
      containerEl.removeEventListener("mousedown", handleTerminalMouseDown);
      containerEl.removeEventListener("mouseup", handleTerminalMouseUp);
      if (searchTimerRef.current) clearTimeout(searchTimerRef.current);
      inputStateRef.current = createTerminalInputState();
      clearCredentialPromptInputMode();
      shellIntegrationRef.current.enabled = false;
      shellIntegrationRef.current.commandRunning = false;
      executeActionCommandRef.current = null;
      replaceInputCommandRef.current = null;
      pasteTextRef.current = () => {};
      resetCredentialAutofill();

      oscDisposable.dispose();
      writeParsedDisposable.dispose();
      dataDisposable.dispose();
      resizeDisposable.dispose();
      scrollDisposable.dispose();
      selectionDisposable.dispose();
      removeLinkPopup();
      removePreviewListener();
      unregisterTerminalContext();

      observer.disconnect();
      if (outputUnlisten) outputUnlisten();
      if (closedUnlisten) closedUnlisten();
      if (focusUnlisten) focusUnlisten();
      if (captureUnlisten) captureUnlisten();
      if (zmodemUnlisten) zmodemUnlisten();
      zmodemHandler.dispose();
      if (pendingOutputFlushRef.current !== null) {
        cancelAnimationFrame(pendingOutputFlushRef.current);
        pendingOutputFlushRef.current = null;
      }
      clearPerformanceOverlayTimer();
      queuedOutputChunksRef.current = [];
      queuedOutputCharsRef.current = 0;
      skippedOutputCharsRef.current = 0;
      outputWriteInFlightRef.current = false;
      outputWriteQueueRef.current = Promise.resolve();
      terminal.dispose();
      terminalRef.current = null;
      fitAddonRef.current = null;
      searchAddonRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  // Appearance, theme, and interaction settings sync.
  // Declared AFTER the terminal creation effect so effects from these hooks
  // run after terminalRef.current is already set on initial mount.
  useTerminalSettings(
    terminalRef,
    fitAddonRef,
    terminalThemeColors,
    appearance,
    terminalSettings,
    interaction,
  );

  // isDark is derived from the terminal theme background so built-in rule colors
  // switch automatically when the user changes themes.
  const isDark = hexLuminance(terminalTheme.colors.terminal.background) < 0.5;
  useKeywordHighlighter(
    terminalRef,
    terminalSettings,
    sessionId,
    isDark,
    performanceMode === "overloaded",
  );

  const { tooltipState, menuState, closeMenu } = useActionLinks(
    terminalRef,
    terminalSettings,
    sessionId,
    executeActionCommandRef,
    performanceMode === "overloaded",
  );

  useEffect(() => {
    if (terminalReady && fitAddonRef.current && terminalRef.current) {
      requestAnimationFrame(() => {
        fitAddonRef.current?.fit();
        terminalRef.current?.refresh(0, Math.max(0, terminalRef.current.rows - 1));
        if (showGutter && performanceMode !== "overloaded") {
          window.dispatchEvent(
            new CustomEvent("nyaterm:refresh-gutter", { detail: { sessionId } }),
          );
        }
      });
    }
  }, [performanceMode, sessionId, showGutter, terminalReady]);

  // Re-fit and focus when tab becomes active
  useEffect(() => {
    if (active && visible && fitAddonRef.current && terminalRef.current) {
      requestAnimationFrame(() => {
        fitAddonRef.current?.fit();
        const terminal = terminalRef.current;
        if (!terminal) return;
        terminal.clearTextureAtlas();
        terminal.refresh(0, Math.max(0, terminal.rows - 1));
        terminal.focus();
      });
    }
  }, [active, visible]);

  useEffect(() => {
    const handleRefresh = () => {
      if (!visible || !fitAddonRef.current || !terminalRef.current) return;

      requestAnimationFrame(() => {
        fitAddonRef.current?.fit();
        terminalRef.current?.refresh(0, Math.max(0, terminalRef.current.rows - 1));
        if (active) {
          terminalRef.current?.focus();
        }
      });
    };

    window.addEventListener("nyaterm:refresh-terminals", handleRefresh);
    return () => {
      window.removeEventListener("nyaterm:refresh-terminals", handleRefresh);
    };
  }, [active, visible]);

  useEffect(() => {
    const handleClear = () => {
      if (!active || !terminalRef.current) return;
      terminalRef.current.clear();
    };

    window.addEventListener("nyaterm:clear-terminal", handleClear);
    return () => {
      window.removeEventListener("nyaterm:clear-terminal", handleClear);
    };
  }, [active]);

  const doFind = useCallback(
    (selection?: string) => {
      if (selection) {
        setSearchQuery(selection);
        setTimeout(() => searchAddonRef.current?.findNext(selection), 50);
      }
      setShowSearchBar(true);
      terminalRef.current?.focus();
    },
    [setShowSearchBar, setSearchQuery, searchAddonRef],
  );

  const handlePasteText = useCallback((text: string) => {
    pasteTextRef.current(text);
  }, []);

  const handleDirectMultiLinePaste = useCallback(() => {
    if (!multiLinePasteText) return;
    const text = multiLinePasteText;
    setMultiLinePasteText(null);
    requestAnimationFrame(() => {
      pasteTextRef.current(text, { skipDialog: true });
    });
  }, [multiLinePasteText]);

  const handleSendMultiLinePasteByLine = useCallback(() => {
    if (!multiLinePasteText) return;
    openSendCommandPanel({
      text: multiLinePasteText,
      sourceSessionId: sessionId,
      sourceSessionType: sessionType,
      dataType: "text",
      sendMode: "line",
      count: 1,
      intervalSeconds: 1,
      target: "current",
    });
    setMultiLinePasteText(null);
  }, [multiLinePasteText, sessionId, sessionType]);

  useEffect(() => {
    doFindRef.current = doFind;
  }, [doFind]);

  return (
    <div className="h-full w-full relative flex" style={{ display: visible ? "flex" : "none" }}>
      {showGutter && terminalReady && (
        <TerminalGutter
          terminalRef={terminalRef}
          showLineNumbers={showLineNumbers}
          showTimestamps={showTimestamps}
          lineTimestamps={lineTimestampsRef.current}
          sessionId={sessionId}
          suspended={performanceMode === "overloaded"}
        />
      )}
      <div className="flex-1 min-w-0 h-full relative">
        <TerminalContextMenu
          terminalRef={terminalRef}
          onFind={doFind}
          onPasteText={handlePasteText}
        >
          <div ref={containerRef} className="h-full w-full" />
        </TerminalContextMenu>

        {performanceOverlay && (
          <div className="pointer-events-none absolute inset-x-3 top-3 z-20 flex justify-end">
            <div
              className="max-w-sm rounded-md border px-3 py-2 text-xs shadow-lg"
              style={{
                borderColor: "var(--df-border)",
                backgroundColor: "var(--df-bg-panel)",
                color: "var(--df-text)",
              }}
            >
              <div className="font-medium">
                {performanceOverlay === "overloaded"
                  ? t("terminal.largeOutputProtectionActive")
                  : t("terminal.largeOutputProtectionRecovered")}
              </div>
              <div className="mt-1 leading-5" style={{ color: "var(--df-text-dimmed)" }}>
                {t(
                  performanceOverlay === "overloaded"
                    ? "terminal.largeOutputProtectionActiveDetail"
                    : "terminal.largeOutputProtectionRecoveredDetail",
                  { skipped: formatSkippedCount(skippedOutputChars) },
                )}
              </div>
            </div>
          </div>
        )}

        {syncOverlay && <SyncActionOverlay overlay={syncOverlay} />}

        <TerminalSearchBar
          show={showSearchBar}
          searchQuery={searchQuery}
          setSearchQuery={setSearchQuery}
          onNext={handleSearchNext}
          onPrev={handleSearchPrev}
          onClose={handleCloseSearch}
        />

        <CommandSuggestions
          suggestions={suggestions}
          visible={commandSuggestionsEnabled && showSuggestions && canShowCommandSuggestions()}
          selectedIndex={selectedIndex}
          cursorPosition={cursorPosition}
          onSelect={handleSelectSuggestion}
          onDismiss={dismissSuggestions}
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
