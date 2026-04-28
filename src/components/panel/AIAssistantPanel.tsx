import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdAutoAwesome,
  MdContentCopy,
  MdHistory,
  MdInput,
  MdOutlineSettings,
  MdSave,
  MdSend,
  MdStop,
} from "react-icons/md";
import { toast } from "sonner";
import PanelHeader from "@/components/layout/PanelHeader";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { useApp } from "@/context/AppContext";
import type { AIErrorDetectedDetail, AIOpenIntent } from "@/lib/aiEvents";
import { AI_ERROR_DETECTED_EVENT } from "@/lib/aiEvents";
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import { buildAIContext, getTerminalContextProvider } from "@/lib/terminalContext";
import { openSettings } from "@/lib/windowManager";
import type {
  AIAction,
  AICommandCard,
  AIMessage,
  AISession,
  AIStreamEventPayload,
  AIStreamStart,
  QuickCommand,
  QuickCommandCategory,
  QuickCommandsConfig,
  RiskLevel,
  SavedConnection,
  SessionPane,
} from "@/types/global";

interface AIAssistantPanelProps {
  activePane: SessionPane | null;
  activeConnection?: SavedConnection | null;
  intent: AIOpenIntent | null;
}

const riskClassName: Record<RiskLevel, string> = {
  low: "border-emerald-500/30 bg-emerald-500/10 text-emerald-600",
  medium: "border-amber-500/30 bg-amber-500/10 text-amber-600",
  high: "border-orange-500/30 bg-orange-500/10 text-orange-600",
  critical: "border-red-500/30 bg-red-500/10 text-red-600",
};

function actionTitle(action: AIAction) {
  switch (action) {
    case "generate_command":
      return "生成命令";
    case "explain_output":
      return "解释最近输出";
    case "explain_selected":
      return "解释选中内容";
    case "analyze_error":
      return "分析错误";
    case "repair_from_selection":
      return "生成修复命令";
  }
}

function createLocalMessage(role: "user" | "assistant", content: string, sessionId = "local") {
  return {
    id: `local-${Date.now()}-${Math.random().toString(36).slice(2)}`,
    sessionId,
    role,
    content,
    createdAt: new Date().toISOString(),
    commandCards: [],
  } satisfies AIMessage;
}

function slugCategory(name: string) {
  return `ai-${
    name
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9\u4e00-\u9fa5]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 48) || "commands"
  }`;
}

function mapRiskColor(riskLevel: RiskLevel) {
  switch (riskLevel) {
    case "critical":
      return "red";
    case "high":
      return "yellow";
    case "medium":
      return "blue";
    case "low":
      return "green";
  }
}

function AICommandCardView({
  card,
  onInsert,
  onSave,
}: {
  card: AICommandCard;
  onInsert: (card: AICommandCard) => void;
  onSave: (card: AICommandCard) => void;
}) {
  const { t } = useTranslation();

  const copy = async () => {
    await navigator.clipboard.writeText(card.command);
    toast.success(t("ai.commandCopied"));
  };

  return (
    <div className="rounded-md border border-border/70 bg-background/65 p-3 text-xs">
      <div className="flex min-w-0 items-start justify-between gap-2">
        <div className="min-w-0">
          <div className="truncate text-sm font-medium">{card.title}</div>
          <div
            className={`mt-1 inline-flex rounded-full border px-2 py-0.5 text-[0.6875rem] font-medium ${riskClassName[card.riskLevel]}`}
          >
            {card.riskLevel}
          </div>
        </div>
      </div>
      <pre className="mt-3 max-h-32 overflow-auto rounded-md border border-border/60 bg-muted/30 p-2 font-mono text-[0.6875rem] leading-5 terminal-scroll whitespace-pre-wrap break-all">
        {card.command}
      </pre>
      <div className="mt-3 space-y-1 leading-5 text-muted-foreground">
        <p>{card.explanation}</p>
        <p>{card.riskReason}</p>
        <p>{card.expectedEffect}</p>
        {card.rollback ? <p>{card.rollback}</p> : null}
      </div>
      <div className="mt-3 flex flex-wrap gap-1.5">
        <Button size="xs" onClick={() => onInsert(card)}>
          <MdInput />
          {t("ai.insertTerminal")}
        </Button>
        <Button size="xs" variant="outline" onClick={() => void copy()}>
          <MdContentCopy />
          {t("ai.copy")}
        </Button>
        <Button size="xs" variant="outline" onClick={() => onSave(card)}>
          <MdSave />
          {t("ai.saveQuickCommand")}
        </Button>
      </div>
    </div>
  );
}

function AIAssistantPanel({ activePane, activeConnection, intent }: AIAssistantPanelProps) {
  const { t } = useTranslation();
  const { appSettings } = useApp();
  const aiSettings = appSettings.ai;
  const [input, setInput] = useState("");
  const [messages, setMessages] = useState<AIMessage[]>([]);
  const [sessions, setSessions] = useState<AISession[]>([]);
  const [showHistory, setShowHistory] = useState(false);
  const [loading, setLoading] = useState(false);
  const [currentSessionId, setCurrentSessionId] = useState<string | null>(null);
  const [streamId, setStreamId] = useState<string | null>(null);
  const [detectedError, setDetectedError] = useState<AIErrorDetectedDetail | null>(null);
  const handledIntentIdRef = useRef<string | null>(null);
  const streamUnlistenRef = useRef<UnlistenFn | null>(null);

  const activeProfile = useMemo(
    () =>
      aiSettings.provider_profiles.find((profile) => profile.id === aiSettings.active_profile_id) ??
      aiSettings.provider_profiles.find((profile) => profile.enabled),
    [aiSettings.active_profile_id, aiSettings.provider_profiles],
  );

  const activeSessionId = activePane?.sessionId ?? null;

  useEffect(() => {
    return () => {
      streamUnlistenRef.current?.();
    };
  }, []);

  useEffect(() => {
    const handler = (event: Event) => {
      const detail = (event as CustomEvent<AIErrorDetectedDetail>).detail;
      if (!detail || detail.sessionId !== activeSessionId) return;
      setDetectedError(detail);
    };
    window.addEventListener(AI_ERROR_DETECTED_EVENT, handler);
    return () => window.removeEventListener(AI_ERROR_DETECTED_EVENT, handler);
  }, [activeSessionId]);

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

  const loadSessionMessages = useCallback(async (sessionId: string) => {
    try {
      const items = await invoke<AIMessage[]>("get_ai_messages", { sessionId });
      setCurrentSessionId(sessionId);
      setMessages(items);
      setShowHistory(false);
    } catch (error) {
      toast.error(getErrorMessage(error));
    }
  }, []);

  const appendAudit = useCallback(
    (params: {
      action: string;
      userInput?: string;
      generatedCommand?: string;
      riskLevel?: RiskLevel;
      insertedToTerminal?: boolean;
      blocked?: boolean;
    }) => {
      void invoke("append_ai_audit", {
        request: {
          connectionId: activeConnection?.id ?? null,
          action: params.action,
          userInput: params.userInput,
          generatedCommand: params.generatedCommand,
          riskLevel: params.riskLevel,
          insertedToTerminal: params.insertedToTerminal ?? false,
          executed: false,
          blocked: params.blocked ?? false,
        },
      }).catch(() => {});
    },
    [activeConnection?.id],
  );

  const startChat = useCallback(
    async (action: AIAction, userInput: string, selectedText?: string) => {
      if (!activePane || activePane.connecting || activePane.connectError) {
        toast.error(t("panel.noActiveSessions"));
        return;
      }
      if (!aiSettings.enabled) {
        toast.error(t("ai.disabled"));
        return;
      }

      setDetectedError(null);
      setLoading(true);
      streamUnlistenRef.current?.();
      streamUnlistenRef.current = null;

      const userMessage = createLocalMessage("user", userInput, currentSessionId ?? "local");
      const assistantId = `assistant-${Date.now()}-${Math.random().toString(36).slice(2)}`;
      setMessages((prev) => [
        ...prev,
        userMessage,
        {
          id: assistantId,
          sessionId: currentSessionId ?? "local",
          role: "assistant",
          content: "",
          createdAt: new Date().toISOString(),
          commandCards: [],
        },
      ]);

      try {
        const context = await buildAIContext({
          pane: activePane,
          connection: activeConnection,
          lineLimit: aiSettings.context_line_limit,
          selectedText,
        });

        const result = await invoke<AIStreamStart>("start_ai_chat_stream", {
          request: {
            sessionId: currentSessionId,
            connectionId: activeConnection?.id ?? null,
            action,
            userInput,
            context,
            options: {
              maxOutputCommands: 5,
              language: "zh-CN",
              safetyMode: "strict",
            },
          },
        });
        setCurrentSessionId(result.sessionId);
        setStreamId(result.streamId);

        const unlisten = await listen<AIStreamEventPayload>(
          `ai-stream-${result.streamId}`,
          (event) => {
            const payload = event.payload;
            if (payload.type === "delta" && payload.textDelta) {
              setMessages((prev) =>
                prev.map((message) =>
                  message.id === assistantId
                    ? { ...message, content: `${message.content}${payload.textDelta}` }
                    : message,
                ),
              );
              return;
            }

            if (payload.type === "done") {
              setLoading(false);
              setStreamId(null);
              setMessages((prev) =>
                prev.map((message) =>
                  message.id === assistantId && payload.message ? payload.message : message,
                ),
              );
              void loadSessions();
              return;
            }

            if (payload.type === "error") {
              setLoading(false);
              setStreamId(null);
              setMessages((prev) =>
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
        streamUnlistenRef.current = unlisten;
        appendAudit({ action: `ai.${action}`, userInput });
      } catch (error) {
        setLoading(false);
        setStreamId(null);
        setMessages((prev) =>
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
      aiSettings.context_line_limit,
      aiSettings.enabled,
      appendAudit,
      currentSessionId,
      loadSessions,
      t,
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
    setInput("");
    void startChat("generate_command", value);
  }, [input, loading, startChat]);

  const cancelStream = useCallback(() => {
    if (!streamId) return;
    void invoke("cancel_ai_chat_stream", { streamId }).catch(() => {});
    setLoading(false);
    setStreamId(null);
  }, [streamId]);

  const insertCommand = useCallback(
    (card: AICommandCard) => {
      const provider = getTerminalContextProvider(activeSessionId);
      if (!provider) {
        toast.error(t("ai.noTerminal"));
        return;
      }
      void provider
        .insertCommand(card.command)
        .then(() => {
          provider.focus();
          appendAudit({
            action: "ai.insert_command",
            generatedCommand: card.command,
            riskLevel: card.riskLevel,
            insertedToTerminal: true,
          });
        })
        .catch((error) => toast.error(getErrorMessage(error)));
    },
    [activeSessionId, appendAudit, t],
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
        const existingCategory = config.categories.find((item) => item.name === categoryName);
        const newCategory: QuickCommandCategory | undefined = existingCategory
          ? undefined
          : { id: slugCategory(categoryName), name: categoryName };
        const categoryId = existingCategory?.id ?? newCategory?.id;
        const command: QuickCommand = {
          id: `ai-${crypto.randomUUID()}`,
          label: card.title,
          command: card.command,
          category_id: categoryId,
          description: `${card.explanation}\n${card.riskReason}`,
          color_tag: mapRiskColor(card.riskLevel),
          icon_tag: "terminal",
          pinned: false,
          execution_mode: "append",
          source: "ai",
          risk_level: card.riskLevel,
        };
        await invoke("upsert_quick_command", { command, newCategory });
        await emit("quick-command-saved", { command, newCategory });
        appendAudit({
          action: "ai.save_quick_command",
          generatedCommand: card.command,
          riskLevel: card.riskLevel,
        });
        toast.success(t("ai.savedQuickCommand"));
      } catch (error) {
        toast.error(getErrorMessage(error));
      }
    },
    [aiSettings.allow_save_command, appendAudit, t],
  );

  const clearHistory = useCallback(async () => {
    await invoke("clear_ai_history");
    setMessages([]);
    setCurrentSessionId(null);
    await loadSessions();
  }, [loadSessions]);

  return (
    <div className="flex h-full flex-col" style={{ backgroundColor: "var(--df-bg-panel)" }}>
      <PanelHeader
        title={t("ai.title")}
        meta={activeProfile?.name ?? t("ai.notConfigured")}
        actions={
          <>
            <Button
              size="icon-xs"
              variant="ghost"
              className="h-6 w-6"
              onClick={() => setShowHistory((value) => !value)}
              title={t("ai.history")}
            >
              <MdHistory />
            </Button>
            <Button
              size="icon-xs"
              variant="ghost"
              className="h-6 w-6"
              onClick={() => openSettings("ai")}
              title={t("ai.settings")}
            >
              <MdOutlineSettings />
            </Button>
          </>
        }
      />

      {detectedError ? (
        <div className="border-b border-border/70 bg-amber-500/10 p-3 text-xs">
          <div className="font-medium text-amber-600">
            {t("ai.errorDetected")}
          </div>
          <div className="mt-2 flex gap-1.5">
            <Button
              size="xs"
              onClick={() =>
                void startChat("analyze_error", t("ai.analyzeDetectedError"))
              }
            >
              {t("ai.analyze")}
            </Button>
            <Button size="xs" variant="ghost" onClick={() => setDetectedError(null)}>
              {t("common.close")}
            </Button>
          </div>
        </div>
      ) : null}

      {showHistory ? (
        <div className="max-h-48 shrink-0 overflow-auto border-b border-border/70 p-2 terminal-scroll">
          <div className="mb-2 flex items-center justify-between gap-2">
            <span className="text-xs font-medium">{t("ai.history")}</span>
            <Button size="xs" variant="ghost" onClick={() => void clearHistory()}>
              {t("common.delete")}
            </Button>
          </div>
          {sessions.length === 0 ? (
            <div className="py-4 text-center text-xs text-muted-foreground">
              {t("ai.noHistory")}
            </div>
          ) : (
            sessions.map((session) => (
              <button
                key={session.id}
                className="mb-1 block w-full rounded-md px-2 py-1.5 text-left text-xs hover:bg-muted/60"
                onClick={() => void loadSessionMessages(session.id)}
              >
                <div className="truncate font-medium">{session.title}</div>
                <div className="truncate text-[0.6875rem] text-muted-foreground">
                  {session.updatedAt}
                </div>
              </button>
            ))
          )}
        </div>
      ) : null}

      <div className="flex-1 overflow-auto p-3 terminal-scroll">
        {messages.length === 0 ? (
          <div className="flex h-full min-h-[12rem] flex-col items-center justify-center gap-3 text-center text-sm text-muted-foreground">
            <MdAutoAwesome className="text-3xl" />
            <div>{t("ai.empty")}</div>
          </div>
        ) : (
          <div className="space-y-3">
            {messages.map((message) => (
              <div
                key={message.id}
                className={`rounded-md border p-3 text-xs leading-5 ${
                  message.role === "user"
                    ? "border-primary/25 bg-primary/10"
                    : "border-border/70 bg-muted/20"
                }`}
              >
                <div className="mb-2 text-[0.6875rem] font-semibold uppercase tracking-[0.12em] text-muted-foreground">
                  {message.role === "user" ? "User" : "AI"}
                </div>
                <div className="whitespace-pre-wrap break-words">{message.content}</div>
                {message.commandCards?.length ? (
                  <div className="mt-3 space-y-2">
                    {message.commandCards.map((card) => (
                      <AICommandCardView
                        key={card.id}
                        card={card}
                        onInsert={insertCommand}
                        onSave={(item) => void saveQuickCommand(item)}
                      />
                    ))}
                  </div>
                ) : null}
              </div>
            ))}
          </div>
        )}
      </div>

      <div className="shrink-0 border-t border-border/70 p-2">
        <div className="flex gap-2">
          <Textarea
            value={input}
            disabled={loading}
            placeholder={t("ai.placeholder")}
            className="min-h-16 resize-none text-xs"
            onChange={(event) => setInput(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
                event.preventDefault();
                submit();
              }
            }}
          />
          {loading ? (
            <Button size="icon-sm" variant="outline" onClick={cancelStream}>
              <MdStop />
            </Button>
          ) : (
            <Button size="icon-sm" onClick={submit} disabled={!input.trim()}>
              <MdSend />
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}

export default memo(AIAssistantPanel);
