import { listen } from "@tauri-apps/api/event";
import { ChevronDownIcon, FolderPlusIcon, MoreHorizontalIcon } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdAdd, MdDelete, MdDriveFileMove, MdEdit, MdLan, MdRouter } from "react-icons/md";
import { toast } from "sonner";
import PanelHeader from "@/components/layout/PanelHeader";
import {
  buildGroupPath,
  type ConnectionOption,
  EmptyState,
  sortLabel,
} from "@/components/network/shared";
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
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useApp } from "@/context/AppContext";
import { invoke } from "@/lib/invoke";
import { cn } from "@/lib/utils";
import { openProxyConfig, openTunnelConfig } from "@/lib/windowManager";
import type {
  NetworkGroup,
  ProxyConfig,
  TunnelConfig,
  TunnelRuntimeState,
  TunnelRuntimeStatus,
} from "@/types/global";

type NetworkTab = "proxy" | "tunnel";
type GroupDialogState = { tab: NetworkTab; group: NetworkGroup | null } | null;
type DeleteGroupState = { tab: NetworkTab; group: NetworkGroup; itemCount: number } | null;
type GroupedSection<T> = {
  id: string;
  label: string;
  group: NetworkGroup | null;
  items: T[];
};

const UNGROUPED_ID = "__ungrouped__";

function sortNetworkGroups(groups: NetworkGroup[]) {
  return [...groups].sort((left, right) => {
    if (left.sort_order !== right.sort_order) return left.sort_order - right.sort_order;
    return sortLabel(left.name, right.name);
  });
}

function buildGroupedSections<T extends { group_id?: string }>(
  items: T[],
  groups: NetworkGroup[],
  ungroupedLabel: string,
): GroupedSection<T>[] {
  const sortedGroups = sortNetworkGroups(groups);
  const validGroupIds = new Set(sortedGroups.map((group) => group.id));
  const groupedItems = new Map<string, T[]>();
  const ungrouped: T[] = [];

  for (const item of items) {
    const groupId = item.group_id;
    if (groupId && validGroupIds.has(groupId)) {
      const groupItems = groupedItems.get(groupId);
      if (groupItems) groupItems.push(item);
      else groupedItems.set(groupId, [item]);
    } else {
      ungrouped.push(item);
    }
  }

  return [
    ...sortedGroups.map((group) => ({
      id: group.id,
      label: group.name,
      group,
      items: groupedItems.get(group.id) ?? [],
    })),
    {
      id: UNGROUPED_ID,
      label: ungroupedLabel,
      group: null,
      items: ungrouped,
    },
  ].filter((section) => section.group || section.items.length > 0);
}

function MoveGroupMenu({
  groups,
  currentGroupId,
  onMove,
}: {
  groups: NetworkGroup[];
  currentGroupId?: string;
  onMove: (groupId: string | undefined) => void;
}) {
  const { t } = useTranslation();
  return (
    <DropdownMenuSub>
      <DropdownMenuSubTrigger>
        <MdDriveFileMove className="text-base" />
        {t("network.moveToGroup")}
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent>
        <DropdownMenuItem disabled={!currentGroupId} onClick={() => onMove(undefined)}>
          {t("network.ungrouped")}
        </DropdownMenuItem>
        {groups.length > 0 ? <DropdownMenuSeparator /> : null}
        {sortNetworkGroups(groups).map((group) => (
          <DropdownMenuItem
            key={group.id}
            disabled={currentGroupId === group.id}
            onClick={() => onMove(group.id)}
          >
            {group.name}
          </DropdownMenuItem>
        ))}
      </DropdownMenuSubContent>
    </DropdownMenuSub>
  );
}

function ProxyRow({
  proxy,
  groups,
  onEdit,
  onDelete,
  onMoveGroup,
}: {
  proxy: ProxyConfig;
  groups: NetworkGroup[];
  onEdit: (proxy: ProxyConfig) => void;
  onDelete: (id: string) => void;
  onMoveGroup: (proxy: ProxyConfig, groupId: string | undefined) => void;
}) {
  const { t } = useTranslation();
  const address = `${proxy.host}:${proxy.port}`;
  const isProxyCommand = proxy.protocol === "proxycommand";

  return (
    <div className="flex items-center gap-3 px-3 py-2.5 transition-colors hover:bg-accent">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <div className="truncate text-sm font-medium" style={{ color: "var(--df-text)" }}>
            {proxy.name}
          </div>
        </div>
        <div className="mt-0.5 truncate text-xs" style={{ color: "var(--df-text-dimmed)" }}>
          {proxy.protocol.toUpperCase()}
        </div>
        <div className="mt-0.5 text-[0.6875rem]" style={{ color: "var(--df-text-muted)" }}>
          {isProxyCommand
            ? proxy.command
            : proxy.username
              ? `${proxy.username}@${address}`
              : address}
        </div>
      </div>

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="icon-sm">
            <MoreHorizontalIcon className="size-4" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem onClick={() => onEdit(proxy)}>
            <MdEdit className="mr-2 text-base" />
            {t("common.edit")}
          </DropdownMenuItem>
          <MoveGroupMenu
            groups={groups}
            currentGroupId={proxy.group_id}
            onMove={(groupId) => onMoveGroup(proxy, groupId)}
          />
          <DropdownMenuSeparator />
          <DropdownMenuItem
            className="text-destructive focus:text-destructive"
            onClick={() => onDelete(proxy.id)}
          >
            <MdDelete className="mr-2 text-base" />
            {t("common.delete")}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

function TunnelRow({
  tunnel,
  runtimeState,
  groups,
  connectionOption,
  onEdit,
  onDelete,
  onToggle,
  onMoveGroup,
}: {
  tunnel: TunnelConfig;
  runtimeState?: TunnelRuntimeState;
  groups: NetworkGroup[];
  connectionOption?: ConnectionOption;
  onEdit: (tunnel: TunnelConfig) => void;
  onDelete: (id: string) => void;
  onToggle: (id: string, open: boolean) => void;
  onMoveGroup: (tunnel: TunnelConfig, groupId: string | undefined) => void;
}) {
  const { t } = useTranslation();
  const typeLabel =
    {
      local: t("network.localTunnel"),
      remote: t("network.remoteTunnel"),
      dynamic: t("network.dynamicTunnel"),
    }[tunnel.tunnel_type] ?? tunnel.tunnel_type;

  const endpoint =
    tunnel.tunnel_type === "dynamic"
      ? `SOCKS5 · ${tunnel.listen_port}`
      : `${tunnel.listen_port} -> ${tunnel.target_host}:${tunnel.target_port}`;

  return (
    <div className="flex items-center gap-3 px-3 py-2.5 transition-colors hover:bg-accent">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <div
            className="min-w-0 flex-1 truncate text-sm font-medium"
            style={{ color: "var(--df-text)" }}
          >
            {tunnel.name || endpoint}
          </div>
          <TunnelRuntimeBadge state={runtimeState} enabled={tunnel.is_open} />
        </div>
        <div className="mt-0.5 truncate text-xs" style={{ color: "var(--df-text-dimmed)" }}>
          {connectionOption?.connection.name ?? t("network.connectionMissing")} · {typeLabel}
        </div>
        <div className="mt-0.5 text-[0.6875rem]" style={{ color: "var(--df-text-muted)" }}>
          {endpoint}
        </div>
      </div>

      <Switch
        checked={tunnel.is_open}
        onCheckedChange={(checked) => onToggle(tunnel.id, checked)}
      />

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="icon-sm">
            <MoreHorizontalIcon className="size-4" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem onClick={() => onEdit(tunnel)}>
            <MdEdit className="mr-2 text-base" />
            {t("common.edit")}
          </DropdownMenuItem>
          <MoveGroupMenu
            groups={groups}
            currentGroupId={tunnel.group_id}
            onMove={(groupId) => onMoveGroup(tunnel, groupId)}
          />
          <DropdownMenuSeparator />
          <DropdownMenuItem
            className="text-destructive focus:text-destructive"
            onClick={() => onDelete(tunnel.id)}
          >
            <MdDelete className="mr-2 text-base" />
            {t("common.delete")}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

function getTunnelRuntimeStatus(enabled: boolean, state?: TunnelRuntimeState): TunnelRuntimeStatus {
  if (state?.status) return state.status;
  return enabled ? "disconnected" : "stopped";
}

function TunnelRuntimeBadge({ state, enabled }: { state?: TunnelRuntimeState; enabled: boolean }) {
  const { t } = useTranslation();
  const status = getTunnelRuntimeStatus(enabled, state);
  const label =
    {
      stopped: t("network.tunnelStatusStopped"),
      starting: t("network.tunnelStatusStarting"),
      running: t("network.tunnelStatusRunning"),
      reconnecting: t("network.tunnelStatusReconnecting"),
      disconnected: t("network.tunnelStatusDisconnected"),
      error: t("network.tunnelStatusError"),
    }[status] ?? status;
  const className =
    {
      stopped: "bg-muted text-muted-foreground",
      starting: "bg-sky-500/10 text-sky-600 dark:text-sky-400",
      running: "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400",
      reconnecting: "bg-sky-500/10 text-sky-600 dark:text-sky-400",
      disconnected: "bg-amber-500/10 text-amber-600 dark:text-amber-400",
      error: "bg-destructive/10 text-destructive",
    }[status] ?? "bg-muted text-muted-foreground";
  const badge = (
    <span
      className={cn(
        "shrink-0 whitespace-nowrap rounded-full px-2 py-0.5 text-[0.625rem] font-medium",
        className,
      )}
    >
      {label}
    </span>
  );

  if (status !== "error" || !state?.error) return badge;

  return (
    <Tooltip>
      <TooltipTrigger asChild>{badge}</TooltipTrigger>
      <TooltipContent className="max-w-72 break-words">{state.error}</TooltipContent>
    </Tooltip>
  );
}

function NetworkGroupSection<T>({
  section,
  collapsed,
  onToggle,
  onRename,
  onDelete,
  children,
}: {
  section: GroupedSection<T>;
  collapsed: boolean;
  onToggle: () => void;
  onRename: (group: NetworkGroup) => void;
  onDelete: (group: NetworkGroup, itemCount: number) => void;
  children: ReactNode;
}) {
  const { t } = useTranslation();
  return (
    <div className="overflow-hidden rounded-md border">
      <div
        className="flex items-center gap-2 px-3 py-2"
        style={{ backgroundColor: "color-mix(in srgb, var(--df-bg-hover) 55%, transparent)" }}
      >
        <button
          type="button"
          className="flex min-w-0 flex-1 items-center gap-2 text-left"
          onClick={onToggle}
        >
          <ChevronDownIcon
            className={cn("size-4 shrink-0 transition-transform", collapsed ? "-rotate-90" : "")}
            style={{ color: "var(--df-text-dimmed)" }}
          />
          <span className="truncate text-xs font-medium" style={{ color: "var(--df-text)" }}>
            {section.label}
          </span>
          <span
            className="rounded-full px-1.5 py-0.5 text-[0.625rem]"
            style={{
              color: "var(--df-text-dimmed)",
              backgroundColor: "color-mix(in srgb, var(--df-text-muted) 12%, transparent)",
            }}
          >
            {section.items.length}
          </span>
        </button>
        {section.group ? (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon-sm">
                <MoreHorizontalIcon className="size-4" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onClick={() => onRename(section.group!)}>
                <MdEdit className="mr-2 text-base" />
                {t("network.renameGroup")}
              </DropdownMenuItem>
              <DropdownMenuItem
                className="text-destructive focus:text-destructive"
                onClick={() => onDelete(section.group!, section.items.length)}
              >
                <MdDelete className="mr-2 text-base" />
                {t("network.deleteGroup")}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        ) : null}
      </div>
      {!collapsed ? (
        section.items.length > 0 ? (
          <div>{children}</div>
        ) : (
          <div className="px-3 py-3 text-xs" style={{ color: "var(--df-text-dimmed)" }}>
            {t("network.groupEmpty")}
          </div>
        )
      ) : null}
    </div>
  );
}

function GroupNameDialog({
  state,
  saving,
  onOpenChange,
  onSave,
}: {
  state: GroupDialogState;
  saving: boolean;
  onOpenChange: (open: boolean) => void;
  onSave: (name: string) => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    if (!state) return;
    setName(state.group?.name ?? "");
    setError("");
  }, [state]);

  const handleSubmit = () => {
    const trimmed = name.trim();
    if (!trimmed) {
      setError(t("network.groupNameRequired"));
      return;
    }
    onSave(trimmed);
  };

  return (
    <Dialog disablePointerDismissal open={!!state} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[420px]">
        <DialogHeader>
          <DialogTitle>
            {state?.group ? t("network.renameGroup") : t("network.newGroup")}
          </DialogTitle>
          <DialogDescription>{t("network.groupDialogDescription")}</DialogDescription>
        </DialogHeader>
        <div className="space-y-1.5">
          <Label className="text-sm">{t("network.groupName")}</Label>
          <Input
            className="h-9 text-sm"
            value={name}
            autoFocus
            onChange={(event) => {
              setName(event.target.value);
              setError("");
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                handleSubmit();
              }
            }}
          />
          {error ? <div className="text-xs text-destructive">{error}</div> : null}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={saving}>
            {t("common.cancel")}
          </Button>
          <Button onClick={handleSubmit} disabled={saving}>
            {saving ? t("common.saving") : t("common.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function DeleteNetworkGroupDialog({
  state,
  onOpenChange,
  onConfirm,
}: {
  state: DeleteGroupState;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}) {
  const { t } = useTranslation();
  return (
    <AlertDialog open={!!state} onOpenChange={onOpenChange}>
      <AlertDialogContent size="sm">
        <AlertDialogHeader>
          <AlertDialogTitle>{t("network.deleteGroup")}</AlertDialogTitle>
          <AlertDialogDescription>
            {t("network.deleteGroupConfirm", {
              name: state?.group.name ?? "",
              count: state?.itemCount ?? 0,
            })}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
          <AlertDialogAction variant="destructive" onClick={onConfirm}>
            {t("common.delete")}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

export default function NetworkPanel() {
  const { t } = useTranslation();
  const { appSettings, savedConnections, savedGroups, updateUi } = useApp();
  const activeTab: NetworkTab =
    appSettings.ui.network_panel_active_tab === "proxy" ? "proxy" : "tunnel";
  const [expandedSections, setExpandedSections] = useState<Set<string>>(new Set());

  const [proxies, setProxies] = useState<ProxyConfig[]>([]);
  const [proxyGroups, setProxyGroups] = useState<NetworkGroup[]>([]);

  const [tunnels, setTunnels] = useState<TunnelConfig[]>([]);
  const [tunnelRuntimeStates, setTunnelRuntimeStates] = useState<
    Record<string, TunnelRuntimeState>
  >({});
  const [tunnelGroups, setTunnelGroups] = useState<NetworkGroup[]>([]);

  const [groupDialog, setGroupDialog] = useState<GroupDialogState>(null);
  const [groupSaving, setGroupSaving] = useState(false);
  const [deleteGroupState, setDeleteGroupState] = useState<DeleteGroupState>(null);

  const groupsById = useMemo(
    () => new Map(savedGroups.map((group) => [group.id, group])),
    [savedGroups],
  );

  const connectionOptions = useMemo<ConnectionOption[]>(() => {
    return [...savedConnections]
      .filter((connection) => connection.type === "ssh")
      .map((connection) => {
        const groupPath = buildGroupPath(connection.group_id, groupsById);
        const subtitle = groupPath
          ? `${groupPath} · ${connection.host}:${connection.port}`
          : `${connection.host}:${connection.port}`;

        return {
          connection,
          groupPath,
          subtitle,
          searchText: [connection.name, connection.host, connection.username, groupPath]
            .filter(Boolean)
            .join(" "),
          disabled: false,
        };
      })
      .sort((left, right) => {
        const pathSort = sortLabel(left.groupPath, right.groupPath);
        return pathSort !== 0 ? pathSort : sortLabel(left.connection.name, right.connection.name);
      });
  }, [groupsById, savedConnections]);

  const connectionOptionMap = useMemo(
    () => new Map(connectionOptions.map((option) => [option.connection.id, option])),
    [connectionOptions],
  );

  const proxySections = useMemo(
    () => buildGroupedSections(proxies, proxyGroups, t("network.ungrouped")),
    [proxies, proxyGroups, t],
  );
  const tunnelSections = useMemo(
    () => buildGroupedSections(tunnels, tunnelGroups, t("network.ungrouped")),
    [tunnels, tunnelGroups, t],
  );

  const toggleSection = useCallback((id: string) => {
    setExpandedSections((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const loadProxyGroups = useCallback(async () => {
    try {
      const next = await invoke<NetworkGroup[]>("get_proxy_groups");
      setProxyGroups(next);
    } catch (error) {
      toast.error(String(error));
    }
  }, []);

  const loadTunnelGroups = useCallback(async () => {
    try {
      const next = await invoke<NetworkGroup[]>("get_tunnel_groups");
      setTunnelGroups(next);
    } catch (error) {
      toast.error(String(error));
    }
  }, []);

  const loadProxies = useCallback(async () => {
    try {
      const next = await invoke<ProxyConfig[]>("get_proxies");
      setProxies(next);
    } catch (error) {
      toast.error(String(error));
    }
  }, []);

  const loadTunnels = useCallback(async () => {
    try {
      const next = await invoke<TunnelConfig[]>("get_tunnels");
      setTunnels(next);
    } catch (error) {
      toast.error(String(error));
    }
  }, []);

  const loadTunnelRuntimeStates = useCallback(async () => {
    try {
      const next = await invoke<TunnelRuntimeState[]>("get_tunnel_runtime_states");
      setTunnelRuntimeStates(Object.fromEntries(next.map((state) => [state.tunnelId, state])));
    } catch (error) {
      toast.error(String(error));
    }
  }, []);

  useEffect(() => {
    void loadProxies();
    void loadProxyGroups();
  }, [loadProxies, loadProxyGroups]);

  useEffect(() => {
    void loadTunnels();
    void loadTunnelRuntimeStates();
    void loadTunnelGroups();
  }, [loadTunnels, loadTunnelGroups, loadTunnelRuntimeStates]);

  useEffect(() => {
    const unlistenRuntimePromise = listen<TunnelRuntimeState>(
      "tunnel-runtime-state-changed",
      (event) => {
        setTunnelRuntimeStates((prev) => ({
          ...prev,
          [event.payload.tunnelId]: event.payload,
        }));
      },
    );
    const unlistenProxySavedPromise = listen("proxy-saved", () => {
      void loadProxies();
      void loadProxyGroups();
    });
    const unlistenTunnelSavedPromise = listen("tunnel-saved", () => {
      void loadTunnels();
      void loadTunnelRuntimeStates();
      void loadTunnelGroups();
    });

    return () => {
      unlistenRuntimePromise.then((unlisten) => unlisten());
      unlistenProxySavedPromise.then((unlisten) => unlisten());
      unlistenTunnelSavedPromise.then((unlisten) => unlisten());
    };
  }, [loadProxies, loadProxyGroups, loadTunnelGroups, loadTunnelRuntimeStates, loadTunnels]);

  const handleDeleteProxy = useCallback(
    async (proxyId: string) => {
      try {
        await invoke("delete_proxy", { proxyId });
        await loadProxies();
      } catch (error) {
        toast.error(String(error));
      }
    },
    [loadProxies],
  );

  const handleDeleteTunnel = useCallback(
    async (tunnelId: string) => {
      try {
        await invoke("delete_tunnel", { tunnelId });
        await Promise.all([loadTunnels(), loadTunnelRuntimeStates()]);
      } catch (error) {
        toast.error(String(error));
      }
    },
    [loadTunnelRuntimeStates, loadTunnels],
  );

  const handleMoveProxyGroup = useCallback(
    async (proxy: ProxyConfig, groupId: string | undefined) => {
      try {
        await invoke("set_proxy_group", { proxyId: proxy.id, groupId });
        await loadProxies();
      } catch (error) {
        toast.error(String(error));
      }
    },
    [loadProxies],
  );

  const handleMoveTunnelGroup = useCallback(
    async (tunnel: TunnelConfig, groupId: string | undefined) => {
      try {
        await invoke("set_tunnel_group", { tunnelId: tunnel.id, groupId });
        await Promise.all([loadTunnels(), loadTunnelRuntimeStates()]);
      } catch (error) {
        toast.error(String(error));
      }
    },
    [loadTunnelRuntimeStates, loadTunnels],
  );

  const handleToggleTunnel = useCallback(
    async (tunnelId: string, open: boolean) => {
      try {
        await invoke(open ? "open_tunnel" : "close_tunnel", { tunnelId });
      } catch (error) {
        toast.error(String(error));
      } finally {
        await Promise.all([loadTunnels(), loadTunnelRuntimeStates()]);
      }
    },
    [loadTunnelRuntimeStates, loadTunnels],
  );

  const handleSaveGroup = useCallback(
    async (name: string) => {
      if (!groupDialog) return;
      setGroupSaving(true);
      try {
        const groups = groupDialog.tab === "proxy" ? proxyGroups : tunnelGroups;
        const group = groupDialog.group
          ? { ...groupDialog.group, name }
          : { id: crypto.randomUUID(), name, sort_order: groups.length };
        await invoke(groupDialog.tab === "proxy" ? "save_proxy_group" : "save_tunnel_group", {
          group,
        });
        if (groupDialog.tab === "proxy") await loadProxyGroups();
        else await loadTunnelGroups();
        setGroupDialog(null);
      } catch (error) {
        toast.error(String(error));
      } finally {
        setGroupSaving(false);
      }
    },
    [groupDialog, loadProxyGroups, loadTunnelGroups, proxyGroups, tunnelGroups],
  );

  const handleConfirmDeleteGroup = useCallback(async () => {
    if (!deleteGroupState) return;
    try {
      await invoke(
        deleteGroupState.tab === "proxy" ? "delete_proxy_group" : "delete_tunnel_group",
        {
          groupId: deleteGroupState.group.id,
        },
      );
      if (deleteGroupState.tab === "proxy") {
        await Promise.all([loadProxyGroups(), loadProxies()]);
      } else {
        await Promise.all([loadTunnelGroups(), loadTunnels(), loadTunnelRuntimeStates()]);
      }
      setDeleteGroupState(null);
    } catch (error) {
      toast.error(String(error));
    }
  }, [
    deleteGroupState,
    loadProxies,
    loadProxyGroups,
    loadTunnelRuntimeStates,
    loadTunnels,
    loadTunnelGroups,
  ]);

  return (
    <aside
      className="nyaterm-wallpaper-transparent-surface flex h-full flex-col overflow-hidden"
      style={{ backgroundColor: "var(--df-bg-panel)" }}
    >
      <PanelHeader
        title={t("panel.network")}
        actions={
          <span className="text-[0.6875rem]" style={{ color: "var(--df-text-dimmed)" }}>
            {activeTab === "tunnel" ? tunnels.length : proxies.length}
          </span>
        }
      />

      <div className="flex-1 overflow-y-auto p-3 terminal-scroll">
        <Tabs
          value={activeTab}
          onValueChange={(value) => {
            updateUi({ network_panel_active_tab: value === "proxy" ? "proxy" : "tunnel" });
          }}
          className="w-full"
        >
          <TabsList className="grid h-8 w-full grid-cols-2">
            <TabsTrigger value="tunnel" className="text-xs">
              {t("network.tunnels")}
            </TabsTrigger>
            <TabsTrigger value="proxy" className="text-xs">
              {t("network.proxy")}
            </TabsTrigger>
          </TabsList>

          <TabsContent value="tunnel" className="mt-3">
            <div className="space-y-2">
              <div className="flex items-center justify-between gap-2">
                <Label className="font-medium text-sm">{t("network.tunnelConfig")}</Label>
                <div className="flex items-center gap-1">
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    title={t("network.newGroup")}
                    onClick={() => setGroupDialog({ tab: "tunnel", group: null })}
                  >
                    <FolderPlusIcon className="size-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-7 px-2 text-primary text-xs"
                    onClick={() => {
                      void openTunnelConfig().catch((error) => toast.error(String(error)));
                    }}
                    disabled={connectionOptions.length === 0}
                    title={
                      connectionOptions.length === 0 ? t("network.bindConnectionFirst") : undefined
                    }
                  >
                    <MdAdd className="text-base mr-1" />
                    {t("network.newTunnel")}
                  </Button>
                </div>
              </div>

              {tunnels.length === 0 && tunnelGroups.length === 0 ? (
                <div className="overflow-hidden rounded-md border">
                  <EmptyState
                    icon={MdLan}
                    title={
                      connectionOptions.length === 0
                        ? t("network.noConnections")
                        : t("network.noTunnels")
                    }
                    description={
                      connectionOptions.length === 0
                        ? t("network.noConnectionsHint")
                        : t("network.tunnelEmptyHint")
                    }
                  />
                </div>
              ) : (
                <div className="space-y-2">
                  {tunnelSections.map((section) => (
                    <NetworkGroupSection
                      key={section.id}
                      section={section}
                      collapsed={!expandedSections.has(`tunnel:${section.id}`)}
                      onToggle={() => toggleSection(`tunnel:${section.id}`)}
                      onRename={(group) => setGroupDialog({ tab: "tunnel", group })}
                      onDelete={(group, itemCount) =>
                        setDeleteGroupState({ tab: "tunnel", group, itemCount })
                      }
                    >
                      {section.items.map((tunnel, index) => (
                        <div
                          key={tunnel.id}
                          className={cn(index < section.items.length - 1 ? "border-b" : undefined)}
                        >
                          <TunnelRow
                            tunnel={tunnel}
                            runtimeState={tunnelRuntimeStates[tunnel.id]}
                            groups={tunnelGroups}
                            connectionOption={
                              tunnel.connection_id
                                ? connectionOptionMap.get(tunnel.connection_id)
                                : undefined
                            }
                            onEdit={(item) => {
                              void openTunnelConfig(item.id).catch((error) =>
                                toast.error(String(error)),
                              );
                            }}
                            onDelete={handleDeleteTunnel}
                            onToggle={handleToggleTunnel}
                            onMoveGroup={handleMoveTunnelGroup}
                          />
                        </div>
                      ))}
                    </NetworkGroupSection>
                  ))}
                </div>
              )}
            </div>
          </TabsContent>

          <TabsContent value="proxy" className="mt-3">
            <div className="space-y-2">
              <div className="flex items-center justify-between gap-2">
                <Label className="font-medium text-sm">{t("network.proxyConfig")}</Label>
                <div className="flex items-center gap-1">
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    title={t("network.newGroup")}
                    onClick={() => setGroupDialog({ tab: "proxy", group: null })}
                  >
                    <FolderPlusIcon className="size-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-7 px-2 text-primary text-xs"
                    onClick={() => {
                      void openProxyConfig().catch((error) => toast.error(String(error)));
                    }}
                  >
                    <MdAdd className="text-base mr-1" />
                    {t("network.newProxy")}
                  </Button>
                </div>
              </div>

              {proxies.length === 0 && proxyGroups.length === 0 ? (
                <div className="overflow-hidden rounded-md border">
                  <EmptyState
                    icon={MdRouter}
                    title={t("network.noProxyConfigs")}
                    description={t("network.proxyEmptyHint")}
                  />
                </div>
              ) : (
                <div className="space-y-2">
                  {proxySections.map((section) => (
                    <NetworkGroupSection
                      key={section.id}
                      section={section}
                      collapsed={!expandedSections.has(`proxy:${section.id}`)}
                      onToggle={() => toggleSection(`proxy:${section.id}`)}
                      onRename={(group) => setGroupDialog({ tab: "proxy", group })}
                      onDelete={(group, itemCount) =>
                        setDeleteGroupState({ tab: "proxy", group, itemCount })
                      }
                    >
                      {section.items.map((proxy, index) => (
                        <div
                          key={proxy.id}
                          className={cn(index < section.items.length - 1 ? "border-b" : undefined)}
                        >
                          <ProxyRow
                            proxy={proxy}
                            groups={proxyGroups}
                            onEdit={(item) => {
                              void openProxyConfig(item.id).catch((error) =>
                                toast.error(String(error)),
                              );
                            }}
                            onDelete={handleDeleteProxy}
                            onMoveGroup={handleMoveProxyGroup}
                          />
                        </div>
                      ))}
                    </NetworkGroupSection>
                  ))}
                </div>
              )}
            </div>
          </TabsContent>
        </Tabs>
      </div>

      <GroupNameDialog
        state={groupDialog}
        saving={groupSaving}
        onOpenChange={(open) => {
          if (!open) setGroupDialog(null);
        }}
        onSave={handleSaveGroup}
      />
      <DeleteNetworkGroupDialog
        state={deleteGroupState}
        onOpenChange={(open) => {
          if (!open) setDeleteGroupState(null);
        }}
        onConfirm={handleConfirmDeleteGroup}
      />
    </aside>
  );
}
