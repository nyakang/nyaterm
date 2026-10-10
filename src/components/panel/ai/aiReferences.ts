import type { FileDocumentPane, SavedConnection, SessionInfo, SessionPane } from "@/types/global";
import { resolveAyaEndpoint } from "./ayaReferences";

export interface AIReferenceOptionBase {
  id: string;
  label: string;
  title: string;
  sessionIds: string[];
  sortTime: number;
}

export interface AISessionReferenceOption extends AIReferenceOptionBase {
  kind: "session";
  pane: Exclude<SessionPane, FileDocumentPane>;
}

export interface AIFileReferenceOption extends AIReferenceOptionBase {
  kind: "file";
  pane: FileDocumentPane;
  host: string | null;
  sizeBytes: number;
}

export type AIReferenceOption = AISessionReferenceOption | AIFileReferenceOption;

export interface AIReferenceGroup {
  id: string;
  label: string;
  title: string;
  host: string | null;
  connectionIds: string[];
  sessions: string[];
  children: AIReferenceOption[];
  sortTime: number;
}

function parseTime(value: string | null | undefined) {
  if (!value) return 0;
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp) ? timestamp : 0;
}

function connectionForPane(pane: SessionPane, savedConnections: SavedConnection[]) {
  return pane.connectionId
    ? (savedConnections.find((connection) => connection.id === pane.connectionId) ?? null)
    : null;
}

function resolveGroup(
  pane: SessionPane,
  connection: SavedConnection | null,
  panes: SessionPane[],
  savedConnections: SavedConnection[],
  ayaEndpoints: boolean,
): Pick<AIReferenceGroup, "id" | "label" | "title" | "host"> {
  if (
    pane.paneKind === "file" &&
    pane.file.backend === "local" &&
    connection?.type !== "local_terminal"
  ) {
    return { id: "local", label: "Local", title: "Local files", host: null };
  }

  const temporary = pane.temporaryConfig;
  const temporarySsh = temporary?.protocol === "ssh" ? temporary : null;
  const host = connection?.host ?? temporarySsh?.host ?? null;
  const port = connection?.port ?? temporarySsh?.port ?? null;
  const username = connection?.username ?? temporarySsh?.username ?? null;
  const isSsh = connection?.type === "ssh" || !!temporarySsh || pane.type === "SSH";

  if (isSsh && host) {
    const endpoint = `${host}${port ? `:${port}` : ""}`;
    const label = connection?.name || endpoint;
    const title = ayaEndpoints
      ? resolveAyaEndpoint(pane, panes, savedConnections).title
      : `${username ? `${username}@` : ""}${endpoint}`;
    const userKey = username ? `${username.toLowerCase()}@` : "";
    return { id: `ssh:${userKey}${host.toLowerCase()}:${port ?? 22}`, label, title, host };
  }

  if (connection) {
    return {
      id: `connection:${connection.id}`,
      label: connection.name || connection.host || pane.name,
      title: connection.host ?? connection.name ?? pane.name,
      host: connection.host ?? null,
    };
  }

  if (temporary) {
    const temporaryHost = "host" in temporary ? temporary.host : null;
    const temporaryName = "name" in temporary ? temporary.name : pane.name;
    return {
      id: `temporary:${temporary.protocol}:${temporaryHost ?? temporaryName}`,
      label: temporaryName || temporaryHost || pane.name,
      title: temporaryHost ?? temporaryName ?? pane.name,
      host: temporaryHost,
    };
  }

  return { id: "local", label: "Local", title: "Local sessions", host: null };
}

function fileTitle(
  pane: FileDocumentPane,
  host: string | null,
  connection: SavedConnection | null,
) {
  if (pane.file.backend === "remote") {
    const user = connection?.username;
    const endpoint = `${connection?.host ?? host ?? pane.name}${connection?.port ? `:${connection.port}` : ""}`;
    return `${user ? `${user}@` : ""}${endpoint}:${pane.file.path}`;
  }
  return `Local:${pane.file.path}`;
}

export function buildAIReferenceGroups(
  panes: SessionPane[],
  sessions: SessionInfo[],
  savedConnections: SavedConnection[],
  ayaEndpoints = false,
): AIReferenceGroup[] {
  const liveSessionById = new Map(sessions.map((session) => [session.id, session]));
  const groups = new Map<string, AIReferenceGroup>();
  const seenSessionIds = new Set<string>();

  const ensureGroup = (pane: SessionPane) => {
    const source =
      ayaEndpoints && pane.paneKind === "file"
        ? (panes.find(
            (item) => item.paneKind === "terminal" && item.sessionId === pane.sessionId,
          ) ?? pane)
        : pane;
    const connection =
      connectionForPane(source, savedConnections) ?? connectionForPane(pane, savedConnections);
    const identity = resolveGroup(
      { ...pane, temporaryConfig: source.temporaryConfig },
      connection,
      panes,
      savedConnections,
      ayaEndpoints,
    );
    let group = groups.get(identity.id);
    if (!group) {
      group = {
        ...identity,
        connectionIds: [],
        sessions: [],
        children: [],
        sortTime: 0,
      };
      groups.set(identity.id, group);
    }
    if (pane.connectionId && !group.connectionIds.includes(pane.connectionId)) {
      group.connectionIds.push(pane.connectionId);
    }
    return { group, connection };
  };

  for (const pane of panes) {
    if (pane.connecting || pane.connectError) continue;
    const { group, connection } = ensureGroup(pane);

    if (pane.paneKind === "file") {
      const sortTime = parseTime(pane.openedAt);
      group.children.push({
        id: `file:${pane.file.backend}:${pane.sessionId}:${pane.file.path}`,
        kind: "file",
        label: pane.name,
        title: ayaEndpoints
          ? `${pane.file.backend === "local" ? "Local" : resolveAyaEndpoint(pane, panes, savedConnections).title}:${pane.file.path}`
          : fileTitle(pane, group.host, connection),
        sessionIds: [pane.sessionId],
        pane,
        host: group.host,
        sizeBytes: pane.file.initial.size,
        sortTime,
      });
      group.sortTime = Math.max(group.sortTime, sortTime);
      continue;
    }

    const liveSession = liveSessionById.get(pane.sessionId);
    if (!liveSession?.connected || seenSessionIds.has(pane.sessionId)) continue;
    seenSessionIds.add(pane.sessionId);
    const sortTime = parseTime(liveSession.started_at);
    group.sessions.push(pane.sessionId);
    group.children.push({
      id: `session:${pane.sessionId}`,
      kind: "session",
      label: pane.name,
      title: group.host ? `${group.title} · ${pane.name}` : pane.name,
      sessionIds: [pane.sessionId],
      pane,
      sortTime,
    });
    group.sortTime = Math.max(group.sortTime, sortTime);
  }

  return [...groups.values()]
    .map((group) => {
      const children = group.children.sort(
        (left, right) => right.sortTime - left.sortTime || left.label.localeCompare(right.label),
      );
      return {
        ...group,
        sessions: children
          .filter((option): option is AISessionReferenceOption => option.kind === "session")
          .map((option) => option.pane.sessionId),
        children,
      };
    })
    .filter((group) => group.children.length > 0)
    .sort((left, right) => right.sortTime - left.sortTime || left.label.localeCompare(right.label));
}

export function formatAIFileReferenceContext(
  references: Array<{ title: string; content: string }>,
  label: string,
) {
  return references.map(({ title, content }) => `${label}: ${title}\n---\n${content}`).join("\n\n");
}
