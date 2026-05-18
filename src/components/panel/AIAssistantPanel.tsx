import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  type CSSProperties,
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
  MdBlock,
  MdCheck,
  MdClose,
  MdContentCopy,
  MdDeleteOutline,
  MdErrorOutline,
  MdExpandLess,
  MdExpandMore,
  MdHistory,
  MdInput,
  MdOutlineSettings,
  MdPlayArrow,
  MdSave,
  MdSearch,
  MdSend,
  MdStop,
} from "react-icons/md";
import ReactMarkdown from "react-markdown";
import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import remarkGfm from "remark-gfm";
import { toast } from "sonner";
import PanelHeader from "@/components/layout/PanelHeader";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
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
import type { AIErrorDetectedDetail, AIOpenIntent } from "@/lib/aiEvents";
import { AI_ERROR_DETECTED_EVENT } from "@/lib/aiEvents";
import {
  getEnabledAIModels,
  getModelProviderLabel,
  isRiskAllowed,
  selectDefaultAIModel,
} from "@/lib/aiSettings";
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import { buildAIContext, getTerminalContextProvider } from "@/lib/terminalContext";
import { openSettings } from "@/lib/windowManager";
import { collectSessionPanes } from "@/lib/workspaceTabs";
import type {
  AgentStepPayload,
  AIAction,
  AICommandCard,
  AIContext,
  AIMessage,
  AIMode,
  AIModelConfigItem,
  AIProviderCredential,
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

type AICommandExecutionStatus =
  | "idle"
  | "auto_executing"
  | "executed"
  | "pending_approval"
  | "rejected"
  | "failed";

interface AICommandExecutionState {
  status: AICommandExecutionStatus;
  error?: string;
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
    case "custom_terminal_action":
      return "终端 AI 功能";
    case "custom_file_action":
      return "文件 AI 功能";
  }
}

function createLocalMessage(role: "user" | "assistant", content: string, sessionId = "local") {
  return {
    id: `local-${Date.now()}-${Math.random().toString(36).slice(2)}`,
    sessionId,
    role,
    content,
    createdAt: new Date().toISOString(),
    reasoningContent: null,
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

type MarkdownNodeProps = {
  children?: ReactNode;
  href?: string;
};

function looksLikeStructuredJsonOutput(content: string) {
  const trimmed = content.trimStart();
  if (trimmed.includes('"text"') && trimmed.includes('"commandCards"')) return true;
  const jsonBody = trimmed.startsWith("```json")
    ? trimmed.slice(trimmed.indexOf("\n") + 1).trimStart()
    : trimmed;
  return jsonBody.startsWith("{") && !jsonBody.startsWith("{{");
}

function extractThinkContent(content: string): { visible: string; reasoning: string | null } {
  const closedRegex = /<think>([\s\S]*?)<\/think>/gi;
  const reasoningParts: string[] = [];

  for (const match of content.matchAll(closedRegex)) {
    const part = match[1]?.trim();
    if (part) reasoningParts.push(part);
  }

  let visible = content.replace(closedRegex, "");

  const openIdx = visible.lastIndexOf("<think>");
  if (openIdx >= 0) {
    const trailing = visible.slice(openIdx + 7).trim();
    if (trailing) reasoningParts.push(trailing);
    visible = visible.slice(0, openIdx);
  }

  visible = visible.replace(/<(?:t(?:h(?:i(?:n(?:k)?)?)?)?)?$/, "");

  return {
    visible: visible.trim(),
    reasoning: reasoningParts.length > 0 ? reasoningParts.join("\n\n") : null,
  };
}

interface QuotedText {
  text: string;
}

function AnimatedStatusText({ label }: { label: string }) {
  return <span className="df-thinking-text font-medium">{label}</span>;
}

type DateGroup = "today" | "yesterday" | "last7Days" | "earlier";

function getDateGroup(dateStr: string): DateGroup {
  const date = new Date(dateStr);
  const now = new Date();
  const todayStart = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const yesterdayStart = new Date(todayStart.getTime() - 86400000);
  const last7Start = new Date(todayStart.getTime() - 6 * 86400000);

  if (date >= todayStart) return "today";
  if (date >= yesterdayStart) return "yesterday";
  if (date >= last7Start) return "last7Days";
  return "earlier";
}

const dateGroupOrder: DateGroup[] = ["today", "yesterday", "last7Days", "earlier"];

function groupSessionsByDate(sessions: AISession[]) {
  const groups: Record<DateGroup, AISession[]> = {
    today: [],
    yesterday: [],
    last7Days: [],
    earlier: [],
  };
  for (const session of sessions) {
    groups[getDateGroup(session.updatedAt)].push(session);
  }
  return groups;
}

function buildPrismThemeFromColors(
  colors: import("@/lib/themes").ThemeColors,
): Record<string, CSSProperties> {
  const t = colors.terminal;
  return {
    'code[class*="language-"]': {
      color: t.foreground,
      background: "none",
      fontFamily: "inherit",
      textAlign: "left",
      whiteSpace: "pre",
      wordSpacing: "normal",
      wordBreak: "normal",
      wordWrap: "normal",
      lineHeight: "1.5",
      tabSize: 4,
    },
    'pre[class*="language-"]': {
      color: t.foreground,
      background: "transparent",
      fontFamily: "inherit",
      textAlign: "left",
      whiteSpace: "pre",
      wordSpacing: "normal",
      wordBreak: "normal",
      wordWrap: "normal",
      lineHeight: "1.5",
      tabSize: 4,
      overflow: "auto",
    },
    comment: { color: t.brightBlack, fontStyle: "italic" },
    prolog: { color: t.brightBlack },
    doctype: { color: t.brightBlack },
    cdata: { color: t.brightBlack },
    punctuation: { color: t.foreground },
    property: { color: t.cyan },
    tag: { color: t.red },
    boolean: { color: t.magenta },
    number: { color: t.magenta },
    constant: { color: t.magenta },
    symbol: { color: t.green },
    deleted: { color: t.red },
    selector: { color: t.green },
    "attr-name": { color: t.yellow },
    string: { color: t.green },
    char: { color: t.green },
    builtin: { color: t.cyan },
    inserted: { color: t.green },
    operator: { color: t.foreground },
    entity: { color: t.yellow, cursor: "help" },
    url: { color: t.cyan },
    variable: { color: t.red },
    atrule: { color: t.yellow },
    "attr-value": { color: t.green },
    function: { color: t.blue },
    "class-name": { color: t.yellow },
    keyword: { color: t.magenta },
    regex: { color: t.cyan },
    important: { color: t.yellow, fontWeight: "bold" },
    bold: { fontWeight: "bold" },
    italic: { fontStyle: "italic" },
    namespace: { opacity: 0.7 },
  };
}

function MarkdownContent({ content }: { content: string }) {
  return (
    <div className="break-words text-xs leading-5">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          p: ({ children }: MarkdownNodeProps) => (
            <p className="my-2 first:mt-0 last:mb-0">{children}</p>
          ),
          ul: ({ children }: MarkdownNodeProps) => (
            <ul className="my-2 list-disc pl-5">{children}</ul>
          ),
          ol: ({ children }: MarkdownNodeProps) => (
            <ol className="my-2 list-decimal pl-5">{children}</ol>
          ),
          li: ({ children }: MarkdownNodeProps) => <li className="my-0.5">{children}</li>,
          a: ({ children, href }: MarkdownNodeProps) => (
            <a
              className="text-primary underline underline-offset-2"
              href={href}
              target="_blank"
              rel="noreferrer"
            >
              {children}
            </a>
          ),
          blockquote: ({ children }: MarkdownNodeProps) => (
            <blockquote className="my-2 border-l-2 border-border pl-3 text-muted-foreground">
              {children}
            </blockquote>
          ),
          pre: ({ children }: MarkdownNodeProps) => (
            <pre className="terminal-scroll my-2 max-h-64 overflow-auto rounded-md border border-border/60 bg-muted/30 p-3 font-mono text-[0.6875rem] leading-5">
              {children}
            </pre>
          ),
          code: ({ children }: MarkdownNodeProps) => (
            <code className="rounded bg-muted/40 px-1 py-0.5 font-mono text-[0.6875rem]">
              {children}
            </code>
          ),
          table: ({ children }: MarkdownNodeProps) => (
            <div className="terminal-scroll my-2 overflow-auto">
              <table className="w-full border-collapse text-left">{children}</table>
            </div>
          ),
          th: ({ children }: MarkdownNodeProps) => (
            <th className="border border-border/60 px-2 py-1 font-medium">{children}</th>
          ),
          td: ({ children }: MarkdownNodeProps) => (
            <td className="border border-border/60 px-2 py-1">{children}</td>
          ),
        }}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}

function AssistantReasoning({ message, loading }: { message: AIMessage; loading: boolean }) {
  const { t } = useTranslation();
  const contentThink = useMemo(() => extractThinkContent(message.content), [message.content]);
  const reasoningContent = message.reasoningContent?.trim() || contentThink.reasoning || undefined;
  const [open, setOpen] = useState(false);

  const hasResponseContent =
    contentThink.visible.length > 0 ||
    (message.reasoningContent && contentThink.visible.length > 0);
  const stillThinking = loading && !hasResponseContent;

  if (!reasoningContent) {
    return stillThinking ? (
      <div className="mt-3 overflow-hidden rounded-md border border-primary/25 bg-primary/8 shadow-sm">
        <div className="px-3 py-2.5 text-[0.6875rem]">
          <AnimatedStatusText label={t("ai.thinking")} />
        </div>
        <div className="h-px bg-gradient-to-r from-transparent via-primary/50 to-transparent animate-pulse" />
      </div>
    ) : null;
  }

  return (
    <Collapsible
      open={open}
      onOpenChange={setOpen}
      className={`mt-3 rounded-md border bg-background/40 ${
        stillThinking ? "border-primary/25 bg-primary/6 shadow-sm" : "border-border/60"
      }`}
    >
      <CollapsibleTrigger asChild>
        <button
          type="button"
          className={`flex w-full items-center justify-between gap-2 px-3 py-2 text-left text-[0.6875rem] font-medium transition hover:text-foreground ${
            stillThinking ? "text-primary" : "text-muted-foreground"
          }`}
        >
          <span className="flex min-w-0 items-center gap-2">
            {stillThinking ? (
              <AnimatedStatusText label={t("ai.thinking")} />
            ) : (
              <span>{t("ai.thoughtComplete")}</span>
            )}
          </span>
          <span className="flex items-center gap-1">
            {open ? t("ai.collapseReasoning") : t("ai.expandReasoning")}
            {open ? <MdExpandLess className="text-sm" /> : <MdExpandMore className="text-sm" />}
          </span>
        </button>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="border-t border-border/60 px-3 py-3">
          <MarkdownContent content={reasoningContent} />
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

interface ParsedStructuredOutput {
  text: string;
  commandCards: AICommandCard[];
}

function tryParseStructuredOutput(content: string): ParsedStructuredOutput | null {
  const stripped = content
    .trim()
    .replace(/^```json\s*\n?/, "")
    .replace(/^```\s*\n?/, "")
    .replace(/```\s*$/, "")
    .trim();
  const start = stripped.indexOf("{");
  const end = stripped.lastIndexOf("}");
  if (start < 0 || end <= start) return null;
  try {
    const obj = JSON.parse(stripped.slice(start, end + 1));
    if (obj && typeof obj.text === "string" && obj.text.trim()) {
      return {
        text: obj.text,
        commandCards: Array.isArray(obj.commandCards) ? obj.commandCards : [],
      };
    }
  } catch {
    /* incomplete JSON */
  }
  return null;
}

function AssistantResponse({
  message,
  loading,
  onEarlyParse,
}: {
  message: AIMessage;
  loading: boolean;
  onEarlyParse?: (parsed: ParsedStructuredOutput) => void;
}) {
  const { t } = useTranslation();
  const contentThink = useMemo(() => extractThinkContent(message.content), [message.content]);
  const displayContent = contentThink.reasoning ? contentThink.visible : message.content;
  const [rawOpen, setRawOpen] = useState(false);
  const earlyParsedRef = useRef(false);

  if (loading && looksLikeStructuredJsonOutput(displayContent)) {
    const parsed = tryParseStructuredOutput(displayContent);
    if (parsed) {
      if (!earlyParsedRef.current && onEarlyParse) {
        earlyParsedRef.current = true;
        queueMicrotask(() => onEarlyParse(parsed));
      }
      return <MarkdownContent content={parsed.text} />;
    }

    return (
      <Collapsible
        open={rawOpen}
        onOpenChange={setRawOpen}
        className="mt-3 rounded-md border border-primary/25 bg-primary/6 shadow-sm"
      >
        <CollapsibleTrigger asChild>
          <button
            type="button"
            className="flex w-full items-center justify-between gap-2 px-3 py-2 text-left text-[0.6875rem] font-medium text-primary transition hover:text-foreground"
          >
            <AnimatedStatusText label={t("ai.formattingResponse")} />
            <span className="flex items-center gap-1 text-primary">
              {rawOpen ? t("ai.collapseReasoning") : t("ai.expandReasoning")}
              {rawOpen ? (
                <MdExpandLess className="text-sm" />
              ) : (
                <MdExpandMore className="text-sm" />
              )}
            </span>
          </button>
        </CollapsibleTrigger>
        <CollapsibleContent>
          <div className="border-t border-border/60 px-3 py-3">
            <pre className="max-h-48 overflow-auto font-mono text-[0.6875rem] leading-5 terminal-scroll whitespace-pre-wrap break-all text-muted-foreground">
              {displayContent || message.content}
            </pre>
          </div>
        </CollapsibleContent>
        <div className="h-px bg-gradient-to-r from-transparent via-primary/40 to-transparent animate-pulse" />
      </Collapsible>
    );
  }

  return <MarkdownContent content={displayContent} />;
}

function AICommandCardView({
  card,
  execution,
  onInsert,
  onSave,
  onAuthorize,
  onReject,
}: {
  card: AICommandCard;
  execution?: AICommandExecutionState;
  onInsert: (card: AICommandCard) => void;
  onSave: (card: AICommandCard) => void;
  onAuthorize: (card: AICommandCard) => void;
  onReject: (card: AICommandCard) => void;
}) {
  const { t } = useTranslation();
  const status = execution?.status ?? "idle";

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
      {status !== "idle" ? (
        <div className="mt-3 rounded-md border border-border/70 bg-muted/20 p-3">
          <div className="flex items-center gap-2 text-xs font-medium">
            {status === "auto_executing" ? <MdPlayArrow /> : null}
            {status === "executed" ? <MdCheck /> : null}
            {status === "pending_approval" ? <MdErrorOutline /> : null}
            {status === "rejected" ? <MdBlock /> : null}
            {status === "failed" ? <MdErrorOutline /> : null}
            <span>
              {status === "auto_executing"
                ? t("ai.commandAutoExecuting")
                : status === "executed"
                  ? t("ai.commandExecuted")
                  : status === "pending_approval"
                    ? t("ai.commandPendingApproval")
                    : status === "rejected"
                      ? t("ai.commandRejected")
                      : t("ai.commandExecutionFailed")}
            </span>
          </div>
          {status === "pending_approval" ? (
            <div className="mt-2 space-y-2 text-[0.6875rem] leading-5 text-muted-foreground">
              <p>
                {t("ai.authorizeCommandDesc", {
                  risk: card.riskLevel,
                })}
              </p>
              <div className="flex flex-wrap gap-1.5">
                <Button size="xs" variant="outline" onClick={() => onReject(card)}>
                  <MdBlock />
                  {t("ai.rejectExecute")}
                </Button>
                <Button size="xs" onClick={() => onAuthorize(card)}>
                  <MdPlayArrow />
                  {t("ai.authorizeExecute")}
                </Button>
              </div>
            </div>
          ) : null}
          {execution?.error ? (
            <div className="mt-2 text-[0.6875rem] text-destructive">{execution.error}</div>
          ) : null}
        </div>
      ) : null}
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

function AgentStepView({
  step,
  prismStyle,
}: {
  step: AgentStepPayload;
  prismStyle: Record<string, CSSProperties>;
}) {
  const { t } = useTranslation();
  const [thoughtOpen, setThoughtOpen] = useState(false);
  const [outputOpen, setOutputOpen] = useState(false);

  const isCommand = step.action.kind === "execute_command";
  const isFinal = step.action.kind === "final_answer";

  const isSuccess = step.status === "completed";
  const isFailed = step.status === "failed" || step.status === "rejected";
  const isRunning = step.status === "running";

  const borderColor = isSuccess
    ? "border-emerald-500"
    : isFailed
      ? "border-destructive"
      : isRunning
        ? "border-primary"
        : "border-amber-500";

  return (
    <div className="pb-3 last:pb-0">
      {/* Header: # N + collapsible thought */}
      <Collapsible open={thoughtOpen} onOpenChange={setThoughtOpen}>
        <CollapsibleTrigger asChild>
          <button
            type="button"
            className="flex w-full items-center gap-1.5 text-[0.6875rem] text-muted-foreground hover:text-foreground"
          >
            <MdExpandMore
              className={`text-sm transition-transform ${thoughtOpen ? "rotate-180" : ""}`}
            />
            <span className="font-semibold text-foreground">#{step.stepIndex + 1}</span>
            <span>
              {step.thought ? t("ai.expandThought") : isFinal ? t("ai.agentStepCompleted") : ""}
            </span>
            {step.observation?.durationMs != null ? (
              <span className="ml-auto tabular-nums text-muted-foreground/70">
                {step.observation.durationMs}ms
              </span>
            ) : null}
          </button>
        </CollapsibleTrigger>
        {step.thought ? (
          <CollapsibleContent>
            <div className="mt-1 ml-5 text-xs leading-5 text-muted-foreground">{step.thought}</div>
          </CollapsibleContent>
        ) : null}
      </Collapsible>

      {/* Command box with status-colored left border */}
      {isCommand && step.action.command ? (
        <div
          className={`mt-2 overflow-hidden rounded-md border-l-[3px] ${borderColor} border border-border/60 bg-muted/20`}
        >
          {/* Shell header */}
          <div className="flex items-center gap-1.5 border-b border-border/40 px-2.5 py-1 text-[0.625rem] text-muted-foreground">
            <span className="font-medium uppercase tracking-wider">shell</span>
            {step.action.riskLevel ? (
              <span
                className={`ml-auto rounded-full border px-1.5 py-0.5 font-medium ${riskClassName[step.action.riskLevel]}`}
              >
                {step.action.riskLevel}
              </span>
            ) : null}
          </div>

          {/* Command body */}
          <SyntaxHighlighter
            language="bash"
            style={prismStyle}
            customStyle={{
              margin: 0,
              padding: "0.5rem 0.625rem",
              fontSize: "0.6875rem",
              lineHeight: "1.25rem",
              background: "transparent",
              borderRadius: 0,
            }}
            wrapLongLines
          >
            {step.action.command}
          </SyntaxHighlighter>

          {/* Collapsible output inside the box */}
          {step.observation ? (
            <Collapsible open={outputOpen} onOpenChange={setOutputOpen}>
              <CollapsibleTrigger asChild>
                <button
                  type="button"
                  className="flex w-full items-center gap-1.5 border-t border-border/40 px-2.5 py-1 text-[0.625rem] text-muted-foreground hover:bg-muted/30"
                >
                  <MdExpandMore
                    className={`text-sm transition-transform ${outputOpen ? "rotate-180" : ""}`}
                  />
                  <span>{outputOpen ? t("ai.collapseOutput") : t("ai.expandOutput")}</span>
                  {step.observation.exitCode != null ? (
                    <span
                      className={`ml-auto font-medium ${step.observation.exitCode === 0 ? "text-emerald-600" : "text-destructive"}`}
                    >
                      {t("ai.stepExitCode", { code: step.observation.exitCode })}
                    </span>
                  ) : null}
                </button>
              </CollapsibleTrigger>
              <CollapsibleContent>
                <pre className="max-h-48 overflow-auto border-t border-border/40 bg-muted/10 px-2.5 py-2 font-mono text-[0.625rem] leading-5 terminal-scroll whitespace-pre-wrap break-all text-muted-foreground">
                  {step.observation.output || "(no output)"}
                </pre>
              </CollapsibleContent>
            </Collapsible>
          ) : null}

          {/* Running indicator inside box */}
          {isRunning ? (
            <div className="border-t border-border/40 px-2.5 py-1.5">
              <AnimatedStatusText label={t("ai.agentExecuting")} />
            </div>
          ) : null}
        </div>
      ) : null}

      {/* Error message */}
      {step.error ? (
        <div className="mt-1.5 ml-5 text-[0.6875rem] text-destructive">{step.error}</div>
      ) : null}

      {/* Final answer */}
      {isFinal && step.action.answer ? (
        <div className="mt-2 ml-5">
          <MarkdownContent content={step.action.answer} />
        </div>
      ) : null}
    </div>
  );
}

function ModelCombobox({
  models,
  credentials,
  selectedModel,
  open,
  onOpenChange,
  onSelect,
  className,
}: {
  models: AIModelConfigItem[];
  credentials: AIProviderCredential[];
  selectedModel: AIModelConfigItem | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSelect: (model: AIModelConfigItem) => void;
  className?: string;
}) {
  const { t } = useTranslation();

  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      <PopoverTrigger asChild>
        <Button
          type="button"
          size="sm"
          variant="outline"
          className={`h-8 min-w-0 max-w-[12rem] justify-between gap-2 px-2 text-xs ${className}`}
          disabled={models.length === 0}
        >
          <span className="truncate">{selectedModel?.name ?? t("ai.modelSelect")}</span>
          <MdExpandMore className="shrink-0 text-sm" />
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-80 p-0" align="start">
        <Command>
          <CommandInput placeholder={t("ai.searchModels")} className="text-xs" />
          <CommandList className="max-h-64 terminal-scroll">
            <CommandEmpty>{t("ai.noModelMatches")}</CommandEmpty>
            <CommandGroup>
              {models.map((model) => {
                const providerLabel = getModelProviderLabel(model, credentials);
                return (
                  <CommandItem
                    key={model.id}
                    value={`${model.name} ${providerLabel} ${model.id}`}
                    onSelect={() => {
                      onSelect(model);
                      onOpenChange(false);
                    }}
                  >
                    <MdCheck
                      className={`text-sm ${selectedModel?.id === model.id ? "opacity-100" : "opacity-0"}`}
                    />
                    <span className="min-w-0 flex-1 truncate">{model.name}</span>
                    {providerLabel ? (
                      <span className="shrink-0 text-[0.625rem] text-muted-foreground">
                        {providerLabel}
                      </span>
                    ) : null}
                  </CommandItem>
                );
              })}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

function AIAssistantPanel({ activePane, activeConnection, intent }: AIAssistantPanelProps) {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings, tabs, savedConnections } = useApp();
  const { theme } = useTheme();
  const aiSettings = appSettings.ai;
  const [input, setInput] = useState("");
  const [quotedText, setQuotedText] = useState<QuotedText | null>(null);
  const [messages, setMessages] = useState<AIMessage[]>([]);
  const [sessions, setSessions] = useState<AISession[]>([]);
  const [showHistory, setShowHistory] = useState(false);
  const [historyQuery, setHistoryQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const [clearHistoryOpen, setClearHistoryOpen] = useState(false);
  const [clearingHistory, setClearingHistory] = useState(false);
  const [currentSessionId, setCurrentSessionId] = useState<string | null>(null);
  const [streamId, setStreamId] = useState<string | null>(null);
  const [streamingAssistantId, setStreamingAssistantId] = useState<string | null>(null);
  const [detectedError, setDetectedError] = useState<AIErrorDetectedDetail | null>(null);
  const [targetPanes, setTargetPanes] = useState<SessionPane[]>([]);
  const [showMentionPopover, setShowMentionPopover] = useState(false);
  const [mentionQuery, setMentionQuery] = useState("");
  const [mentionIndex, setMentionIndex] = useState(0);
  const [commandExecution, setCommandExecution] = useState<Record<string, AICommandExecutionState>>(
    {},
  );
  const [agentStepsMap, setAgentStepsMap] = useState<Record<string, AgentStepPayload[]>>({});
  const [modelPopoverOpen, setModelPopoverOpen] = useState(false);
  const handledIntentIdRef = useRef<string | null>(null);
  const historyButtonRef = useRef<HTMLButtonElement | null>(null);
  const historyCardRef = useRef<HTMLDivElement | null>(null);
  const mentionPopoverRef = useRef<HTMLDivElement | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const streamUnlistenRef = useRef<UnlistenFn | null>(null);
  const scrollContainerRef = useRef<HTMLDivElement>(null);
  const shouldAutoScrollRef = useRef(true);
  const cancelledRef = useRef(false);

  const enabledModels = useMemo(() => getEnabledAIModels(aiSettings), [aiSettings]);
  const selectedModel = useMemo(() => selectDefaultAIModel(aiSettings), [aiSettings]);
  const prismStyle = useMemo(() => buildPrismThemeFromColors(theme.colors), [theme.colors]);
  const mode = aiSettings.default_mode ?? "ask";

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

  // biome-ignore lint/correctness/useExhaustiveDependencies: reset selection whenever the filtered mention list changes.
  useEffect(() => {
    setMentionIndex(0);
  }, [filteredMentionPanes]);

  const effectivePanes = targetPanes.length > 0 ? targetPanes : activePane ? [activePane] : [];
  const effectiveSessionId = effectivePanes[0]?.sessionId ?? null;

  const activeSessionId = activePane?.sessionId ?? null;

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

  const cleanupStreamListener = useCallback(() => {
    streamUnlistenRef.current?.();
    streamUnlistenRef.current = null;
  }, []);

  useEffect(() => {
    return () => {
      cleanupStreamListener();
    };
  }, [cleanupStreamListener]);

  useEffect(() => {
    const watchedIds = new Set(effectivePanes.map((p) => p.sessionId));
    if (activeSessionId) watchedIds.add(activeSessionId);
    const handler = (event: Event) => {
      const detail = (event as CustomEvent<AIErrorDetectedDetail>).detail;
      if (!detail || !watchedIds.has(detail.sessionId)) return;
      setDetectedError(detail);
    };
    window.addEventListener(AI_ERROR_DETECTED_EVENT, handler);
    return () => window.removeEventListener(AI_ERROR_DETECTED_EVENT, handler);
  }, [activeSessionId, effectivePanes]);

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
      executed?: boolean;
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
          executed: params.executed ?? false,
          blocked: params.blocked ?? false,
        },
      }).catch(() => {});
    },
    [activeConnection?.id],
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

  const setCommandState = useCallback(
    (cardId: string, status: AICommandExecutionStatus, error?: string) => {
      setCommandExecution((prev) => ({ ...prev, [cardId]: { status, error } }));
    },
    [],
  );

  const executeCommandCard = useCallback(
    async (card: AICommandCard, source: "auto" | "authorized") => {
      const targetSessionId = effectiveSessionId ?? activeSessionId;
      const provider = getTerminalContextProvider(targetSessionId);
      if (!provider) {
        const error = t("ai.noTerminal");
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

      setCommandState(card.id, "auto_executing");
      try {
        await provider.executeCommand(card.command);
        provider.focus();
        setCommandState(card.id, "executed");
        appendAudit({
          action: source === "auto" ? "ai.agent_auto_execute" : "ai.agent_authorized_execute",
          generatedCommand: card.command,
          riskLevel: card.riskLevel,
          executed: true,
        });
      } catch (error) {
        const message = getErrorMessage(error);
        setCommandState(card.id, "failed", message);
        appendAudit({
          action: "ai.agent_execute_failed",
          generatedCommand: card.command,
          riskLevel: card.riskLevel,
        });
        toast.error(message);
      }
    },
    [activeSessionId, appendAudit, effectiveSessionId, setCommandState, t],
  );

  const handleAgentCommandCards = useCallback(
    (cards: AICommandCard[], requestMode: AIMode, allowedRisk: RiskLevel) => {
      if (requestMode !== "agent") return;
      for (const card of cards) {
        if (isRiskAllowed(card.riskLevel, allowedRisk)) {
          void executeCommandCard(card, "auto");
        } else {
          setCommandState(card.id, "pending_approval");
        }
      }
    },
    [executeCommandCard, setCommandState],
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
      if (!requestModel) {
        toast.error(t("ai.noEnabledModels"));
        return;
      }
      const requestMode = mode;
      const allowedRisk = aiSettings.allowed_command_risk_level ?? "medium";

      setDetectedError(null);
      setLoading(true);
      cancelledRef.current = false;
      cleanupStreamListener();

      const userMessage = createLocalMessage("user", userInput, currentSessionId ?? "local");
      const assistantId = `assistant-${Date.now()}-${Math.random().toString(36).slice(2)}`;
      const requestStreamId = `ai-stream-${crypto.randomUUID()}`;
      setMessages((prev) => [
        ...prev,
        userMessage,
        {
          id: assistantId,
          sessionId: currentSessionId ?? "local",
          role: "assistant",
          content: requestMode === "agent" ? "" : "",
          createdAt: new Date().toISOString(),
          reasoningContent: null,
          commandCards: [],
        },
      ]);
      setStreamingAssistantId(assistantId);

      try {
        const unlisten = await listen<AIStreamEventPayload | AgentStepPayload>(
          `ai-stream-${requestStreamId}`,
          (event) => {
            const raw = event.payload as unknown as Record<string, unknown>;
            if (raw.streamId !== requestStreamId) return;

            if ("stepIndex" in raw) {
              const step = raw as unknown as AgentStepPayload;
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
              if (payload.sessionId) setCurrentSessionId(payload.sessionId);
              return;
            }

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

            if (payload.type === "reasoning_delta" && payload.reasoningDelta) {
              setMessages((prev) =>
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
              cleanupStreamListener();
              setLoading(false);
              setStreamId(null);
              setStreamingAssistantId(null);
              const newMsgId = payload.message?.id;
              if (payload.message) {
                setMessages((prev) =>
                  prev.map((message) => (message.id === assistantId ? payload.message! : message)),
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
              if (requestMode !== "agent") {
                handleAgentCommandCards(
                  payload.message?.commandCards ?? payload.commandCards ?? [],
                  requestMode,
                  allowedRisk,
                );
              }
              void loadSessions();
              return;
            }

            if (payload.type === "error") {
              const wasCancelled = cancelledRef.current;
              cleanupStreamListener();
              setLoading(false);
              setStreamId(null);
              setStreamingAssistantId(null);
              if (!wasCancelled) {
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
            }
          },
        );
        streamUnlistenRef.current = unlisten;
        setStreamId(requestStreamId);

        const context = await buildMergedContext(panes, selectedText);
        const primaryConn = panes[0].connectionId
          ? (savedConnections.find((c) => c.id === panes[0].connectionId) ?? null)
          : activeConnection;

        const result = await invoke<AIStreamStart>("start_ai_chat_stream", {
          request: {
            streamId: requestStreamId,
            sessionId: currentSessionId,
            connectionId: primaryConn?.id ?? null,
            terminalSessionId: panes[0]?.sessionId ?? null,
            action,
            userInput,
            mode: requestMode,
            modelId: requestModel.id,
            modelName: requestModel.name,
            context,
            options: {
              maxOutputCommands: 2,
              language: "zh-CN",
              safetyMode: "strict",
              allowedCommandRiskLevel: allowedRisk,
            },
          },
        });
        setCurrentSessionId(result.sessionId);
        setStreamId(result.streamId);
        appendAudit({ action: `ai.${action}`, userInput });
      } catch (error) {
        cleanupStreamListener();
        setLoading(false);
        setStreamId(null);
        setStreamingAssistantId(null);
        if (!cancelledRef.current) {
          setMessages((prev) =>
            prev.map((message) =>
              message.id === assistantId
                ? { ...message, content: getErrorMessage(error) }
                : message,
            ),
          );
          toast.error(getErrorMessage(error));
        }
      }
    },
    [
      activeConnection,
      aiSettings.allowed_command_risk_level,
      aiSettings.enabled,
      appendAudit,
      buildMergedContext,
      cleanupStreamListener,
      currentSessionId,
      effectivePanes,
      handleAgentCommandCards,
      loadSessions,
      mode,
      savedConnections,
      selectedModel,
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
    const fullInput = quotedText ? `> ${quotedText.text}\n\n${value}` : value;
    setInput("");
    setQuotedText(null);
    shouldAutoScrollRef.current = true;
    void startChat("generate_command", fullInput);
  }, [input, loading, quotedText, startChat]);

  const cancelStream = useCallback(() => {
    if (!streamId) return;
    cancelledRef.current = true;
    void invoke("cancel_ai_chat_stream", { streamId }).catch(() => {});
    cleanupStreamListener();
    setLoading(false);
    setStreamId(null);
    setStreamingAssistantId(null);
  }, [cleanupStreamListener, streamId]);

  const insertCommand = useCallback(
    (card: AICommandCard) => {
      const insertSessionId = effectiveSessionId ?? activeSessionId;
      const provider = getTerminalContextProvider(insertSessionId);
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
    [activeSessionId, appendAudit, effectiveSessionId, t],
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
        riskLevel: card.riskLevel,
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
      setMessages([]);
      setCurrentSessionId(null);
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
          setMessages([]);
          setCurrentSessionId(null);
        }
        await loadSessions();
      } catch (error) {
        toast.error(getErrorMessage(error));
      }
    },
    [currentSessionId, loadSessions],
  );

  const dateGroupLabel: Record<DateGroup, string> = useMemo(
    () => ({
      today: t("ai.dateToday"),
      yesterday: t("ai.dateYesterday"),
      last7Days: t("ai.dateLast7Days"),
      earlier: t("ai.dateEarlier"),
    }),
    [t],
  );

  const groupedSessions = useMemo(() => groupSessionsByDate(filteredSessions), [filteredSessions]);

  const newChat = useCallback(() => {
    if (loading) return;
    setMessages([]);
    setCurrentSessionId(null);
    setInput("");
    setQuotedText(null);
    setDetectedError(null);
    setTargetPanes([]);
    setCommandExecution({});
    setAgentStepsMap({});
    setShowMentionPopover(false);
    shouldAutoScrollRef.current = true;
  }, [loading]);

  const handleCopySelection = useCallback(() => {
    const sel = window.getSelection()?.toString();
    if (sel) {
      void navigator.clipboard.writeText(sel);
    }
  }, []);

  const handleQuoteSelection = useCallback(() => {
    const sel = window.getSelection()?.toString()?.trim();
    if (sel) {
      setQuotedText({ text: sel });
      textareaRef.current?.focus();
    }
  }, []);

  const handleInputChange = useCallback(
    (event: React.ChangeEvent<HTMLTextAreaElement>) => {
      const value = event.target.value;
      setInput(value);

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
    [showMentionPopover],
  );

  const selectMentionPane = useCallback(
    (pane: SessionPane) => {
      setTargetPanes((prev) => {
        const exists = prev.some((p) => p.sessionId === pane.sessionId);
        return exists ? prev.filter((p) => p.sessionId !== pane.sessionId) : [...prev, pane];
      });

      const cursorPos = textareaRef.current?.selectionStart ?? input.length;
      const textBeforeCursor = input.slice(0, cursorPos);
      const textAfterCursor = input.slice(cursorPos);
      const cleaned = textBeforeCursor.replace(/@\S*$/, "");
      setInput(`${cleaned}${textAfterCursor}`);
      setShowMentionPopover(false);
      setMentionQuery("");
      textareaRef.current?.focus();
    },
    [input],
  );

  const removeTargetPane = useCallback((sessionId: string) => {
    setTargetPanes((prev) => prev.filter((p) => p.sessionId !== sessionId));
  }, []);

  return (
    <div
      className="relative flex h-full flex-col"
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
        if (showMentionPopover && !mentionPopoverRef.current?.contains(target)) {
          setShowMentionPopover(false);
        }
      }}
    >
      <PanelHeader
        title={t("ai.title")}
        meta={selectedModel?.name ?? t("ai.notConfigured")}
        actions={
          <>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  ref={historyButtonRef}
                  size="icon-sm"
                  variant="ghost"
                  onClick={() => setShowHistory((value) => !value)}
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
              dateGroupOrder.map((group) => {
                const items = groupedSessions[group];
                if (items.length === 0) return null;
                return (
                  <div key={group} className="mb-1">
                    <div className="px-2 py-1 text-[0.625rem] font-semibold uppercase tracking-wider text-muted-foreground/70">
                      {dateGroupLabel[group]}
                    </div>
                    {items.map((session) => (
                      <div
                        key={session.id}
                        className="group flex items-center gap-1 rounded-md px-2 py-1.5 hover:bg-muted/60"
                      >
                        <button
                          type="button"
                          className="min-w-0 flex-1 text-left text-xs"
                          onClick={() => void loadSessionMessages(session.id)}
                        >
                          <div className="truncate font-medium">{session.title}</div>
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
                    ))}
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
                ) : !selectedModel ? (
                  <div className="flex flex-col items-center gap-4 px-4">
                    <div className="flex size-12 items-center justify-center rounded-full border border-amber-500/30 bg-amber-500/10">
                      <MdErrorOutline className="text-2xl text-amber-500" />
                    </div>
                    <div className="space-y-1">
                      <div className="text-sm font-medium text-foreground">{t("ai.setupTitle")}</div>
                    </div>
                    <div className="w-full space-y-2 text-left text-xs">
                      <div className="flex items-start gap-2 rounded-md border border-border/60 bg-muted/30 px-3 py-2">
                        <span className="mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-full bg-primary/15 text-[0.625rem] font-bold text-primary">1</span>
                        <span>{t("ai.setupStep1")}</span>
                      </div>
                      <div className="flex items-start gap-2 rounded-md border border-border/60 bg-muted/30 px-3 py-2">
                        <span className="mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-full bg-primary/15 text-[0.625rem] font-bold text-primary">2</span>
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
                              setMessages((prev) =>
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
                  onClick={() => setQuotedText(null)}
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
                    value={mode}
                    onValueChange={(default_mode) =>
                      updateAppSettings({
                        ai: { ...aiSettings, default_mode: default_mode as AIMode },
                      })
                    }
                  >
                    <SelectTrigger size="sm" className="w-full text-xs">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent position="popper">
                      <SelectItem value="ask">{t("ai.modeAsk")}</SelectItem>
                      <SelectItem value="agent">{t("ai.modeAgent")}</SelectItem>
                    </SelectContent>
                  </Select>
                </div>

                <div className="w-2/3 min-w-0">
                  <ModelCombobox
                    models={enabledModels}
                    credentials={aiSettings.provider_credentials}
                    selectedModel={selectedModel}
                    open={modelPopoverOpen}
                    onOpenChange={setModelPopoverOpen}
                    onSelect={(model) =>
                      updateAppSettings({ ai: { ...aiSettings, default_model_id: model.id } })
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
                    disabled={!input.trim() || !selectedModel || !aiSettings.enabled}
                  >
                    <MdSend />
                  </Button>
                )}
              </div>
            </div>
          </div>
        </div>
      </div>

      <AlertDialog open={clearHistoryOpen} onOpenChange={setClearHistoryOpen}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>{t("ai.clearHistoryTitle")}</AlertDialogTitle>
            <AlertDialogDescription>{t("ai.clearHistoryDesc")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={clearingHistory}>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={clearingHistory}
              onClick={(event) => {
                event.preventDefault();
                void clearHistory();
              }}
            >
              {t("ai.clearHistory")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

export default memo(AIAssistantPanel);
