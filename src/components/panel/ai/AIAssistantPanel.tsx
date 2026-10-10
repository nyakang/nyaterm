import { randomUUID } from "@/lib/uuid";
import { supports } from "@/lib/backend/runtime";
import { emit, listen, type UnlistenFn } from "@/lib/backend/api";
import {
  type ClipboardEvent,
  memo,
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
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
  MdExpandMore,
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
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useApp } from "@/context/AppContext";
import { useTheme } from "@/context/ThemeContext";
import type { AIErrorDetectedDetail } from "@/lib/aiEvents";
import { AI_ERROR_DETECTED_EVENT } from "@/lib/aiEvents";
import {
  getModelReasoningOptions,
  resolveAILanguage,
  selectDefaultAIModel,
} from "@/lib/aiSettings";
import { classifyAIStreamControlEvent } from "@/lib/aiStreamEvent";
import { getErrorMessage } from "@/lib/errors";
import { getFileDocumentController } from "@/lib/fileDocumentRegistry";
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
  AIFileAttachment,
  AIFileReference,
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
  SessionInfo,
  SessionPane,
} from "@/types/global";
import { AgentStepView } from "./AgentStepView";
import { AICommandCardView } from "./AICommandCardView";
import {
  type AIInlineMention,
  AIReferenceComposer,
  type AIReferenceComposerHandle,
} from "./AIReferenceComposer";
import { AIUserMessageContent } from "./AIUserMessageContent";
import { AssistantReasoning } from "./AssistantReasoning";
import { AssistantResponse } from "./AssistantResponse";
import {
  type AIReferenceGroup,
  type AIReferenceOption,
  buildAIReferenceGroups,
  formatAIFileReferenceContext,
} from "./aiReferences";
import { buildAyaContext, buildAyaTarget, resolveAyaPanes } from "./ayaReferences";
import { ModelCombobox } from "./ModelCombobox";
import { ReferenceTooltip } from "./ReferenceTooltip";
import type { AIReferenceClipboardPayload } from "./referenceClipboard";
import {
  applyPastedText,
  applyReferenceSelection,
  referenceOptionAvailable,
  referenceTreeChildren,
  visibleReferenceChildren,
} from "./referenceSelection";
import type {
  AIAssistantPanelProps,
  AICommandExecutionState,
  AICommandExecutionStatus,
  QuotedText,
} from "./types";
import { actionTitle, buildPrismThemeFromColors, createLocalMessage, slugCategory } from "./utils";

type ReferenceCandidate =
  | { kind: "host"; id: string; label: string; title: string; sessionIds: string[] }
  | AIReferenceOption;

type VisibleMentionItem =
  | { kind: "host"; id: string; group: AIReferenceGroup }
  | {
      kind: "session" | "file";
      id: string;
      group: AIReferenceGroup;
      option: AIReferenceOption;
      depth: 1 | 2;
    };

interface DraftMention extends AIInlineMention {
  sessionIds: string[];
  fileReference?: AIFileReference;
}

interface AIDraft {
  text: string;
  mentions: DraftMention[];
  quotedText: QuotedText | null;
  action: AIAction | null;
}

type AIPanelView = { mode: "draft" } | { mode: "session"; sessionId: string };
type AIRunMode = "ask" | "nyaterm_agent" | "codex_agent" | "claude_code_agent";

interface AIStreamRuntime {
  streamId: string;
  aiSessionId: string;
  assistantMessageId: string;
}

const EMPTY_DRAFT: AIDraft = { text: "", mentions: [], quotedText: null, action: null };

function isGenaiModel(model: AIModelConfigItem | null | undefined) {
  return (model?.backend ?? "genai") === "genai";
}

function getEnabledGenaiModels(settings: {
  models?: AIModelConfigItem[] | null;
}) {
  return (settings.models ?? []).filter(
    (model) => model.enabled && isGenaiModel(model),
  );
}

function resolveRunMode(mode: AIMode, agentKind: AIAgentKind | null | undefined): AIRunMode {
  if (mode !== "agent") return "ask";
  if (agentKind === "codex") return "codex_agent";
  if (agentKind === "claude_code") return "claude_code_agent";
  return "nyaterm_agent";
}

const AI_WORKSPACE_SCOPE_KEY = "workspace:ai";

function buildAIScopeKey(pane: SessionPane | null) {
  return pane ? `terminal:${pane.sessionId}` : "unbound:";
}

function buildAIWorkspaceScope(): AISessionScope {
  return {
    type: "workspace",
    targetId: "main",
    connectionIds: [],
    label: "AI Assistant",
  };
}

function buildTerminalOwnerScope(pane: SessionPane | null): AISessionScope {
  if (!pane) return { type: "unbound", targetId: null, connectionIds: [], label: null };
  return {
    type: "terminal",
    targetId: pane.sessionId,
    connectionIds: pane.connectionId ? [pane.connectionId] : [],
    label: pane.name,
  };
}

function sameAIScope(left: AISessionScope | null | undefined, right: AISessionScope) {
  return left?.type === right.type && left.targetId === right.targetId;
}

function AIAssistantPanel({ activePane, activeConnection, intent }: AIAssistantPanelProps) {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings, tabs, savedConnections, savedGroups } = useApp();
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
  const [historyLoadingSessionId, setHistoryLoadingSessionId] = useState<string | null>(null);
  const [historyLoadError, setHistoryLoadError] = useState<string | null>(null);
  const [clearHistoryOpen, setClearHistoryOpen] = useState(false);
  const [clearingHistory, setClearingHistory] = useState(false);
  const [detectedError, setDetectedError] = useState<AIErrorDetectedDetail | null>(null);
  const [showMentionPopover, setShowMentionPopover] = useState(false);
  const [mentionQuery, setMentionQuery] = useState("");
  const [mentionRange, setMentionRange] = useState<{ start: number; end: number } | null>(null);
  const [mentionIndex, setMentionIndex] = useState(0);
  const [expandedMentionGroups, setExpandedMentionGroups] = useState<Set<string>>(new Set());
  const [collapsedMentionGroups, setCollapsedMentionGroups] = useState<Set<string>>(new Set());
  const [commandExecution, setCommandExecution] = useState<Record<string, AICommandExecutionState>>(
    {},
  );
  const [agentStepsMap, setAgentStepsMap] = useState<Record<string, AgentStepPayload[]>>({});
  const [liveSessions, setLiveSessions] = useState<SessionInfo[]>([]);
  const [modelPopoverOpen, setModelPopoverOpen] = useState(false);
  const [showExecutionMenu, setShowExecutionMenu] = useState(false);
  const [autoModeDialogOpen, setAutoModeDialogOpen] = useState(false);
  const [pendingExecutionMode, setPendingExecutionMode] =
    useState<AIAgentCommandExecutionMode | null>(null);
  const handledIntentIdRef = useRef<string | null>(null);
  const historyLoadRequestRef = useRef(0);
  const executionMenuButtonRef = useRef<HTMLButtonElement | null>(null);
  const executionMenuRef = useRef<HTMLDivElement | null>(null);
  const historyButtonRef = useRef<HTMLButtonElement | null>(null);
  const historyCardRef = useRef<HTMLDivElement | null>(null);
  const mentionPopoverRef = useRef<HTMLDivElement | null>(null);
  const composerRef = useRef<AIReferenceComposerHandle | null>(null);
  const isComposingRef = useRef(false);
  const streamUnlistenersRef = useRef<Map<string, UnlistenFn>>(new Map());
  const streamSessionByStreamIdRef = useRef<Map<string, string>>(new Map());
  const scrollContainerRef = useRef<HTMLDivElement>(null);
  const shouldAutoScrollRef = useRef(true);

  const storedSelectedModel = useMemo(() => selectDefaultAIModel(aiSettings), [aiSettings]);
  const prismStyle = useMemo(() => buildPrismThemeFromColors(theme.colors), [theme.colors]);
  const mode = aiSettings.default_mode ?? "ask";
  const agentKind = aiSettings.default_agent_kind ?? "nyaterm";
  const configuredRunMode = supports("aiAgents")
    ? resolveRunMode(mode, agentKind)
    : "ask";
  const codexAgentEnabled =
    supports("aiAgents") && (aiSettings.codex?.enabled ?? false);
  const claudeCodeAgentEnabled =
    supports("aiAgents") && (aiSettings.claude_code?.enabled ?? false);
  const runMode =
    (configuredRunMode === "codex_agent" && !codexAgentEnabled) ||
    (configuredRunMode === "claude_code_agent" && !claudeCodeAgentEnabled)
      ? "ask"
      : configuredRunMode;
  const genaiModels = useMemo(() => getEnabledGenaiModels(aiSettings), [aiSettings]);
  const selectedModel = useMemo(() => {
    if (runMode === "codex_agent" || runMode === "claude_code_agent") return null;
    return (
      (isGenaiModel(storedSelectedModel) ? storedSelectedModel : null) ?? genaiModels[0] ?? null
    );
  }, [genaiModels, runMode, storedSelectedModel]);
  const configuredReasoningEffort = aiSettings.default_reasoning_effort ?? "auto";
  const selectedReasoningEffort = getModelReasoningOptions(selectedModel).includes(
    configuredReasoningEffort,
  )
    ? configuredReasoningEffort
    : "auto";
  const selectableModels = runMode === "ask" || runMode === "nyaterm_agent" ? genaiModels : [];
  const externalModelLabel =
    runMode === "codex_agent"
      ? (aiSettings.codex?.default_model ?? "Codex")
      : runMode === "claude_code_agent"
        ? (aiSettings.claude_code?.default_model ?? "Claude Code")
        : null;
  const agentExecutionMode = aiSettings.agent_command_execution_mode ?? "confirm_each";
  const agentBackgroundExecutionEnabled = aiSettings.agent_background_execution_enabled ?? false;
  // AyaAgent 对话属于当前主窗口 workspace；其他模式保留原有 terminal scope。
  const usesWorkspaceScope = runMode === "nyaterm_agent";
  const scopeKey = useMemo(
    () => (usesWorkspaceScope ? AI_WORKSPACE_SCOPE_KEY : buildAIScopeKey(activePane)),
    [activePane, usesWorkspaceScope],
  );
  const ownerScope = useMemo(
    () => (usesWorkspaceScope ? buildAIWorkspaceScope() : buildTerminalOwnerScope(activePane)),
    [activePane, usesWorkspaceScope],
  );
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
  const isExternalAgentMode = runMode === "codex_agent" || runMode === "claude_code_agent";

  useEffect(() => {
    if (configuredRunMode === runMode) return;
    updateAppSettings({
      ai: { ...aiSettings, default_mode: "ask", default_agent_kind: "nyaterm" },
    });
  }, [aiSettings, configuredRunMode, runMode, updateAppSettings]);

  const openTerminalScopeKeys = useMemo(() => {
    const keys = new Set<string>();
    for (const tab of tabs) {
      for (const pane of collectSessionPanes(tab.root)) {
        if (pane.paneKind === "terminal") keys.add(buildAIScopeKey(pane));
      }
    }
    return keys;
  }, [tabs]);

  useEffect(() => {
    let disposed = false;
    const refreshLiveSessions = async () => {
      try {
        const sessions = await invoke<SessionInfo[]>("list_sessions");
        if (!disposed) setLiveSessions(sessions);
      } catch {
        if (!disposed) setLiveSessions([]);
      }
    };

    void refreshLiveSessions();
    const unlisten = listen("sessions-changed", () => void refreshLiveSessions());
    return () => {
      disposed = true;
      unlisten.then((dispose) => dispose()).catch(() => {});
    };
  }, []);

  const allSessionPanes = useMemo(() => {
    const panes: SessionPane[] = [];
    for (const tab of tabs) {
      for (const pane of collectSessionPanes(tab.root)) {
        if (!pane.connecting && !pane.connectError) panes.push(pane);
      }
    }
    return panes;
  }, [tabs]);

  const referenceGroups = useMemo(
    () =>
      buildAIReferenceGroups(
        allSessionPanes,
        liveSessions,
        savedConnections,
        runMode === "nyaterm_agent",
      ),
    [allSessionPanes, liveSessions, runMode, savedConnections],
  );
  const referenceCandidates = useMemo<ReferenceCandidate[]>(
    () =>
      referenceGroups.flatMap((group) => [
        {
          kind: "host" as const,
          id: `host:${group.id}`,
          label: group.id === "local" ? t("ai.localReferenceRoot") : group.label,
          title: group.title,
          sessionIds: group.sessions,
        },
        ...group.children,
      ]),
    [referenceGroups, t],
  );

  const filteredReferenceGroups = useMemo(() => {
    const query = mentionQuery.trim().toLowerCase();
    if (!query) return referenceGroups;
    return referenceGroups
      .map((group) => {
        const groupMatches = `${group.label} ${group.title} ${group.host ?? ""}`
          .toLowerCase()
          .includes(query);
        const children = group.children.filter((option) =>
          `${option.label} ${option.title} ${option.id} ${option.sessionIds.join(" ")}`
            .toLowerCase()
            .includes(query),
        );
        return {
          ...group,
          children: groupMatches ? group.children : children,
          queryExpanded: groupMatches || children.length > 0,
        };
      })
      .filter((group) => group.queryExpanded);
  }, [mentionQuery, referenceGroups]);
  const visibleMentionItems = useMemo<VisibleMentionItem[]>(
    () =>
      filteredReferenceGroups.flatMap((group) => {
        const root = { kind: "host" as const, id: group.id, group };
        const expanded =
          !collapsedMentionGroups.has(group.id) &&
          (!!mentionQuery.trim() || expandedMentionGroups.has(group.id));
        return expanded
          ? [
              root,
              ...referenceTreeChildren(group).map(({ option, depth }) => ({
                kind: option.kind,
                id: option.id,
                group,
                option,
                depth,
              })),
            ]
          : [root];
      }),
    [collapsedMentionGroups, expandedMentionGroups, filteredReferenceGroups, mentionQuery],
  );
  const referencedSessionIds = useMemo(
    () => [...new Set(currentDraft.mentions.flatMap((mention) => mention.sessionIds))],
    [currentDraft.mentions],
  );
  const targetPanes = useMemo(
    () =>
      referencedSessionIds
        .map((sessionId) =>
          allSessionPanes.find((pane) => pane.sessionId === sessionId && pane.paneKind !== "file"),
        )
        .filter((pane): pane is SessionPane => !!pane),
    [allSessionPanes, referencedSessionIds],
  );

  // biome-ignore lint/correctness/useExhaustiveDependencies: reset selection whenever the filtered tree changes.
  useEffect(() => {
    setMentionIndex(0);
  }, [filteredReferenceGroups]);

  useEffect(() => {
    setMentionIndex((index) => Math.min(index, Math.max(0, visibleMentionItems.length - 1)));
  }, [visibleMentionItems.length]);

  const effectivePanes = useMemo(() => {
    if (runMode === "nyaterm_agent") {
      return resolveAyaPanes(allSessionPanes, activePane, currentDraft.mentions).panes;
    }
    const paneMap = new Map<string, SessionPane>();
    for (const pane of targetPanes) paneMap.set(pane.sessionId, pane);
    if (activePane && !paneMap.has(activePane.sessionId)) {
      paneMap.set(activePane.sessionId, activePane);
    }
    return [...paneMap.values()];
  }, [activePane, allSessionPanes, currentDraft.mentions, runMode, targetPanes]);
  const panelMeta =
    effectivePanes.length > 1 && activePane
      ? t("ai.panelMetaMultiTarget", {
          target: activePane.name,
          count: effectivePanes.length - 1,
        })
      : (activePane?.name ?? selectedModel?.name ?? externalModelLabel ?? t("ai.notConfigured"));
  useEffect(() => {
    if (!selectedModel) return;
    if (
      selectedModel.id === aiSettings.default_model_id &&
      configuredReasoningEffort === selectedReasoningEffort
    ) {
      return;
    }
    updateAppSettings({
      ai: {
        ...aiSettings,
        default_model_id: selectedModel.id,
        default_reasoning_effort: selectedReasoningEffort,
      },
    });
  }, [
    aiSettings,
    configuredReasoningEffort,
    selectedModel,
    selectedReasoningEffort,
    updateAppSettings,
  ]);

  const filteredSessions = useMemo(() => {
    const keyword = historyQuery.trim().toLowerCase();
    if (!keyword) return sessions;

    return sessions.filter((session) =>
      [session.title, session.createdAt, session.updatedAt, session.id].some((value) =>
        value.toLowerCase().includes(keyword),
      ),
    );
  }, [historyQuery, sessions]);

  const updateDraftForScope = useCallback((updater: (draft: AIDraft) => AIDraft) => {
    setDraftsByScope((prev) => ({
      ...prev,
      [scopeKey]: updater(prev[scopeKey] ?? EMPTY_DRAFT),
    }));
  }, [scopeKey]);

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

  // biome-ignore lint/correctness/useExhaustiveDependencies: invalidate pending history loads whenever the AI scope changes.
  useEffect(() => {
    historyLoadRequestRef.current += 1;
    setHistoryLoadingSessionId(null);
    setHistoryLoadError(null);
  }, [scopeKey]);

  const loadSessionMessages = useCallback(async (sessionId: string, requestId: number) => {
    const items = await invoke<AIMessage[]>("get_ai_messages", {
      sessionId,
    });
    if (historyLoadRequestRef.current !== requestId) return false;
    setMessagesBySessionId((prev) => ({ ...prev, [sessionId]: items }));
    setActiveSessionIdByScope((prev) => ({
      ...prev,
      [scopeKey]: sessionId,
    }));
    setPanelViewByScope((prev) => ({
      ...prev,
      [scopeKey]: { mode: "session", sessionId },
    }));
    setHistoryLoadingSessionId(null);
    setHistoryLoadError(null);
    setShowHistory(false);
    return true;
  }, [scopeKey]);

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
          ai: {
            ...aiSettings,
            default_mode: "ask",
            default_agent_kind: "nyaterm",
          },
        });
        return;
      }

      if (nextMode === "nyaterm_agent") {
        const nextModel = getEnabledGenaiModels(aiSettings)[0];
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
        if (!claudeCodeAgentEnabled) return;
        updateAppSettings({
          ai: {
            ...aiSettings,
            default_mode: "agent",
            default_agent_kind: "claude_code",
          },
        });
        return;
      }

      if (!codexAgentEnabled) return;

      updateAppSettings({
        ai: {
          ...aiSettings,
          default_mode: "agent",
          default_agent_kind: "codex",
        },
      });
      return;
    },
    [aiSettings, claudeCodeAgentEnabled, codexAgentEnabled, t, updateAppSettings],
  );

  const buildTargetForPane = useCallback(
    (pane: SessionPane): AITerminalTarget => {
      if (runMode === "nyaterm_agent") {
        return buildAyaTarget(pane, allSessionPanes, savedConnections);
      }
      const conn = pane.connectionId
        ? (savedConnections.find((item) => item.id === pane.connectionId) ??
          null)
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
    [activeConnection, activePane?.sessionId, allSessionPanes, runMode, savedConnections],
  );

  const ayaExecutionLabel =
    runMode === "nyaterm_agent"
      ? resolveAyaPanes(allSessionPanes, activePane, currentDraft.mentions)
          .executionIds.map((id) => {
            const pane = effectivePanes.find((item) => item.sessionId === id);
            return pane ? buildTargetForPane(pane).label : id;
          })
          .join(", ")
      : "";
  const ayaFileSourceTitles = currentDraft.mentions
    .filter((mention) => mention.kind === "file")
    .map((mention) => mention.title);

  const buildTargetContexts = useCallback(
    async (
      panes: SessionPane[],
      selectedText?: string,
    ): Promise<AITargetContext[]> => {
      const lineLimit = Math.max(
        1,
        Math.floor(aiSettings.context_line_limit / Math.max(1, panes.length)),
      );
      // Load runtime profiles once for all selected targets. A failed lookup
      // leaves the profile unknown instead of using obsolete saved settings.
      const sessions = await invoke<SessionInfo[]>("list_sessions").catch(
        () => [],
      );
      const sessionInfoById = new Map(
        sessions.map((session) => [session.id, session]),
      );
      return Promise.all(
        panes.map(async (pane, index) => {
          const conn = pane.connectionId
            ? (savedConnections.find((item) => item.id === pane.connectionId) ??
              null)
            : pane.sessionId === activePane?.sessionId
              ? activeConnection
              : null;
          return {
            target: buildTargetForPane(pane),
            context: await buildAIContext({
              pane,
              connection: conn,
              groups: savedGroups,
              sessionInfo: sessionInfoById.get(pane.sessionId),
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
      savedGroups,
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
    async (
      action: AIAction,
      userInput: string,
      selectedText?: string,
      fileReferences: AIFileReference[] = [],
      draftMentions: DraftMention[] = [],
      referenceOffset = 0,
    ) => {
      const aya =
        runMode === "nyaterm_agent"
          ? resolveAyaPanes(allSessionPanes, activePane, draftMentions)
          : null;
      const panes = aya?.panes ?? effectivePanes;
      if (aya?.missingIds.length) {
        toast.error(t("ai.referenceUnavailable"));
        return;
      }
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
      if (!requestModel && requestAgentKind === "nyaterm") {
        toast.error(t("ai.noEnabledModels"));
        return;
      }
      const requestModelId = requestAgentKind === "nyaterm" ? (requestModel?.id ?? null) : null;
      const requestModelName =
        requestAgentKind === "codex"
          ? (aiSettings.codex?.default_model ?? null)
          : requestAgentKind === "claude_code"
            ? (aiSettings.claude_code?.default_model ?? null)
            : (requestModel?.name ?? null);
      const requestMode: AIMode = runMode === "ask" ? "ask" : "agent";
      const requestSessionId =
        currentSession?.agentKind && currentSession.agentKind !== requestAgentKind
          ? null
          : currentSessionId;

      setDetectedError(null);
      const assistantId = `assistant-${Date.now()}-${Math.random().toString(36).slice(2)}`;
      const requestStreamId = `ai-stream-${randomUUID()}`;
      let resolvedSessionId = requestSessionId ?? `pending-${requestStreamId}`;
      const ayaContext = aya
        ? buildAyaContext(draftMentions, fileReferences, aya.executionIds, referenceOffset)
        : undefined;
      const references = ayaContext?.references ?? [];
      const messageFileReferences = draftMentions
        .map((mention) => mention.fileReference)
        .filter((reference): reference is AIFileReference => !!reference);
      const attachmentReferences = messageFileReferences.length
        ? messageFileReferences
        : fileReferences;
      const attachments: AIFileAttachment[] = aya
        ? []
        : attachmentReferences.map((reference) => ({
            id: reference.id,
            name: reference.name,
            path: reference.path,
            mimeType: reference.mimeType,
            sizeBytes: reference.sizeBytes,
            host: reference.host,
            connectionId: reference.connectionId,
            terminalSessionId: reference.terminalSessionId,
            backend: reference.backend,
          }));
      const userMessage = createLocalMessage(
        "user",
        userInput,
        resolvedSessionId,
        attachments,
        references,
      );
      const assistantMessage: AIMessage = {
        id: assistantId,
        sessionId: resolvedSessionId,
        role: "assistant",
        content: "",
        createdAt: new Date().toISOString(),
        reasoningContent: null,
        commandCards: [],
      };
      setActiveSessionIdByScope((prev) => ({
        ...prev,
        [scopeKey]: resolvedSessionId,
      }));
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
              ...pendingMessages.map((message) => ({
                ...message,
                sessionId: nextSessionId,
              })),
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
        if (requestSessionId && !sameAIScope(currentSession?.scope, ownerScope)) {
          await invoke<AISession>("rebind_ai_session", {
            sessionId: requestSessionId,
            ownerScope,
          });
        }
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
                    ? {
                        ...message,
                        content: `${message.content}${payload.textDelta}`,
                      }
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

            const controlEvent = classifyAIStreamControlEvent(payload.type);
            if (controlEvent.kind === "warning") {
              const warning =
                payload.error === "codex_mcp_fallback"
                  ? t("ai.codexMcpFallback")
                  : (payload.error ?? t("ai.requestFailed"));
              toast.warning(warning);
              return;
            }

            if (controlEvent.kind === "done") {
              if (payload.sessionId) bindRealSessionId(payload.sessionId);
              if (controlEvent.terminatesStream) cleanupStreamListener(requestStreamId);
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

            if (controlEvent.kind === "error") {
              const errorMessage = payload.error?.startsWith("claude_mcp_unavailable:")
                ? t("ai.claudeMcpUnavailable", {
                    reason: payload.error.slice("claude_mcp_unavailable:".length),
                  })
                : (payload.error ?? t("ai.requestFailed"));
              if (controlEvent.terminatesStream) cleanupStreamListener(requestStreamId);
              updateMessagesForSession(resolvedSessionId, (prev) =>
                prev.map((message) =>
                  message.id === assistantId
                    ? {
                        ...message,
                        content: errorMessage,
                      }
                    : message,
                ),
              );
              toast.error(errorMessage);
            }
          },
        );
        streamUnlistenersRef.current.set(requestStreamId, unlisten);

        const targets = panes.map(buildTargetForPane);
        const targetContexts = await buildTargetContexts(panes, selectedText);
        // The primary context belongs to the default target. Other targets keep
        // their own complete snapshots rather than mixing hosts and metadata.
        const primaryContext = targetContexts[0]?.context ?? {
          connectionName: null,
          host: null,
          port: null,
          username: null,
          cwd: null,
          os: null,
          arch: null,
          recentOutput: "",
          selectedText: "",
          inputBuffer: "",
        };
        const fileContext = formatAIFileReferenceContext(
          fileReferences.map((reference) => ({
            title: `${reference.host ? `${reference.host}:` : ""}${reference.path}`,
            content: reference.content,
          })),
          t("ai.referencedFile"),
        );
        const context: AIContext = {
          ...primaryContext,
          selectedText: ayaContext
            ? primaryContext.selectedText
            : [primaryContext.selectedText, fileContext].filter(Boolean).join("\n\n"),
          ...(ayaContext ? { ayaContext } : {}),
        };
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
            defaultTargetSessionId:
              aya && aya.executionIds.length !== 1
                ? null
                : (aya?.executionIds[0] ?? panes[0]?.sessionId ?? null),
            existingExternalSessionId:
              currentSession?.agentKind === requestAgentKind
                ? (currentSession.externalSessionId ?? null)
                : null,
            ownerScope,
            targets,
            targetContexts,
            attachments,
            references,
            action,
            userInput,
            mode: requestMode,
            modelId: requestModelId,
            modelName: requestModelName,
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
        void invoke("cancel_ai_chat_stream", {
          streamId: requestStreamId,
        }).catch(() => {});
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
      activePane,
      allSessionPanes,
      aiSettings.claude_code?.default_model,
      aiSettings.claude_code?.permission_mode,
      aiSettings.codex?.default_model,
      aiSettings.codex?.permission_mode,
      aiSettings.enabled,
      aiSettings.external_agent_permission_mode,
      appSettings.ui.language,
      appendAudit,
      buildTargetContexts,
      buildTargetForPane,
      cleanupStreamListener,
      currentSession,
      currentSessionId,
      effectivePanes,
      loadSessions,
      ownerScope,
      runMode,
      savedConnections,
      selectedModel,
      t,
      updateMessagesForSession,
      scopeKey,
    ],
  );

  useEffect(() => {
    if (!intent || handledIntentIdRef.current === intent.id) return;
    handledIntentIdRef.current = intent.id;
    const fallbackText = actionTitle(intent.action);
    const prompt = intent.userInput?.trim() || fallbackText;
    if (intent.fileReference) {
      const fileReference = intent.fileReference;
      const sizeBytes = new TextEncoder().encode(fileReference.content).byteLength;
      if (sizeBytes > aiSettings.max_ai_file_size_bytes) {
        toast.error(
          t("ai.fileReferenceTooLarge", {
            limit: aiSettings.max_ai_file_size_bytes.toLocaleString(),
          }),
        );
        return;
      }
      const stagedReference = { ...fileReference, sizeBytes };
      const token = `@${fileReference.name}`;
      const prefix = prompt ? `${prompt} ` : "";
      const start = prefix.length;
      updateDraftForScope((draft) => ({
        ...draft,
        action: intent.action,
        text: `${prefix}${token} `,
        mentions: [
          {
            id: fileReference.id,
            kind: "file",
            label: fileReference.name,
            title: `${fileReference.host ? `${fileReference.host}:` : ""}${fileReference.path}`,
            start,
            end: start + token.length,
            sessionIds: [fileReference.terminalSessionId],
            fileReference: stagedReference,
          },
        ],
        quotedText: null,
      }));
      setShowMentionPopover(false);
      setMentionQuery("");
      setMentionRange(null);
      requestAnimationFrame(() => composerRef.current?.focusAt(start + token.length + 1));
      return;
    }
    void startChat(intent.action, prompt, intent.selectedText);
  }, [aiSettings.max_ai_file_size_bytes, intent, startChat, t, updateDraftForScope]);

  const submit = useCallback(() => {
    const value = input.trim();
    if (!value || loading) return;
    const fullInput = quotedText ? `> ${quotedText.text}\n\n${value}` : value;
    const action = currentDraft.action ?? "generate_command";
    const uniqueFileReferences = new Map<string, AIFileReference>();
    for (const mention of currentDraft.mentions) {
      if (mention.fileReference)
        uniqueFileReferences.set(mention.fileReference.id, mention.fileReference);
    }
    const fileReferences = [...uniqueFileReferences.values()].map((reference) => {
      if (runMode !== "nyaterm_agent") return reference;
      const pane = allSessionPanes.find(
        (item) =>
          item.paneKind === "file" &&
          item.sessionId === reference.terminalSessionId &&
          item.file.backend === reference.backend &&
          item.file.path === reference.path,
      );
      const snapshot = pane ? getFileDocumentController(pane.id)?.getSnapshot?.() : undefined;
      const content = snapshot?.content ?? reference.content;
      return { ...reference, content, sizeBytes: new TextEncoder().encode(content).byteLength };
    });
    if (runMode === "nyaterm_agent") {
      if (resolveAyaPanes(allSessionPanes, activePane, currentDraft.mentions).missingIds.length) {
        toast.error(t("ai.referenceUnavailable"));
        return;
      }
      if (
        fileReferences.reduce((sum, file) => sum + (file.sizeBytes ?? 0), 0) >
        aiSettings.max_ai_file_size_bytes
      ) {
        toast.error(
          t("ai.fileReferenceTooLarge", {
            limit: aiSettings.max_ai_file_size_bytes.toLocaleString(),
          }),
        );
        return;
      }
    }
    const offset =
      (quotedText ? `> ${quotedText.text}\n\n`.length : 0) -
      (input.length - input.trimStart().length);
    updateDraftForScope(() => EMPTY_DRAFT);
    setShowMentionPopover(false);
    setMentionQuery("");
    setMentionRange(null);
    shouldAutoScrollRef.current = true;
    void startChat(action, fullInput, undefined, fileReferences, currentDraft.mentions, offset);
  }, [
    activePane,
    aiSettings.max_ai_file_size_bytes,
    allSessionPanes,
    runMode,
    t,
    currentDraft.action,
    currentDraft.mentions,
    input,
    loading,
    quotedText,
    startChat,
    updateDraftForScope,
  ]);

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
          id: `ai-${randomUUID()}`,
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
    if (loading || historyLoadingSessionId) return;
    historyLoadRequestRef.current += 1;
    setHistoryLoadingSessionId(null);
    setHistoryLoadError(null);
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
  }, [historyLoadingSessionId, loadSessions, loading]);

  const deleteSession = useCallback(
    async (sessionId: string) => {
      if (historyLoadingSessionId === sessionId) return;
      try {
        await invoke("delete_ai_session", { sessionId });
        if (currentSessionId === sessionId) {
          historyLoadRequestRef.current += 1;
          setHistoryLoadingSessionId(null);
          setHistoryLoadError(null);
          setActiveSessionIdByScope((prev) => ({ ...prev, [scopeKey]: null }));
          setPanelViewByScope((prev) => ({
            ...prev,
            [scopeKey]: { mode: "draft" },
          }));
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
    [currentSessionId, historyLoadingSessionId, loadSessions, scopeKey],
  );

  const historySections = useMemo(() => {
    const current: AISession[] = [];
    const sameConnection: AISession[] = [];
    const other: AISession[] = [];
    for (const session of filteredSessions) {
      const scope = session.scope;
      if (
        session.id === currentSessionId ||
        (scope?.type === "terminal" && scope.targetId === activePane?.sessionId)
      ) {
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
      {
        key: "current",
        label: t("ai.historyCurrentTerminal"),
        sessions: current,
      },
      {
        key: "same",
        label: t("ai.historySameConnection"),
        sessions: sameConnection,
      },
      { key: "other", label: t("ai.historyOtherSessions"), sessions: other },
    ];
  }, [activePane?.connectionId, activePane?.sessionId, currentSessionId, filteredSessions, t]);

  const isSessionUsedByAnotherScope = useCallback(
    (sessionId: string) =>
      !!streamRuntimeBySession[sessionId] ||
      Object.entries(activeSessionIdByScope).some(
        ([key, value]) => key !== scopeKey && value === sessionId && openTerminalScopeKeys.has(key),
      ),
    [activeSessionIdByScope, openTerminalScopeKeys, streamRuntimeBySession, scopeKey],
  );

  const openHistorySession = useCallback(
    async (session: AISession) => {
      const requestId = ++historyLoadRequestRef.current;
      setHistoryLoadingSessionId(session.id);
      setHistoryLoadError(null);
      try {
        if (!sameAIScope(session.scope, ownerScope)) {
          await invoke<AISession>("rebind_ai_session", {
            sessionId: session.id,
            ownerScope,
          });
          if (historyLoadRequestRef.current !== requestId) return;
          setActiveSessionIdByScope((prev) => ({ ...prev, [scopeKey]: session.id }));
          try {
            const nextSessions = await invoke<AISession[]>("get_ai_sessions");
            if (historyLoadRequestRef.current !== requestId) return;
            setSessions(nextSessions);
          } catch {
            if (historyLoadRequestRef.current !== requestId) return;
          }
        }
        await loadSessionMessages(session.id, requestId);
      } catch (error) {
        if (historyLoadRequestRef.current !== requestId) return;
        const message = getErrorMessage(error);
        setHistoryLoadError(message);
        setHistoryLoadingSessionId(null);
        toast.error(`${t("ai.historyLoadFailed")}: ${message}`);
      }
    },
    [loadSessionMessages, ownerScope, scopeKey, t],
  );

  const newChat = useCallback(() => {
    if (loading) return;
    historyLoadRequestRef.current += 1;
    setHistoryLoadingSessionId(null);
    setHistoryLoadError(null);
    setActiveSessionIdByScope((prev) => ({ ...prev, [scopeKey]: null }));
    setPanelViewByScope((prev) => ({ ...prev, [scopeKey]: { mode: "draft" } }));
    updateDraftForScope(() => EMPTY_DRAFT);
    setDetectedError(null);
    setCommandExecution({});
    setShowMentionPopover(false);
    setMentionQuery("");
    setMentionRange(null);
    shouldAutoScrollRef.current = true;
  }, [loading, scopeKey, updateDraftForScope]);

  const handleCopySelection = useCallback(() => {
    const sel = window.getSelection()?.toString();
    if (!sel) return;
    // 先走浏览器 copy 事件，让消息/编辑器的引用元数据一并写入自定义剪贴板格式。
    if (typeof document.execCommand === "function" && document.execCommand("copy")) return;
    void navigator.clipboard.writeText(sel);
  }, []);

  const handleQuoteSelection = useCallback(() => {
    const sel = window.getSelection()?.toString()?.trim();
    if (sel) {
      updateDraftForScope((draft) => ({ ...draft, quotedText: { text: sel } }));
      composerRef.current?.focus();
    }
  }, [updateDraftForScope]);

  const handleComposerChange = useCallback(
    (value: string, mentions: AIInlineMention[]) => {
      updateDraftForScope((draft) => ({
        ...draft,
        text: value,
        mentions: mentions.map((mention) => {
          const previous = draft.mentions.find((item) => item.id === mention.id);
          return {
            ...mention,
            sessionIds: previous?.sessionIds ?? [],
            fileReference: previous?.fileReference,
          };
        }),
      }));
    },
    [updateDraftForScope],
  );

  const handleMentionQuery = useCallback(
    (query: string, range: { start: number; end: number } | null) => {
      setMentionQuery(query);
      setMentionRange(range);
      setShowMentionPopover(!!range);
      if (range) {
        setCollapsedMentionGroups(new Set());
        if (!showMentionPopover) setMentionIndex(0);
      }
    },
    [showMentionPopover],
  );

  const closeMentionPopover = useCallback(() => {
    setShowMentionPopover(false);
    setMentionQuery("");
    setMentionRange(null);
    setCollapsedMentionGroups(new Set());
  }, []);

  const toggleMentionGroup = useCallback((groupId: string, expanded: boolean) => {
    if (expanded) {
      setCollapsedMentionGroups((previous) => new Set(previous).add(groupId));
      setExpandedMentionGroups((previous) => {
        const next = new Set(previous);
        next.delete(groupId);
        return next;
      });
      return;
    }
    setCollapsedMentionGroups((previous) => {
      const next = new Set(previous);
      next.delete(groupId);
      return next;
    });
    setExpandedMentionGroups((previous) => new Set(previous).add(groupId));
  }, []);

  const buildDraftMention = useCallback(
    (option: ReferenceCandidate): DraftMention | null => {
      const group = referenceGroups.find((candidateGroup) =>
        option.kind === "host"
          ? `host:${candidateGroup.id}` === option.id
          : candidateGroup.children.some((child) => child.id === option.id),
      );
      const available =
        !!group &&
        (option.kind === "host"
          ? group.sessions.length > 0
          : referenceOptionAvailable(group, option));
      if (!available) {
        toast.error(t("ai.referenceUnavailable"));
        return null;
      }
      if (option.kind !== "file") {
        return {
          id: option.id,
          kind: option.kind,
          label: option.label,
          title: option.title,
          start: 0,
          end: 0,
          sessionIds: option.sessionIds,
        };
      }
      const current =
        getFileDocumentController(option.pane.id)?.getSnapshot?.() ?? option.pane.file.initial;
      const sizeBytes = new TextEncoder().encode(current.content).byteLength;
      const currentFileIds = new Set<string>();
      const currentFileBytes = currentDraft.mentions.reduce((total, item) => {
        const reference = item.fileReference;
        if (!reference || currentFileIds.has(reference.id)) return total;
        currentFileIds.add(reference.id);
        return total + (reference.sizeBytes ?? 0);
      }, 0);
      const alreadyReferenced = currentFileIds.has(option.id);
      const maximum = Math.max(0, aiSettings.max_ai_file_size_bytes);
      if (sizeBytes > maximum || (!alreadyReferenced && currentFileBytes + sizeBytes > maximum)) {
        toast.error(t("ai.fileReferenceTooLarge", { limit: maximum.toLocaleString() }));
        return null;
      }
      return {
        id: option.id,
        kind: "file",
        label: option.label,
        title: option.title,
        start: 0,
        end: 0,
        sessionIds: [option.pane.sessionId],
        fileReference: {
          id: option.id,
          name: option.pane.name,
          path: option.pane.file.path,
          backend: option.pane.file.backend,
          terminalSessionId: option.pane.sessionId,
          connectionId: option.pane.connectionId ?? null,
          host: option.host,
          sizeBytes,
          mimeType: "text/plain",
          content: current.content,
          openedAt: option.pane.openedAt,
        },
      };
    },
    [aiSettings.max_ai_file_size_bytes, currentDraft.mentions, referenceGroups, t],
  );

  const insertMention = useCallback(
    (
      option:
        | AIReferenceOption
        | { kind: "host"; id: string; label: string; title: string; sessionIds: string[] },
    ) => {
      const range = mentionRange ?? { start: input.length, end: input.length };
      const mention = buildDraftMention(option);
      if (!mention) return;
      const result = applyReferenceSelection(currentDraft, mention, range);
      updateDraftForScope((draft) => ({ ...draft, text: result.text, mentions: result.mentions }));
      closeMentionPopover();
      requestAnimationFrame(() => composerRef.current?.focusAt(result.caret));
    },
    [
      buildDraftMention,
      closeMentionPopover,
      currentDraft,
      input,
      mentionRange,
      updateDraftForScope,
    ],
  );

  const handlePasteReferences = useCallback(
    (
      event: ClipboardEvent<HTMLDivElement>,
      pastedText: string,
      range: { start: number; end: number },
      payload: AIReferenceClipboardPayload | null,
    ) => {
      const candidates = referenceCandidates;
      const findCandidate = (reference: {
        id?: string;
        kind: "host" | "session" | "file";
        label: string;
        title?: string;
        sessionIds?: string[];
        file?: { path?: string | null; backend?: string | null; terminalSessionId?: string | null };
      }) => {
        const exact = reference.id
          ? candidates.filter((candidate) => candidate.id === reference.id)
          : [];
        if (exact.length === 1) return exact[0];
        const matches = candidates.filter((candidate) => {
          if (candidate.kind !== reference.kind || candidate.label !== reference.label)
            return false;
          if (candidate.kind === "file") {
            return (
              (!reference.file?.path || candidate.pane.file.path === reference.file.path) &&
              (!reference.file?.backend ||
                candidate.pane.file.backend === reference.file.backend) &&
              (!reference.file?.terminalSessionId ||
                candidate.pane.sessionId === reference.file.terminalSessionId)
            );
          }
          return (
            !reference.title ||
            candidate.title === reference.title ||
            (!!reference.sessionIds?.length &&
              candidate.sessionIds.some((id) => reference.sessionIds?.includes(id)))
          );
        });
        return matches.length === 1 ? matches[0] : null;
      };

      if (!payload || payload.text !== pastedText) return false;
      const mentions: DraftMention[] = [];
      for (const serialized of payload.references) {
        if (
          serialized.start < 0 ||
          serialized.end <= serialized.start ||
          serialized.end > pastedText.length ||
          pastedText.slice(serialized.start, serialized.end) !== `@${serialized.label}`
        ) {
          continue;
        }
        const candidate = findCandidate(serialized);
        if (!candidate) continue;
        const mention = buildDraftMention(candidate);
        if (!mention) continue;
        mentions.push({
          ...mention,
          start: serialized.start,
          end: serialized.end,
        });
      }
      if (mentions.length === 0) return false;
      event.preventDefault();
      const result = applyPastedText(currentDraft, pastedText, range, mentions);
      updateDraftForScope((draft) => ({ ...draft, text: result.text, mentions: result.mentions }));
      requestAnimationFrame(() => composerRef.current?.focusAt(result.caret));
      return true;
    },
    [buildDraftMention, currentDraft, referenceCandidates, updateDraftForScope],
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
          {selected ? (
            <MdCheck className="mt-0.5 shrink-0 text-primary" />
          ) : null}
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
          setMentionQuery("");
          setMentionRange(null);
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
              disabled={
                sessions.length === 0 ||
                loading ||
                clearingHistory ||
                historyLoadingSessionId !== null
              }
              onClick={() => setClearHistoryOpen(true)}
            >
              {t("ai.clearHistory")}
            </Button>
          </div>
          {historyLoadError ? (
            <div className="border-b border-border/70 px-3 py-2 text-[0.6875rem] text-destructive">
              {t("ai.historyLoadFailed")}: {historyLoadError}
            </div>
          ) : null}
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
                      const isLoadingHistory = historyLoadingSessionId === session.id;
                      const exactScope = sameAIScope(session.scope, ownerScope);
                      return (
                        <div
                          key={session.id}
                          className="group flex items-center gap-1 rounded-md px-2 py-1.5 hover:bg-muted/60"
                        >
                          <button
                            type="button"
                            className="min-w-0 flex-1 text-left text-xs"
                            disabled={isLoadingHistory || (inUse && !exactScope)}
                            aria-busy={isLoadingHistory}
                            onClick={() => void openHistorySession(session)}
                          >
                            <div className="truncate font-medium">{session.title}</div>
                            {isLoadingHistory ? (
                              <div className="truncate text-[0.625rem] text-muted-foreground">
                                {t("ai.historyLoading")}
                              </div>
                            ) : inUse && !exactScope ? (
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
                            disabled={isLoadingHistory || clearingHistory}
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
                ) : !isExternalAgentMode && !selectedModel ? (
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
                          <div className="whitespace-pre-wrap break-words">
                            <AIUserMessageContent message={message} />
                          </div>
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
        {runMode === "nyaterm_agent" && currentDraft.mentions.length > 0 ? (
          <div className="mb-1.5 space-y-0.5 text-[0.625rem] text-muted-foreground">
            <ReferenceTooltip text={ayaExecutionLabel}>
              <div className="truncate">
                {t("ai.ayaExecutionTargets")}: {ayaExecutionLabel}
              </div>
            </ReferenceTooltip>
            {ayaFileSourceTitles.length > 0 ? (
              <ReferenceTooltip text={ayaFileSourceTitles.join("\n")}>
                <div className="truncate">
                  {t("ai.ayaFileSources")}: {ayaFileSourceTitles.join(", ")}
                </div>
              </ReferenceTooltip>
            ) : null}
          </div>
        ) : null}
        <div className="relative">
          {showMentionPopover ? (
            <div
              ref={mentionPopoverRef}
              className="absolute bottom-full left-0 right-0 z-30 mb-1 flex max-h-64 flex-col overflow-hidden rounded-md border bg-popover text-popover-foreground shadow-lg"
              style={{ borderColor: "var(--df-border)" }}
            >
              <div className="min-h-0 overflow-auto p-1 terminal-scroll">
                {visibleMentionItems.length === 0 ? (
                  <div className="px-2 py-3 text-center text-xs text-muted-foreground">
                    {t("ai.noReferenceMatches")}
                  </div>
                ) : (
                  visibleMentionItems.map((item, idx) => {
                    const focused = idx === mentionIndex;
                    if (item.kind === "host") {
                      const scopeTitle = `${item.group.title}\n${t("ai.referenceHostScope", { count: item.group.sessions.length })}`;
                      const fileCount =
                        referenceGroups
                          .find((group) => group.id === item.group.id)
                          ?.children.filter((option) => option.kind === "file").length ?? 0;
                      const expanded =
                        !collapsedMentionGroups.has(item.group.id) &&
                        (!!mentionQuery.trim() || expandedMentionGroups.has(item.group.id));
                      const disabled = item.group.sessions.length === 0;
                      return (
                        <div
                          key={item.id}
                          ref={(element) => {
                            if (focused) element?.scrollIntoView({ block: "nearest" });
                          }}
                          className={`flex items-center rounded ${focused ? "bg-accent" : ""}`}
                        >
                          <button
                            type="button"
                            aria-label={
                              expanded
                                ? t("ai.collapseReferenceGroup")
                                : t("ai.expandReferenceGroup")
                            }
                            aria-expanded={expanded}
                            disabled={visibleReferenceChildren(item.group).length === 0}
                            className="flex size-7 shrink-0 items-center justify-center rounded hover:bg-muted/60 disabled:invisible"
                            onMouseDown={(event) => event.preventDefault()}
                            onClick={() => toggleMentionGroup(item.group.id, expanded)}
                          >
                            <MdExpandMore
                              className={`transition-transform ${expanded ? "rotate-180" : ""}`}
                            />
                          </button>
                          <ReferenceTooltip
                            text={scopeTitle}
                            disabled={disabled}
                            triggerClassName="flex min-w-0 flex-1"
                          >
                            <button
                              type="button"
                              disabled={disabled}
                              aria-label={scopeTitle}
                              data-reference-host-group={item.group.id}
                              className="flex min-w-0 flex-1 items-center gap-2 rounded px-1.5 py-1.5 text-left text-xs disabled:opacity-50"
                              onMouseDown={(event) => event.preventDefault()}
                              onClick={() =>
                                insertMention({
                                  kind: "host",
                                  id: `host:${item.group.id}`,
                                  label:
                                    item.group.id === "local"
                                      ? t("ai.localReferenceRoot")
                                      : item.group.label,
                                  title: item.group.title,
                                  sessionIds: item.group.sessions,
                                })
                              }
                              onPointerEnter={() => setMentionIndex(idx)}
                            >
                              <span className="size-2 shrink-0 rounded-full bg-primary/70" />
                              <span className="min-w-0 flex-1 truncate font-semibold">
                                {item.group.id === "local"
                                  ? t("ai.localReferenceRoot")
                                  : item.group.label}
                                {runMode === "nyaterm_agent" && item.group.host ? (
                                  <span className="block truncate text-[0.625rem] font-normal text-muted-foreground">
                                    {item.group.title}
                                  </span>
                                ) : null}
                              </span>
                              <span className="shrink-0 text-[0.625rem] text-muted-foreground">
                                {t("ai.referenceGroupCounts", {
                                  sessions: item.group.sessions.length,
                                  files: fileCount,
                                })}
                              </span>
                            </button>
                          </ReferenceTooltip>
                        </div>
                      );
                    }

                    const option = item.option;
                    const startedAt =
                      option.kind === "session" && option.sortTime > 0
                        ? new Date(option.sortTime).toLocaleString(undefined, {
                            month: "2-digit",
                            day: "2-digit",
                            hour: "2-digit",
                            minute: "2-digit",
                            second: "2-digit",
                          })
                        : "-";
                    const optionTitle =
                      option.kind === "session"
                        ? `${option.title}\n${t("ai.referenceSessionStartedAt", { time: startedAt })}\n${option.pane.sessionId}`
                        : option.title;
                    const selectedFileIds = new Set<string>();
                    const selectedFileBytes = currentDraft.mentions.reduce((total, mention) => {
                      const reference = mention.fileReference;
                      if (
                        !reference ||
                        reference.id === option.id ||
                        selectedFileIds.has(reference.id)
                      ) {
                        return total;
                      }
                      selectedFileIds.add(reference.id);
                      return total + (reference.sizeBytes ?? 0);
                    }, 0);
                    const optionAvailable = referenceOptionAvailable(item.group, option);
                    const overLimit =
                      option.kind === "file" &&
                      (option.sizeBytes > aiSettings.max_ai_file_size_bytes ||
                        selectedFileBytes + option.sizeBytes > aiSettings.max_ai_file_size_bytes);
                    const disabled = !optionAvailable || overLimit;
                    const detail =
                      option.kind === "file" ? t("ai.fileReference") : option.pane.type;
                    const optionIndent =
                      item.depth === 2
                        ? "ml-8 border-l border-border/50 pl-1"
                        : "ml-2 border-l border-border/50 pl-1";
                    return (
                      <div key={item.id} data-reference-depth={item.depth} className={optionIndent}>
                        <ReferenceTooltip
                          text={
                            !optionAvailable
                              ? t("ai.referenceUnavailable")
                              : overLimit
                                ? t("ai.fileReferenceTooLarge", {
                                    limit: aiSettings.max_ai_file_size_bytes.toLocaleString(),
                                  })
                                : optionTitle
                          }
                          disabled={disabled}
                          triggerClassName="block w-full"
                        >
                          <button
                            ref={(element) => {
                              if (focused) element?.scrollIntoView({ block: "nearest" });
                            }}
                            type="button"
                            disabled={disabled}
                            data-reference-option={option.id}
                            data-reference-depth={item.depth}
                            className={`flex w-full items-center gap-2 rounded px-2 py-1.5 pl-8 text-left text-xs disabled:cursor-not-allowed disabled:opacity-50 ${focused ? "bg-accent" : ""}`}
                            onMouseDown={(event) => event.preventDefault()}
                            onClick={() => insertMention(option)}
                            onPointerEnter={() => setMentionIndex(idx)}
                          >
                            <span className="size-2 shrink-0 rounded-full bg-muted-foreground/40" />
                            <span className="min-w-0 flex-1 font-medium">
                              <span className="block truncate">{option.label}</span>
                              {option.kind === "session" ? (
                                <span className="block truncate text-[0.625rem] font-normal text-muted-foreground">
                                  {t("ai.referenceSessionStartedAt", { time: startedAt })} ·{" "}
                                  {option.pane.sessionId.slice(0, 8)}
                                </span>
                              ) : null}
                            </span>
                            <span className="max-w-[45%] shrink-0 truncate text-[0.625rem] text-muted-foreground">
                              {detail}
                            </span>
                          </button>
                        </ReferenceTooltip>
                      </div>
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
                  onClick={() =>
                    updateDraftForScope((draft) => ({
                      ...draft,
                      quotedText: null,
                    }))
                  }
                >
                  <MdClose className="text-xs" />
                </button>
              </div>
            ) : null}
            <AIReferenceComposer
              ref={composerRef}
              value={input}
              mentions={currentDraft.mentions}
              disabled={loading || !aiSettings.enabled}
              placeholder={aiSettings.enabled ? t("ai.placeholder") : t("ai.goToSettingsToEnable")}
              className="max-h-32 min-h-16 overflow-y-auto whitespace-pre-wrap break-words rounded-md border border-input bg-background px-2 py-2 text-xs leading-5 text-foreground outline-none terminal-scroll focus-visible:ring-1 focus-visible:ring-[var(--df-primary)]"
              onChange={handleComposerChange}
              onMentionQuery={handleMentionQuery}
              onCompositionStart={() => {
                isComposingRef.current = true;
              }}
              onCompositionEnd={() => {
                isComposingRef.current = false;
              }}
              onPasteReferences={handlePasteReferences}
              onKeyDown={(event) => {
                const composing =
                  isComposingRef.current || event.nativeEvent.isComposing || event.keyCode === 229;
                if (showMentionPopover) {
                  if (event.key === "Escape") {
                    event.preventDefault();
                    closeMentionPopover();
                    return;
                  }
                  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                    event.preventDefault();
                    setMentionIndex((index) => {
                      if (visibleMentionItems.length === 0) return 0;
                      const delta = event.key === "ArrowDown" ? 1 : -1;
                      return (
                        (index + delta + visibleMentionItems.length) % visibleMentionItems.length
                      );
                    });
                    return;
                  }
                  const item = visibleMentionItems[mentionIndex];
                  if (
                    event.key === "ArrowRight" &&
                    item?.kind === "host" &&
                    visibleReferenceChildren(item.group).length > 0
                  ) {
                    event.preventDefault();
                    setCollapsedMentionGroups((previous) => {
                      const next = new Set(previous);
                      next.delete(item.group.id);
                      return next;
                    });
                    setExpandedMentionGroups((previous) => new Set(previous).add(item.group.id));
                    return;
                  }
                  if (event.key === "ArrowLeft" && item && item.kind !== "host") {
                    event.preventDefault();
                    setExpandedMentionGroups((previous) => {
                      const next = new Set(previous);
                      next.delete(item.group.id);
                      return next;
                    });
                    setCollapsedMentionGroups((previous) => new Set(previous).add(item.group.id));
                    return;
                  }
                  if (event.key === "Enter" && !composing) {
                    event.preventDefault();
                    if (!item) {
                      closeMentionPopover();
                    } else if (item.kind === "host") {
                      if (item.group.sessions.length > 0) {
                        insertMention({
                          kind: "host",
                          id: `host:${item.group.id}`,
                          label:
                            item.group.id === "local"
                              ? t("ai.localReferenceRoot")
                              : item.group.label,
                          title: item.group.title,
                          sessionIds: item.group.sessions,
                        });
                      }
                    } else {
                      const limit = aiSettings.max_ai_file_size_bytes;
                      const currentFileIds = new Set<string>();
                      const currentBytes = currentDraft.mentions.reduce((total, mention) => {
                        const reference = mention.fileReference;
                        if (
                          !reference ||
                          reference.id === item.option.id ||
                          currentFileIds.has(reference.id)
                        ) {
                          return total;
                        }
                        currentFileIds.add(reference.id);
                        return total + (reference.sizeBytes ?? 0);
                      }, 0);
                      if (
                        referenceOptionAvailable(item.group, item.option) &&
                        (item.option.kind !== "file" ||
                          (item.option.sizeBytes <= limit &&
                            currentBytes + item.option.sizeBytes <= limit))
                      ) {
                        insertMention(item.option);
                      }
                    }
                    return;
                  }
                }
                if (event.key === "Enter" && !event.shiftKey && !composing) {
                  event.preventDefault();
                  submit();
                }
              }}
            />
            <div className="flex w-full items-center justify-between gap-2">
              <div className="flex flex-1 min-w-0 items-center gap-2">
                <div className="min-w-0 max-w-[45%] shrink-0">
                  <Select
                    value={runMode}
                    onValueChange={(value) => selectRunMode(value as AIRunMode)}
                  >
                    <SelectTrigger size="sm" className="w-fit max-w-full text-xs">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent position="popper">
                      <SelectItem value="ask">{t("ai.modeAsk")}</SelectItem>
                      <SelectItem value="nyaterm_agent" disabled={!supports("aiAgents")}>
                        {t("ai.modeNyatermAgent")}
                      </SelectItem>
                      <SelectItem value="codex_agent" disabled={!codexAgentEnabled}>
                        {t("ai.modeCodexAgent")}
                      </SelectItem>
                      <SelectItem value="claude_code_agent" disabled={!claudeCodeAgentEnabled}>
                        {t("ai.modeClaudeCodeAgent")}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>

                <div className="min-w-0 flex-1">
                  {externalModelLabel ? (
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      className="h-8 w-fit max-w-full min-w-0 justify-start px-2 text-xs"
                      disabled
                    >
                      <span className="truncate">{externalModelLabel}</span>
                    </Button>
                  ) : (
                    <ModelCombobox
                      models={selectableModels}
                      credentials={aiSettings.provider_credentials}
                      selectedModel={selectedModel}
                      selectedReasoningEffort={selectedReasoningEffort}
                      open={modelPopoverOpen}
                      onOpenChange={setModelPopoverOpen}
                      onSelect={(model) => {
                        const default_reasoning_effort = getModelReasoningOptions(model).includes(
                          configuredReasoningEffort,
                        )
                          ? configuredReasoningEffort
                          : "auto";
                        updateAppSettings({
                          ai: {
                            ...aiSettings,
                            default_model_id: model.id,
                            default_reasoning_effort,
                          },
                        });
                      }}
                      onSelectReasoningEffort={(default_reasoning_effort) =>
                        updateAppSettings({
                          ai: { ...aiSettings, default_reasoning_effort },
                        })
                      }
                    />
                  )}
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
                      (!selectedModel && !isExternalAgentMode) ||
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
