import { closeSearchPanel } from "@codemirror/search";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { join, tempDir } from "@tauri-apps/api/path";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openPath } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdClose,
  MdDescription,
  MdDns,
  MdDoneAll,
  MdKeyboardArrowDown,
  MdOpenInNew,
  MdRefresh,
  MdSave,
} from "react-icons/md";
import { toast } from "sonner";
import ReloadDirtyDialog from "@/components/dialog/remote-file-editor/ReloadDirtyDialog";
import RemoteFileConflictDialog from "@/components/dialog/remote-file-editor/RemoteFileConflictDialog";
import UnsavedChangesDialog from "@/components/dialog/remote-file-editor/UnsavedChangesDialog";
import ChildWindowHeader from "@/components/layout/ChildWindowHeader";
import {
  getLocalPathName,
  languageFromFilename,
  type TextFileOpenResult,
} from "@/components/panel/file-explorer/model";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useApp } from "@/context/AppContext";
import { useChildWindowCommand } from "@/hooks/useChildWindowCommand";
import { useFileEditorZoom } from "@/hooks/useFileEditorZoom";
import { CHILD_WINDOW_COMMANDS } from "@/lib/childWindowProtocol";
import {
  type CursorPosition,
  codeMirrorFileViewExtensions,
  getCursorPosition,
  getDisplayLanguage,
} from "@/lib/codeMirrorFileView";
import { getErrorMessage } from "@/lib/errors";
import { clampFileEditorFontSize } from "@/lib/fileEditorFontSize";
import { MAX_EDITOR_FILE_BYTES } from "@/lib/fileEditorLimits";
import { invoke } from "@/lib/invoke";
import { cn, formatSize, parseJsonSearchParam } from "@/lib/utils";
import type { FileWindowTarget } from "@/lib/windowManager";

type FileEditorBackendKind = "remote" | "local";

interface RemoteFileEditorData {
  sessionId: string;
  backend?: FileEditorBackendKind;
  path?: string;
  remotePath?: string;
  name: string;
  size: number;
  mtime: number;
  target?: FileWindowTarget;
}

interface RemoteFileEditorOpenPayload {
  targetLabel?: string;
  data: RemoteFileEditorData;
}

interface EditorTab {
  id: string;
  sessionId: string;
  backend: FileEditorBackendKind;
  path: string;
  remotePath: string;
  name: string;
  size: number;
  mtime: number;
  target?: FileWindowTarget;
  content: string;
  baseSize: number;
  baseMtime: number;
  baseMtimeNanos?: string;
  baseContentHash: string;
  loading: boolean;
  saving: boolean;
  dirty: boolean;
  error: string;
  lastSavedAt: number | null;
  language: string;
}

interface WriteRemoteFileTextResult {
  status: "saved" | "conflict";
  mtime?: number;
  mtimeNanos?: string;
  size?: number;
  contentHash?: string;
}

function getEditorDataPath(data: Pick<RemoteFileEditorData, "path" | "remotePath">) {
  return data.path ?? data.remotePath ?? "";
}

function tabId(data: Pick<RemoteFileEditorData, "backend" | "sessionId" | "path" | "remotePath">) {
  const backend = data.backend ?? "remote";
  return `${backend}\n${data.sessionId}\n${getEditorDataPath(data)}`;
}

function getParentDirectoryName(path: string) {
  const normalized = path.replace(/[\\/]+$/, "");
  if (!normalized || normalized === "/" || /^[a-zA-Z]:$/.test(normalized)) return normalized || "/";
  const index = Math.max(normalized.lastIndexOf("/"), normalized.lastIndexOf("\\"));
  const parent = index > 0 ? normalized.slice(0, index) : "";
  if (!parent || parent === "/" || /^[a-zA-Z]:$/.test(parent)) return parent || "/";
  return getLocalPathName(parent, parent);
}

function getTabBaseLabel(tab: EditorTab) {
  return tab.name || getLocalPathName(tab.path, tab.path);
}

function formatRemoteMtime(mtime?: number) {
  if (!mtime) return "";
  const timestamp = mtime < 10_000_000_000 ? mtime * 1000 : mtime;
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "short",
    timeStyle: "short",
  }).format(new Date(timestamp));
}

function createTab(data: RemoteFileEditorData): EditorTab {
  const path = getEditorDataPath(data);
  return {
    sessionId: data.sessionId,
    backend: data.backend ?? "remote",
    path,
    remotePath: path,
    name: data.name,
    size: data.size,
    mtime: data.mtime,
    target: data.target,
    id: tabId(data),
    content: "",
    baseSize: data.size,
    baseMtime: data.mtime,
    baseMtimeNanos: undefined,
    baseContentHash: "",
    loading: true,
    saving: false,
    dirty: false,
    error: "",
    lastSavedAt: null,
    language: languageFromFilename(data.name || path),
  };
}

function formatTargetLabel(target?: FileWindowTarget) {
  if (!target) return "";
  return target.label;
}

export default function RemoteFileEditorPage() {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings } = useApp();
  useFileEditorZoom(updateAppSettings);
  const editorFontSize = clampFileEditorFontSize(
    appSettings.transfer.internal_editor_font_size,
  );
  const initialData = useMemo(() => {
    const params = new URLSearchParams(window.location.search);
    return parseJsonSearchParam<RemoteFileEditorData>(params.get("data"));
  }, []);
  const editorParentRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  const forceCloseRef = useRef(false);
  const suppressEditorUpdateRef = useRef(false);
  const editorStatesRef = useRef<Record<string, EditorState>>({});
  const tabsRef = useRef<EditorTab[]>(initialData ? [createTab(initialData)] : []);
  const activeTabIdRef = useRef(initialData ? tabId(initialData) : "");
  const [tabs, setTabs] = useState<EditorTab[]>(tabsRef.current);
  const [activeTabId, setActiveTabId] = useState(activeTabIdRef.current);
  const [conflictTabId, setConflictTabId] = useState<string | null>(null);
  const [closeConfirmOpen, setCloseConfirmOpen] = useState(false);
  const [pendingCloseTabId, setPendingCloseTabId] = useState<string | null>(null);
  const [reloadConfirmTabId, setReloadConfirmTabId] = useState<string | null>(null);
  const [cursorPosition, setCursorPosition] = useState<CursorPosition>({ line: 1, column: 1 });

  const activeTab = useMemo(
    () => tabs.find((tab) => tab.id === activeTabId) ?? tabs[0] ?? null,
    [activeTabId, tabs],
  );
  const dirtyTabs = useMemo(() => tabs.filter((tab) => tab.dirty), [tabs]);
  const savingTabs = useMemo(() => tabs.filter((tab) => tab.saving), [tabs]);
  const duplicateTabNames = useMemo(() => {
    const counts = new Map<string, number>();
    for (const tab of tabs) {
      const label = getTabBaseLabel(tab);
      counts.set(label, (counts.get(label) ?? 0) + 1);
    }
    return counts;
  }, [tabs]);

  const updateTabs = useCallback((updater: (tabs: EditorTab[]) => EditorTab[]) => {
    const next = updater(tabsRef.current);
    tabsRef.current = next;
    setTabs(next);
  }, []);

  const updateTab = useCallback(
    (id: string, updater: (tab: EditorTab) => EditorTab) => {
      updateTabs((current) => current.map((tab) => (tab.id === id ? updater(tab) : tab)));
    },
    [updateTabs],
  );

  const closeWindow = useCallback(() => {
    forceCloseRef.current = true;
    getCurrentWindow()
      .close()
      .catch(() => {});
  }, []);

  const createEditorState = useCallback(
    (content: string, language: string) =>
      EditorState.create({
        doc: content,
        extensions: codeMirrorFileViewExtensions(language, {
          editable: true,
          updateListener: EditorView.updateListener.of((update) => {
            const id = activeTabIdRef.current;
            if (!id) return;
            editorStatesRef.current[id] = update.state;
            if (update.docChanged || update.selectionSet) {
              setCursorPosition(getCursorPosition(update.state));
            }
            if (!update.docChanged || suppressEditorUpdateRef.current) return;
            const next = update.state.doc.toString();
            updateTab(id, (tab) => ({ ...tab, content: next, dirty: true }));
          }),
        }),
      }),
    [updateTab],
  );

  const setEditorState = useCallback((state: EditorState) => {
    const view = viewRef.current;
    if (!view) return;
    try {
      suppressEditorUpdateRef.current = true;
      view.setState(state);
    } finally {
      suppressEditorUpdateRef.current = false;
    }
    setCursorPosition(getCursorPosition(state));
    window.requestAnimationFrame(() => view.focus());
  }, []);

  const rememberCurrentEditorState = useCallback(() => {
    const id = activeTabIdRef.current;
    const view = viewRef.current;
    if (!id || !view) return;
    editorStatesRef.current[id] = view.state;
  }, []);

  const activateTab = useCallback(
    (id: string) => {
      rememberCurrentEditorState();
      activeTabIdRef.current = id;
      setActiveTabId(id);
    },
    [rememberCurrentEditorState],
  );

  const removeUnsupportedTab = useCallback(
    (id: string) => {
      delete editorStatesRef.current[id];
      const currentTabs = tabsRef.current;
      if (currentTabs.length <= 1) {
        closeWindow();
        return;
      }

      const index = currentTabs.findIndex((item) => item.id === id);
      const nextTabs = currentTabs.filter((item) => item.id !== id);
      updateTabs(() => nextTabs);
      if (activeTabIdRef.current === id) {
        const nextActive = nextTabs[Math.min(Math.max(index, 0), nextTabs.length - 1)];
        activeTabIdRef.current = nextActive.id;
        setActiveTabId(nextActive.id);
      }
    },
    [closeWindow, updateTabs],
  );

  const openExternalFile = useCallback(
    async (tab: EditorTab) => {
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
    },
    [appSettings.transfer.default_editor],
  );

  const loadFile = useCallback(
    async (id: string, fallbackTab?: EditorTab) => {
      const tab = tabsRef.current.find((item) => item.id === id) ?? fallbackTab;
      if (!tab) return;

      updateTab(id, (current) => ({ ...current, loading: true, error: "" }));
      try {
        const result = await invoke<TextFileOpenResult>(
          tab.backend === "local" ? "open_local_file_text" : "open_remote_file_text",
          {
            sessionId: tab.sessionId,
            path: tab.path,
            maxBytes: MAX_EDITOR_FILE_BYTES,
          },
        );
        if (result.status === "unsupported") {
          toast.info(
            t(
              result.reason === "binary"
                ? "fileExplorer.binaryOpenExternal"
                : "fileExplorer.unsupportedEncodingOpenExternal",
            ),
          );
          try {
            await openExternalFile(tab);
            removeUnsupportedTab(id);
          } catch (externalError) {
            updateTab(id, (current) => ({
              ...current,
              loading: false,
              error: getErrorMessage(externalError) || t("fileEditor.openExternalFailed"),
            }));
          }
          return;
        }

        const file = result.file;
        updateTab(id, (current) => ({
          ...current,
          content: file.content,
          baseSize: file.size,
          baseMtime: file.mtime ?? current.mtime ?? 0,
          baseMtimeNanos: file.mtimeNanos,
          baseContentHash: file.contentHash,
          size: file.size,
          mtime: file.mtime ?? current.mtime ?? 0,
          loading: false,
          dirty: false,
          error: "",
          lastSavedAt: null,
        }));
        const nextState = createEditorState(file.content, tab.language);
        editorStatesRef.current[id] = nextState;
        if (activeTabIdRef.current === id) {
          setEditorState(nextState);
        }
      } catch (err) {
        updateTab(id, (current) => ({
          ...current,
          loading: false,
          error: getErrorMessage(err) || t("fileEditor.loadFailed"),
        }));
      }
    },
    [createEditorState, openExternalFile, removeUnsupportedTab, setEditorState, t, updateTab],
  );

  const addOrFocusTab = useCallback(
    (data: RemoteFileEditorData) => {
      const id = tabId(data);
      const exists = tabsRef.current.some((tab) => tab.id === id);
      if (!exists) {
        const nextTab = createTab(data);
        updateTabs((current) => [...current, nextTab]);
        void loadFile(id, nextTab);
      }
      activateTab(id);
    },
    [activateTab, loadFile, updateTabs],
  );

  useEffect(() => {
    if (!initialData) return;
    void loadFile(tabId(initialData));
  }, [initialData, loadFile]);

  useChildWindowCommand<RemoteFileEditorOpenPayload>(
    CHILD_WINDOW_COMMANDS.remoteFileEditorOpen,
    (payload) => {
      const currentWindow = getCurrentWindow();
      if (payload.targetLabel && payload.targetLabel !== currentWindow.label) return;
      addOrFocusTab(payload.data);
    },
  );

  useEffect(() => {
    const currentWindow = getCurrentWindow();
    const fileLabel = activeTab ? activeTab.name || activeTab.remotePath : t("fileEditor.title");
    const title = activeTab ? `${activeTab.dirty ? "* " : ""}${fileLabel}` : t("fileEditor.title");
    currentWindow.setTitle(title).catch(() => {});
  }, [activeTab, t]);

  useEffect(() => {
    const parent = editorParentRef.current;
    if (!parent) return;

    const initialTab = activeTabIdRef.current
      ? tabsRef.current.find((tab) => tab.id === activeTabIdRef.current)
      : null;
    const initialState =
      (initialTab && editorStatesRef.current[initialTab.id]) ??
      createEditorState(initialTab?.content ?? "", initialTab?.language ?? "plaintext");
    if (initialTab) {
      editorStatesRef.current[initialTab.id] = initialState;
    }

    const view = new EditorView({
      parent,
      state: initialState,
    });
    viewRef.current = view;
    window.requestAnimationFrame(() => view.focus());

    return () => {
      view.destroy();
      viewRef.current = null;
    };
  }, [createEditorState]);

  useEffect(() => {
    if (!activeTabId) return;
    const currentTab = tabsRef.current.find((tab) => tab.id === activeTabId);
    if (!currentTab) return;
    activeTabIdRef.current = currentTab.id;
    const state =
      editorStatesRef.current[currentTab.id] ??
      createEditorState(currentTab.content, currentTab.language);
    editorStatesRef.current[currentTab.id] = state;
    setEditorState(state);
  }, [activeTabId, createEditorState, setEditorState]);

  useEffect(() => {
    const currentWindow = getCurrentWindow();
    let unlisten: (() => void) | undefined;

    currentWindow
      .onCloseRequested((event) => {
        if (forceCloseRef.current || dirtyTabs.length === 0) return;
        event.preventDefault();
        setPendingCloseTabId(null);
        setCloseConfirmOpen(true);
      })
      .then((dispose) => {
        unlisten = dispose;
      })
      .catch(() => {});

    return () => {
      unlisten?.();
    };
  }, [dirtyTabs.length]);

  const requestReloadTab = useCallback(
    (tab: EditorTab) => {
      if (tab.dirty) {
        setReloadConfirmTabId(tab.id);
        return;
      }
      void loadFile(tab.id);
    },
    [loadFile],
  );

  const saveFile = useCallback(
    async (id: string, force = false) => {
      const tab = tabsRef.current.find((item) => item.id === id);
      if (!tab || tab.saving) return false;
      updateTab(id, (current) => ({ ...current, saving: true, error: "" }));
      try {
        const result = await invoke<WriteRemoteFileTextResult>(
          tab.backend === "local" ? "write_local_file_text" : "write_remote_file_text",
          {
            sessionId: tab.sessionId,
            path: tab.path,
            content: tab.content,
            expectedMtime: tab.baseMtime,
            expectedSize: tab.baseSize,
            expectedMtimeNanos: tab.baseMtimeNanos,
            expectedHash: tab.baseContentHash || undefined,
            force,
          },
        );
        if (result.status === "conflict") {
          setConflictTabId(id);
          return false;
        }
        updateTab(id, (current) => ({
          ...current,
          baseMtime: result.mtime ?? current.baseMtime,
          baseMtimeNanos: result.mtimeNanos ?? current.baseMtimeNanos,
          baseSize: result.size ?? new Blob([current.content]).size,
          baseContentHash: result.contentHash ?? current.baseContentHash,
          mtime: result.mtime ?? current.mtime,
          size: result.size ?? current.size,
          dirty: false,
          saving: false,
          lastSavedAt: Date.now(),
        }));
        toast.success(t("fileEditor.saved"));
        return true;
      } catch (err) {
        updateTab(id, (current) => ({
          ...current,
          saving: false,
          error: getErrorMessage(err) || t("fileEditor.saveFailed"),
        }));
        return false;
      } finally {
        updateTab(id, (current) => ({ ...current, saving: false }));
      }
    },
    [t, updateTab],
  );

  const saveAllDirty = useCallback(async () => {
    const ids = tabsRef.current.filter((tab) => tab.dirty).map((tab) => tab.id);
    for (const id of ids) {
      const saved = await saveFile(id, false);
      if (!saved) return false;
    }
    return true;
  }, [saveFile]);

  const openExternal = useCallback(
    (tab: EditorTab) => {
      openExternalFile(tab).catch((err) => {
        toast.error(getErrorMessage(err) || t("fileEditor.openExternalFailed"));
      });
    },
    [openExternalFile, t],
  );

  const closeTab = useCallback(
    (id: string, force = false) => {
      const tab = tabsRef.current.find((item) => item.id === id);
      if (!tab) return;
      if (tab.dirty && !force) {
        setPendingCloseTabId(id);
        setCloseConfirmOpen(true);
        return;
      }

      const currentTabs = tabsRef.current;
      if (currentTabs.length <= 1) {
        closeWindow();
        return;
      }

      const index = currentTabs.findIndex((item) => item.id === id);
      const nextTabs = currentTabs.filter((item) => item.id !== id);
      updateTabs(() => nextTabs);
      if (activeTabIdRef.current === id) {
        const nextActive = nextTabs[Math.min(index, nextTabs.length - 1)];
        activeTabIdRef.current = nextActive.id;
        setActiveTabId(nextActive.id);
      }
    },
    [closeWindow, updateTabs],
  );

  const handleDiscard = useCallback(() => {
    if (pendingCloseTabId) {
      closeTab(pendingCloseTabId, true);
      setPendingCloseTabId(null);
      setCloseConfirmOpen(false);
      return;
    }
    closeWindow();
  }, [closeTab, closeWindow, pendingCloseTabId]);

  const handleSaveAndClose = useCallback(async () => {
    if (pendingCloseTabId) {
      const saved = await saveFile(pendingCloseTabId, false);
      if (!saved) return;
      closeTab(pendingCloseTabId, true);
      setPendingCloseTabId(null);
      setCloseConfirmOpen(false);
      return;
    }

    const saved = await saveAllDirty();
    if (!saved) return;
    setCloseConfirmOpen(false);
    closeWindow();
  }, [closeTab, closeWindow, pendingCloseTabId, saveAllDirty, saveFile]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLocaleLowerCase() === "s") {
        event.preventDefault();
        if (event.shiftKey) {
          void saveAllDirty();
        } else if (activeTabIdRef.current) {
          void saveFile(activeTabIdRef.current, false);
        }
      }
      if (event.key === "Escape") {
        const view = viewRef.current;
        if (view) closeSearchPanel(view);
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [saveAllDirty, saveFile]);

  if (!initialData) return null;

  const statusText = activeTab?.saving
    ? t("common.saving")
    : activeTab?.dirty
      ? t("fileEditor.unsaved")
      : activeTab?.lastSavedAt
        ? t("fileEditor.saved")
        : t("fileEditor.ready");
  const pendingCloseHasSaving = pendingCloseTabId
    ? Boolean(tabs.find((tab) => tab.id === pendingCloseTabId)?.saving)
    : savingTabs.length > 0;
  const activeMtimeText = formatRemoteMtime(activeTab?.mtime);
  const activeTarget = activeTab?.target;
  const activeTargetLabel =
    formatTargetLabel(activeTarget) ||
    (activeTab?.backend !== "local" ? activeTab?.sessionId || t("fileEditor.remoteTarget") : "");
  const activeTargetTitle = [activeTargetLabel, activeTarget?.detail].filter(Boolean).join(" - ");
  const shouldShowTarget = activeTab?.backend !== "local" && !!activeTargetLabel;
  const activeHeaderTitle = activeTab
    ? `${dirtyTabs.length > 0 ? "* " : ""}${activeTab.name || activeTab.remotePath}`
    : t("fileEditor.title");

  return (
    <div
      className="flex h-full min-h-0 flex-col overflow-hidden bg-background text-foreground"
      data-file-editor-root="true"
    >
      <ChildWindowHeader
        title={activeHeaderTitle}
        icon={<MdDescription className="text-base" />}
        windowControls
        onClose={() => {
          if (dirtyTabs.length > 0) setCloseConfirmOpen(true);
          else closeWindow();
        }}
      />

      <div className="flex min-h-0 flex-1 flex-col">
        <div className="flex h-10 shrink-0 overflow-hidden border-b bg-muted/15">
          <div role="tablist" className="flex min-w-0 flex-1 items-stretch overflow-hidden">
            {tabs.map((tab) => {
              const baseLabel = getTabBaseLabel(tab);
              const hasDuplicateName = (duplicateTabNames.get(baseLabel) ?? 0) > 1;
              const tabLabel = hasDuplicateName
                ? `${baseLabel} · ${getParentDirectoryName(tab.remotePath)}`
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
                  title={tab.remotePath}
                  onClick={() => activateTab(tab.id)}
                  onKeyDown={(event) => {
                    if (event.key !== "Enter" && event.key !== " ") return;
                    event.preventDefault();
                    activateTab(tab.id);
                  }}
                >
                  {tab.dirty && <span className="sr-only">{t("fileEditor.unsaved")}</span>}
                  {tab.dirty && (
                    <span
                      aria-hidden="true"
                      className="h-1.5 w-1.5 shrink-0 rounded-full bg-primary"
                    />
                  )}
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
                  aria-label={t("terminal.openTabs")}
                >
                  <MdKeyboardArrowDown className="text-lg" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-80">
                {tabs.map((tab) => {
                  const baseLabel = getTabBaseLabel(tab);
                  const hasDuplicateName = (duplicateTabNames.get(baseLabel) ?? 0) > 1;
                  const tabLabel = hasDuplicateName
                    ? `${baseLabel} · ${getParentDirectoryName(tab.remotePath)}`
                    : baseLabel;
                  const isActive = tab.id === activeTabId;

                  return (
                    <DropdownMenuItem
                      key={tab.id}
                      className={cn("items-start gap-2 py-2", isActive && "bg-accent/60")}
                      onClick={() => activateTab(tab.id)}
                    >
                      <span
                        className={cn(
                          "mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full",
                          tab.dirty ? "bg-primary" : "bg-transparent",
                        )}
                      />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate font-mono text-xs text-foreground">
                          {tabLabel}
                        </span>
                        <span className="block truncate font-mono text-[11px] text-muted-foreground">
                          {tab.remotePath}
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
              title={activeTab?.remotePath}
            >
              {activeTab?.remotePath}
            </div>
          </div>
          <div className="flex shrink-0 flex-wrap items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              className="h-8 gap-1.5"
              disabled={!activeTab || activeTab.loading}
              onClick={() => activeTab && requestReloadTab(activeTab)}
            >
              <MdRefresh className="text-sm" />
              {t("fileEditor.reload")}
            </Button>
            <Tooltip>
              <TooltipTrigger asChild>
                <span className="inline-flex">
                  <Button
                    variant="outline"
                    size="sm"
                    className="h-8 gap-1.5"
                    disabled={!activeTab || activeTab.loading}
                    onClick={() => activeTab && openExternal(activeTab)}
                  >
                    <MdOpenInNew className="text-sm" />
                    {t("fileEditor.openExternal")}
                  </Button>
                </span>
              </TooltipTrigger>
              <TooltipContent side="bottom">{t("fileEditor.openExternalTooltip")}</TooltipContent>
            </Tooltip>
            <Button
              size="sm"
              className="h-8 gap-1.5"
              disabled={!activeTab || activeTab.saving || activeTab.loading}
              onClick={() => activeTab && void saveFile(activeTab.id, false)}
            >
              <MdSave className="text-sm" />
              {activeTab?.saving ? t("common.saving") : t("common.save")}
            </Button>
            <Button
              variant="outline"
              size="sm"
              className="h-8 gap-1.5"
              disabled={dirtyTabs.length === 0 || savingTabs.length > 0}
              onClick={() => void saveAllDirty()}
            >
              <MdDoneAll className="text-sm" />
              {t("fileEditor.saveAll")}
            </Button>
          </div>
        </div>

        {activeTab?.error && (
          <div className="shrink-0 border-b border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive">
            {activeTab.error}
          </div>
        )}

        <div className="relative min-h-0 flex-1">
          {activeTab?.loading && (
            <div className="absolute inset-0 z-10 flex items-center justify-center bg-background/80 text-sm text-muted-foreground pointer-events-none">
              {t("common.loading")}
            </div>
          )}
          <div
            ref={editorParentRef}
            className="h-full min-h-0"
            style={{ fontSize: `${editorFontSize}px` }}
          />
        </div>

        <div className="flex h-6 shrink-0 items-center justify-between gap-3 border-t bg-muted/15 px-3 font-mono text-[11px] text-muted-foreground">
          <div className="flex min-w-0 items-center gap-2 overflow-hidden">
            <span className="shrink-0">
              {getDisplayLanguage(activeTab?.language ?? "plaintext")}
            </span>
            <span aria-hidden="true" className="shrink-0">
              ·
            </span>
            <span className="shrink-0">
              {t("fileEditor.lineColumn", {
                line: cursorPosition.line,
                column: cursorPosition.column,
              })}
            </span>
            <span aria-hidden="true" className="shrink-0">
              ·
            </span>
            <span className="shrink-0">{statusText}</span>
            {dirtyTabs.length > 1 ? (
              <>
                <span aria-hidden="true" className="shrink-0">
                  ·
                </span>
                <span className="truncate">
                  {t("fileEditor.unsavedFilesDesc", { count: dirtyTabs.length })}
                </span>
              </>
            ) : null}
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <span>{formatSize(activeTab?.size ?? 0)}</span>
            <span aria-hidden="true">·</span>
            <span>{t("fileEditor.encodingUtf8")}</span>
            <span aria-hidden="true">·</span>
            <span>{t("fileEditor.lineEndingLf")}</span>
            {activeMtimeText ? (
              <>
                <span aria-hidden="true">·</span>
                <span>{t("fileEditor.modifiedAt", { time: activeMtimeText })}</span>
              </>
            ) : null}
          </div>
        </div>
      </div>

      <RemoteFileConflictDialog
        open={!!conflictTabId}
        onOpenChange={(open) => {
          if (!open) setConflictTabId(null);
        }}
        onDiscardAndReload={() => {
          const id = conflictTabId;
          setConflictTabId(null);
          if (id) void loadFile(id);
        }}
        onForceSave={() => {
          const id = conflictTabId;
          setConflictTabId(null);
          if (id) void saveFile(id, true);
        }}
      />

      <ReloadDirtyDialog
        open={!!reloadConfirmTabId}
        onOpenChange={(open) => {
          if (!open) setReloadConfirmTabId(null);
        }}
        onConfirm={() => {
          const id = reloadConfirmTabId;
          setReloadConfirmTabId(null);
          if (id) void loadFile(id);
        }}
      />

      <UnsavedChangesDialog
        open={closeConfirmOpen}
        dirtyCount={dirtyTabs.length}
        hasPendingTab={!!pendingCloseTabId}
        saving={pendingCloseHasSaving}
        onOpenChange={setCloseConfirmOpen}
        onSaveAndClose={handleSaveAndClose}
        onDiscard={handleDiscard}
      />
    </div>
  );
}
