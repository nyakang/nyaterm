import type { CSSProperties } from "react";
import type { ThemeColors } from "@/lib/themes";
import type {
  AIAction,
  AIFileAttachment,
  AIMessage,
  AIReferenceMetadata,
  AISession,
} from "@/types/global";

export function actionTitle(action: AIAction) {
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

export function createLocalMessage(
  role: "user" | "assistant",
  content: string,
  sessionId = "local",
  attachments: AIFileAttachment[] = [],
  references: AIReferenceMetadata[] = [],
) {
  return {
    id: `local-${Date.now()}-${Math.random().toString(36).slice(2)}`,
    sessionId,
    role,
    content,
    createdAt: new Date().toISOString(),
    reasoningContent: null,
    commandCards: [],
    attachments,
    references,
  } satisfies AIMessage;
}

export function slugCategory(name: string) {
  return `ai-${
    name
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9\u4e00-\u9fa5]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 48) || "commands"
  }`;
}

export type DateGroup = "today" | "yesterday" | "last7Days" | "earlier";

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

export const dateGroupOrder: DateGroup[] = ["today", "yesterday", "last7Days", "earlier"];

export function groupSessionsByDate(sessions: AISession[]) {
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

export function buildPrismThemeFromColors(colors: ThemeColors): Record<string, CSSProperties> {
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
