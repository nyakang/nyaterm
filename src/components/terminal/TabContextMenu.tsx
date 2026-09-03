import { type ReactNode, useCallback } from "react";
import { useTranslation } from "react-i18next";
import {
  MdAutoAwesome,
  MdCallSplit,
  MdClose,
  MdCloseFullscreen,
  MdColorLens,
  MdContentCopy,
  MdDriveFileRenameOutline,
  MdHorizontalSplit,
  MdInfoOutline,
  MdInput,
  MdLinkOff,
  MdLock,
  MdLockOpen,
  MdMerge,
  MdPlayArrow,
  MdRefresh,
  MdVerticalSplit,
} from "react-icons/md";
import { TbArrowBarToRight, TbCircleDotFilled } from "react-icons/tb";
import { toast } from "sonner";
import { useApp } from "@/context/AppContext";
import { openAIAssistant } from "@/lib/aiEvents";
import { hasMatchingTemporaryConfig } from "@/lib/appWorkspace";
import { getActivePane } from "@/lib/workspaceTabs";
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
  onMultiplexSshSession: (tab: Tab) => void | Promise<void>;
  onDuplicateSessionWithCommand: (tab: Tab) => void | Promise<void>;
  onMultiplexSshSessionWithCommand: (tab: Tab) => void | Promise<void>;
  onReconnectSession: (tab: Tab) => void | Promise<void>;
  onDisconnectSession: (tab: Tab) => void | Promise<void>;
  onSplitSession: (
    tab: Tab,
    direction: PaneSplitDirection,
  ) => void | Promise<void>;
  onUnsplit?: () => void;
  onCloseSession: (tab: Tab) => void | Promise<void>;
  onCloseAll: () => void | Promise<void>;
  onCloseInactive: (keepTabId: string) => void | Promise<void>;
  onCloseRight: (tabId: string) => void | Promise<void>;
  onSessionInfo: (tab: Tab) => void | Promise<void>;
  onActivateTab: (tabId: string) => void;
  canCopyIp: boolean;
  onRenameTab: (tab: Tab) => void;
  onCopyTabName: (tab: Tab) => void | Promise<void>;
  onCopyServerIp: (tab: Tab) => void | Promise<void>;
}

export default function TabContextMenu({
  children,
  tooltipContent,
  tab,
  tabs,
  onDuplicateSession,
  onMultiplexSshSession,
  onDuplicateSessionWithCommand,
  onMultiplexSshSessionWithCommand,
  onReconnectSession,
  onDisconnectSession,
  onSplitSession,
  onUnsplit,
  onCloseSession,
  onCloseAll,
  onCloseInactive,
  onCloseRight,
  onSessionInfo,
  onActivateTab,
  canCopyIp,
  onRenameTab,
  onCopyTabName,
  onCopyServerIp,
}: TabContextMenuProps) {
  const { t } = useTranslation();
  const { updateTab } = useApp();

  const activePane = getActivePane(tab);
  const tabIndex = tabs.findIndex((item) => item.id === tab.id);
  const isTerminalPane = activePane?.paneKind === "terminal";
  const supportsSessionSpawn =
    !!activePane &&
    isTerminalPane &&
    (activePane.type === "Local" ||
      !!activePane.connectionId ||
      hasMatchingTemporaryConfig(activePane));
  const supportsSshMultiplex =
    !!activePane &&
    isTerminalPane &&
    activePane.type === "SSH" &&
    (!!activePane.connectionId ||
      activePane.temporaryConfig?.protocol === "ssh");
  const supportsReconnect = supportsSessionSpawn;
  const supportsDisconnect = !!activePane && isTerminalPane;
  const supportsAI = !!activePane && isTerminalPane;
  const supportsSplit = supportsSessionSpawn;
  const supportsSessionInfo = !!activePane?.connectionId && isTerminalPane;
  const showSessionActionsGroup =
    supportsSessionSpawn ||
    supportsSshMultiplex ||
    supportsDisconnect ||
    supportsAI;
  const showSplitActionsGroup = supportsSplit;
  const canSpawnSession = supportsSessionSpawn;
  const canReconnect = supportsReconnect && !activePane.connecting;
  const canMultiplexSsh =
    supportsSshMultiplex &&
    !activePane.connecting &&
    !activePane.connectError &&
    !!activePane.sessionId;
  const canDisconnect =
    supportsDisconnect && !activePane.connecting && !activePane.connectError;
  const canSplit = supportsSplit;
  const canUseAI =
    supportsAI && !activePane.connecting && !activePane.connectError;
  const canCloseInactive = tabs.length > 1;
  const canCloseRight = tabIndex !== -1 && tabIndex < tabs.length - 1;
  const canCloseTab = !!activePane && !tab.locked;
  const canSessionInfo = supportsSessionInfo;
  const iconClass = "mr-2 text-[0.875rem] text-muted-foreground";

  const handleSetColor = useCallback(
    async (color: string | undefined) => {
      try {
        await updateTab(
          tab.id,
          { tabColor: color },
          { immediatePersist: true },
        );
      } catch {
        toast.error(t("tabCtx.colorFailed"));
      }
    },
    [t, tab.id, updateTab],
  );

  const handleToggleLocked = useCallback(async () => {
    try {
      await updateTab(
        tab.id,
        { locked: !tab.locked },
        { immediatePersist: true },
      );
    } catch {
      toast.error(t("tabCtx.lockToggleFailed"));
    }
  }, [t, tab.id, tab.locked, updateTab]);

  const handleRestoreAutomaticTitle = useCallback(async () => {
    try {
      await updateTab(
        tab.id,
        { customName: undefined },
        { immediatePersist: true },
      );
    } catch {
      toast.error(t("tabCtx.restoreAutomaticTitleFailed"));
    }
  }, [t, tab.id, updateTab]);

  const handleOpenAI = useCallback(
    (action: "explain_output" | "analyze_error") => {
      onActivateTab(tab.id);
      requestAnimationFrame(() => openAIAssistant({ action }));
    },
    [onActivateTab, tab.id],
  );

  return (
    <ContextMenu>
      {tooltipContent ? (
        <Tooltip>
          <TooltipTrigger asChild>
            <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
          </TooltipTrigger>
          <TooltipContent
            side="bottom"
            sideOffset={6}
            showArrow
            className="max-w-xs"
          >
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

        <ContextMenuItem onClick={() => onRenameTab(tab)}>
          <MdDriveFileRenameOutline className={iconClass} />
          {t("tabCtx.rename")}
        </ContextMenuItem>

        {tab.customName ? (
          <ContextMenuItem onClick={() => void handleRestoreAutomaticTitle()}>
            <MdAutoAwesome className={iconClass} />
            {t("tabCtx.restoreAutomaticTitle")}
          </ContextMenuItem>
        ) : null}

        <ContextMenuItem onClick={() => void handleToggleLocked()}>
          {tab.locked ? (
            <MdLockOpen className={iconClass} />
          ) : (
            <MdLock className={iconClass} />
          )}
          {tab.locked ? t("tabCtx.unlockTab") : t("tabCtx.lockTab")}
        </ContextMenuItem>

        <ContextMenuItem onClick={() => void onCopyTabName(tab)}>
          <MdContentCopy className={iconClass} />
          {t("tabCtx.copyName")}
        </ContextMenuItem>

        {canCopyIp && (
          <ContextMenuItem onClick={() => void onCopyServerIp(tab)}>
            <MdContentCopy className={iconClass} />
            {t("tabCtx.copyIp")}
          </ContextMenuItem>
        )}

        {showSessionActionsGroup && (
          <>
            <ContextMenuSeparator />

            {supportsSessionSpawn && (
              <>
                <ContextMenuItem
                  disabled={!canSpawnSession}
                  onClick={() => void onDuplicateSession(tab)}
                >
                  <MdPlayArrow className={iconClass} />
                  {t("tabCtx.duplicate")}
                </ContextMenuItem>
                <ContextMenuItem
                  disabled={!canSpawnSession}
                  onClick={() => void onDuplicateSessionWithCommand(tab)}
                >
                  <MdInput className={iconClass} />
                  {t("tabCtx.duplicateWithCommand")}
                </ContextMenuItem>
              </>
            )}

            {supportsSshMultiplex && (
              <ContextMenuSub>
                <ContextMenuSubTrigger disabled={!canMultiplexSsh}>
                  <MdCallSplit className={iconClass} />
                  {t("tabCtx.sshAdvanced")}
                </ContextMenuSubTrigger>
                <ContextMenuSubContent>
                  <ContextMenuItem
                    disabled={!canMultiplexSsh}
                    onClick={() => void onMultiplexSshSession(tab)}
                  >
                    <MdCallSplit className={iconClass} />
                    {t("tabCtx.multiplexSsh")}
                  </ContextMenuItem>
                  <ContextMenuItem
                    disabled={!canMultiplexSsh}
                    onClick={() => void onMultiplexSshSessionWithCommand(tab)}
                  >
                    <MdInput className={iconClass} />
                    {t("tabCtx.multiplexSshWithCommand")}
                  </ContextMenuItem>
                </ContextMenuSubContent>
              </ContextMenuSub>
            )}

            {supportsReconnect && (
              <ContextMenuItem
                disabled={!canReconnect}
                onClick={() => void onReconnectSession(tab)}
              >
                <MdRefresh className={iconClass} />
                {t("tabCtx.reconnect")}
              </ContextMenuItem>
            )}

            {supportsDisconnect && (
              <ContextMenuItem
                disabled={!canDisconnect}
                onClick={() => void onDisconnectSession(tab)}
              >
                <MdLinkOff className={iconClass} />
                {t("tabCtx.disconnect")}
              </ContextMenuItem>
            )}

            {supportsAI && (
              <ContextMenuSub>
                <ContextMenuSubTrigger>
                  <MdAutoAwesome className={iconClass} />
                  {t("ai.title")}
                </ContextMenuSubTrigger>
                <ContextMenuSubContent>
                  <ContextMenuItem
                    disabled={!canUseAI}
                    onClick={() => handleOpenAI("explain_output")}
                  >
                    {t("ai.explainRecent")}
                  </ContextMenuItem>
                  <ContextMenuItem
                    disabled={!canUseAI}
                    onClick={() => handleOpenAI("analyze_error")}
                  >
                    {t("ai.analyzeError")}
                  </ContextMenuItem>
                </ContextMenuSubContent>
              </ContextMenuSub>
            )}
          </>
        )}

        {showSplitActionsGroup && (
          <>
            <ContextMenuSeparator />

            {supportsSplit && (
              <>
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
              </>
            )}

            {onUnsplit && (
              <ContextMenuItem onClick={onUnsplit}>
                <MdMerge className={iconClass} />
                {t("tabCtx.unsplit")}
              </ContextMenuItem>
            )}
          </>
        )}

        <ContextMenuSeparator />

        <ContextMenuItem
          disabled={!canCloseTab}
          onClick={() => void onCloseSession(tab)}
        >
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
          <TbCircleDotFilled className={iconClass} />
          {t("tabCtx.closeInactive")}
        </ContextMenuItem>

        <ContextMenuItem
          disabled={!canCloseRight}
          onClick={() => void onCloseRight(tab.id)}
        >
          <TbArrowBarToRight className={iconClass} />
          {t("tabCtx.closeRight")}
        </ContextMenuItem>

        {supportsSessionInfo && (
          <ContextMenuItem
            disabled={!canSessionInfo}
            onClick={() => void onSessionInfo(tab)}
          >
            <MdInfoOutline className={iconClass} />
            {t("tabCtx.sessionInfo")}
          </ContextMenuItem>
        )}
      </ContextMenuContent>
    </ContextMenu>
  );
}
