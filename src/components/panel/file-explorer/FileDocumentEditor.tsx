import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdRefresh, MdSave } from "react-icons/md";
import { toast } from "sonner";
import ReloadDirtyDialog from "@/components/dialog/remote-file-editor/ReloadDirtyDialog";
import RemoteFileConflictDialog from "@/components/dialog/remote-file-editor/RemoteFileConflictDialog";
import { Button } from "@/components/ui/button";
import { codeMirrorFileViewExtensions } from "@/lib/codeMirrorFileView";
import { getErrorMessage } from "@/lib/errors";
import { MAX_EDITOR_FILE_BYTES } from "@/lib/fileEditorLimits";
import {
  type FileDocumentSaveResult,
  registerFileDocument,
  updateFileDocumentState,
} from "@/lib/fileDocumentRegistry";
import { useApp } from "@/context/AppContext";
import { invoke } from "@/lib/invoke";
import { clampFileEditorFontSize } from "@/lib/fileEditorFontSize";
import { formatSize } from "@/lib/utils";
import type { FileDocumentPane } from "@/types/global";
import { languageFromFilename, type TextFileOpenResult } from "./model";

interface WriteFileTextResult {
  status: "saved" | "conflict";
  mtime?: number;
  mtimeNanos?: string;
  size?: number;
  contentHash?: string;
}

interface FileDocumentEditorProps {
  pane: FileDocumentPane;
  active: boolean;
}

export default function FileDocumentEditor({ pane, active }: FileDocumentEditorProps) {
  const { t } = useTranslation();
  const { appSettings } = useApp();
  const editorFontSize = clampFileEditorFontSize(
    appSettings.transfer.internal_editor_font_size,
  );
  const editorParentRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  const suppressUpdateRef = useRef(false);
  const savingRef = useRef(false);
  const savePromiseRef = useRef<Promise<FileDocumentSaveResult> | null>(null);
  const contentRef = useRef(pane.file.initial.content);
  const baseRef = useRef({
    content: pane.file.initial.content,
    size: pane.file.initial.size,
    mtime: pane.file.initial.mtime,
    mtimeNanos: pane.file.initial.mtimeNanos,
    contentHash: pane.file.initial.contentHash,
  });
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [size, setSize] = useState(pane.file.initial.size);
  const [mtime, setMtime] = useState(pane.file.initial.mtime);
  const [error, setError] = useState("");
  const [lastSavedAt, setLastSavedAt] = useState<number | null>(null);
  const [conflictOpen, setConflictOpen] = useState(false);
  const [reloadConfirmOpen, setReloadConfirmOpen] = useState(false);

  const replaceEditorContent = useCallback((content: string) => {
    const view = viewRef.current;
    contentRef.current = content;
    if (!view) return;
    suppressUpdateRef.current = true;
    try {
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: content },
      });
    } finally {
      suppressUpdateRef.current = false;
    }
  }, []);

  const discard = useCallback(() => {
    replaceEditorContent(baseRef.current.content);
    setDirty(false);
    setError("");
  }, [replaceEditorContent]);

  const save = useCallback(
    async (force = false): Promise<FileDocumentSaveResult> => {
      if (savingRef.current) return savePromiseRef.current ?? "saved";
      const runSave = async (): Promise<FileDocumentSaveResult> => {
        savingRef.current = true;
        setSaving(true);
        setError("");
        try {
          const result = await invoke<WriteFileTextResult>(
            pane.file.backend === "local" ? "write_local_file_text" : "write_remote_file_text",
            {
              sessionId: pane.sessionId,
              path: pane.file.path,
              content: contentRef.current,
              expectedMtime: baseRef.current.mtime,
              expectedSize: baseRef.current.size,
              expectedMtimeNanos: baseRef.current.mtimeNanos,
              expectedHash: baseRef.current.contentHash,
              force,
            },
          );
          if (result.status === "conflict") {
            setConflictOpen(true);
            return "conflict";
          }

          const nextSize = result.size ?? new Blob([contentRef.current]).size;
          const nextMtime = result.mtime ?? baseRef.current.mtime;
          const nextMtimeNanos = result.mtimeNanos ?? baseRef.current.mtimeNanos;
          const nextContentHash = result.contentHash ?? baseRef.current.contentHash;
          baseRef.current = {
            content: contentRef.current,
            size: nextSize,
            mtime: nextMtime,
            mtimeNanos: nextMtimeNanos,
            contentHash: nextContentHash,
          };
          setSize(nextSize);
          setMtime(nextMtime);
          setDirty(false);
          setLastSavedAt(Date.now());
          toast.success(t("fileEditor.saved"));
          return "saved";
        } catch (saveError) {
          setError(getErrorMessage(saveError) || t("fileEditor.saveFailed"));
          return "conflict";
        } finally {
          savingRef.current = false;
          savePromiseRef.current = null;
          setSaving(false);
        }
      };
      const promise = runSave();
      savePromiseRef.current = promise;
      return promise;
    },
    [pane.file.backend, pane.file.path, pane.sessionId, t],
  );

  const reload = useCallback(async () => {
    setError("");
    try {
      const result = await invoke<TextFileOpenResult>(
        pane.file.backend === "local" ? "open_local_file_text" : "open_remote_file_text",
        {
          sessionId: pane.sessionId,
          path: pane.file.path,
          maxBytes: MAX_EDITOR_FILE_BYTES,
        },
      );
      if (result.status === "unsupported") {
        setError(
          t(
            result.reason === "binary"
              ? "fileEditor.reloadBinary"
              : "fileEditor.reloadUnsupportedEncoding",
          ),
        );
        return;
      }

      baseRef.current = {
        content: result.file.content,
        size: result.file.size,
        mtime: result.file.mtime ?? baseRef.current.mtime,
        mtimeNanos: result.file.mtimeNanos,
        contentHash: result.file.contentHash,
      };
      replaceEditorContent(result.file.content);
      setSize(result.file.size);
      setMtime(result.file.mtime ?? baseRef.current.mtime);
      setDirty(false);
      setLastSavedAt(null);
    } catch (reloadError) {
      setError(getErrorMessage(reloadError) || t("fileEditor.loadFailed"));
    }
  }, [pane.file.backend, pane.file.path, pane.sessionId, replaceEditorContent, t]);

  useEffect(() => {
    const parent = editorParentRef.current;
    if (!parent) return;

    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: contentRef.current,
        extensions: codeMirrorFileViewExtensions(
          languageFromFilename(pane.name || pane.file.path),
          {
            editable: true,
            updateListener: EditorView.updateListener.of((update) => {
              if (!update.docChanged || suppressUpdateRef.current) return;
              contentRef.current = update.state.doc.toString();
              setDirty(contentRef.current !== baseRef.current.content);
            }),
          },
        ),
      }),
    });
    viewRef.current = view;

    return () => {
      view.destroy();
      viewRef.current = null;
    };
  }, [pane.file.path, pane.name]);

  useEffect(() => {
    if (active) viewRef.current?.focus();
  }, [active]);

  const saveRef = useRef(save);
  const discardRef = useRef(discard);
  useEffect(() => {
    saveRef.current = save;
    discardRef.current = discard;
  }, [discard, save]);

  useEffect(
    () =>
      registerFileDocument(pane.id, {
        save: (force) => saveRef.current(force),
        discard: () => discardRef.current(),
      }),
    [pane.id],
  );

  useEffect(() => {
    updateFileDocumentState(pane.id, { dirty, saving });
  }, [dirty, pane.id, saving]);

  const modifiedText = mtime
    ? new Intl.DateTimeFormat(undefined, {
        dateStyle: "short",
        timeStyle: "short",
      }).format(new Date(mtime < 10_000_000_000 ? mtime * 1000 : mtime))
    : "";

  return (
    <div
      className="flex h-full min-h-0 flex-col bg-background/60"
      data-file-document-mode="edit"
      data-file-editor-root="true"
      onKeyDown={(event) => {
        if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
          event.preventDefault();
          void save();
        }
      }}
    >
      <div className="flex h-9 shrink-0 items-center gap-2 border-b px-2 text-xs">
        <span className="rounded bg-primary/10 px-2 py-0.5 font-medium text-primary">
          {t("fileEditor.title")}
        </span>
        <span className="min-w-0 flex-1 truncate text-muted-foreground" title={pane.file.path}>
          {pane.file.path}
        </span>
        {dirty ? <span className="text-amber-500">{t("fileEditor.unsaved")}</span> : null}
        {saving ? <span className="text-muted-foreground">{t("fileEditor.saving")}</span> : null}
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-7 w-7"
          title={t("fileEditor.reload")}
          aria-label={t("fileEditor.reload")}
          disabled={saving}
          onClick={() => (dirty ? setReloadConfirmOpen(true) : void reload())}
        >
          <MdRefresh className="h-4 w-4" />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-7 w-7"
          title={t("fileEditor.save")}
          aria-label={t("fileEditor.save")}
          disabled={!dirty || saving}
          onClick={() => void save()}
        >
          <MdSave className="h-4 w-4" />
        </Button>
      </div>
      {error ? (
        <div className="shrink-0 border-b border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive">
          {error}
        </div>
      ) : null}
      <div
        ref={editorParentRef}
        className="min-h-0 flex-1"
        style={{ fontSize: `${editorFontSize}px` }}
      />
      <div className="flex h-7 shrink-0 items-center justify-between border-t px-3 text-[11px] text-muted-foreground">
        <span>{languageFromFilename(pane.name || pane.file.path).toLocaleUpperCase()}</span>
        <span className="flex items-center gap-2">
          <span>{formatSize(size)}</span>
          <span>{t("fileEditor.encodingUtf8")}</span>
          {modifiedText ? <span>{t("fileEditor.modifiedAt", { time: modifiedText })}</span> : null}
          {lastSavedAt ? <span>{t("fileEditor.saved")}</span> : null}
        </span>
      </div>

      <RemoteFileConflictDialog
        open={conflictOpen}
        onOpenChange={setConflictOpen}
        onDiscardAndReload={() => {
          setConflictOpen(false);
          void reload();
        }}
        onForceSave={() => {
          setConflictOpen(false);
          void save(true);
        }}
      />
      <ReloadDirtyDialog
        open={reloadConfirmOpen}
        onOpenChange={setReloadConfirmOpen}
        onConfirm={() => {
          setReloadConfirmOpen(false);
          void reload();
        }}
      />
    </div>
  );
}
