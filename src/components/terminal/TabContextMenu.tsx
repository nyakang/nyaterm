import { type ReactNode, useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdAutoAwesome,
  MdClose,
  MdCloseFullscreen,
  MdColorLens,
  MdContentCopy,
  MdDriveFileRenameOutline,
  MdHorizontalSplit,
  MdInfoOutline,
  MdPlayArrow,
  MdRefresh,
  MdVerticalSplit,
} from "react-icons/md";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { useApp } from "@/context/AppContext";
import { openAIAssistant } from "@/lib/aiEvents";
import { getActivePane, getTabDisplayName } from "@/lib/workspaceTabs";
import type { PaneSplitDirection, Tab } from "@/types/global";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "../ui/context-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "../ui/tooltip";

const TAB_PRESET_COLORS = [
  { name: "Red", value: "#ef4444" },
  { name: "Orange", value: "#f97316" },
  { name: "Amber", value: "#f59e0b" },
  { name: "Yellow", value: "#eab308" },
  { name: "Green", value: "#22c55e" },
  { name: "Emerald", value: "#10b981" },
  { name: "Cyan", value: "#06b6d4" },
  { name: "Blue", value: "#3b82f6" },
  { name: "Indigo", value: "#6366f1" },
  { name: "Purple", value: "#a855f7" },
  { name: "Pink", value: "#ec4899" },
];

interface TabContextMenuProps {
  children: ReactNode;
  tooltipContent?: ReactNode;
  tab: Tab;
  tabs: Tab[];
  onDuplicateSession: (tab: Tab) => void | Promise<void>;
  onReconnectSession: (tab: Tab) => void | Promise<void>;
  onSplitSession: (tab: Tab, direction: PaneSplitDirection) => void | Promise<void>;
  onCloseSession: (tab: Tab) => void | Promise<void>;
  onCloseAll: () => void | Promise<void>;
  onCloseInactive: (keepTabId: string) => void | Promise<void>;
  onCloseRight: (tabId: string) => void | Promise<void>;
  onSessionInfo: (tab: Tab) => void | Promise<void>;
  onActivateTab: (tabId: string) => void;
}

export default function TabContextMenu({
  children,
  tooltipContent,
  tab,
  tabs,
  onDuplicateSession,
  onReconnectSession,
  onSplitSession,
  onCloseSession,
  onCloseAll,
  onCloseInactive,
  onCloseRight,
  onSessionInfo,
  onActivateTab,
}: TabContextMenuProps) {
  const { t } = useTranslation();
  const { updateTab } = useApp();
  const [renameOpen, setRenameOpen] = useState(false);
  const [renameValue, setRenameValue] = useState("");

  const activePane = getActivePane(tab);
  const displayName = getTabDisplayName(tab);
  const tabIndex = tabs.findIndex((item) => item.id === tab.id);
  const canSpawnSession =
    !!activePane && (activePane.type === "Local" || !!activePane.connectionId);
  const canReconnect = !!activePane && !activePane.connecting && canSpawnSession;
  const canSplit = canSpawnSession;
  const canUseAI = !!activePane && !activePane.connecting && !activePane.connectError;
  const canCloseInactive = tabs.length > 1;
  const canCloseRight = tabIndex !== -1 && tabIndex < tabs.length - 1;
  const canSessionInfo = !!activePane?.connectionId;
  const iconClass = "mr-2 text-[0.875rem] text-muted-foreground";

  const handleSetColor = useCallback(
    async (color: string | undefined) => {
      try {
        await updateTab(tab.id, { tabColor: color }, { immediatePersist: true });
      } catch {
        toast.error(t("tabCtx.colorFailed"));
      }
    },
    [t, tab.id, updateTab],
  );

  const handleRenameOpen = useCallback(() => {
    setRenameValue(displayName);
    setRenameOpen(true);
  }, [displayName]);

  const handleRenameSubmit = useCallback(async () => {
    const trimmed = renameValue.trim();
    if (!trimmed) {
      toast.error(t("tabCtx.renameEmpty"));
      return;
    }
    if (trimmed.length > 64) {
      return;
    }

    try {
      await updateTab(tab.id, { customName: trimmed }, { immediatePersist: true });
      setRenameOpen(false);
    } catch {
      toast.error(t("tabCtx.renameFailed"));
    }
  }, [renameValue, t, tab.id, updateTab]);

  const handleCopyName = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(displayName);
      toast.success(t("tabCtx.nameCopied"));
    } catch {
      toast.error(t("tabCtx.copyFailed"));
    }
  }, [displayName, t]);

  const handleOpenAI = useCallback(
    (action: "explain_output" | "analyze_error") => {
      onActivateTab(tab.id);
      requestAnimationFrame(() => openAIAssistant({ action }));
    },
    [onActivateTab, tab.id],
  );

  return (
    <>
      <ContextMenu>
        {tooltipContent ? (
          <Tooltip>
            <TooltipTrigger asChild>
              <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
            </TooltipTrigger>
            <TooltipContent side="bottom" sideOffset={6} showArrow className="max-w-xs truncate">
              {tooltipContent}
            </TooltipContent>
          </Tooltip>
        ) : (
          <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
        )}
        <ContextMenuContent className="min-w-[220px]">
          <ContextMenuSub>
            <ContextMenuSubTrigger>
              <MdColorLens className={iconClass} />
              {t("tabCtx.setColor")}
            </ContextMenuSubTrigger>
            <ContextMenuSubContent>
              <div className="grid grid-cols-6 gap-1 p-2">
                {TAB_PRESET_COLORS.map((color) => (
                  <button
                    key={color.value}
                    className="h-5 w-5 rounded-full border border-transparent transition-transform hover:scale-125 focus:outline-none"
                    style={{
                      backgroundColor: color.value,
                      boxShadow:
                        tab.tabColor === color.value
                          ? `0 0 0 2px var(--df-bg), 0 0 0 4px ${color.value}`
                          : undefined,
                    }}
                    title={color.name}
                    onClick={() => void handleSetColor(color.value)}
                  />
                ))}
              </div>
              {tab.tabColor && (
                <>
                  <ContextMenuSeparator />
                  <ContextMenuItem onClick={() => void handleSetColor(undefined)}>
                    <MdClose className={iconClass} />
                    {t("tabCtx.resetColor")}
                  </ContextMenuItem>
                </>
              )}
            </ContextMenuSubContent>
          </ContextMenuSub>

          <ContextMenuItem onClick={handleRenameOpen}>
            <MdDriveFileRenameOutline className={iconClass} />
            {t("tabCtx.rename")}
          </ContextMenuItem>

          <ContextMenuItem onClick={() => void handleCopyName()}>
            <MdContentCopy className={iconClass} />
            {t("tabCtx.copyName")}
          </ContextMenuItem>

          <ContextMenuSeparator />

          <ContextMenuItem disabled={!canSpawnSession} onClick={() => void onDuplicateSession(tab)}>
            <MdPlayArrow className={iconClass} />
            {t("tabCtx.duplicate")}
          </ContextMenuItem>

          <ContextMenuItem disabled={!canReconnect} onClick={() => void onReconnectSession(tab)}>
            <MdRefresh className={iconClass} />
            {t("tabCtx.reconnect")}
          </ContextMenuItem>

          <ContextMenuSub>
            <ContextMenuSubTrigger>
              <MdAutoAwesome className={iconClass} />
              {t("ai.title")}
            </ContextMenuSubTrigger>
            <ContextMenuSubContent>
              <ContextMenuItem disabled={!canUseAI} onClick={() => handleOpenAI("explain_output")}>
                {t("ai.explainRecent")}
              </ContextMenuItem>
              <ContextMenuItem disabled={!canUseAI} onClick={() => handleOpenAI("analyze_error")}>
                {t("ai.analyzeError")}
              </ContextMenuItem>
            </ContextMenuSubContent>
          </ContextMenuSub>

          <ContextMenuSeparator />

          <ContextMenuItem
            disabled={!canSplit}
            onClick={() => void onSplitSession(tab, "horizontal")}
          >
            <MdHorizontalSplit className={iconClass} />
            {t("tabCtx.splitHorizontal")}
          </ContextMenuItem>

          <ContextMenuItem
            disabled={!canSplit}
            onClick={() => void onSplitSession(tab, "vertical")}
          >
            <MdVerticalSplit className={iconClass} />
            {t("tabCtx.splitVertical")}
          </ContextMenuItem>

          <ContextMenuSeparator />

          <ContextMenuItem disabled={!activePane} onClick={() => void onCloseSession(tab)}>
            <MdClose className={iconClass} />
            {t("tabCtx.close")}
          </ContextMenuItem>

          <ContextMenuItem onClick={() => void onCloseAll()}>
            <MdCloseFullscreen className={iconClass} />
            {t("tabCtx.closeAll")}
          </ContextMenuItem>

          <ContextMenuItem
            disabled={!canCloseInactive}
            onClick={() => void onCloseInactive(tab.id)}
          >
            {t("tabCtx.closeInactive")}
          </ContextMenuItem>

          <ContextMenuItem disabled={!canCloseRight} onClick={() => void onCloseRight(tab.id)}>
            {t("tabCtx.closeRight")}
          </ContextMenuItem>

          <ContextMenuItem disabled={!canSessionInfo} onClick={() => void onSessionInfo(tab)}>
            <MdInfoOutline className={iconClass} />
            {t("tabCtx.sessionInfo")}
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>

      <Dialog open={renameOpen} onOpenChange={(open) => !open && setRenameOpen(false)}>
        <DialogContent showCloseButton={false} className="max-w-xs">
          <DialogHeader>
            <DialogTitle className="text-sm">{t("tabCtx.renameTitle")}</DialogTitle>
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
            <Button variant="outline" size="sm" onClick={() => setRenameOpen(false)}>
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
    </>
  );
}
