import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { memo, type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { LuMessageSquarePlus, LuQuote } from "react-icons/lu";
import {
  MdAutoAwesome,
  MdAutoMode,
  MdCheck,
  MdClose,
  MdContentCopy,
  MdDeleteOutline,
  MdErrorOutline,
  MdHistory,
  MdOutlineSettings,
  MdRule,
  MdSearch,
  MdSend,
  MdStop,
  MdWarningAmber,
} from "react-icons/md";
import { toast } from "sonner";
import { AIAssistantDialogs } from "@/components/dialog/ai/AIAssistantDialogs";
import PanelHeader from "@/components/layout/PanelHeader";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useApp } from "@/context/AppContext";
import { useTheme } from "@/context/ThemeContext";
import type { AIErrorDetectedDetail } from "@/lib/aiEvents";
import { AI_ERROR_DETECTED_EVENT } from "@/lib/aiEvents";
import { getEnabledAIModels, resolveAILanguage, selectDefaultAIModel } from "@/lib/aiSettings";
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import { getNextQuickCommandCategorySortOrder } from "@/lib/quickCommandCategories";
import { buildAIContext, getTerminalContextProvider } from "@/lib/terminalContext";
import { openSettings } from "@/lib/windowManager";
import { collectSessionPanes } from "@/lib/workspaceTabs";
import type {
  AgentStepPayload,
  AIAction,
  AIAgentCommandExecutionMode,
  AIAgentKind,
  AICommandCard,
  AIContext,
  AIMessage,
  AIMode,
  AIModelConfigItem,
  AISession,
  AISessionScope,
  AIStreamEventPayload,
  AIStreamStart,
  AITargetContext,
  AITerminalTarget,
  QuickCommand,
  QuickCommandCategory,
  QuickCommandsConfig,
  SessionPane,
} from "@/types/global";
import { AgentStepView } from "./AgentStepView";
import { AICommandCardView } from "./AICommandCardView";
import { AssistantReasoning } from "./AssistantReasoning";
import { AssistantResponse } from "./AssistantResponse";
import { ModelCombobox } from "./ModelCombobox";
import type {
  AIAssistantPanelProps,
  AICommandExecutionState,
  AICommandExecutionStatus,
  QuotedText,
} from "./types";
import { actionTitle, buildPrismThemeFromColors, createLocalMessage, slugCategory } from "./utils";

interface AIDraft {
  text: string;
  quotedText: QuotedText | null;
  targetPaneIds: string[];
}

type AIPanelView = { mode: "draft" } | { mode: "session"; sessionId: string };
type AIRunMode = "ask" | "nyaterm_agent" | "codex_agent" | "claude_code_agent";

interface AIStreamRuntime {
  streamId: string;
  aiSessionId: string;
  assistantMessageId: string;
}

const EMPTY_DRAFT: AIDraft = { text: "", quotedText: null, targetPaneIds: [] };

function isCodexModel(model: AIModelConfigItem | null | undefined) {
  return model?.backend === "codex";
}

function isGenaiModel(model: AIModelConfigItem | null | undefined) {
  return (model?.backend ?? "genai") === "genai";
}

function resolveRunMode(mode: AIMode, agentKind: AIAgentKind | null | undefined): AIRunMode {
  if (mode !== "agent") return "ask";
  if (agentKind === "codex") return "codex_agent";
  if (agentKind === "claude_code") return "claude_code_agent";
  return "nyaterm_agent";
}

function buildAIScopeKey(pane: SessionPane | null) {
  return pane ? `terminal:${pane.sessionId}` : "unbound:";
}

function buildOwnerScope(pane: SessionPane | null): AISessionScope {
  if (!pane) return { type: "unbound", targetId: null, connectionIds: [], label: null };
  return {
    type: "terminal",
    targetId: pane.sessionId,
    connectionIds: pane.connectionId ? [pane.connectionId] : [],
    label: pane.name,
  };
}

function AIAssistantPanel({ activePane, activeConnection, intent }: AIAssistantPanelProps) {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings, tabs, savedConnections } = useApp();
  const { theme } = useTheme();
  const aiSettings = appSettings.ai;
  const [sessions, setSessions] = useState<AISession[]>([]);
  const [activeSessionIdByScope, setActiveSessionIdByScope] = useState<
    Record<string, string | null>
  >({});
  const [messagesBySessionId, setMessagesBySessionId] = useState<Record<string, AIMessage[]>>({});
  const [draftsByScope, setDraftsByScope] = useState<Record<string, AIDraft>>({});
  const [panelViewByScope, setPanelViewByScope] = useState<Record<string, AIPanelView>>({});
  const [streamRuntimeBySession, setStreamRuntimeBySession] = useState<
    Record<string, AIStreamRuntime>
  >({});
  const [showHistory, setShowHistory] = useState(false);
  const [historyQuery, setHistoryQuery] = useState("");
  const [clearHistoryOpen, setClearHistoryOpen] = useState(false);
  const [clearingHistory, setClearingHistory] = useState(false);
  const [detectedError, setDetectedError] = useState<AIErrorDetectedDetail | null>(null);
  const [showMentionPopover, setShowMentionPopover] = useState(false);
  const [mentionQuery, setMentionQuery] = useState("");
  const [mentionIndex, setMentionIndex] = useState(0);
  const [commandExecution, setCommandExecution] = useState<Record<string, AICommandExecutionState>>(
    {},
  );
  const [agentStepsMap, setAgentStepsMap] = useState<Record<string, AgentStepPayload[]>>({});
  const [modelPopoverOpen, setModelPopoverOpen] = useState(false);
  const [showExecutionMenu, setShowExecutionMenu] = useState(false);
  const [autoModeDialogOpen, setAutoModeDialogOpen] = useState(false);
  const [pendingExecutionMode, setPendingExecutionMode] =
    useState<AIAgentCommandExecutionMode | null>(null);
  const handledIntentIdRef = useRef<string | null>(null);
  const executionMenuButtonRef = useRef<HTMLButtonElement | null>(null);
  const executionMenuRef = useRef<HTMLDivElement | null>(null);
  const historyButtonRef = useRef<HTMLButtonElement | null>(null);
  const historyCardRef = useRef<HTMLDivElement | null>(null);
  const mentionPopoverRef = useRef<HTMLDivElement | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const streamUnlistenersRef = useRef<Map<string, UnlistenFn>>(new Map());
  const streamSessionByStreamIdRef = useRef<Map<string, string>>(new Map());
  const scrollContainerRef = useRef<HTMLDivElement>(null);
  const shouldAutoScrollRef = useRef(true);

  const enabledModels = useMemo(() => getEnabledAIModels(aiSettings), [aiSettings]);
  const storedSelectedModel = useMemo(() => selectDefaultAIModel(aiSettings), [aiSettings]);
  const prismStyle = useMemo(() => buildPrismThemeFromColors(theme.colors), [theme.colors]);
  const mode = aiSettings.default_mode ?? "ask";
  const agentKind = aiSettings.default_agent_kind ?? "nyaterm";
  const runMode = resolveRunMode(mode, agentKind);
  const codexModels = useMemo(
    () => enabledModels.filter((model) => isCodexModel(model)),
    [enabledModels],
  );
  const genaiModels = useMemo(
    () => enabledModels.filter((model) => isGenaiModel(model)),
    [enabledModels],
  );
  const selectedModel = useMemo(() => {
    if (runMode === "claude_code_agent") return null;
    if (runMode === "codex_agent") {
      const configuredModel = aiSettings.codex?.default_model ?? null;
      return (
        (isCodexModel(storedSelectedModel) ? storedSelectedModel : null) ??
        codexModels.find(
          (model) => model.id === configuredModel || model.name === configuredModel,
        ) ??
        codexModels[0] ??
        null
      );
    }
    return (
      (isGenaiModel(storedSelectedModel) ? storedSelectedModel : null) ?? genaiModels[0] ?? null
    );
  }, [aiSettings.codex?.default_model, codexModels, genaiModels, runMode, storedSelectedModel]);
  const selectableModels =
    runMode === "codex_agent" ? codexModels : runMode === "claude_code_agent" ? [] : genaiModels;
  const externalModelLabel =
    runMode === "claude_code_agent"
      ? (aiSettings.claude_code?.default_model ?? "Claude Code")
      : null;
  const agentExecutionMode = aiSettings.agent_command_execution_mode ?? "confirm_each";
  const agentBackgroundExecutionEnabled = aiSettings.agent_background_execution_enabled ?? false;
  const scopeKey = useMemo(() => buildAIScopeKey(activePane), [activePane]);
  const ownerScope = useMemo(() => buildOwnerScope(activePane), [activePane]);
  const currentPanelView = panelViewByScope[scopeKey] ?? null;
  const currentSessionId =
    currentPanelView?.mode === "draft"
      ? null
      : (currentPanelView?.sessionId ?? activeSessionIdByScope[scopeKey] ?? null);
  const currentSession = useMemo(
    () => sessions.find((session) => session.id === currentSessionId) ?? null,
    [currentSessionId, sessions],
  );
  const currentDraft = draftsByScope[scopeKey] ?? EMPTY_DRAFT;
  const input = currentDraft.text;
  const quotedText = currentDraft.quotedText;
  const messages = currentSessionId ? (messagesBySessionId[currentSessionId] ?? []) : [];
  const currentStreamRuntime = currentSessionId ? streamRuntimeBySession[currentSessionId] : null;
  const loading = !!currentStreamRuntime;
  const streamingAssistantId = currentStreamRuntime?.assistantMessageId ?? null;

  const allSessionPanes = useMemo(() => {
    const panes: SessionPane[] = [];
    for (const tab of tabs) {
      for (const pane of collectSessionPanes(tab.root)) {
        if (!pane.connecting && !pane.connectError) {
          panes.push(pane);
        }
      }
    }
    return panes;
  }, [tabs]);

  const filteredMentionPanes = useMemo(() => {
    const q = mentionQuery.toLowerCase();
    if (!q) return allSessionPanes;
    return allSessionPanes.filter(
      (p) =>
        p.name.toLowerCase().includes(q) ||
        p.sessionId.toLowerCase().includes(q) ||
        p.type.toLowerCase().includes(q),
    );
  }, [allSessionPanes, mentionQuery]);

  const targetPanes = useMemo(
    () =>
      currentDraft.targetPaneIds
        .map((sessionId) => allSessionPanes.find((pane) => pane.sessionId === sessionId))
        .filter((pane): pane is SessionPane => !!pane),
    [allSessionPanes, currentDraft.targetPaneIds],
  );

  // biome-ignore lint/correctness/useExhaustiveDependencies: reset selection whenever the filtered mention list changes.
  useEffect(() => {
    setMentionIndex(0);
  }, [filteredMentionPanes]);

  const effectivePanes = useMemo(() => {
    const paneMap = new Map<string, SessionPane>();
    if (activePane) paneMap.set(activePane.sessionId, activePane);
    for (const pane of targetPanes) {
      paneMap.set(pane.sessionId, pane);
    }
    return [...paneMap.values()];
  }, [activePane, targetPanes]);
  const panelMeta =
    effectivePanes.length > 1 && activePane
      ? t("ai.panelMetaMultiTarget", { target: activePane.name, count: effectivePanes.length - 1 })
      : (activePane?.name ?? selectedModel?.name ?? externalModelLabel ?? t("ai.notConfigured"));
  useEffect(() => {
    if (!selectedModel || selectedModel.id === aiSettings.default_model_id) return;
    updateAppSettings({ ai: { ...aiSettings, default_model_id: selectedModel.id } });
  }, [aiSettings, selectedModel, updateAppSettings]);

  const filteredSessions = useMemo(() => {
    const keyword = historyQuery.trim().toLowerCase();
    if (!keyword) return sessions;

    return sessions.filter((session) =>
      [session.title, session.createdAt, session.updatedAt, session.id].some((value) =>
        value.toLowerCase().includes(keyword),
      ),
    );
  }, [historyQuery, sessions]);

  const updateDraftForScope = useCallback(
    (updater: (draft: AIDraft) => AIDraft) => {
      setDraftsByScope((prev) => ({
        ...prev,
        [scopeKey]: updater(prev[scopeKey] ?? EMPTY_DRAFT),
      }));
    },
    [scopeKey],
  );

  const updateMessagesForSession = useCallback(
    (sessionId: string, updater: (messages: AIMessage[]) => AIMessage[]) => {
      setMessagesBySessionId((prev) => ({
        ...prev,
        [sessionId]: updater(prev[sessionId] ?? []),
      }));
    },
    [],
  );

  const cleanupStreamListener = useCallback((finishedStreamId: string) => {
    streamUnlistenersRef.current.get(finishedStreamId)?.();
    streamUnlistenersRef.current.delete(finishedStreamId);
    const aiSessionId = streamSessionByStreamIdRef.current.get(finishedStreamId);
    streamSessionByStreamIdRef.current.delete(finishedStreamId);
    if (aiSessionId) {
      setStreamRuntimeBySession((prev) => {
        const runtime = prev[aiSessionId];
        if (!runtime || runtime.streamId !== finishedStreamId) return prev;
        const { [aiSessionId]: _, ...rest } = prev;
        return rest;
      });
    }
  }, []);

  useEffect(() => {
    return () => {
      for (const unlisten of streamUnlistenersRef.current.values()) {
        unlisten();
      }
      streamUnlistenersRef.current.clear();
      streamSessionByStreamIdRef.current.clear();
    };
  }, []);

  useEffect(() => {
    const watchedIds = new Set(effectivePanes.map((p) => p.sessionId));
    if (activePane?.sessionId) watchedIds.add(activePane.sessionId);
    const handler = (event: Event) => {
      const detail = (event as CustomEvent<AIErrorDetectedDetail>).detail;
      if (!detail || !watchedIds.has(detail.sessionId)) return;
      setDetectedError(detail);
    };
    window.addEventListener(AI_ERROR_DETECTED_EVENT, handler);
    return () => window.removeEventListener(AI_ERROR_DETECTED_EVENT, handler);
  }, [activePane?.sessionId, effectivePanes]);

  const handleMessagesScroll = useCallback(() => {
    const el = scrollContainerRef.current;
    if (!el) return;
    shouldAutoScrollRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
  }, []);

  // biome-ignore lint/correctness/useExhaustiveDependencies: scroll when chat messages or agent steps change.
  useEffect(() => {
    if (!shouldAutoScrollRef.current) return;
    requestAnimationFrame(() => {
      const el = scrollContainerRef.current;
      if (el) el.scrollTop = el.scrollHeight;
    });
  }, [messages, agentStepsMap]);

  const loadSessions = useCallback(async () => {
    try {
      setSessions(await invoke<AISession[]>("get_ai_sessions"));
    } catch {
      setSessions([]);
    }
  }, []);

  useEffect(() => {
    void loadSessions();
  }, [loadSessions]);

  const loadSessionMessages = useCallback(
    async (sessionId: string) => {
      try {
        const items = await invoke<AIMessage[]>("get_ai_messages", { sessionId });
        setMessagesBySessionId((prev) => ({ ...prev, [sessionId]: items }));
        setActiveSessionIdByScope((prev) => ({ ...prev, [scopeKey]: sessionId }));
        setPanelViewByScope((prev) => ({
          ...prev,
          [scopeKey]: { mode: "session", sessionId },
        }));
        setShowHistory(false);
      } catch (error) {
        toast.error(getErrorMessage(error));
      }
    },
    [scopeKey],
  );

  const appendAudit = useCallback(
    (params: {
      action: string;
      userInput?: string;
      generatedCommand?: string;
      insertedToTerminal?: boolean;
      executed?: boolean;
      blocked?: boolean;
    }) => {
      void invoke("append_ai_audit", {
        request: {
          connectionId: activeConnection?.id ?? null,
          action: params.action,
          userInput: params.userInput,
          generatedCommand: params.generatedCommand,
          riskLevel: null,
          insertedToTerminal: params.insertedToTerminal ?? false,
          executed: params.executed ?? false,
          blocked: params.blocked ?? false,
        },
      }).catch(() => {});
    },
    [activeConnection?.id],
  );

  const selectRunMode = useCallback(
    (nextMode: AIRunMode) => {
      if (nextMode === "ask") {
        updateAppSettings({
          ai: { ...aiSettings, default_mode: "ask", default_agent_kind: "nyaterm" },
        });
        return;
      }

      if (nextMode === "nyaterm_agent") {
        const nextModel = isGenaiModel(selectedModel) ? selectedModel : genaiModels[0];
        if (!nextModel) {
          toast.error(t("ai.noGenaiAgentModel"));
          return;
        }
        updateAppSettings({
          ai: {
            ...aiSettings,
            default_mode: "agent",
            default_agent_kind: "nyaterm",
            default_model_id: nextModel.id,
          },
        });
        return;
      }

      if (nextMode === "claude_code_agent") {
        updateAppSettings({
          ai: { ...aiSettings, default_mode: "agent", default_agent_kind: "claude_code" },
        });
        return;
      }

      const configuredModel = aiSettings.codex?.default_model ?? null;
      const nextModel =
        (isCodexModel(selectedModel) ? selectedModel : null) ??
        codexModels.find(
          (model) => model.id === configuredModel || model.name === configuredModel,
        ) ??
        codexModels[0];

      if (!nextModel) {
        toast.error(t("ai.noCodexAgentModel"));
        return;
      }

      updateAppSettings({
        ai: {
          ...aiSettings,
          default_mode: "agent",
          default_agent_kind: "codex",
          default_model_id: nextModel.id,
        },
      });
      return;
    },
    [aiSettings, codexModels, genaiModels, selectedModel, t, updateAppSettings],
  );

  const buildMergedContext = useCallback(
    async (panes: SessionPane[], selectedText?: string): Promise<AIContext> => {
      if (panes.length === 0) {
        return buildAIContext({
          pane: null,
          connection: null,
          lineLimit: aiSettings.context_line_limit,
          selectedText,
        });
      }
      if (panes.length === 1) {
        const conn = panes[0].connectionId
          ? (savedConnections.find((c) => c.id === panes[0].connectionId) ?? null)
          : activeConnection;
        return buildAIContext({
          pane: panes[0],
          connection: conn,
          lineLimit: aiSettings.context_line_limit,
          selectedText,
        });
      }
      const contexts = await Promise.all(
        panes.map((p) => {
          const conn = p.connectionId
            ? (savedConnections.find((c) => c.id === p.connectionId) ?? null)
            : null;
          return buildAIContext({
            pane: p,
            connection: conn,
            lineLimit: Math.floor(aiSettings.context_line_limit / panes.length),
          });
        }),
      );
      const merged: AIContext = {
        connectionName: contexts.map((c) => c.connectionName ?? "-").join(", "),
        host: contexts.map((c) => c.host ?? "-").join(", "),
        port: contexts[0]?.port ?? null,
        username: contexts.map((c) => c.username ?? "-").join(", "),
        cwd: contexts.map((c) => c.cwd ?? "-").join(", "),
        os: contexts[0]?.os ?? null,
        arch: contexts[0]?.arch ?? null,
        recentOutput: contexts
          .map((c, i) => `[${panes[i].name}]\n${c.recentOutput}`)
          .filter((s) => s.trim().length > panes[0].name.length + 4)
          .join("\n---\n"),
        selectedText:
          selectedText ??
          contexts
            .map((c) => c.selectedText)
            .filter(Boolean)
            .join("\n"),
        inputBuffer: contexts
          .map((c) => c.inputBuffer)
          .filter(Boolean)
          .join("\n"),
      };
      return merged;
    },
    [activeConnection, aiSettings.context_line_limit, savedConnections],
  );

  const buildTargetForPane = useCallback(
    (pane: SessionPane): AITerminalTarget => {
      const conn = pane.connectionId
        ? (savedConnections.find((item) => item.id === pane.connectionId) ?? null)
        : pane.sessionId === activePane?.sessionId
          ? activeConnection
          : null;
      return {
        terminalSessionId: pane.sessionId,
        connectionId: pane.connectionId ?? conn?.id ?? null,
        label: pane.name,
        host: conn?.host ?? null,
        username: conn?.username ?? null,
        sessionType: pane.type,
      };
    },
    [activeConnection, activePane?.sessionId, savedConnections],
  );

  const buildTargetContexts = useCallback(
    async (panes: SessionPane[], selectedText?: string): Promise<AITargetContext[]> => {
      const lineLimit = Math.max(
        1,
        Math.floor(aiSettings.context_line_limit / Math.max(1, panes.length)),
      );
      return Promise.all(
        panes.map(async (pane, index) => {
          const conn = pane.connectionId
            ? (savedConnections.find((item) => item.id === pane.connectionId) ?? null)
            : pane.sessionId === activePane?.sessionId
              ? activeConnection
              : null;
          return {
            target: buildTargetForPane(pane),
            context: await buildAIContext({
              pane,
              connection: conn,
              lineLimit,
              selectedText: index === 0 ? selectedText : undefined,
            }),
          };
        }),
      );
    },
    [
      activeConnection,
      activePane?.sessionId,
      aiSettings.context_line_limit,
      buildTargetForPane,
      savedConnections,
    ],
  );

  const setCommandState = useCallback(
    (cardId: string, status: AICommandExecutionStatus, error?: string) => {
      setCommandExecution((prev) => ({ ...prev, [cardId]: { status, error } }));
    },
    [],
  );

  const executeCommandCard = useCallback(
    async (card: AICommandCard, source: "auto" | "authorized") => {
      const targetSessionId = card.target?.terminalSessionId;
      if (!targetSessionId) {
        const error = t("ai.commandTargetMissing");
        setCommandState(card.id, "failed", error);
        toast.error(error);
        return;
      }
      const provider = getTerminalContextProvider(targetSessionId);
      if (!provider) {
        const error = t("ai.commandTargetUnavailable");
        setCommandState(card.id, "failed", error);
        toast.error(error);
        return;
      }
      if (!provider.executeCommand) {
        const error = t("ai.executeUnsupported");
        setCommandState(card.id, "failed", error);
        toast.error(error);
        return;
      }

      try {
        await provider.executeCommand(card.command);
        provider.focus();
        setCommandState(card.id, "executed");
        appendAudit({
          action: source === "auto" ? "ai.agent_auto_execute" : "ai.agent_authorized_execute",
          generatedCommand: card.command,
          executed: true,
        });
      } catch (error) {
        const message = getErrorMessage(error);
        setCommandState(card.id, "failed", message);
        appendAudit({
          action: "ai.agent_execute_failed",
          generatedCommand: card.command,
        });
        toast.error(message);
      }
    },
    [appendAudit, setCommandState, t],
  );

  const startChat = useCallback(
    async (action: AIAction, userInput: string, selectedText?: string) => {
      const panes = effectivePanes;
      if (panes.length === 0) {
        toast.error(t("panel.noActiveSessions"));
        return;
      }
      if (!aiSettings.enabled) {
        toast.error(t("ai.disabled"));
        return;
      }
      const requestModel = selectedModel;
      const requestAgentKind: AIAgentKind =
        runMode === "codex_agent"
          ? "codex"
          : runMode === "claude_code_agent"
            ? "claude_code"
            : "nyaterm";
      if (!requestModel && requestAgentKind !== "claude_code") {
        toast.error(t("ai.noEnabledModels"));
        return;
      }
      const requestMode: AIMode = runMode === "ask" ? "ask" : "agent";
      const requestSessionId =
        currentSession?.agentKind && currentSession.agentKind !== requestAgentKind
          ? null
          : currentSessionId;

      setDetectedError(null);
      const assistantId = `assistant-${Date.now()}-${Math.random().toString(36).slice(2)}`;
      const requestStreamId = `ai-stream-${crypto.randomUUID()}`;
      let resolvedSessionId = requestSessionId ?? `pending-${requestStreamId}`;
      const userMessage = createLocalMessage("user", userInput, resolvedSessionId);
      const assistantMessage: AIMessage = {
        id: assistantId,
        sessionId: resolvedSessionId,
        role: "assistant",
        content: "",
        createdAt: new Date().toISOString(),
        reasoningContent: null,
        commandCards: [],
      };
      setActiveSessionIdByScope((prev) => ({ ...prev, [scopeKey]: resolvedSessionId }));
      setPanelViewByScope((prev) => ({
        ...prev,
        [scopeKey]: { mode: "session", sessionId: resolvedSessionId },
      }));
      updateMessagesForSession(resolvedSessionId, (prev) => [
        ...prev,
        userMessage,
        assistantMessage,
      ]);
      setStreamRuntimeBySession((prev) => ({
        ...prev,
        [resolvedSessionId]: {
          streamId: requestStreamId,
          aiSessionId: resolvedSessionId,
          assistantMessageId: assistantId,
        },
      }));
      streamSessionByStreamIdRef.current.set(requestStreamId, resolvedSessionId);

      const bindRealSessionId = (nextSessionId: string) => {
        if (nextSessionId === resolvedSessionId) return;
        const previousSessionId = resolvedSessionId;
        resolvedSessionId = nextSessionId;
        streamSessionByStreamIdRef.current.set(requestStreamId, nextSessionId);
        setMessagesBySessionId((prev) => {
          const pendingMessages = prev[previousSessionId] ?? [];
          const existingMessages = prev[nextSessionId] ?? [];
          const { [previousSessionId]: _, ...rest } = prev;
          return {
            ...rest,
            [nextSessionId]: [
              ...existingMessages,
              ...pendingMessages.map((message) => ({ ...message, sessionId: nextSessionId })),
            ],
          };
        });
        setActiveSessionIdByScope((prev) =>
          prev[scopeKey] === previousSessionId ? { ...prev, [scopeKey]: nextSessionId } : prev,
        );
        setPanelViewByScope((prev) => ({
          ...prev,
          [scopeKey]: { mode: "session", sessionId: nextSessionId },
        }));
        setStreamRuntimeBySession((prev) => {
          const runtime = prev[previousSessionId];
          const { [previousSessionId]: _, ...rest } = prev;
          return runtime
            ? {
                ...rest,
                [nextSessionId]: {
                  ...runtime,
                  aiSessionId: nextSessionId,
                },
              }
            : prev;
        });
      };

      try {
        const unlisten = await listen<AIStreamEventPayload | AgentStepPayload>(
          `ai-stream-${requestStreamId}`,
          (event) => {
            const raw = event.payload as unknown as Record<string, unknown>;
            if (raw.streamId !== requestStreamId) return;

            if ("stepIndex" in raw) {
              const step = raw as unknown as AgentStepPayload;
              if (step.sessionId) bindRealSessionId(step.sessionId);
              setAgentStepsMap((prev) => {
                const steps = prev[assistantId] ?? [];
                const existing = steps.findIndex((s) => s.stepIndex === step.stepIndex);
                if (existing >= 0) {
                  const next = [...steps];
                  next[existing] = step;
                  return { ...prev, [assistantId]: next };
                }
                return { ...prev, [assistantId]: [...steps, step] };
              });
              return;
            }

            const payload = raw as unknown as AIStreamEventPayload;

            if (payload.type === "start") {
              if (payload.sessionId) bindRealSessionId(payload.sessionId);
              return;
            }

            if (payload.type === "delta" && payload.textDelta) {
              updateMessagesForSession(resolvedSessionId, (prev) =>
                prev.map((message) =>
                  message.id === assistantId
                    ? { ...message, content: `${message.content}${payload.textDelta}` }
                    : message,
                ),
              );
              return;
            }

            if (payload.type === "reasoning_delta" && payload.reasoningDelta) {
              updateMessagesForSession(resolvedSessionId, (prev) =>
                prev.map((message) =>
                  message.id === assistantId
                    ? {
                        ...message,
                        reasoningContent: `${message.reasoningContent ?? ""}${payload.reasoningDelta}`,
                      }
                    : message,
                ),
              );
              return;
            }

            if (payload.type === "done") {
              if (payload.sessionId) bindRealSessionId(payload.sessionId);
              cleanupStreamListener(requestStreamId);
              const newMsgId = payload.message?.id;
              if (payload.message) {
                updateMessagesForSession(resolvedSessionId, (prev) =>
                  prev.map((message) =>
                    message.id === assistantId
                      ? { ...payload.message!, sessionId: resolvedSessionId }
                      : message,
                  ),
                );
              }
              if (newMsgId && newMsgId !== assistantId) {
                setAgentStepsMap((prev) => {
                  const steps = prev[assistantId];
                  if (!steps) return prev;
                  const { [assistantId]: _, ...rest } = prev;
                  return { ...rest, [newMsgId]: steps };
                });
              }
              void loadSessions();
              return;
            }

            if (payload.type === "error") {
              cleanupStreamListener(requestStreamId);
              updateMessagesForSession(resolvedSessionId, (prev) =>
                prev.map((message) =>
                  message.id === assistantId
                    ? {
                        ...message,
                        content: payload.error ?? t("ai.requestFailed"),
                      }
                    : message,
                ),
              );
              toast.error(payload.error ?? t("ai.requestFailed"));
            }
          },
        );
        streamUnlistenersRef.current.set(requestStreamId, unlisten);

        const context = await buildMergedContext(panes, selectedText);
        const targets = panes.map(buildTargetForPane);
        const targetContexts = await buildTargetContexts(panes, selectedText);
        const primaryConn = panes[0].connectionId
          ? (savedConnections.find((c) => c.id === panes[0].connectionId) ?? null)
          : activeConnection;

        const resolvedLanguage = resolveAILanguage(appSettings.ui.language);
        const result = await invoke<AIStreamStart>("start_ai_chat_stream", {
          request: {
            streamId: requestStreamId,
            sessionId: requestSessionId,
            connectionId: primaryConn?.id ?? null,
            terminalSessionId: panes[0]?.sessionId ?? null,
            agentKind: requestAgentKind,
            permissionMode:
              requestAgentKind === "codex"
                ? (aiSettings.codex?.permission_mode ??
                  aiSettings.external_agent_permission_mode ??
                  "confirm")
                : requestAgentKind === "claude_code"
                  ? (aiSettings.claude_code?.permission_mode ??
                    aiSettings.external_agent_permission_mode ??
                    "confirm")
                  : "confirm",
            defaultTargetSessionId: panes[0]?.sessionId ?? null,
            existingExternalSessionId:
              currentSession?.agentKind === requestAgentKind
                ? (currentSession.externalSessionId ?? null)
                : null,
            ownerScope,
            targets,
            targetContexts,
            action,
            userInput,
            mode: requestMode,
            modelId: requestModel?.id ?? null,
            modelName: requestModel?.name ?? null,
            context,
            options: {
              maxOutputCommands: 2,
              language: resolvedLanguage,
              safetyMode: "strict",
            },
          },
        });
        bindRealSessionId(result.sessionId);
        appendAudit({ action: `ai.${action}`, userInput });
      } catch (error) {
        void invoke("cancel_ai_chat_stream", { streamId: requestStreamId }).catch(() => {});
        cleanupStreamListener(requestStreamId);
        updateMessagesForSession(resolvedSessionId, (prev) =>
          prev.map((message) =>
            message.id === assistantId ? { ...message, content: getErrorMessage(error) } : message,
          ),
        );
        toast.error(getErrorMessage(error));
      }
    },
    [
      activeConnection,
      aiSettings.claude_code?.permission_mode,
      aiSettings.codex?.permission_mode,
      aiSettings.enabled,
      aiSettings.external_agent_permission_mode,
      appSettings.ui.language,
      appendAudit,
      buildTargetContexts,
      buildTargetForPane,
      buildMergedContext,
      cleanupStreamListener,
      currentSession,
      currentSessionId,
      effectivePanes,
      loadSessions,
      ownerScope,
      runMode,
      savedConnections,
      selectedModel,
      scopeKey,
      t,
      updateMessagesForSession,
    ],
  );

  useEffect(() => {
    if (!intent || handledIntentIdRef.current === intent.id) return;
    handledIntentIdRef.current = intent.id;
    const fallbackText = actionTitle(intent.action);
    void startChat(intent.action, intent.userInput?.trim() || fallbackText, intent.selectedText);
  }, [intent, startChat]);

  const submit = useCallback(() => {
    const value = input.trim();
    if (!value || loading) return;
    const fullInput = quotedText ? `> ${quotedText.text}\n\n${value}` : value;
    updateDraftForScope((draft) => ({ ...draft, text: "", quotedText: null, targetPaneIds: [] }));
    shouldAutoScrollRef.current = true;
    void startChat("generate_command", fullInput);
  }, [input, loading, quotedText, startChat, updateDraftForScope]);

  const cancelStream = useCallback(() => {
    const activeStreamId = currentStreamRuntime?.streamId;
    if (!activeStreamId) return;
    void invoke("cancel_ai_chat_stream", { streamId: activeStreamId }).catch(() => {});
    cleanupStreamListener(activeStreamId);
  }, [cleanupStreamListener, currentStreamRuntime?.streamId]);

  const insertCommand = useCallback(
    (card: AICommandCard) => {
      const insertSessionId = card.target?.terminalSessionId;
      if (!insertSessionId) {
        toast.error(t("ai.commandTargetMissing"));
        return;
      }
      const provider = getTerminalContextProvider(insertSessionId);
      if (!provider) {
        toast.error(t("ai.commandTargetUnavailable"));
        return;
      }
      void provider
        .insertCommand(card.command)
        .then(() => {
          provider.focus();
          appendAudit({
            action: "ai.insert_command",
            generatedCommand: card.command,
            insertedToTerminal: true,
          });
        })
        .catch((error) => toast.error(getErrorMessage(error)));
    },
    [appendAudit, t],
  );

  const saveQuickCommand = useCallback(
    async (card: AICommandCard) => {
      if (!aiSettings.allow_save_command) {
        toast.error(t("ai.saveDisabled"));
        return;
      }

      try {
        const config = await invoke<QuickCommandsConfig>("get_quick_commands");
        const categoryName = card.category || t("ai.quickCommandCategory");
        const existingCategory = config.categories.find(
          (item) => !item.parent_id && item.name === categoryName,
        );
        const baseCategoryId = slugCategory(categoryName);
        let newCategoryId = baseCategoryId;
        for (let index = 2; config.categories.some((item) => item.id === newCategoryId); index++) {
          newCategoryId = `${baseCategoryId}-${index}`;
        }
        const newCategory: QuickCommandCategory | undefined = existingCategory
          ? undefined
          : {
              id: newCategoryId,
              name: categoryName,
              sort_order: getNextQuickCommandCategorySortOrder(config.categories, null),
            };
        const categoryId = existingCategory?.id ?? newCategory?.id;
        const command: QuickCommand = {
          id: `ai-${crypto.randomUUID()}`,
          label: card.title,
          command: card.command,
          category_id: categoryId,
          description: card.explanation,
          color_tag: "blue",
          icon_tag: "terminal",
          pinned: false,
          execution_mode: "append",
          source: "ai",
        };
        await invoke("upsert_quick_command", { command, newCategory });
        await emit("quick-command-saved", { command, newCategory });
        appendAudit({
          action: "ai.save_quick_command",
          generatedCommand: card.command,
        });
        toast.success(t("ai.savedQuickCommand"));
      } catch (error) {
        toast.error(getErrorMessage(error));
      }
    },
    [aiSettings.allow_save_command, appendAudit, t],
  );

  const authorizeCommand = useCallback(
    (card: AICommandCard) => {
      void executeCommandCard(card, "authorized");
    },
    [executeCommandCard],
  );

  const rejectCommand = useCallback(
    (card: AICommandCard) => {
      setCommandState(card.id, "rejected");
      appendAudit({
        action: "ai.agent_reject_execute",
        generatedCommand: card.command,
        blocked: true,
      });
    },
    [appendAudit, setCommandState],
  );

  const clearHistory = useCallback(async () => {
    if (loading) return;
    setClearingHistory(true);
    try {
      await invoke("clear_ai_history");
      for (const unlisten of streamUnlistenersRef.current.values()) {
        unlisten();
      }
      streamUnlistenersRef.current.clear();
      streamSessionByStreamIdRef.current.clear();
      setStreamRuntimeBySession({});
      setMessagesBySessionId({});
      setActiveSessionIdByScope({});
      setPanelViewByScope({});
      setHistoryQuery("");
      setClearHistoryOpen(false);
      await loadSessions();
    } catch (error) {
      toast.error(getErrorMessage(error));
    } finally {
      setClearingHistory(false);
    }
  }, [loadSessions, loading]);

  const deleteSession = useCallback(
    async (sessionId: string) => {
      try {
        await invoke("delete_ai_session", { sessionId });
        if (currentSessionId === sessionId) {
          setActiveSessionIdByScope((prev) => ({ ...prev, [scopeKey]: null }));
          setPanelViewByScope((prev) => ({ ...prev, [scopeKey]: { mode: "draft" } }));
        }
        setMessagesBySessionId((prev) => {
          const { [sessionId]: _, ...rest } = prev;
          return rest;
        });
        await loadSessions();
      } catch (error) {
        toast.error(getErrorMessage(error));
      }
    },
    [currentSessionId, loadSessions, scopeKey],
  );

  const historySections = useMemo(() => {
    const current: AISession[] = [];
    const sameConnection: AISession[] = [];
    const other: AISession[] = [];
    for (const session of filteredSessions) {
      const scope = session.scope;
      if (scope?.type === "terminal" && scope.targetId === activePane?.sessionId) {
        current.push(session);
      } else if (
        activePane?.connectionId &&
        (scope?.connectionIds?.includes(activePane.connectionId) ||
          session.connectionId === activePane.connectionId)
      ) {
        sameConnection.push(session);
      } else {
        other.push(session);
      }
    }
    return [
      { key: "current", label: t("ai.historyCurrentTerminal"), sessions: current },
      { key: "same", label: t("ai.historySameConnection"), sessions: sameConnection },
      { key: "other", label: t("ai.historyOtherSessions"), sessions: other },
    ];
  }, [activePane?.connectionId, activePane?.sessionId, filteredSessions, t]);

  const isSessionUsedByAnotherScope = useCallback(
    (sessionId: string) =>
      Object.entries(activeSessionIdByScope).some(
        ([key, value]) => key !== scopeKey && value === sessionId,
      ),
    [activeSessionIdByScope, scopeKey],
  );

  const openHistorySession = useCallback(
    async (session: AISession) => {
      if (!activePane) {
        await loadSessionMessages(session.id);
        return;
      }
      const exactScope =
        session.scope?.type === "terminal" && session.scope.targetId === activePane.sessionId;
      if (!exactScope) {
        await invoke<AISession>("rebind_ai_session", {
          sessionId: session.id,
          ownerScope,
        });
        setActiveSessionIdByScope((prev) => {
          const next = { ...prev, [scopeKey]: session.id };
          for (const [key, value] of Object.entries(next)) {
            if (key !== scopeKey && value === session.id) next[key] = null;
          }
          return next;
        });
        await loadSessions();
      }
      await loadSessionMessages(session.id);
    },
    [activePane, loadSessionMessages, loadSessions, ownerScope, scopeKey],
  );

  const newChat = useCallback(() => {
    if (loading) return;
    setActiveSessionIdByScope((prev) => ({ ...prev, [scopeKey]: null }));
    setPanelViewByScope((prev) => ({ ...prev, [scopeKey]: { mode: "draft" } }));
    updateDraftForScope(() => EMPTY_DRAFT);
    setDetectedError(null);
    setCommandExecution({});
    setShowMentionPopover(false);
    shouldAutoScrollRef.current = true;
  }, [loading, scopeKey, updateDraftForScope]);

  const handleCopySelection = useCallback(() => {
    const sel = window.getSelection()?.toString();
    if (sel) {
      void navigator.clipboard.writeText(sel);
    }
  }, []);

  const handleQuoteSelection = useCallback(() => {
    const sel = window.getSelection()?.toString()?.trim();
    if (sel) {
      updateDraftForScope((draft) => ({ ...draft, quotedText: { text: sel } }));
      textareaRef.current?.focus();
    }
  }, [updateDraftForScope]);

  const handleInputChange = useCallback(
    (event: React.ChangeEvent<HTMLTextAreaElement>) => {
      const value = event.target.value;
      updateDraftForScope((draft) => ({ ...draft, text: value }));

      const cursorPos = event.target.selectionStart;
      const textBeforeCursor = value.slice(0, cursorPos);
      const atMatch = textBeforeCursor.match(/@(\S*)$/);
      if (atMatch) {
        setMentionQuery(atMatch[1]);
        if (!showMentionPopover) setMentionIndex(0);
        setShowMentionPopover(true);
      } else {
        setShowMentionPopover(false);
        setMentionQuery("");
      }
    },
    [showMentionPopover, updateDraftForScope],
  );

  const selectMentionPane = useCallback(
    (pane: SessionPane) => {
      updateDraftForScope((draft) => {
        const exists = draft.targetPaneIds.includes(pane.sessionId);
        return {
          ...draft,
          targetPaneIds: exists
            ? draft.targetPaneIds.filter((id) => id !== pane.sessionId)
            : [...draft.targetPaneIds, pane.sessionId],
        };
      });

      const cursorPos = textareaRef.current?.selectionStart ?? input.length;
      const textBeforeCursor = input.slice(0, cursorPos);
      const textAfterCursor = input.slice(cursorPos);
      const cleaned = textBeforeCursor.replace(/@\S*$/, "");
      updateDraftForScope((draft) => ({ ...draft, text: `${cleaned}${textAfterCursor}` }));
      setShowMentionPopover(false);
      setMentionQuery("");
      textareaRef.current?.focus();
    },
    [input, updateDraftForScope],
  );

  const removeTargetPane = useCallback(
    (sessionId: string) => {
      updateDraftForScope((draft) => ({
        ...draft,
        targetPaneIds: draft.targetPaneIds.filter((id) => id !== sessionId),
      }));
    },
    [updateDraftForScope],
  );

  const updateAgentExecutionMode = useCallback(
    (nextMode: AIAgentCommandExecutionMode) => {
      updateAppSettings({
        ai: { ...aiSettings, agent_command_execution_mode: nextMode },
      });
    },
    [aiSettings, updateAppSettings],
  );

  const handleAgentExecutionModeChange = useCallback(
    (value: string) => {
      const nextMode = value as AIAgentCommandExecutionMode;
      if (nextMode === "auto" && agentExecutionMode !== "auto") {
        setPendingExecutionMode(nextMode);
        setAutoModeDialogOpen(true);
        return;
      }
      updateAgentExecutionMode(nextMode);
    },
    [agentExecutionMode, updateAgentExecutionMode],
  );

  const confirmAutoExecutionMode = useCallback(() => {
    updateAgentExecutionMode(pendingExecutionMode ?? "auto");
    setPendingExecutionMode(null);
    setAutoModeDialogOpen(false);
  }, [pendingExecutionMode, updateAgentExecutionMode]);

  const updateAgentBackgroundExecution = useCallback(
    (enabled: boolean) => {
      updateAppSettings({
        ai: { ...aiSettings, agent_background_execution_enabled: enabled },
      });
    },
    [aiSettings, updateAppSettings],
  );

  const renderExecutionModeItem = useCallback(
    (
      value: AIAgentCommandExecutionMode,
      icon: ReactNode,
      label: string,
      desc: string,
      danger = false,
    ) => {
      const selected = agentExecutionMode === value;
      return (
        <button
          type="button"
          key={value}
          className={`flex w-full items-start gap-2 rounded px-2 py-2 text-left hover:bg-muted/60 ${
            selected ? "bg-accent" : ""
          } ${danger ? "text-amber-600 hover:text-amber-600" : ""}`}
          onClick={() => {
            handleAgentExecutionModeChange(value);
            setShowExecutionMenu(false);
          }}
        >
          <span
            className={`mt-0.5 shrink-0 ${danger ? "text-amber-500" : "text-muted-foreground"}`}
          >
            {icon}
          </span>
          <span className="min-w-0 flex-1">
            <span className="block text-xs font-medium text-foreground">{label}</span>
            <span className="mt-0.5 block text-[0.6875rem] leading-4 text-muted-foreground">
              {desc}
            </span>
          </span>
          {selected ? <MdCheck className="mt-0.5 shrink-0 text-primary" /> : null}
        </button>
      );
    },
    [agentExecutionMode, handleAgentExecutionModeChange],
  );

  return (
    <div
      className="nyaterm-wallpaper-transparent-surface relative flex h-full flex-col"
      style={{ backgroundColor: "var(--df-bg-panel)" }}
      onPointerDownCapture={(event) => {
        const target = event.target as Node;
        if (showHistory) {
          if (
            !historyCardRef.current?.contains(target) &&
            !historyButtonRef.current?.contains(target)
          ) {
            setShowHistory(false);
          }
        }
        if (showExecutionMenu) {
          if (
            !executionMenuRef.current?.contains(target) &&
            !executionMenuButtonRef.current?.contains(target)
          ) {
            setShowExecutionMenu(false);
          }
        }
        if (showMentionPopover && !mentionPopoverRef.current?.contains(target)) {
          setShowMentionPopover(false);
        }
      }}
    >
      <PanelHeader
        title={t("ai.title")}
        meta={panelMeta}
        actions={
          <>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  ref={executionMenuButtonRef}
                  size="icon-sm"
                  variant="ghost"
                  disabled={loading}
                  className={
                    agentExecutionMode === "auto" ? "text-amber-600 hover:text-amber-600" : ""
                  }
                  aria-label={t("ai.agentCommandExecutionMode")}
                  aria-expanded={showExecutionMenu}
                  onClick={() => {
                    setShowHistory(false);
                    setShowExecutionMenu((value) => !value);
                  }}
                >
                  {agentExecutionMode === "auto" ? (
                    <MdWarningAmber />
                  ) : agentExecutionMode === "smart" ? (
                    <MdAutoMode />
                  ) : (
                    <MdRule />
                  )}
                </Button>
              </TooltipTrigger>
              <TooltipContent side="top">{t("ai.agentCommandExecutionMode")}</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  ref={historyButtonRef}
                  size="icon-sm"
                  variant="ghost"
                  onClick={() => {
                    setShowExecutionMenu(false);
                    setShowHistory((value) => !value);
                  }}
                  aria-expanded={showHistory}
                >
                  <MdHistory />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="top">{t("ai.history")}</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button size="icon-sm" variant="ghost" onClick={() => openSettings("ai")}>
                  <MdOutlineSettings />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="top">{t("ai.settings")}</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button size="icon-sm" variant="ghost" onClick={newChat} disabled={loading}>
                  <LuMessageSquarePlus />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="top">{t("ai.newChat")}</TooltipContent>
            </Tooltip>
          </>
        }
      />

      {showExecutionMenu ? (
        <div
          ref={executionMenuRef}
          className="absolute right-2 top-10 z-30 w-64 overflow-hidden rounded-md border bg-popover p-1 text-popover-foreground shadow-lg"
          style={{ borderColor: "var(--df-border)" }}
        >
          <div className="px-2 py-1.5 text-xs font-medium">{t("ai.agentCommandExecutionMode")}</div>
          {renderExecutionModeItem(
            "confirm_each",
            <MdRule />,
            t("ai.executionModeConfirmEach"),
            t("ai.executionModeConfirmEachDesc"),
          )}
          {renderExecutionModeItem(
            "smart",
            <MdAutoMode />,
            t("ai.executionModeSmart"),
            t("ai.executionModeSmartDesc"),
          )}
          <div className="-mx-1 my-1 h-px bg-border" />
          {renderExecutionModeItem(
            "auto",
            <MdWarningAmber />,
            t("ai.executionModeAuto"),
            t("ai.executionModeAutoDesc"),
            true,
          )}
          <div className="-mx-1 my-1 h-px bg-border" />
          <div className="px-2 py-1.5 text-xs font-medium">{t("ai.executionMethod")}</div>
          <button
            type="button"
            className="flex w-full items-start gap-2 rounded px-2 py-2 text-left hover:bg-muted/60"
            onClick={() => updateAgentBackgroundExecution(!agentBackgroundExecutionEnabled)}
          >
            <Checkbox
              checked={agentBackgroundExecutionEnabled}
              className="mt-0.5 shrink-0"
              onCheckedChange={(checked) => updateAgentBackgroundExecution(checked === true)}
              onClick={(event) => event.stopPropagation()}
            />
            <span className="min-w-0 flex-1">
              <span className="block text-xs font-medium text-foreground">
                {t("ai.backgroundAgentExecution")}
              </span>
              <span className="mt-0.5 block text-[0.6875rem] leading-4 text-muted-foreground">
                {t("ai.backgroundAgentExecutionDesc")}
              </span>
            </span>
          </button>
        </div>
      ) : null}

      {showHistory ? (
        <div
          ref={historyCardRef}
          className="absolute left-2 right-2 top-10 z-30 flex flex-col overflow-hidden rounded-md border bg-popover text-popover-foreground shadow-lg"
          style={{
            borderColor: "var(--df-border)",
            maxHeight: "min(22rem, calc(100% - 3rem))",
          }}
        >
          <div className="border-b border-border/70 p-2">
            <div className="relative">
              <MdSearch className="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-sm text-muted-foreground" />
              <Input
                value={historyQuery}
                placeholder={t("ai.historySearchPlaceholder")}
                className="h-8 pl-8 text-xs"
                autoFocus
                onChange={(event) => setHistoryQuery(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Escape") {
                    setShowHistory(false);
                  }
                }}
              />
            </div>
          </div>
          <div className="flex items-center justify-between gap-2 border-b border-border/70 px-2 py-1.5">
            <span className="text-xs font-medium">{t("ai.history")}</span>
            <Button
              size="xs"
              variant="ghost"
              disabled={sessions.length === 0 || loading || clearingHistory}
              onClick={() => setClearHistoryOpen(true)}
            >
              {t("ai.clearHistory")}
            </Button>
          </div>
          <div className="min-h-0 overflow-auto p-2 terminal-scroll">
            {filteredSessions.length === 0 ? (
              <div className="py-4 text-center text-xs text-muted-foreground">
                {sessions.length === 0 ? t("ai.noHistory") : t("ai.noHistoryMatches")}
              </div>
            ) : (
              historySections.map((section) => {
                if (section.sessions.length === 0) return null;
                return (
                  <div key={section.key} className="mb-1">
                    <div className="px-2 py-1 text-[0.625rem] font-semibold uppercase tracking-wider text-muted-foreground/70">
                      {section.label}
                    </div>
                    {section.sessions.map((session) => {
                      const inUse = isSessionUsedByAnotherScope(session.id);
                      const exactScope =
                        session.scope?.type === "terminal" &&
                        session.scope.targetId === activePane?.sessionId;
                      return (
                        <div
                          key={session.id}
                          className="group flex items-center gap-1 rounded-md px-2 py-1.5 hover:bg-muted/60"
                        >
                          <button
                            type="button"
                            className="min-w-0 flex-1 text-left text-xs"
                            disabled={inUse && !exactScope}
                            onClick={() => void openHistorySession(session)}
                          >
                            <div className="truncate font-medium">{session.title}</div>
                            {inUse && !exactScope ? (
                              <div className="truncate text-[0.625rem] text-muted-foreground">
                                {t("ai.historyInUse")}
                              </div>
                            ) : !exactScope ? (
                              <div className="truncate text-[0.625rem] text-muted-foreground">
                                {t("ai.historyMoveToCurrent")}
                              </div>
                            ) : null}
                          </button>
                          <button
                            type="button"
                            className="shrink-0 rounded p-0.5 text-muted-foreground/50 opacity-0 transition-opacity hover:text-destructive group-hover:opacity-100"
                            title={t("ai.deleteSession")}
                            onClick={(e) => {
                              e.stopPropagation();
                              void deleteSession(session.id);
                            }}
                          >
                            <MdDeleteOutline className="text-sm" />
                          </button>
                        </div>
                      );
                    })}
                  </div>
                );
              })
            )}
          </div>
        </div>
      ) : null}

      {detectedError ? (
        <div className="border-b border-border/70 bg-amber-500/10 p-3 text-xs">
          <div className="font-medium text-amber-600">{t("ai.errorDetected")}</div>
          <div className="mt-2 flex gap-1.5">
            <Button
              size="xs"
              onClick={() => void startChat("analyze_error", t("ai.analyzeDetectedError"))}
            >
              {t("ai.analyze")}
            </Button>
            <Button size="xs" variant="ghost" onClick={() => setDetectedError(null)}>
              {t("common.close")}
            </Button>
          </div>
        </div>
      ) : null}

      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div
            ref={scrollContainerRef}
            onScroll={handleMessagesScroll}
            className="flex-1 select-text overflow-auto p-3 terminal-scroll"
          >
            {messages.length === 0 ? (
              <div className="flex h-full min-h-[12rem] flex-col items-center justify-center gap-3 text-center text-sm text-muted-foreground">
                {!aiSettings.enabled ? (
                  <>
                    <MdAutoAwesome className="text-3xl" />
                    <div>{t("ai.goToSettingsToEnable")}</div>
                  </>
                ) : runMode !== "claude_code_agent" && !selectedModel ? (
                  <div className="flex flex-col items-center gap-4 px-4">
                    <div className="flex size-12 items-center justify-center rounded-full border border-amber-500/30 bg-amber-500/10">
                      <MdErrorOutline className="text-2xl text-amber-500" />
                    </div>
                    <div className="space-y-1">
                      <div className="text-sm font-medium text-foreground">
                        {t("ai.setupTitle")}
                      </div>
                    </div>
                    <div className="w-full space-y-2 text-left text-xs">
                      <div className="flex items-start gap-2 rounded-md border border-border/60 bg-muted/30 px-3 py-2">
                        <span className="mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-full bg-primary/15 text-[0.625rem] font-bold text-primary">
                          1
                        </span>
                        <span>{t("ai.setupStep1")}</span>
                      </div>
                      <div className="flex items-start gap-2 rounded-md border border-border/60 bg-muted/30 px-3 py-2">
                        <span className="mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-full bg-primary/15 text-[0.625rem] font-bold text-primary">
                          2
                        </span>
                        <span>{t("ai.setupStep2")}</span>
                      </div>
                    </div>
                    <Button size="sm" className="mt-1 gap-1.5" onClick={() => openSettings("ai")}>
                      <MdOutlineSettings className="text-sm" />
                      {t("ai.setupAction")}
                    </Button>
                  </div>
                ) : (
                  <>
                    <MdAutoAwesome className="text-3xl" />
                    <div>{t("ai.empty")}</div>
                  </>
                )}
              </div>
            ) : (
              <div className="space-y-3">
                {messages.map((message) => {
                  const messageSteps =
                    message.role === "assistant" ? (agentStepsMap[message.id] ?? []) : [];

                  return (
                    <div key={message.id} className="space-y-3">
                      {messageSteps.length > 0 ? (
                        <div className="rounded-md border border-border/70 bg-muted/20 p-3 text-xs leading-5">
                          <div className="mb-3 text-[0.6875rem] font-semibold uppercase tracking-[0.12em] text-muted-foreground">
                            Agent
                          </div>
                          {messageSteps.map((step) => (
                            <AgentStepView
                              key={step.stepIndex}
                              step={step}
                              prismStyle={prismStyle}
                            />
                          ))}
                        </div>
                      ) : null}
                      <div
                        className={`rounded-md border p-3 text-xs leading-5 ${
                          message.role === "user"
                            ? "border-primary/25 bg-primary/10"
                            : "border-border/70 bg-muted/20"
                        }`}
                      >
                        <div className="mb-2 text-[0.6875rem] font-semibold uppercase tracking-[0.12em] text-muted-foreground">
                          {message.role === "user" ? "User" : "AI"}
                        </div>
                        {message.role === "assistant" ? (
                          <AssistantReasoning
                            message={message}
                            loading={loading && streamingAssistantId === message.id}
                          />
                        ) : null}
                        {message.role === "assistant" ? (
                          <AssistantResponse
                            message={message}
                            loading={loading && streamingAssistantId === message.id}
                            onEarlyParse={(parsed) => {
                              updateMessagesForSession(message.sessionId, (prev) =>
                                prev.map((m) =>
                                  m.id === message.id
                                    ? {
                                        ...m,
                                        content: parsed.text,
                                        commandCards: parsed.commandCards,
                                      }
                                    : m,
                                ),
                              );
                            }}
                          />
                        ) : (
                          <div className="whitespace-pre-wrap break-words">{message.content}</div>
                        )}
                        {message.commandCards?.length ? (
                          <div className="mt-3 space-y-2">
                            {message.commandCards.map((card) => (
                              <AICommandCardView
                                key={card.id}
                                card={card}
                                execution={commandExecution[card.id]}
                                onInsert={insertCommand}
                                onSave={(item) => void saveQuickCommand(item)}
                                onAuthorize={authorizeCommand}
                                onReject={rejectCommand}
                              />
                            ))}
                          </div>
                        ) : null}
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem onClick={handleQuoteSelection}>
            <LuQuote className="mr-2" />
            {t("ai.quote")}
          </ContextMenuItem>
          <ContextMenuItem onClick={handleCopySelection}>
            <MdContentCopy className="mr-2" />
            {t("ai.copy")}
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>

      <div className="shrink-0 border-t border-border/70 p-2">
        {targetPanes.length > 0 ? (
          <div className="mb-1.5 flex flex-wrap items-center gap-1">
            <span className="text-[0.625rem] font-medium text-muted-foreground">
              {t("ai.targetSession")}:
            </span>
            {targetPanes.map((p) => (
              <span
                key={p.sessionId}
                className="inline-flex items-center gap-0.5 rounded-full border border-primary/30 bg-primary/10 px-1.5 py-0.5 text-[0.625rem] font-medium text-primary"
              >
                {p.name}
                <button
                  type="button"
                  className="ml-0.5 rounded-full p-0 hover:text-destructive"
                  onClick={() => removeTargetPane(p.sessionId)}
                >
                  <MdClose className="text-[0.625rem]" />
                </button>
              </span>
            ))}
          </div>
        ) : null}
        <div className="relative">
          {showMentionPopover ? (
            <div
              ref={mentionPopoverRef}
              className="absolute bottom-full left-0 right-0 z-30 mb-1 flex max-h-48 flex-col overflow-hidden rounded-md border bg-popover text-popover-foreground shadow-lg"
              style={{ borderColor: "var(--df-border)" }}
            >
              <div className="min-h-0 overflow-auto p-1 terminal-scroll">
                {filteredMentionPanes.length === 0 ? (
                  <div className="px-2 py-3 text-center text-xs text-muted-foreground">
                    {t("ai.noSessions")}
                  </div>
                ) : (
                  filteredMentionPanes.map((pane, idx) => {
                    const isSelected = targetPanes.some((p) => p.sessionId === pane.sessionId);
                    const isFocused = idx === mentionIndex;
                    return (
                      <button
                        key={pane.sessionId}
                        ref={(el) => {
                          if (isFocused && el) el.scrollIntoView({ block: "nearest" });
                        }}
                        type="button"
                        className={`flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-xs hover:bg-muted/60 ${isFocused ? "bg-accent" : ""} ${isSelected ? "bg-primary/10" : ""}`}
                        onClick={() => selectMentionPane(pane)}
                        onPointerEnter={() => setMentionIndex(idx)}
                      >
                        <span
                          className={`size-2 shrink-0 rounded-full ${isSelected ? "bg-primary" : "bg-muted-foreground/40"}`}
                        />
                        <span className="min-w-0 truncate font-medium">{pane.name}</span>
                        <span className="ml-auto shrink-0 text-[0.625rem] text-muted-foreground">
                          {pane.type}
                        </span>
                      </button>
                    );
                  })
                )}
              </div>
            </div>
          ) : null}
          <div className="space-y-2">
            {quotedText ? (
              <div className="flex items-center gap-1.5 rounded-md border border-primary/25 bg-primary/6">
                <div className="w-[3px] self-stretch shrink-0 rounded-l-md bg-primary/60" />
                <LuQuote className="shrink-0 text-[0.625rem] text-primary/70" />
                <span className="min-w-0 flex-1 truncate py-1.5 text-[0.6875rem] text-muted-foreground">
                  {quotedText.text}
                </span>
                <button
                  type="button"
                  className="mr-1.5 shrink-0 rounded p-0.5 text-muted-foreground/70 hover:text-foreground"
                  onClick={() => updateDraftForScope((draft) => ({ ...draft, quotedText: null }))}
                >
                  <MdClose className="text-xs" />
                </button>
              </div>
            ) : null}
            <Textarea
              ref={textareaRef}
              value={input}
              disabled={loading || !aiSettings.enabled}
              placeholder={aiSettings.enabled ? t("ai.placeholder") : t("ai.goToSettingsToEnable")}
              className="max-h-32 min-h-16 resize-none overflow-y-auto text-xs terminal-scroll"
              onChange={handleInputChange}
              onKeyDown={(event) => {
                if (showMentionPopover) {
                  if (event.key === "Escape") {
                    event.preventDefault();
                    setShowMentionPopover(false);
                    return;
                  }
                  if (event.key === "ArrowDown") {
                    event.preventDefault();
                    setMentionIndex((i) =>
                      filteredMentionPanes.length === 0 ? 0 : (i + 1) % filteredMentionPanes.length,
                    );
                    return;
                  }
                  if (event.key === "ArrowUp") {
                    event.preventDefault();
                    setMentionIndex((i) =>
                      filteredMentionPanes.length === 0
                        ? 0
                        : (i - 1 + filteredMentionPanes.length) % filteredMentionPanes.length,
                    );
                    return;
                  }
                  if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                    event.preventDefault();
                    const target = filteredMentionPanes[mentionIndex];
                    if (target) selectMentionPane(target);
                    else setShowMentionPopover(false);
                    return;
                  }
                }
                if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
                  event.preventDefault();
                  submit();
                }
              }}
            />
            <div className="flex w-full items-center justify-between gap-2">
              <div className="flex flex-1 min-w-0 items-center gap-2">
                <div className="w-1/3 min-w-0">
                  <Select
                    value={runMode}
                    onValueChange={(value) => selectRunMode(value as AIRunMode)}
                  >
                    <SelectTrigger size="sm" className="w-full text-xs">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent position="popper">
                      <SelectItem value="ask">{t("ai.modeAsk")}</SelectItem>
                      <SelectItem value="nyaterm_agent">{t("ai.modeNyatermAgent")}</SelectItem>
                      <SelectItem value="codex_agent">{t("ai.modeCodexAgent")}</SelectItem>
                      <SelectItem value="claude_code_agent">
                        {t("ai.modeClaudeCodeAgent")}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>

                <div className="w-2/3 min-w-0">
                  <ModelCombobox
                    models={selectableModels}
                    credentials={aiSettings.provider_credentials}
                    selectedModel={selectedModel}
                    selectedReasoningEffort={aiSettings.default_reasoning_effort ?? "auto"}
                    open={modelPopoverOpen}
                    onOpenChange={setModelPopoverOpen}
                    onSelect={(model) =>
                      updateAppSettings({ ai: { ...aiSettings, default_model_id: model.id } })
                    }
                    onSelectReasoningEffort={(default_reasoning_effort) =>
                      updateAppSettings({ ai: { ...aiSettings, default_reasoning_effort } })
                    }
                    className="w-full truncate"
                  />
                </div>
              </div>

              <div className="flex-shrink-0">
                {loading ? (
                  <Button size="icon-sm" variant="outline" onClick={cancelStream}>
                    <MdStop />
                  </Button>
                ) : (
                  <Button
                    size="icon-sm"
                    onClick={submit}
                    disabled={
                      !input.trim() ||
                      (!selectedModel && runMode !== "claude_code_agent") ||
                      !aiSettings.enabled
                    }
                  >
                    <MdSend />
                  </Button>
                )}
              </div>
            </div>
          </div>
        </div>
      </div>

      <AIAssistantDialogs
        clearHistoryOpen={clearHistoryOpen}
        clearingHistory={clearingHistory}
        autoModeDialogOpen={autoModeDialogOpen}
        onClearHistoryOpenChange={setClearHistoryOpen}
        onAutoModeDialogOpenChange={(open) => {
          setAutoModeDialogOpen(open);
          if (!open) setPendingExecutionMode(null);
        }}
        onClearHistory={() => void clearHistory()}
        onConfirmAutoExecutionMode={confirmAutoExecutionMode}
      />
    </div>
  );
}

export default memo(AIAssistantPanel);
