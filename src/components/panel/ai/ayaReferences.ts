import type {
  AIFileReference,
  AIReferenceMetadata,
  AITerminalTarget,
  AyaReferenceContext,
  SavedConnection,
  SessionPane,
} from "@/types/global";
import type { AIInlineMention } from "./AIReferenceComposer";

export interface AyaDraftMention extends AIInlineMention {
  sessionIds: string[];
  fileReference?: AIFileReference;
}

export function resolveAyaEndpoint(
  pane: SessionPane,
  panes: SessionPane[],
  connections: SavedConnection[],
) {
  const sourcePane =
    pane.paneKind === "file"
      ? (panes.find((item) => item.paneKind === "terminal" && item.sessionId === pane.sessionId) ??
        pane)
      : pane;
  const connection = connections.find(
    (item) => item.id === (pane.connectionId ?? sourcePane.connectionId),
  );
  const temporary = sourcePane.temporaryConfig;
  const ssh = temporary?.protocol === "ssh" ? temporary : null;
  const host = connection?.host ?? ssh?.host ?? null;
  const username = connection?.username ?? ssh?.username ?? null;
  const port = connection?.port ?? ssh?.port ?? (pane.type === "SSH" ? 22 : null);
  const endpoint = host
    ? `${username ? `${username}@` : ""}${host.includes(":") ? `[${host}]` : host}${port ? `:${port}` : ""}`
    : pane.type === "Local"
      ? "Local"
      : pane.name;
  const jump = connections.find((item) => item.id === connection?.network?.proxy_jump_id);
  const jumpEndpoint = jump?.host
    ? `${jump.username ? `${jump.username}@` : ""}${jump.host}:${jump.port ?? 22}`
    : null;
  return {
    host,
    username,
    endpoint,
    title: jumpEndpoint ? `${endpoint} (via ${jumpEndpoint})` : endpoint,
  };
}

export function buildAyaTarget(
  pane: SessionPane,
  panes: SessionPane[],
  connections: SavedConnection[],
): AITerminalTarget {
  const identity = resolveAyaEndpoint(pane, panes, connections);
  return {
    terminalSessionId: pane.sessionId,
    connectionId: pane.connectionId ?? null,
    label: identity.title,
    host: identity.host,
    username: identity.username,
    sessionType: pane.type,
  };
}

export function resolveAyaPanes(
  panes: SessionPane[],
  activePane: SessionPane | null,
  mentions: AyaDraftMention[],
) {
  const explicitIds = [
    ...new Set(mentions.filter((item) => item.kind !== "file").flatMap((item) => item.sessionIds)),
  ];
  const sourceIds = [
    ...new Set(mentions.filter((item) => item.kind === "file").flatMap((item) => item.sessionIds)),
  ];
  // 显式 host/session 优先；仅有文件引用时使用当前活动终端执行，来源只提供上下文。
  const executionIds = explicitIds.length ? explicitIds : activePane ? [activePane.sessionId] : [];
  const contextIds = [
    ...new Set([...executionIds, ...sourceIds, ...(activePane ? [activePane.sessionId] : [])]),
  ];
  const findPane = (id: string) =>
    panes.find((item) => item.sessionId === id && item.paneKind === "terminal") ??
    panes.find((item) => item.sessionId === id && item.paneKind === "file") ??
    (activePane?.sessionId === id ? activePane : undefined);
  return {
    panes: contextIds.map(findPane).filter((pane): pane is SessionPane => !!pane),
    executionIds,
    missingIds: contextIds.filter((id) => !findPane(id)),
  };
}

export function buildAyaContext(
  mentions: AyaDraftMention[],
  files: AIFileReference[],
  executionIds: string[],
  offset: number,
): AyaReferenceContext {
  const references: AIReferenceMetadata[] = mentions.map((mention) => ({
    id: mention.id,
    name: mention.label,
    kind: mention.kind,
    title: mention.title,
    start: mention.start + offset,
    end: mention.end + offset,
    sessionIds: mention.sessionIds,
    path: mention.fileReference?.path,
    host: mention.fileReference?.host,
    backend: mention.fileReference?.backend,
    terminalSessionId: mention.fileReference?.terminalSessionId,
    connectionId: mention.fileReference?.connectionId,
    sizeBytes: files.find((file) => file.id === mention.id)?.sizeBytes,
  }));
  const uniqueFiles = [...new Map(files.map((file) => [file.id, file])).values()];
  return {
    references,
    files: uniqueFiles.map((file) => ({
      referenceId: file.id,
      sourceSessionId: file.terminalSessionId,
      sourceEndpoint: file.host ?? "Local",
      backend: file.backend,
      path: file.path,
      content: file.content,
    })),
    executionTargetSessionIds: executionIds,
  };
}
