import { listen } from "@tauri-apps/api/event";
import { memo, useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdDriveFileRenameOutline,
  MdLinkOff,
  MdMoreHoriz,
  MdRefresh,
  MdSearch,
} from "react-icons/md";
import { toast } from "sonner";
import PanelHeader from "@/components/layout/PanelHeader";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { useApp } from "@/context/AppContext";
import { invoke } from "@/lib/invoke";
import { findTabBySessionId, getTabDisplayName } from "@/lib/workspaceTabs";
import type { SessionInfo } from "@/types/global";

interface ActiveSessionsProps {
  onSessionClick: (sessionId: string) => void;
  onSessionReconnect: (sessionId: string) => Promise<void> | void;
  onSessionDisconnect: (sessionId: string) => Promise<void> | void;
  canReconnect: (sessionId: string) => boolean;
}

/** List of active sessions (polled). Click switches to that session's tab. */
function ActiveSessions({
  onSessionClick,
  onSessionReconnect,
  onSessionDisconnect,
  canReconnect,
}: ActiveSessionsProps) {
  const { t } = useTranslation();
  const { tabs, updateTab } = useApp();
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [search, setSearch] = useState("");
  const [busyActions, setBusyActions] = useState<Record<string, "reconnect" | "disconnect">>({});
  const [renameTabId, setRenameTabId] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");

  const fetchSessions = useCallback(async () => {
    try {
      const sess = await invoke<SessionInfo[]>("list_sessions");
      sess.sort(
        (a, b) =>
          a.name.localeCompare(b.name, undefined, { sensitivity: "base" }) ||
          a.session_type.localeCompare(b.session_type),
      );
      setSessions(sess);
    } catch {
      // Backend might not be ready yet
    }
  }, []);

  const runAction = useCallback(
    async (sessionId: string, action: "reconnect" | "disconnect") => {
      setBusyActions((prev) => ({ ...prev, [sessionId]: action }));
      try {
        if (action === "reconnect") {
          await onSessionReconnect(sessionId);
        } else {
          await onSessionDisconnect(sessionId);
        }
      } finally {
        setBusyActions((prev) => {
          const next = { ...prev };
          delete next[sessionId];
          return next;
        });
      }
    },
    [onSessionDisconnect, onSessionReconnect],
  );

  useEffect(() => {
    fetchSessions();
    const unlisten = listen("sessions-changed", () => {
      fetchSessions();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [fetchSessions]);

  const sessionRows = useMemo(
    () =>
      sessions.map((session) => {
        const tab = findTabBySessionId(tabs, session.id);
        return {
          session,
          tab,
          displayName: tab ? getTabDisplayName(tab) : session.name,
        };
      }),
    [sessions, tabs],
  );

  const query = search.trim().toLowerCase();
  const filteredRows = query
    ? sessionRows.filter(({ session, displayName }) =>
        `${displayName} ${session.name} ${session.session_type} ${session.id}`
          .toLowerCase()
          .includes(query),
      )
    : sessionRows;

  const renameTab = renameTabId ? tabs.find((tab) => tab.id === renameTabId) : null;

  const handleRenameOpen = useCallback((tabId: string, displayName: string) => {
    setRenameTabId(tabId);
    setRenameValue(displayName);
  }, []);

  const handleRenameClose = useCallback(() => {
    setRenameTabId(null);
    setRenameValue("");
  }, []);

  const handleRenameSubmit = useCallback(async () => {
    if (!renameTab) {
      handleRenameClose();
      return;
    }

    const trimmed = renameValue.trim();
    if (!trimmed) {
      toast.error(t("tabCtx.renameEmpty"));
      return;
    }
    if (trimmed.length > 64) {
      return;
    }

    try {
      await updateTab(renameTab.id, { customName: trimmed }, { immediatePersist: true });
      handleRenameClose();
    } catch {
      toast.error(t("tabCtx.renameFailed"));
    }
  }, [handleRenameClose, renameTab, renameValue, t, updateTab]);

  return (
    <div className="h-full flex flex-col overflow-hidden">
      <PanelHeader
        title={t("panel.activeSessions")}
        actions={
          <span className="text-[0.6875rem]" style={{ color: "var(--df-text-dimmed)" }}>
            {query ? `${filteredRows.length}/${sessions.length}` : sessions.length}
          </span>
        }
      />

      <div
        className="nyaterm-wallpaper-transparent-surface border-b px-2 py-1.5"
        style={{ borderColor: "var(--df-border)", backgroundColor: "var(--df-bg-panel)" }}
      >
        <div className="relative shrink-0">
          <MdSearch
            className="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-[0.875rem]"
            style={{ color: "var(--df-text-dimmed)" }}
          />
          <Input
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder={t("activeSessions.searchPlaceholder")}
            className="h-7 border-0 pl-7 text-xs shadow-none placeholder:text-[var(--df-text-dimmed)] text-[var(--df-text)] bg-[var(--df-bg-hover)] focus-visible:ring-1 focus-visible:ring-[var(--df-primary)] focus-visible:bg-transparent"
          />
        </div>
      </div>

      <div className="terminal-scroll flex-1 overflow-y-auto p-2 text-xs space-y-1">
        {sessions.length === 0 ? (
          <div
            className="text-center py-4 text-[0.6875rem]"
            style={{ color: "var(--df-text-dimmed)" }}
          >
            {t("panel.noActiveSessions")}
          </div>
        ) : filteredRows.length === 0 ? (
          <div
            className="text-center py-4 text-[0.6875rem]"
            style={{ color: "var(--df-text-dimmed)" }}
          >
            {t("activeSessions.noMatches")}
          </div>
        ) : (
          filteredRows.map(({ session, tab, displayName }) => (
            <div
              key={session.id}
              className={`flex items-center gap-2 rounded-md p-2 transition-colors df-hover ${!session.connected ? "opacity-50" : ""}`}
              onClick={() => onSessionClick(session.id)}
            >
              <div
                className="w-2 h-2 rounded-full shrink-0"
                style={{ backgroundColor: session.connected ? "#22c55e" : "var(--df-text-dimmed)" }}
              />
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="truncate" style={{ color: "var(--df-text)" }}>
                    {displayName}
                  </span>
                  <span
                    className="rounded px-1.5 py-0.5 text-[0.625rem] uppercase tracking-wide"
                    style={{
                      color: "var(--df-text-dimmed)",
                      backgroundColor: "var(--df-bg-hover)",
                    }}
                  >
                    {session.session_type}
                  </span>
                </div>
                <div
                  className="truncate font-mono text-[0.625rem]"
                  style={{ color: "var(--df-text-dimmed)" }}
                  title={session.id}
                >
                  {session.id}
                </div>
              </div>
              <div className="flex items-center gap-1 shrink-0">
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground disabled:opacity-40"
                  disabled={!tab}
                  onClick={(event) => {
                    event.stopPropagation();
                    if (tab) {
                      handleRenameOpen(tab.id, displayName);
                    }
                  }}
                  aria-label={t("tabCtx.rename")}
                >
                  <MdDriveFileRenameOutline className="h-4 w-4" />
                </Button>
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon"
                      className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground disabled:opacity-40"
                      onClick={(event) => event.stopPropagation()}
                      aria-label={t("common.more")}
                    >
                      <MdMoreHoriz className="h-4 w-4" />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end" className="min-w-40">
                    <DropdownMenuItem
                      disabled={!!busyActions[session.id] || !canReconnect(session.id)}
                      onSelect={() => void runAction(session.id, "reconnect")}
                    >
                      <MdRefresh
                        className={`h-4 w-4 ${
                          busyActions[session.id] === "reconnect" ? "animate-spin" : ""
                        }`}
                      />
                      {t("tabCtx.reconnect")}
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      disabled={!!busyActions[session.id]}
                      variant="destructive"
                      onSelect={() => void runAction(session.id, "disconnect")}
                    >
                      <MdLinkOff className="h-4 w-4" />
                      {t("tabCtx.disconnect")}
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
            </div>
          ))
        )}
      </div>

      <Dialog
        disablePointerDismissal
        open={!!renameTabId}
        onOpenChange={(open) => !open && handleRenameClose()}
      >
        <DialogContent showCloseButton={false} className="max-w-xs">
          <DialogHeader>
            <DialogTitle className="text-sm">{t("tabCtx.renameTitle")}</DialogTitle>
            <DialogDescription className="sr-only">{t("tabCtx.renameTitle")}</DialogDescription>
          </DialogHeader>
          <div className="space-y-2">
            <Input
              className="text-sm"
              value={renameValue}
              onChange={(event) => setRenameValue(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  void handleRenameSubmit();
                }
              }}
              maxLength={64}
              autoFocus
            />
          </div>
          <DialogFooter>
            <Button variant="outline" size="sm" onClick={handleRenameClose}>
              {t("dialog.cancel")}
            </Button>
            <Button
              size="sm"
              onClick={() => void handleRenameSubmit()}
              disabled={!renameValue.trim()}
            >
              {t("dialog.save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

export default memo(ActiveSessions);
