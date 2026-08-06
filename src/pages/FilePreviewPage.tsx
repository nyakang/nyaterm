import { listen } from "@tauri-apps/api/event";
import { join, tempDir } from "@tauri-apps/api/path";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openPath } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdClose,
  MdDns,
  MdKeyboardArrowDown,
  MdOpenInNew,
  MdRefresh,
  MdVisibility,
} from "react-icons/md";
import { toast } from "sonner";
import ChildWindowHeader from "@/components/layout/ChildWindowHeader";
import {
  FilePreviewContent,
  type FilePreviewContentData,
  type FilePreviewLoadSummary,
} from "@/components/panel/file-explorer/FilePreviewContent";
import { getFilePreviewKind, getLocalPathName } from "@/components/panel/file-explorer/model";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useApp } from "@/context/AppContext";
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import { cn, formatSize, parseJsonSearchParam } from "@/lib/utils";
import type { FilePreviewWindowData } from "@/lib/windowManager";

type PreviewBackendKind = "remote" | "local";

interface FilePreviewOpenPayload {
  targetLabel?: string;
  data: FilePreviewWindowData;
}

interface PreviewTab extends FilePreviewContentData {
  id: string;
  backend: PreviewBackendKind;
  target?: FilePreviewWindowData["target"];
  reloadKey: number;
  loadStatus: FilePreviewLoadSummary["status"];
  loadError: string;
}

function getPreviewDataPath(data: Pick<FilePreviewWindowData, "path">) {
  return data.path;
}

function tabId(data: Pick<FilePreviewWindowData, "backend" | "sessionId" | "path">) {
  const backend = data.backend ?? "remote";
  return `${backend}\n${data.sessionId}\n${getPreviewDataPath(data)}`;
}

function createTab(data: FilePreviewWindowData): PreviewTab {
  return {
    id: tabId(data),
    sessionId: data.sessionId,
    backend: data.backend ?? "remote",
    target: data.target,
    path: data.path,
    name: data.name,
    size: data.size,
    mtime: data.mtime,
    reloadKey: 0,
    loadStatus: "idle",
    loadError: "",
  };
}

function getTabBaseLabel(tab: PreviewTab) {
  return tab.name || getLocalPathName(tab.path, tab.path);
}

function getParentDirectoryName(path: string) {
  const normalized = path.replace(/[\\/]+$/, "");
  if (!normalized || normalized === "/" || /^[a-zA-Z]:$/.test(normalized)) return normalized || "/";
  const index = Math.max(normalized.lastIndexOf("/"), normalized.lastIndexOf("\\"));
  const parent = index > 0 ? normalized.slice(0, index) : "";
  if (!parent || parent === "/" || /^[a-zA-Z]:$/.test(parent)) return parent || "/";
  return getLocalPathName(parent, parent);
}

function formatPreviewMtime(mtime?: number) {
  if (!mtime) return "";
  const timestamp = mtime < 10_000_000_000 ? mtime * 1000 : mtime;
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "short",
    timeStyle: "short",
  }).format(new Date(timestamp));
}

function formatTargetLabel(target?: FilePreviewWindowData["target"]) {
  if (!target) return "";
  return target.label;
}

export default function FilePreviewPage() {
  const { t } = useTranslation();
  const { appSettings } = useApp();
  const initialData = useMemo(() => {
    const params = new URLSearchParams(window.location.search);
    return parseJsonSearchParam<FilePreviewWindowData>(params.get("data"));
  }, []);
  const tabsRef = useRef<PreviewTab[]>(initialData ? [createTab(initialData)] : []);
  const activeTabIdRef = useRef(initialData ? tabId(initialData) : "");
  const [tabs, setTabs] = useState<PreviewTab[]>(tabsRef.current);
  const [activeTabId, setActiveTabId] = useState(activeTabIdRef.current);

  const activeTab = useMemo(
    () => tabs.find((tab) => tab.id === activeTabId) ?? tabs[0] ?? null,
    [activeTabId, tabs],
  );
  const duplicateTabNames = useMemo(() => {
    const counts = new Map<string, number>();
    for (const tab of tabs) {
      const label = getTabBaseLabel(tab);
      counts.set(label, (counts.get(label) ?? 0) + 1);
    }
    return counts;
  }, [tabs]);

  const updateTabs = useCallback((updater: (tabs: PreviewTab[]) => PreviewTab[]) => {
    const next = updater(tabsRef.current);
    tabsRef.current = next;
    setTabs(next);
  }, []);

  const updateTab = useCallback(
    (id: string, updater: (tab: PreviewTab) => PreviewTab) => {
      updateTabs((current) => current.map((tab) => (tab.id === id ? updater(tab) : tab)));
    },
    [updateTabs],
  );

  const activateTab = useCallback((id: string) => {
    activeTabIdRef.current = id;
    setActiveTabId(id);
  }, []);

  const addOrFocusTab = useCallback(
    (data: FilePreviewWindowData) => {
      const nextTab = createTab(data);
      const existing = tabsRef.current.find((tab) => tab.id === nextTab.id);
      if (!existing) {
        updateTabs((current) => [...current, nextTab]);
      }
      activateTab(nextTab.id);
    },
    [activateTab, updateTabs],
  );

  useEffect(() => {
    const currentWindow = getCurrentWindow();
    let unlisten: (() => void) | undefined;

    listen<FilePreviewOpenPayload>("file-preview-open", (event) => {
      if (event.payload.targetLabel && event.payload.targetLabel !== currentWindow.label) return;
      addOrFocusTab(event.payload.data);
    })
      .then((dispose) => {
        unlisten = dispose;
      })
      .catch(() => {});

    return () => {
      unlisten?.();
    };
  }, [addOrFocusTab]);

  useEffect(() => {
    const currentWindow = getCurrentWindow();
    const fileLabel = activeTab ? activeTab.name || activeTab.path : t("filePreview.title");
    currentWindow.setTitle(fileLabel).catch(() => {});
  }, [activeTab, t]);

  const closeWindow = useCallback(() => {
    getCurrentWindow()
      .close()
      .catch(() => {});
  }, []);

  const closeTab = useCallback(
    (id: string) => {
      const currentTabs = tabsRef.current;
      if (currentTabs.length <= 1) {
        closeWindow();
        return;
      }

      const index = currentTabs.findIndex((tab) => tab.id === id);
      const nextTabs = currentTabs.filter((tab) => tab.id !== id);
      updateTabs(() => nextTabs);
      if (activeTabIdRef.current === id) {
        const nextActive = nextTabs[Math.min(index, nextTabs.length - 1)];
        activeTabIdRef.current = nextActive.id;
        setActiveTabId(nextActive.id);
      }
    },
    [closeWindow, updateTabs],
  );

  const reloadTab = useCallback(
    (tab: PreviewTab) => {
      updateTab(tab.id, (current) => ({
        ...current,
        reloadKey: current.reloadKey + 1,
        loadStatus: "loading",
        loadError: "",
      }));
    },
    [updateTab],
  );

  const openExternal = useCallback(
    (tab: PreviewTab) => {
      const run = async () => {
        if (tab.backend === "local") {
          await openPath(tab.path, appSettings.transfer.default_editor || undefined);
          return;
        }

        const root = await tempDir();
        const safeName = await invoke<string>("sanitize_download_file_name", { name: tab.name });
        const localPath = await join(
          root,
          "nyaterm",
          tab.sessionId,
          Date.now().toString(),
          safeName,
        );
        await invoke("download_remote_file", {
          sessionId: tab.sessionId,
          remotePath: tab.path,
          localPath,
        });
        await invoke("start_file_watch", {
          sessionId: tab.sessionId,
          localPath,
          remotePath: tab.path,
        });
        await openPath(localPath, appSettings.transfer.default_editor || undefined);
      };

      run().catch((error) => {
        toast.error(getErrorMessage(error) || t("filePreview.openExternalFailed"));
      });
    },
    [appSettings.transfer.default_editor, t],
  );

  const handleLoadStateChange = useCallback(
    (id: string, summary: FilePreviewLoadSummary) => {
      updateTab(id, (tab) => ({
        ...tab,
        loadStatus: summary.status,
        loadError: summary.message ?? "",
      }));
    },
    [updateTab],
  );

  if (!initialData) return null;

  const activeKind = activeTab ? getFilePreviewKind(activeTab.name) : "text";
  const activeMtimeText = formatPreviewMtime(activeTab?.mtime);
  const activeStatusText =
    activeTab?.loadStatus === "loading"
      ? t("filePreview.loading")
      : activeTab?.loadStatus === "error"
        ? t("filePreview.error")
        : t("filePreview.ready");
  const activeTarget = activeTab?.target;
  const activeTargetLabel =
    formatTargetLabel(activeTarget) ||
    (activeTab?.backend !== "local"
      ? activeTab?.sessionId || t("filePreview.remoteTarget")
      : "");
  const activeTargetTitle = [activeTargetLabel, activeTarget?.detail].filter(Boolean).join(" - ");
  const shouldShowTarget = activeTab?.backend !== "local" && !!activeTargetLabel;
  const activeHeaderTitle = activeTab ? activeTab.name || activeTab.path : t("filePreview.title");

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden bg-background text-foreground">
      <ChildWindowHeader
        title={activeHeaderTitle}
        icon={<MdVisibility className="text-base" />}
        windowControls
        onClose={closeWindow}
      />

      <div className="flex min-h-0 flex-1 flex-col">
        <div className="flex h-10 shrink-0 overflow-hidden border-b bg-muted/15">
          <div role="tablist" className="flex min-w-0 flex-1 items-stretch overflow-hidden">
            {tabs.map((tab) => {
              const baseLabel = getTabBaseLabel(tab);
              const hasDuplicateName = (duplicateTabNames.get(baseLabel) ?? 0) > 1;
              const tabLabel = hasDuplicateName
                ? `${baseLabel} - ${getParentDirectoryName(tab.path)}`
                : baseLabel;
              const isActive = tab.id === activeTabId;

              return (
                <div
                  role="tab"
                  tabIndex={0}
                  aria-selected={isActive}
                  key={tab.id}
                  className={cn(
                    "group flex h-full min-w-[96px] max-w-[240px] shrink-0 cursor-default items-center gap-2 border-r px-3 text-xs transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/50",
                    isActive
                      ? "border-t border-t-primary/50 border-r-border border-b border-b-background bg-background text-foreground"
                      : "border-r-border/70 border-b border-b-border bg-transparent text-muted-foreground hover:bg-muted/30 hover:text-foreground",
                  )}
                  title={tab.path}
                  onClick={() => activateTab(tab.id)}
                  onKeyDown={(event) => {
                    if (event.key !== "Enter" && event.key !== " ") return;
                    event.preventDefault();
                    activateTab(tab.id);
                  }}
                >
                  <span className="min-w-0 flex-1 truncate font-mono">{tabLabel}</span>
                  <button
                    type="button"
                    className={cn(
                      "flex h-5 w-5 shrink-0 items-center justify-center rounded-sm text-muted-foreground transition hover:bg-muted hover:text-foreground",
                      isActive ? "opacity-70" : "opacity-0 group-hover:opacity-70",
                    )}
                    aria-label={t("common.close")}
                    onClick={(event) => {
                      event.stopPropagation();
                      closeTab(tab.id);
                    }}
                  >
                    <MdClose className="text-sm" />
                  </button>
                </div>
              );
            })}
          </div>

          {tabs.length > 1 && (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  className="flex h-full w-9 shrink-0 items-center justify-center border-l bg-muted/10 text-muted-foreground transition-colors hover:bg-muted/30 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/50"
                  aria-label={t("filePreview.tabs")}
                >
                  <MdKeyboardArrowDown className="text-lg" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-80">
                {tabs.map((tab) => {
                  const baseLabel = getTabBaseLabel(tab);
                  const hasDuplicateName = (duplicateTabNames.get(baseLabel) ?? 0) > 1;
                  const tabLabel = hasDuplicateName
                    ? `${baseLabel} - ${getParentDirectoryName(tab.path)}`
                    : baseLabel;
                  const isActive = tab.id === activeTabId;

                  return (
                    <DropdownMenuItem
                      key={tab.id}
                      className={cn("items-start gap-2 py-2", isActive && "bg-accent/60")}
                      onClick={() => activateTab(tab.id)}
                    >
                      <span className="min-w-0 flex-1">
                        <span className="block truncate font-mono text-xs text-foreground">
                          {tabLabel}
                        </span>
                        <span className="block truncate font-mono text-[11px] text-muted-foreground">
                          {tab.path}
                        </span>
                      </span>
                    </DropdownMenuItem>
                  );
                })}
              </DropdownMenuContent>
            </DropdownMenu>
          )}
        </div>

        <div className="flex min-h-0 shrink-0 flex-col gap-2 border-b bg-muted/10 px-3 py-1.5 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex min-w-0 flex-1 items-center gap-2 overflow-hidden">
            {shouldShowTarget ? (
              <div
                className="flex max-w-[180px] shrink-0 items-center gap-1.5 rounded-sm border bg-background/70 px-2 py-1 text-xs text-muted-foreground"
                title={activeTargetTitle || activeTargetLabel}
              >
                <MdDns className="shrink-0 text-sm" />
                <span className="truncate">{activeTargetLabel}</span>
              </div>
            ) : null}
            <div
              className="min-w-0 truncate font-mono text-xs text-muted-foreground"
              title={activeTab?.path}
            >
              {activeTab?.path}
            </div>
          </div>
          <div className="flex shrink-0 flex-wrap items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              className="h-8 gap-1.5"
              disabled={!activeTab || activeTab.loadStatus === "loading"}
              onClick={() => activeTab && reloadTab(activeTab)}
            >
              <MdRefresh className="text-sm" />
              {t("filePreview.reload")}
            </Button>
            <Tooltip>
              <TooltipTrigger asChild>
                <span className="inline-flex">
                  <Button
                    variant="outline"
                    size="sm"
                    className="h-8 gap-1.5"
                    disabled={!activeTab}
                    onClick={() => activeTab && openExternal(activeTab)}
                  >
                    <MdOpenInNew className="text-sm" />
                    {t("filePreview.openExternal")}
                  </Button>
                </span>
              </TooltipTrigger>
              <TooltipContent side="bottom">{t("fileEditor.openExternalTooltip")}</TooltipContent>
            </Tooltip>
          </div>
        </div>

        <div className="relative min-h-0 flex-1 overflow-hidden">
          {tabs.map((tab) => (
            <div
              key={tab.id}
              className={cn("absolute inset-0 min-h-0", tab.id !== activeTabId && "hidden")}
            >
              <FilePreviewContent
                data={tab}
                reloadKey={tab.reloadKey}
                active={tab.id === activeTabId}
                onLoadStateChange={(summary) => handleLoadStateChange(tab.id, summary)}
              />
            </div>
          ))}
        </div>

        <div className="flex h-6 shrink-0 items-center justify-between gap-3 border-t bg-muted/15 px-3 font-mono text-[11px] text-muted-foreground">
          <div className="flex min-w-0 items-center gap-2 overflow-hidden">
            <span className="shrink-0">{t(`filePreview.type.${activeKind}`)}</span>
            <span aria-hidden="true" className="shrink-0">
              -
            </span>
            <span className="shrink-0">{activeStatusText}</span>
            {activeTab?.loadError ? (
              <>
                <span aria-hidden="true" className="shrink-0">
                  -
                </span>
                <span className="truncate text-destructive">{activeTab.loadError}</span>
              </>
            ) : null}
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <span>{formatSize(activeTab?.size ?? 0)}</span>
            {activeMtimeText ? (
              <>
                <span aria-hidden="true">-</span>
                <span>{t("filePreview.modifiedAt", { time: activeMtimeText })}</span>
              </>
            ) : null}
          </div>
        </div>
      </div>
    </div>
  );
}
