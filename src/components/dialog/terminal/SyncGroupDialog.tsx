import { type ReactNode, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdAdd,
  MdCellTower,
  MdCheckCircle,
  MdDelete,
  MdDoneAll,
  MdGroup,
  MdRadioButtonUnchecked,
  MdSearch,
} from "react-icons/md";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Switch } from "@/components/ui/switch";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { useApp } from "@/context/AppContext";
import {
  addSessionToGroup,
  createSyncGroup,
  nextGroupColor,
  removeSessionFromGroup,
  toggleGroupEnabled,
} from "@/lib/syncInputGroups";
import { cn } from "@/lib/utils";
import { collectSessionPanes } from "@/lib/workspaceTabs";

interface SyncGroupDialogProps {
  open: boolean;
  onClose: () => void;
}

interface LiveSessionInfo {
  sessionId: string;
  name: string;
  type: string;
  connectionId?: string;
  host?: string;
}

export default function SyncGroupDialog({ open, onClose }: SyncGroupDialogProps) {
  const { t } = useTranslation();
  const { syncGroups, setSyncGroups, tabs, savedConnections } = useApp();
  const [selectedGroupId, setSelectedGroupId] = useState<string | null>(null);
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState("");

  const liveSessions = useMemo<LiveSessionInfo[]>(() => {
    const sessions: LiveSessionInfo[] = [];
    const seen = new Set<string>();
    for (const tab of tabs) {
      for (const pane of collectSessionPanes(tab.root)) {
        if (pane.connecting || pane.connectError || seen.has(pane.sessionId)) {
          continue;
        }
        seen.add(pane.sessionId);
        const conn = pane.connectionId
          ? savedConnections.find((c) => c.id === pane.connectionId)
          : undefined;
        sessions.push({
          sessionId: pane.sessionId,
          name: pane.name,
          type: pane.type,
          connectionId: pane.connectionId,
          host: conn?.host,
        });
      }
    }
    return sessions;
  }, [tabs, savedConnections]);

  const liveSessionIds = useMemo(
    () => new Set(liveSessions.map((s) => s.sessionId)),
    [liveSessions],
  );

  const selectedGroup = syncGroups.find((g) => g.id === selectedGroupId) ?? null;

  const filteredSessions = useMemo(() => {
    const query = searchQuery.trim().toLowerCase();

    return liveSessions.filter((session) => {
      if (!query) return true;
      return [session.name, session.type, session.host, session.sessionId]
        .filter(Boolean)
        .some((value) => value?.toLowerCase().includes(query));
    });
  }, [liveSessions, searchQuery]);

  const filteredSessionIds = useMemo(
    () => filteredSessions.map((s) => s.sessionId),
    [filteredSessions],
  );

  useEffect(() => {
    if (!open) return;
    if (syncGroups.length === 0) {
      setSelectedGroupId(null);
      return;
    }
    if (!selectedGroupId || !syncGroups.some((g) => g.id === selectedGroupId)) {
      setSelectedGroupId(syncGroups[0].id);
    }
  }, [open, selectedGroupId, syncGroups]);

  useEffect(() => {
    if (!open) {
      setSearchQuery("");
      setPendingDeleteId(null);
    }
  }, [open]);

  useEffect(() => {
    if (!open || !syncGroups.some((g) => g.pausedSessionIds.length > 0)) return;
    setSyncGroups((prev) =>
      prev.map((g) => (g.pausedSessionIds.length > 0 ? { ...g, pausedSessionIds: [] } : g)),
    );
  }, [open, setSyncGroups, syncGroups]);

  const handleAddGroup = () => {
    const color = nextGroupColor(syncGroups);
    const name = `${t("syncGroup.newGroup")} ${syncGroups.length + 1}`;
    const group = createSyncGroup(name, color);
    setSyncGroups((prev) => [...prev, group]);
    setSelectedGroupId(group.id);
    setSearchQuery("");
  };

  const handleConfirmDelete = () => {
    if (!pendingDeleteId) return;
    setSyncGroups((prev) => prev.filter((g) => g.id !== pendingDeleteId));
    if (selectedGroupId === pendingDeleteId) setSelectedGroupId(null);
    setPendingDeleteId(null);
  };

  const handleToggleEnabled = (id: string) => {
    setSyncGroups((prev) => prev.map((g) => (g.id === id ? toggleGroupEnabled(g) : g)));
  };

  const handleRenameGroup = (id: string, name: string) => {
    setSyncGroups((prev) => prev.map((g) => (g.id === id ? { ...g, name } : g)));
  };

  const handleToggleSession = (groupId: string, sessionId: string) => {
    setSyncGroups((prev) =>
      prev.map((g) => {
        if (g.id !== groupId) return g;
        return g.sessionIds.includes(sessionId)
          ? removeSessionFromGroup(g, sessionId)
          : addSessionToGroup(g, sessionId);
      }),
    );
  };

  const handleSelectAll = () => {
    if (!selectedGroup) return;
    setSyncGroups((prev) =>
      prev.map((g) =>
        g.id === selectedGroup.id
          ? {
              ...g,
              sessionIds: liveSessions.map((s) => s.sessionId),
              pausedSessionIds: [],
            }
          : g,
      ),
    );
  };

  const handleDeselectAll = () => {
    if (!selectedGroup) return;
    setSyncGroups((prev) =>
      prev.map((g) =>
        g.id === selectedGroup.id ? { ...g, sessionIds: [], pausedSessionIds: [] } : g,
      ),
    );
  };

  const handleAddFiltered = () => {
    if (!selectedGroup) return;
    setSyncGroups((prev) =>
      prev.map((g) => {
        if (g.id !== selectedGroup.id) return g;
        const nextIds = new Set(g.sessionIds);
        for (const sessionId of filteredSessionIds) nextIds.add(sessionId);
        return { ...g, sessionIds: [...nextIds], pausedSessionIds: [] };
      }),
    );
  };

  const handleRemoveFiltered = () => {
    if (!selectedGroup) return;
    const removeIds = new Set(filteredSessionIds);
    setSyncGroups((prev) =>
      prev.map((g) =>
        g.id === selectedGroup.id
          ? {
              ...g,
              sessionIds: g.sessionIds.filter((id) => !removeIds.has(id)),
              pausedSessionIds: [],
            }
          : g,
      ),
    );
  };

  const handleSelectSameHost = () => {
    if (!selectedGroup || selectedGroup.sessionIds.length === 0) return;
    const selectedHosts = new Set<string>();
    for (const sid of selectedGroup.sessionIds) {
      const session = liveSessions.find((ls) => ls.sessionId === sid);
      if (session?.host) selectedHosts.add(session.host);
    }
    if (selectedHosts.size === 0) return;
    const matchingSessions = liveSessions
      .filter((s) => s.host && selectedHosts.has(s.host))
      .map((s) => s.sessionId);
    setSyncGroups((prev) =>
      prev.map((g) =>
        g.id === selectedGroup.id
          ? {
              ...g,
              sessionIds: matchingSessions,
              pausedSessionIds: [],
            }
          : g,
      ),
    );
  };

  const pendingDeleteGroup = pendingDeleteId
    ? syncGroups.find((g) => g.id === pendingDeleteId)
    : null;

  return (
    <Dialog disablePointerDismissal open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="w-[min(900px,calc(100vw-24px))] h-[min(500px,calc(100vh-56px))] flex flex-col gap-0 p-0 overflow-hidden">
        <DialogHeader className="px-5 py-4 border-b shrink-0">
          <div className="min-w-0">
            <DialogTitle className="flex items-center gap-2 text-base">
              <MdCellTower className="text-lg text-primary" />
              {t("syncGroup.title")}
            </DialogTitle>
            <DialogDescription className="sr-only">{t("syncGroup.description")}</DialogDescription>
          </div>
        </DialogHeader>

        <div className="flex flex-1 min-h-0 overflow-hidden">
          <aside className="w-52 shrink-0 flex flex-col border-r bg-muted/20">
            <div className="flex items-center justify-between gap-2 px-3 py-3 border-b">
              <div>
                <div className="text-xs font-semibold uppercase text-muted-foreground">
                  {t("syncGroup.groups")}
                </div>
                <div className="text-[11px] text-muted-foreground">{syncGroups.length}</div>
              </div>
              <TooltipProvider delayDuration={300}>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      variant="outline"
                      size="icon-xs"
                      onClick={handleAddGroup}
                      aria-label={t("syncGroup.newGroup")}
                    >
                      <MdAdd />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="right" className="text-xs">
                    {t("syncGroup.newGroup")}
                  </TooltipContent>
                </Tooltip>
              </TooltipProvider>
            </div>

            <ScrollArea className="flex-1 min-h-0">
              <div className="flex flex-col gap-1 p-2">
                {syncGroups.length === 0 && (
                  <div className="px-3 py-8 text-center">
                    <MdGroup className="mx-auto mb-2 text-2xl text-muted-foreground/50" />
                    <p className="text-xs leading-relaxed text-muted-foreground">
                      {t("syncGroup.noGroups")}
                    </p>
                  </div>
                )}
                {syncGroups.map((group) => {
                  const isSelected = selectedGroupId === group.id;
                  const memberLiveCount = group.sessionIds.filter((id) =>
                    liveSessionIds.has(id),
                  ).length;

                  return (
                    <button
                      key={group.id}
                      type="button"
                      className={cn(
                        "flex w-full items-center gap-2.5 rounded-md border px-3 py-2 text-left transition-colors",
                        isSelected
                          ? "border-primary/30 bg-primary/10 text-foreground"
                          : "border-transparent hover:border-border hover:bg-background/70",
                      )}
                      onClick={() => setSelectedGroupId(group.id)}
                    >
                      <span
                        className="h-9 w-1.5 rounded-full shrink-0"
                        style={{ backgroundColor: group.color }}
                      />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-sm font-medium">
                          {group.name || t("syncGroup.unnamedGroup")}
                        </span>
                        <span className="mt-0.5 flex items-center gap-1.5 text-[11px] text-muted-foreground">
                          <span>
                            {t("syncGroup.sessionCount", {
                              count: memberLiveCount,
                            })}
                          </span>
                        </span>
                      </span>
                      <span
                        className={cn(
                          "h-2 w-2 rounded-full",
                          group.enabled ? "bg-emerald-500" : "bg-muted-foreground/40",
                        )}
                      />
                    </button>
                  );
                })}
              </div>
            </ScrollArea>
          </aside>

          <section className="flex-1 flex flex-col min-h-0 min-w-0">
            {selectedGroup ? (
              <>
                <div className="shrink-0 border-b px-4 py-3">
                  <div className="flex items-center gap-2">
                    <div
                      className="h-8 w-1.5 rounded-full shrink-0"
                      style={{ backgroundColor: selectedGroup.color }}
                    />
                    <Input
                      value={selectedGroup.name}
                      onChange={(e) => handleRenameGroup(selectedGroup.id, e.target.value)}
                      aria-label={t("syncGroup.groupName")}
                      className="h-8 min-w-0 flex-1 bg-background/60 text-sm font-medium"
                    />
                    <div className="flex h-8 items-center gap-2 rounded-md border bg-muted/20 px-2">
                      <span className="text-xs text-muted-foreground">
                        {t("syncGroup.enabled")}
                      </span>
                      <Switch
                        checked={selectedGroup.enabled}
                        onCheckedChange={() => handleToggleEnabled(selectedGroup.id)}
                      />
                    </div>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      aria-label={t("syncGroup.deleteGroup")}
                      className="text-destructive hover:text-destructive"
                      onClick={() => setPendingDeleteId(selectedGroup.id)}
                    >
                      <MdDelete />
                    </Button>
                  </div>
                </div>

                <div className="flex shrink-0 items-center gap-2 border-b px-4 py-2.5">
                  <div className="relative min-w-[220px] flex-1">
                    <MdSearch className="absolute left-3 top-1/2 -translate-y-1/2 text-base text-muted-foreground" />
                    <Input
                      value={searchQuery}
                      onChange={(e) => setSearchQuery(e.target.value)}
                      placeholder={t("syncGroup.searchPlaceholder")}
                      className="h-8 pl-9 text-sm"
                    />
                  </div>
                </div>

                <ScrollArea className="flex-1 min-h-0">
                  <div className="flex flex-col gap-1 p-3">
                    {liveSessions.length === 0 ? (
                      <EmptyState icon={<MdCellTower />} text={t("syncGroup.noSessions")} />
                    ) : filteredSessions.length === 0 ? (
                      <EmptyState icon={<MdSearch />} text={t("syncGroup.noSessionMatches")} />
                    ) : (
                      filteredSessions.map((session) => {
                        const isMember = selectedGroup.sessionIds.includes(session.sessionId);
                        const otherGroups = syncGroups.filter(
                          (g) =>
                            g.id !== selectedGroup.id && g.sessionIds.includes(session.sessionId),
                        );

                        return (
                          <div
                            key={session.sessionId}
                            className={cn(
                              "group flex items-center gap-3 rounded-md border px-3 py-2.5 transition-colors",
                              isMember
                                ? "border-primary/25 bg-primary/10"
                                : "border-transparent hover:border-border hover:bg-muted/25",
                            )}
                          >
                            <Checkbox
                              checked={isMember}
                              onCheckedChange={() =>
                                handleToggleSession(selectedGroup.id, session.sessionId)
                              }
                              aria-label={session.name}
                            />
                            <div className="min-w-0 flex-1">
                              <div className="flex min-w-0 items-center gap-2">
                                <span className="truncate text-sm font-medium">{session.name}</span>
                                {isMember && (
                                  <MdCheckCircle
                                    className="shrink-0 text-sm"
                                    style={{ color: selectedGroup.color }}
                                  />
                                )}
                              </div>
                              <div className="mt-0.5 flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
                                <span className="shrink-0">{session.type}</span>
                                {session.host && (
                                  <span className="truncate font-mono">{session.host}</span>
                                )}
                                {otherGroups.length > 0 && (
                                  <span className="truncate text-amber-500">
                                    {t("syncGroup.alsoInGroups", {
                                      names: otherGroups.map((g) => g.name).join(", "),
                                    })}
                                  </span>
                                )}
                              </div>
                            </div>
                          </div>
                        );
                      })
                    )}
                  </div>
                </ScrollArea>

                <div className="shrink-0 border-t px-4 py-2.5">
                  <div className="flex flex-wrap items-center gap-2">
                    <Button variant="outline" size="sm" onClick={handleSelectAll}>
                      <MdDoneAll />
                      {t("syncGroup.selectAll")}
                    </Button>
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={handleAddFiltered}
                      disabled={filteredSessions.length === 0}
                    >
                      <MdAdd />
                      {t("syncGroup.addFiltered")}
                    </Button>
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={handleRemoveFiltered}
                      disabled={filteredSessions.length === 0}
                    >
                      <MdRadioButtonUnchecked />
                      {t("syncGroup.removeFiltered")}
                    </Button>
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={handleSelectSameHost}
                      disabled={
                        !selectedGroup.sessionIds.length || !liveSessions.some((s) => s.host)
                      }
                    >
                      {t("syncGroup.selectSameHost")}
                    </Button>
                    <Button
                      className="ml-auto"
                      variant="ghost"
                      size="sm"
                      onClick={handleDeselectAll}
                      disabled={selectedGroup.sessionIds.length === 0}
                    >
                      {t("syncGroup.deselectAll")}
                    </Button>
                  </div>
                </div>
              </>
            ) : (
              <div className="flex-1 flex items-center justify-center">
                <div className="max-w-sm px-8 text-center">
                  <MdCellTower className="mx-auto mb-3 text-4xl text-muted-foreground/30" />
                  <p className="text-sm text-muted-foreground">
                    {syncGroups.length === 0
                      ? t("syncGroup.noGroups")
                      : t("syncGroup.noGroupSelected")}
                  </p>
                  <Button className="mt-4" size="sm" onClick={handleAddGroup}>
                    <MdAdd />
                    {t("syncGroup.newGroup")}
                  </Button>
                </div>
              </div>
            )}
          </section>
        </div>
      </DialogContent>

      <Dialog open={!!pendingDeleteId} onOpenChange={(o) => !o && setPendingDeleteId(null)}>
        <DialogContent className="max-w-sm">
          <DialogHeader>
            <DialogTitle>{t("syncGroup.deleteGroup")}</DialogTitle>
            <DialogDescription>
              {pendingDeleteGroup
                ? t("syncGroup.deleteGroupConfirm", {
                    name: pendingDeleteGroup.name,
                  })
                : ""}
            </DialogDescription>
          </DialogHeader>
          <div className="flex justify-end gap-2 pt-2">
            <Button variant="outline" size="sm" onClick={() => setPendingDeleteId(null)}>
              {t("common.cancel")}
            </Button>
            <Button variant="destructive" size="sm" onClick={handleConfirmDelete}>
              {t("syncGroup.deleteGroup")}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </Dialog>
  );
}

function EmptyState({ icon, text }: { icon: ReactNode; text: string }) {
  return (
    <div className="flex flex-col items-center justify-center px-4 py-12 text-center text-muted-foreground">
      <div className="mb-2 text-3xl opacity-40">{icon}</div>
      <p className="text-sm">{text}</p>
    </div>
  );
}
