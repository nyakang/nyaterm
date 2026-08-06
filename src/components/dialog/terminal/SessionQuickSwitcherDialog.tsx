import { Search } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { invoke } from "@/lib/invoke";
import type { SavedConnection, SessionType } from "@/types/global";

export interface QuickSwitcherSession {
  id: string;
  name: string;
  sessionType: SessionType;
  connectionName?: string;
  tabName?: string;
  connecting?: boolean;
  connectError?: string;
}

type QuickSwitcherItem =
  | {
      kind: "session";
      id: string;
      title: string;
      subtitle: string;
      display: string;
      order: number;
      session: QuickSwitcherSession;
    }
  | {
      kind: "connection";
      id: string;
      title: string;
      subtitle: string;
      display: string;
      order: number;
      connection: SavedConnection;
    };

interface FuzzySearchCandidate {
  id: string;
  value: string;
  display: string;
}

interface FuzzySearchResult {
  id: string;
  value: string;
  display: string;
  score: number;
  indices: number[];
}

interface SessionQuickSwitcherProps {
  open: boolean;
  activeSessionId: string | null;
  workspaceSessions: QuickSwitcherSession[];
  savedConnections: SavedConnection[];
  onClose: () => void;
  onSelectSession: (sessionId: string) => void;
  onOpenConnection: (connection: SavedConnection) => void;
  onNewSshSession: () => void;
}

function getConnectionTarget(connection: SavedConnection) {
  if (connection.type === "serial") return connection.port_name ?? "";
  if (connection.type === "local_terminal")
    return connection.working_dir || connection.shell_path || "";

  const host = connection.host ?? "";
  const port = connection.port ? `:${connection.port}` : "";
  return `${host}${port}`;
}

function getConnectionSubtitle(connection: SavedConnection) {
  const target = getConnectionTarget(connection);
  const username = connection.username ? `${connection.username}@` : "";
  const type = connection.type.replace("_", " ");
  return [type, `${username}${target}`.trim()].filter(Boolean).join(" - ");
}

function stringifySearchParts(parts: unknown[]) {
  return parts
    .filter((part) => part !== undefined && part !== null && part !== "")
    .map(String)
    .join(" ");
}

export default function SessionQuickSwitcher({
  open,
  activeSessionId,
  workspaceSessions,
  savedConnections,
  onClose,
  onSelectSession,
  onOpenConnection,
  onNewSshSession,
}: SessionQuickSwitcherProps) {
  const { t } = useTranslation();
  const searchRef = useRef<HTMLInputElement | null>(null);
  const searchRequestIdRef = useRef(0);
  const lastMatchedItemIdsRef = useRef<string[] | null>(null);
  const itemButtonRefs = useRef(new Map<string, HTMLButtonElement>());
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [matchedItemIds, setMatchedItemIds] = useState<string[] | null>(null);

  useEffect(() => {
    if (!open) return;

    setQuery("");
    setSelectedIndex(0);
    setMatchedItemIds(null);
    lastMatchedItemIdsRef.current = null;
    window.setTimeout(() => searchRef.current?.focus(), 0);
  }, [open]);

  const items = useMemo<QuickSwitcherItem[]>(() => {
    const sessionItems = workspaceSessions.map((session, index) => {
      const status = session.connecting
        ? t("savedConnections.connecting", { name: session.name })
        : session.connectError
          ? t("terminal.connectionFailed")
          : session.sessionType;
      const subtitle = [status, session.connectionName, session.tabName]
        .filter(Boolean)
        .join(" - ");

      return {
        kind: "session" as const,
        id: `session:${session.id}`,
        title: session.name,
        subtitle,
        display: stringifySearchParts([
          session.name,
          session.sessionType,
          session.connectionName,
          session.tabName,
          session.id,
          session.connectError,
          status,
        ]),
        order: index,
        session,
      };
    });
    const connectionItems = savedConnections.map((connection, index) => {
      const subtitle = getConnectionSubtitle(connection);
      return {
        kind: "connection" as const,
        id: `connection:${connection.id}`,
        title: connection.name,
        subtitle,
        display: stringifySearchParts([
          connection.name,
          connection.description,
          connection.type,
          connection.host,
          connection.port,
          connection.username,
          connection.port_name,
          connection.shell_path,
          connection.shell_args,
          connection.working_dir,
          subtitle,
        ]),
        order: workspaceSessions.length + index,
        connection,
      };
    });

    return [...sessionItems, ...connectionItems];
  }, [savedConnections, t, workspaceSessions]);

  const candidates = useMemo<FuzzySearchCandidate[]>(
    () =>
      items.map((item) => ({
        id: item.id,
        value: item.id,
        display: item.display,
      })),
    [items],
  );

  const itemById = useMemo(() => new Map(items.map((item) => [item.id, item])), [items]);

  useEffect(() => {
    if (!open) return;

    const pattern = query.trim();
    const requestId = searchRequestIdRef.current + 1;
    searchRequestIdRef.current = requestId;

    if (!pattern) {
      setMatchedItemIds(null);
      return;
    }

    const timer = window.setTimeout(async () => {
      try {
        const results = await invoke<FuzzySearchResult[]>("fuzzy_search_candidates", {
          pattern,
          items: candidates,
          limit: 50,
        });
        if (requestId !== searchRequestIdRef.current) return;

        const ids = results
          .filter((result) => itemById.has(result.id))
          .sort((left, right) => {
            const leftItem = itemById.get(left.id);
            const rightItem = itemById.get(right.id);
            if (!leftItem || !rightItem) return 0;
            return right.score - left.score || leftItem.order - rightItem.order;
          })
          .map((result) => result.id);

        lastMatchedItemIdsRef.current = ids;
        setMatchedItemIds(ids);
      } catch {
        if (requestId === searchRequestIdRef.current) {
          setMatchedItemIds(lastMatchedItemIdsRef.current ?? []);
        }
      }
    }, 80);

    return () => window.clearTimeout(timer);
  }, [candidates, itemById, open, query]);

  const filteredItems = useMemo(
    () => (matchedItemIds ? matchedItemIds.flatMap((id) => itemById.get(id) ?? []) : items),
    [itemById, items, matchedItemIds],
  );
  const selectedItemId = filteredItems[selectedIndex]?.id;

  useEffect(() => {
    setSelectedIndex((index) =>
      filteredItems.length === 0 ? 0 : Math.min(index, filteredItems.length - 1),
    );
  }, [filteredItems.length]);

  useEffect(() => {
    if (!open || !selectedItemId) return;

    itemButtonRefs.current.get(selectedItemId)?.scrollIntoView({
      block: "nearest",
      inline: "nearest",
    });
  }, [open, selectedItemId]);

  const selectItem = (item: QuickSwitcherItem) => {
    if (item.kind === "session") {
      onSelectSession(item.session.id);
      return;
    }
    onOpenConnection(item.connection);
  };

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => !nextOpen && onClose()}>
      <DialogContent
        showCloseButton={false}
        className="top-[18vh] w-[min(40rem,calc(100vw-2rem))] max-w-none translate-y-0 gap-0 overflow-hidden rounded-md p-0 shadow-2xl"
        onOpenAutoFocus={(event) => event.preventDefault()}
      >
        <DialogHeader className="sr-only">
          <DialogTitle>{t("sessionQuickSwitcher.title")}</DialogTitle>
          <DialogDescription className="sr-only">
            {t("sessionQuickSwitcher.searchPlaceholder")}
          </DialogDescription>
        </DialogHeader>

        <div className="relative border-b">
          <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            ref={searchRef}
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setSelectedIndex(0);
            }}
            onKeyDown={(event) => {
              if (event.key === "ArrowDown") {
                event.preventDefault();
                setSelectedIndex((index) =>
                  filteredItems.length === 0 ? 0 : (index + 1) % filteredItems.length,
                );
              } else if (event.key === "ArrowUp") {
                event.preventDefault();
                setSelectedIndex((index) =>
                  filteredItems.length === 0
                    ? 0
                    : (index - 1 + filteredItems.length) % filteredItems.length,
                );
              } else if (event.key === "Enter") {
                event.preventDefault();
                const selected = filteredItems[selectedIndex];
                if (selected) {
                  selectItem(selected);
                }
              } else if (event.key === "Escape") {
                event.preventDefault();
                onClose();
              }
            }}
            placeholder={t("sessionQuickSwitcher.searchPlaceholder")}
            className="h-11 rounded-none border-0 bg-transparent pl-10 pr-3 text-sm shadow-none focus-visible:ring-0"
          />
        </div>

        <div className="max-h-[min(24rem,55vh)] overflow-y-auto">
          {filteredItems.map((item, index) => {
            const active = item.kind === "session" && item.session.id === activeSessionId;
            const selected = index === selectedIndex;

            return (
              <button
                key={item.id}
                ref={(element) => {
                  if (element) {
                    itemButtonRefs.current.set(item.id, element);
                  } else {
                    itemButtonRefs.current.delete(item.id);
                  }
                }}
                type="button"
                className={`flex w-full items-center gap-3 px-3 py-2 text-left text-xs ${
                  selected ? "bg-primary/15" : "hover:bg-accent/70"
                }`}
                onMouseEnter={() => setSelectedIndex(index)}
                onClick={() => selectItem(item)}
              >
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm font-medium">{item.title}</span>
                  <span className="block truncate text-xs text-muted-foreground">
                    {item.subtitle}
                  </span>
                </span>
                {active ? (
                  <span className="shrink-0 rounded bg-primary/10 px-1.5 py-0.5 text-[0.625rem] text-primary">
                    {t("sessionQuickSwitcher.active")}
                  </span>
                ) : null}
                {item.kind === "connection" ? (
                  <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[0.625rem] text-muted-foreground">
                    {t("sessionQuickSwitcher.saved")}
                  </span>
                ) : null}
              </button>
            );
          })}

          {filteredItems.length === 0 ? (
            <div className="px-3 py-6 text-center text-xs text-muted-foreground">
              {items.length === 0
                ? t("sessionQuickSwitcher.noSessions")
                : t("sessionQuickSwitcher.noMatches")}
            </div>
          ) : null}
        </div>

        <div className="flex items-center justify-between gap-2 border-t px-3 py-2">
          <span className="text-xs text-muted-foreground">
            Enter {t("sessionQuickSwitcher.open")} / Esc {t("sessionQuickSwitcher.close")}
          </span>
          <Button size="sm" className="h-7 px-2 text-xs" onClick={onNewSshSession}>
            {t("sessionQuickSwitcher.newSsh")}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
