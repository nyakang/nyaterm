import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { join, tempDir } from "@tauri-apps/api/path";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { openPath as openUrl } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { useTranslation } from "react-i18next";
import {
  MdArrowUpward,
  MdContentCopy,
  MdCreateNewFolder,
  MdDelete,
  MdDownload,
  MdFolderOff,
  MdInfo,
  MdLink,
  MdNoteAdd,
  MdRefresh,
  MdSend,
  MdSync,
  MdSyncLock,
  MdUpload,
} from "react-icons/md";
import { toast } from "sonner";
import AutoUploadDialog, { type AutoUploadDialogData } from "../dialog/file-explorer/AutoUploadDialog";
import MoveDialog, { type MoveDialogData } from "../dialog/file-explorer/MoveDialog";
import NewItemDialog, { type NewItemDialogData } from "../dialog/file-explorer/NewItemDialog";
import NewSymlinkDialog, { type NewSymlinkDialogData } from "../dialog/file-explorer/NewSymlinkDialog";
import PropertiesDialog, { type PropertiesDialogData } from "../dialog/file-explorer/PropertiesDialog";
import RenameDialog, { type RenameDialogData } from "../dialog/file-explorer/RenameDialog";
import DeleteDialog, { type DeleteDialogData } from "../dialog/file-explorer/DeleteDialog";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";

import { FileEntry, FileExplorerProps } from "@/types/global";
import { formatSize } from "@/lib/utils";
import { FileListItem } from "./FileListItem";

interface TransferEventPayload {
  session_id: string;
  direction: string;
  status: string;
  remote_path?: string;
}

function getParentPath(path: string) {
  const normalized = path !== "/" && path.endsWith("/") ? path.slice(0, -1) : path;
  const index = normalized.lastIndexOf("/");
  return index <= 0 ? "/" : normalized.slice(0, index);
}

/** Remote file browser for active SSH session. Lists dirs/files, supports navigation. */
export default function FileExplorer({ activeSessionId }: FileExplorerProps) {
  const { t } = useTranslation();

  const [files, setFiles] = useState<FileEntry[]>([]);
  const [currentPath, setCurrentPath] = useState("");
  const [homeDir, setHomeDir] = useState("");
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [isEditingPath, setIsEditingPath] = useState(false);
  const [pathInputText, setPathInputText] = useState("");
  const [directoryLoading, setDirectoryLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [renameDialogData, setRenameDialogData] = useState<RenameDialogData | null>(null);
  const [deleteDialogData, setDeleteDialogData] = useState<DeleteDialogData | null>(null);
  const [moveDialogData, setMoveDialogData] = useState<MoveDialogData | null>(null);
  const [newItemDialogData, setNewItemDialogData] = useState<NewItemDialogData | null>(null);
  const [newSymlinkDialogData, setNewSymlinkDialogData] = useState<NewSymlinkDialogData | null>(null);
  const [autoUploadDialogData, setAutoUploadDialogData] = useState<AutoUploadDialogData | null>(
    null,
  );
  const [propertiesDialogData, setPropertiesDialogData] = useState<PropertiesDialogData | null>(
    null,
  );
  const [autoSyncCwd, setAutoSyncCwd] = useState(false);
  const [, setAlwaysUploadFiles] = useState<Set<string>>(new Set());

  const sessionCacheRef = useRef<Map<string, { files: FileEntry[]; currentPath: string; homeDir: string }>>(new Map());
  const prevSessionIdRef = useRef<string | null>(null);
  const pendingManualRefreshUploadsRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    const unlisten = listen<{ session_id: string; local_path: string; remote_path: string }>(
      "file-modified",
      (e) => {
        const { session_id, local_path, remote_path } = e.payload;
        const watchKey = `${session_id}:${local_path}`;

        setAlwaysUploadFiles((prev) => {
          if (prev.has(watchKey)) {
            // File was marked "Always list", just upload silently
            invoke("upload_local_file", {
              sessionId: session_id,
              localPath: local_path,
              remotePath: remote_path,
            }).catch((err) => toast.error(String(err)));
            return prev;
          } else {
            // Trigger the dialog
            setAutoUploadDialogData({
              sessionId: session_id,
              localPath: local_path,
              remotePath: remote_path,
            });
            return prev;
          }
        });
      },
    );

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const loadDirectory = useCallback(
    async (path: string) => {
      if (!activeSessionId) return;
      setDirectoryLoading(true);
      setError(null);

      try {
        const entries = await invoke<FileEntry[]>("list_remote_dir", {
          sessionId: activeSessionId,
          path,
        });
        entries.sort((a, b) => {
          if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
          return a.name.localeCompare(b.name);
        });
        setFiles(entries);
        setCurrentPath(path);

        const cached = sessionCacheRef.current.get(activeSessionId);
        sessionCacheRef.current.set(activeSessionId, {
          files: entries,
          currentPath: path,
          homeDir: cached?.homeDir ?? homeDir,
        });
      } catch (e) {
        const msg = String(e);
        if (files.length > 0) {
          toast.error(msg);
        } else {
          setError(msg);
        }
      } finally {
        setDirectoryLoading(false);
      }
    },
    [activeSessionId, files.length, homeDir],
  );

  useEffect(() => {
    const cache = sessionCacheRef.current;
    const prevId = prevSessionIdRef.current;

    if (prevId && prevId !== activeSessionId) {
      cache.set(prevId, { files, currentPath, homeDir });
    }
    prevSessionIdRef.current = activeSessionId;

    if (!activeSessionId) {
      setFiles([]);
      setCurrentPath("");
      setHomeDir("");
      return;
    }

    const cached = cache.get(activeSessionId);
    if (cached) {
      setFiles(cached.files);
      setCurrentPath(cached.currentPath);
      setHomeDir(cached.homeDir);
      setError(null);
      return;
    }

    let cancelled = false;
    (async () => {
      try {
        const home = await invoke<string>("get_home_dir", { sessionId: activeSessionId });
        if (cancelled) return;
        setHomeDir(home);
        loadDirectory(home);
      } catch {
        if (cancelled) return;
        loadDirectory("~");
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [activeSessionId]);

  // Move Tauri listeners that do NOT rely on standard React side-effects to the top hook
  useEffect(() => {
    const unlisten = listen<{ session_id: string; local_path: string; remote_path: string }>(
      "file-modified",
      (e) => {
        const { session_id, local_path, remote_path } = e.payload;
        const watchKey = `${session_id}:${local_path}`;

        setAlwaysUploadFiles((prev) => {
          if (prev.has(watchKey)) {
            invoke("upload_local_file", {
              sessionId: session_id,
              localPath: local_path,
              remotePath: remote_path,
            }).catch((err) => toast.error(String(err)));
            return prev;
          } else {
            setAutoUploadDialogData({
              sessionId: session_id,
              localPath: local_path,
              remotePath: remote_path,
            });
            return prev;
          }
        });
      },
    );

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    if (!activeSessionId || !currentPath) return;

    const unlisten = listen<TransferEventPayload>("transfer-event", (event) => {
      const { session_id, direction, status, remote_path } = event.payload;
      const uploadKey = remote_path ? `${session_id}:${remote_path}` : null;

      if (
        session_id !== activeSessionId ||
        direction !== "upload" ||
        !remote_path ||
        !uploadKey ||
        !pendingManualRefreshUploadsRef.current.has(uploadKey)
      ) {
        return;
      }

      if (status === "completed" || status === "error") {
        pendingManualRefreshUploadsRef.current.delete(uploadKey);
      }

      if (status === "completed" && getParentPath(remote_path) === currentPath) {
        loadDirectory(currentPath);
      }
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [activeSessionId, currentPath, loadDirectory]);

  const handleItemClick = (entry: FileEntry) => {
    if (entry.is_dir) {
      const newPath = currentPath === "/" ? `/${entry.name}` : `${currentPath}/${entry.name}`;
      loadDirectory(newPath);
    } else {
      setSelectedFile(entry.name);
    }
  };

  const handleNewFile = () => {
    if (!activeSessionId) return;
    setNewItemDialogData({
      sessionId: activeSessionId,
      currentDirPath: currentPath,
      type: "file",
    });
  };

  const handleNewFolder = () => {
    if (!activeSessionId) return;
    setNewItemDialogData({
      sessionId: activeSessionId,
      currentDirPath: currentPath,
      type: "folder",
    });
  };

  const handleNewSymlink = () => {
    if (!activeSessionId) return;
    setNewSymlinkDialogData({ sessionId: activeSessionId, currentDirPath: currentPath });
  };

  const handleCurrentDirProperties = () => {
    if (!activeSessionId || !currentPath) return;
    const name = currentPath.split("/").filter(Boolean).pop() || currentPath;
    setPropertiesDialogData({ sessionId: activeSessionId, fullPath: currentPath, name, is_dir: true });
  };

  const handleCopyCurrentPath = () => {
    navigator.clipboard.writeText(currentPath);
  };

  const handleSendCurrentPathToTerminal = () => {
    if (!activeSessionId) return;
    invoke("write_to_session", { sessionId: activeSessionId, data: currentPath });
    emit(`focus-terminal-${activeSessionId}`);
  };

  const handleDownloadSelected = () => {
    if (!selectedFile) return;
    const entry = files.find((f) => f.name === selectedFile);
    if (entry) handleDownload(entry);
  };

  const handleDeleteSelected = () => {
    if (!selectedFile) return;
    const entry = files.find((f) => f.name === selectedFile);
    if (entry) handleDelete(entry);
  };

  const handleGoUp = () => {
    if (!currentPath || currentPath === "/") return;
    const parts = currentPath.split("/");
    parts.pop();
    loadDirectory(parts.join("/") || "/");
  };

  const handleSyncCwd = useCallback(async () => {
    if (!activeSessionId) return;
    try {
      const cwd = await invoke<string>("get_terminal_cwd", { sessionId: activeSessionId });
      if (cwd && cwd !== currentPath) {
        loadDirectory(cwd);
      }
    } catch (e) {
      toast.error(`${t("fileExplorer.syncFailed")}: ${e}`);
    }
  }, [activeSessionId, currentPath, loadDirectory, t]);

  useEffect(() => {
    if (!autoSyncCwd || !activeSessionId) return;
    const unlisten = listen<string>(`cwd-changed-${activeSessionId}`, (event) => {
      const newCwd = event.payload;
      if (newCwd) {
        loadDirectory(newCwd);
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [autoSyncCwd, activeSessionId, loadDirectory]);

  const getEntryFullPath = (entry: FileEntry) => {
    return currentPath === "/" ? `/${entry.name}` : `${currentPath}/${entry.name}`;
  };

  const handleCopyPath = (entry: FileEntry, mode: "dir" | "name" | "full") => {
    let text = "";
    if (mode === "dir") text = currentPath;
    else if (mode === "name") text = entry.name;
    else text = getEntryFullPath(entry);
    navigator.clipboard.writeText(text);
  };

  const handleSendToTerminal = (entry: FileEntry, mode: "dir" | "name" | "full") => {
    if (!activeSessionId) return;
    let text = "";
    if (mode === "dir") text = currentPath;
    else if (mode === "name") text = entry.name;
    else text = getEntryFullPath(entry);

    invoke("write_to_session", {
      sessionId: activeSessionId,
      data: text,
    });
    emit(`focus-terminal-${activeSessionId}`);
  };

  const handleDelete = (entry: FileEntry) => {
    if (!activeSessionId) return;
    setDeleteDialogData({
      sessionId: activeSessionId,
      path: getEntryFullPath(entry),
      name: entry.name,
    });
  };

  const handleDownload = async (entry: FileEntry) => {
    if (!activeSessionId || entry.is_dir) return;
    try {
      const localPath = await saveDialog({ defaultPath: entry.name });
      if (!localPath) return;
      await invoke("download_remote_file", {
        sessionId: activeSessionId,
        remotePath: getEntryFullPath(entry),
        localPath,
      });
    } catch (e) {
      toast.error(String(e));
    }
  };

  const handleUpload = async () => {
    if (!activeSessionId) return;
    let uploadKey: string | null = null;
    try {
      const localPath = await openDialog({ multiple: false, directory: false });
      if (!localPath || typeof localPath !== "string") return;

      const fileName = localPath.split(/[\\/]/).pop() || "uploaded_file";
      const remotePath = currentPath === "/" ? `/${fileName}` : `${currentPath}/${fileName}`;
      uploadKey = `${activeSessionId}:${remotePath}`;

      pendingManualRefreshUploadsRef.current.add(uploadKey);
      await invoke("upload_local_file", {
        sessionId: activeSessionId,
        localPath,
        remotePath,
      });
    } catch (e) {
      toast.error(String(e));
      if (uploadKey) {
        pendingManualRefreshUploadsRef.current.delete(uploadKey);
      }
    }
  };

  const handleOpenDefault = async (entry: FileEntry) => {
    if (!activeSessionId || entry.is_dir) return;
    try {
      const tDir = await tempDir();
      const downloadTimestamp = Date.now().toString();
      const localPath = await join(
        tDir,
        "dragonfly",
        activeSessionId,
        downloadTimestamp,
        entry.name,
      );
      await invoke("download_remote_file", {
        sessionId: activeSessionId,
        remotePath: getEntryFullPath(entry),
        localPath,
      });

      // Start watching the file for auto-upload
      await invoke("start_file_watch", {
        sessionId: activeSessionId,
        localPath,
        remotePath: getEntryFullPath(entry),
      });

      await openUrl(localPath);
    } catch (e) {
      toast.error(String(e));
    }
  };

  const displayPath = (() => {
    if (!homeDir || !currentPath) return currentPath || "~";
    if (currentPath === homeDir) return "~";
    if (currentPath.startsWith(`${homeDir}/`)) return `~${currentPath.slice(homeDir.length)}`;
    return currentPath;
  })();

  return (
    <aside
      className="h-full flex flex-col overflow-hidden"
      style={{ backgroundColor: "var(--df-bg-panel)" }}
    >
      <div
        className="p-2 text-[0.625rem] uppercase tracking-wider font-bold border-b flex justify-between items-center"
        style={{ color: "var(--df-text-muted)", borderColor: "var(--df-border)" }}
      >
        <span>{t("panel.fileExplorer")}</span>
      </div>

      {activeSessionId && (
        <div
          className="flex items-center px-1.5 py-1 border-b gap-0.5"
          style={{ backgroundColor: "var(--df-bg-panel)", borderColor: "var(--df-border)" }}
        >

          <Button variant="ghost" size="icon" className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground" onClick={handleNewFile} title={t("fileExplorer.newFile")}>
            <MdNoteAdd className="h-4 w-4" />
          </Button>
          <Button variant="ghost" size="icon" className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground" onClick={handleNewFolder} title={t("fileExplorer.newFolder")}>
            <MdCreateNewFolder className="h-4 w-4" />
          </Button>

          <div className="w-px h-4 mx-1" style={{ backgroundColor: "var(--df-border)" }} />

          <Button variant="ghost" size="icon" className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground" onClick={handleUpload} title={t("fileExplorer.upload")}>
            <MdUpload className="h-4 w-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground"
            onClick={handleDownloadSelected}
            title={t("fileExplorer.downloadSelected")}
            disabled={!selectedFile || files.find((f) => f.name === selectedFile)?.is_dir}
          >
            <MdDownload className="h-4 w-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="h-7 w-7 rounded-md text-muted-foreground hover:text-destructive"
            onClick={handleDeleteSelected}
            disabled={!selectedFile}
            title={t("fileExplorer.delete")}
          >
            <MdDelete className="h-4 w-4" />
          </Button>

          <div className="w-px h-4 mx-1" style={{ backgroundColor: "var(--df-border)" }} />

          <Button variant="ghost" size="icon" className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground" onClick={handleGoUp} title={t("fileExplorer.goUp")}>
            <MdArrowUpward className="h-4 w-4" />
          </Button>
          <Button variant="ghost" size="icon" className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground" onClick={() => loadDirectory(currentPath)} title={t("fileExplorer.refresh")}>
            <MdRefresh className="h-4 w-4" />
          </Button>
        </div>
      )}

      {activeSessionId && (
        <div
          className="px-2 py-1 border-b flex items-center"
          style={{ borderColor: "var(--df-border)", minHeight: "26px" }}
        >
          {isEditingPath ? (
            <input
              autoFocus
              className="w-full text-[0.625rem] font-mono bg-transparent outline-none m-0 p-0"
              style={{ color: "var(--df-text)" }}
              value={pathInputText}
              onChange={(e) => setPathInputText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  let p = pathInputText.trim();
                  if (p) {
                    if (p.startsWith("~/")) {
                      p = homeDir + p.substring(1);
                    } else if (p === "~") {
                      p = homeDir;
                    }
                    loadDirectory(p);
                  }
                  setIsEditingPath(false);
                } else if (e.key === "Escape") {
                  setIsEditingPath(false);
                }
              }}
              onBlur={() => setIsEditingPath(false)}
            />
          ) : (
            <div
              className="text-[0.625rem] font-mono truncate cursor-text transition-colors flex-1"
              style={{ color: "var(--df-text-dimmed)" }}
              onMouseEnter={(e) => (e.currentTarget.style.color = "var(--df-text)")}
              onMouseLeave={(e) => (e.currentTarget.style.color = "var(--df-text-dimmed)")}
              onClick={() => {
                setPathInputText(currentPath || homeDir);
                setIsEditingPath(true);
              }}
              title={t("fileExplorer.editPath")}
            >
              {displayPath}
            </div>
          )}
        </div>
      )}

      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div className="flex-1 overflow-y-auto p-2 text-sm terminal-scroll">
            {!activeSessionId ? (
              <div className="text-center py-8 text-xs" style={{ color: "var(--df-text-dimmed)" }}>
                <MdFolderOff className="text-xl block mx-auto mb-2" />
                <div className="text-sm block mb-2">{t("fileExplorer.connectToSession")}</div>
              </div>
            ) : directoryLoading ? (
              <div className="text-center py-4 text-xs" style={{ color: "var(--df-text-dimmed)" }}>
                {t("fileExplorer.loading")}
              </div>
            ) : error ? (
              <div className="text-center text-red-400 py-4 text-xs">{error}</div>
            ) : files.length === 0 ? (
              <div className="text-center py-4 text-xs" style={{ color: "var(--df-text-dimmed)" }}>
                {t("fileExplorer.emptyDirectory")}
              </div>
            ) : (
              <ul className="space-y-0.5">
                {files.map((entry) => (
                  <FileListItem
                    key={entry.name}
                    entry={entry}
                    isSelected={selectedFile === entry.name}
                    activeSessionId={activeSessionId}
                    onSelect={(entry) => setSelectedFile(entry.name)}
                    onItemClick={handleItemClick}
                    onOpenDefault={handleOpenDefault}
                    onRefresh={() => loadDirectory(currentPath)}
                    onUpload={handleUpload}
                    onDownload={handleDownload}
                    onRename={(entry) => {
                      if (activeSessionId)
                        setRenameDialogData({
                          sessionId: activeSessionId,
                          oldPath: getEntryFullPath(entry),
                          name: entry.name,
                          currentDirPath: currentPath,
                        });
                    }}
                    onMove={(entry) => {
                      if (activeSessionId)
                        setMoveDialogData({
                          sessionId: activeSessionId,
                          oldPath: getEntryFullPath(entry),
                          name: entry.name,
                        });
                    }}
                    onDelete={handleDelete}
                    onCopyPath={handleCopyPath}
                    onSendToTerminal={handleSendToTerminal}
                    onProperties={(entry) => {
                      if (activeSessionId) {
                        setPropertiesDialogData({
                          sessionId: activeSessionId,
                          fullPath: getEntryFullPath(entry),
                          name: entry.name,
                          is_dir: entry.is_dir,
                        });
                      }
                    }}
                  />
                ))}
              </ul>
            )}
          </div>
        </ContextMenuTrigger>
        {activeSessionId && (
          <ContextMenuContent className="w-52">
            <ContextMenuItem onClick={() => loadDirectory(currentPath)}>
              <MdRefresh className="mr-2 h-4 w-4" />
              {t("fileExplorer.refresh")}
            </ContextMenuItem>
            <ContextMenuItem onClick={handleUpload}>
              <MdUpload className="mr-2 h-4 w-4" />
              {t("fileExplorer.upload")}
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem onClick={handleNewFile}>
              <MdNoteAdd className="mr-2 h-4 w-4" />
              {t("fileExplorer.newFile")}
            </ContextMenuItem>
            <ContextMenuItem onClick={handleNewFolder}>
              <MdCreateNewFolder className="mr-2 h-4 w-4" />
              {t("fileExplorer.newFolder")}
            </ContextMenuItem>
            <ContextMenuItem onClick={handleNewSymlink}>
              <MdLink className="mr-2 h-4 w-4" />
              {t("fileExplorer.newSymlink")}
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem onClick={handleCopyCurrentPath}>
              <MdContentCopy className="mr-2 h-4 w-4" />
              {t("fileExplorer.copyDirPath")}
            </ContextMenuItem>
            <ContextMenuItem onClick={handleSendCurrentPathToTerminal}>
              <MdSend className="mr-2 h-4 w-4" />
              {t("fileExplorer.sendDirPathToTerminal")}
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem onClick={handleCurrentDirProperties}>
              <MdInfo className="mr-2 h-4 w-4" />
              {t("fileExplorer.properties")}
            </ContextMenuItem>
          </ContextMenuContent>
        )}
      </ContextMenu>

      {activeSessionId && (
        <div
          className="px-2 py-1.5 text-[0.6875rem] border-t flex items-center justify-between shrink-0"
          style={{
            color: "var(--df-text-dimmed)",
            borderColor: "var(--df-border)",
            backgroundColor: "var(--df-bg-panel)",
          }}
        >
          <div className="flex gap-4">
            {!directoryLoading && !error && files.length > 0 && (
              <>
                <span>{t("fileExplorer.totalItems", { count: files.length })}</span>
                {files.some((f) => !f.is_dir) && (
                  <span>{formatSize(files.filter((f) => !f.is_dir).reduce((sum, f) => sum + f.size, 0))}</span>
                )}
              </>
            )}
          </div>
          <div className="flex items-center gap-0.5">
            <Button
              variant="ghost"
              size="icon"
              className="h-6 w-6 rounded-md text-muted-foreground hover:text-foreground"
              onClick={handleSyncCwd}
              title={t("fileExplorer.syncTerminalPath")}
            >
              <MdSync className="h-[0.875rem] w-[0.875rem]" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              className={`h-6 w-6 rounded-md ${autoSyncCwd ? "text-primary" : "text-muted-foreground hover:text-foreground"}`}
              onClick={() => setAutoSyncCwd((v) => !v)}
              title={t("fileExplorer.autoSyncTerminalPath")}
            >
              <MdSyncLock className="h-[0.875rem] w-[0.875rem]" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              className="h-6 w-6 rounded-md text-muted-foreground hover:text-foreground"
              onClick={() => {
                if (activeSessionId && currentPath) {
                  invoke("write_to_session", {
                    sessionId: activeSessionId,
                    data: `${currentPath}`,
                  });
                  emit(`focus-terminal-${activeSessionId}`);
                }
              }}
              title={t("fileExplorer.sendToTerminal")}
            >
              <MdSend className="h-[0.875rem] w-[0.875rem]" />
            </Button>
          </div>
        </div>
      )}

      {renameDialogData && (
        <RenameDialog
          data={renameDialogData}
          onClose={() => setRenameDialogData(null)}
          onSuccess={() => loadDirectory(currentPath)}
        />
      )}

      {deleteDialogData && (
        <DeleteDialog
          data={deleteDialogData}
          onClose={() => setDeleteDialogData(null)}
          onSuccess={() => loadDirectory(currentPath)}
        />
      )}

      {moveDialogData && (
        <MoveDialog
          data={moveDialogData}
          onClose={() => setMoveDialogData(null)}
          onSuccess={() => loadDirectory(currentPath)}
        />
      )}

      {newItemDialogData && (
        <NewItemDialog
          data={newItemDialogData}
          onClose={() => setNewItemDialogData(null)}
          onSuccess={async (result) => {
            await loadDirectory(currentPath);
            if (result.openAfterCreate) {
              const mockEntry: FileEntry = {
                name: result.name,
                is_dir: result.is_dir,
                is_symlink: false,
                size: 0,
                permissions: "",
              };
              if (result.is_dir) {
                handleItemClick(mockEntry);
              } else {
                handleOpenDefault(mockEntry);
              }
            }
          }}
        />
      )}

      {autoUploadDialogData && (
        <AutoUploadDialog
          data={autoUploadDialogData}
          onClose={() => setAutoUploadDialogData(null)}
          onAlwaysUpload={(sessionId, localPath) => {
            const key = `${sessionId}:${localPath}`;
            setAlwaysUploadFiles((prev) => new Set([...prev, key]));
          }}
        />
      )}

      {propertiesDialogData && (
        <PropertiesDialog
          data={propertiesDialogData}
          onClose={() => setPropertiesDialogData(null)}
        />
      )}

      {newSymlinkDialogData && (
        <NewSymlinkDialog
          data={newSymlinkDialogData}
          onClose={() => setNewSymlinkDialogData(null)}
          onSuccess={() => loadDirectory(currentPath)}
        />
      )}
    </aside>
  );
}
