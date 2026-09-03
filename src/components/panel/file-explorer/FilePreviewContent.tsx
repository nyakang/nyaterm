import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { useVirtualizer } from "@tanstack/react-virtual";
import Papa from "papaparse";
import type {
  PDFDocumentProxy,
  RenderTask,
} from "pdfjs-dist/types/src/display/api";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdChevronLeft,
  MdChevronRight,
  MdErrorOutline,
  MdFitScreen,
  MdKeyboardArrowDown,
  MdKeyboardArrowUp,
  MdRestartAlt,
  MdRotateLeft,
  MdRotateRight,
  MdUnfoldMore,
  MdZoomIn,
  MdZoomOut,
} from "react-icons/md";
import ReactMarkdown from "react-markdown";
import { TransformComponent, TransformWrapper } from "react-zoom-pan-pinch";
import remarkGfm from "remark-gfm";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { codeMirrorFileViewExtensions } from "@/lib/codeMirrorFileView";
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import { cn } from "@/lib/utils";
import type { FileDocumentPane } from "@/types/global";
import FileDocumentEditor from "./FileDocumentEditor";
import {
  type FileExplorerBackendKind,
  type FilePreviewKind,
  getFileExtension,
  getFilePreviewKind,
  imageMimeFromFilename,
  languageFromFilename,
  type RemoteBinaryFile,
  type RemoteTextFile,
} from "./model";

const TEXT_PREVIEW_MAX_BYTES = 5 * 1024 * 1024;
const CSV_PREVIEW_MAX_BYTES = 10 * 1024 * 1024;
const BINARY_PREVIEW_MAX_BYTES = 25 * 1024 * 1024;
const CSV_ROW_HEIGHT = 30;
const IMAGE_WHEEL_ZOOM_STEP = 0.001;
const IMAGE_BUTTON_ZOOM_STEP = 0.2;
type PdfJsApi = typeof import("pdfjs-dist");
type CsvSortDirection = "asc" | "desc";

export interface FilePreviewContentData {
  sessionId: string;
  backend?: FileExplorerBackendKind;
  path: string;
  name: string;
  size: number;
  mtime: number;
}

export interface FilePreviewLoadSummary {
  status: "idle" | "loading" | "ready" | "error";
  kind: FilePreviewKind;
  message?: string;
}

interface FilePreviewModeProps {
  mode: "preview";
  data: FilePreviewContentData;
  reloadKey?: number;
  active?: boolean;
  className?: string;
  onLoadStateChange?: (summary: FilePreviewLoadSummary) => void;
}

interface FileEditModeProps {
  mode: "edit";
  pane: FileDocumentPane;
  active?: boolean;
  className?: string;
}

export type FilePreviewContentProps = FilePreviewModeProps | FileEditModeProps;

type PreviewLoadState =
  | { status: "idle" | "loading" }
  | { status: "error"; message: string; kind: FilePreviewKind }
  | {
      status: "text";
      kind: "markdown" | "csv" | "json" | "text";
      file: RemoteTextFile;
    }
  | { status: "binary"; kind: "image" | "pdf"; file: RemoteBinaryFile };

export function FilePreviewContent(props: FilePreviewContentProps) {
  if (props.mode === "edit") {
    return (
      <div className={cn("h-full min-h-0 overflow-hidden", props.className)}>
        <FileDocumentEditor pane={props.pane} active={props.active ?? true} />
      </div>
    );
  }

  return <FilePreview {...props} />;
}

function FilePreview({
  data,
  reloadKey = 0,
  active = true,
  className,
  onLoadStateChange,
}: FilePreviewModeProps) {
  const { t } = useTranslation();
  const [state, setState] = useState<PreviewLoadState>({ status: "idle" });
  const requestIdRef = useRef(0);
  const onLoadStateChangeRef = useRef(onLoadStateChange);

  useEffect(() => {
    onLoadStateChangeRef.current = onLoadStateChange;
  }, [onLoadStateChange]);

  useEffect(() => {
    void reloadKey;
    const requestId = requestIdRef.current + 1;
    requestIdRef.current = requestId;
    const previewKind = getFilePreviewKind(data.name);
    const notify = (summary: FilePreviewLoadSummary) => {
      onLoadStateChangeRef.current?.(summary);
    };

    if (previewKind === "unsupported") {
      setState({
        status: "error",
        kind: previewKind,
        message: t("filePreview.unsupported"),
      });
      notify({
        status: "error",
        kind: previewKind,
        message: t("filePreview.unsupported"),
      });
      return;
    }

    setState({ status: "loading" });
    notify({ status: "loading", kind: previewKind });

    const backend = data.backend ?? "remote";
    const commandPrefix = backend === "local" ? "local" : "remote";
    const isBinary = previewKind === "image" || previewKind === "pdf";
    const maxBytes =
      previewKind === "csv"
        ? CSV_PREVIEW_MAX_BYTES
        : isBinary
          ? BINARY_PREVIEW_MAX_BYTES
          : TEXT_PREVIEW_MAX_BYTES;
    const command = isBinary
      ? `read_${commandPrefix}_file_bytes`
      : `read_${commandPrefix}_file_text`;

    invoke<RemoteBinaryFile | RemoteTextFile>(command, {
      sessionId: data.sessionId,
      path: data.path,
      maxBytes,
    })
      .then((file) => {
        if (requestIdRef.current !== requestId) return;
        if (isBinary) {
          setState({
            status: "binary",
            kind: previewKind,
            file: file as RemoteBinaryFile,
          });
        } else {
          setState({
            status: "text",
            kind: previewKind,
            file: file as RemoteTextFile,
          });
        }
        notify({ status: "ready", kind: previewKind });
      })
      .catch((error) => {
        if (requestIdRef.current !== requestId) return;
        const message = getErrorMessage(error) || String(error);
        setState({
          status: "error",
          kind: previewKind,
          message,
        });
        notify({ status: "error", kind: previewKind, message });
      });
  }, [data.backend, data.name, data.path, data.sessionId, reloadKey, t]);

  return (
    <div
      className={cn("h-full min-h-0 overflow-hidden", className)}
      data-file-document-mode="preview"
    >
      {renderPreviewBody(state, data.name, t, active)}
    </div>
  );
}

function renderPreviewBody(
  state: PreviewLoadState,
  fileName: string,
  t: ReturnType<typeof useTranslation>["t"],
  active: boolean,
) {
  switch (state.status) {
    case "idle":
    case "loading":
      return (
        <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
          {t("filePreview.loading")}
        </div>
      );
    case "error":
      return <PreviewError message={state.message} />;
    case "binary":
      return state.kind === "image" ? (
        active ? (
          <ImagePreview file={state.file} fileName={fileName} />
        ) : null
      ) : (
        <PdfPreview file={state.file} />
      );
    case "text":
      if (state.kind === "markdown")
        return <MarkdownPreview content={state.file.content} />;
      if (state.kind === "csv") {
        return (
          <CsvPreview
            content={state.file.content}
            extension={getFileExtension(fileName)}
          />
        );
      }
      if (state.kind === "json")
        return <JsonPreview content={state.file.content} />;
      return <TextPreview content={state.file.content} fileName={fileName} />;
  }
}

function PreviewError({ message }: { message: string }) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 px-6 text-center text-sm text-muted-foreground">
      <MdErrorOutline className="h-7 w-7 text-destructive" />
      <div className="max-w-xl break-words">{message}</div>
    </div>
  );
}

function binaryFileBytes(file: RemoteBinaryFile) {
  const { contentBytes } = file;
  if (contentBytes instanceof Uint8Array) return contentBytes;
  if (contentBytes instanceof ArrayBuffer) return new Uint8Array(contentBytes);
  return Uint8Array.from(contentBytes);
}

function ImagePreview({
  file,
  fileName,
}: {
  file: RemoteBinaryFile;
  fileName: string;
}) {
  const { t } = useTranslation();
  const [scale, setScale] = useState(1);
  const [rotation, setRotation] = useState(0);
  const [src, setSrc] = useState("");
  const bytes = useMemo(() => binaryFileBytes(file), [file]);
  const mime = imageMimeFromFilename(fileName);

  useEffect(() => {
    const url = URL.createObjectURL(new Blob([bytes], { type: mime }));
    setSrc(url);
    return () => URL.revokeObjectURL(url);
  }, [bytes, mime]);

  return (
    <TransformWrapper
      key={src}
      centerOnInit
      centerZoomedOut
      minScale={0.1}
      maxScale={8}
      limitToBounds={false}
      wheel={{ step: IMAGE_WHEEL_ZOOM_STEP }}
      panning={{ velocityDisabled: true }}
      onTransform={(_, state) => setScale(state.scale)}
    >
      {({ centerView, resetTransform, zoomIn, zoomOut }) => (
        <div className="flex h-full min-h-0 flex-col">
          <PreviewToolbar>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="h-7 w-7"
              aria-label={t("filePreview.fit")}
              title={t("filePreview.fit")}
              onClick={() => {
                centerView(1, 120);
                setScale(1);
              }}
            >
              <MdFitScreen className="h-4 w-4" />
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="h-7 w-7"
              aria-label={t("filePreview.zoomOut")}
              title={t("filePreview.zoomOut")}
              onClick={() => zoomOut(IMAGE_BUTTON_ZOOM_STEP, 120)}
            >
              <MdZoomOut className="h-4 w-4" />
            </Button>
            <span className="min-w-12 text-center text-xs text-muted-foreground">
              {Math.round(scale * 100)}%
            </span>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="h-7 w-7"
              aria-label={t("filePreview.zoomIn")}
              title={t("filePreview.zoomIn")}
              onClick={() => zoomIn(IMAGE_BUTTON_ZOOM_STEP, 120)}
            >
              <MdZoomIn className="h-4 w-4" />
            </Button>
            <span className="mx-1 h-4 w-px bg-border" />
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="h-7 w-7"
              aria-label={t("filePreview.rotateLeft")}
              title={t("filePreview.rotateLeft")}
              onClick={() => setRotation((value) => value - 90)}
            >
              <MdRotateLeft className="h-4 w-4" />
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="h-7 w-7"
              aria-label={t("filePreview.rotateRight")}
              title={t("filePreview.rotateRight")}
              onClick={() => setRotation((value) => value + 90)}
            >
              <MdRotateRight className="h-4 w-4" />
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="h-7 w-7"
              aria-label={t("filePreview.resetView")}
              title={t("filePreview.resetView")}
              onClick={() => {
                resetTransform(120);
                setRotation(0);
                setScale(1);
              }}
            >
              <MdRestartAlt className="h-4 w-4" />
            </Button>
          </PreviewToolbar>
          <div className="min-h-0 flex-1 bg-background/60">
            <TransformComponent
              wrapperClass="!h-full !w-full"
              contentClass="!h-full !w-full"
              wrapperStyle={{ width: "100%", height: "100%" }}
              contentStyle={{ width: "100%", height: "100%" }}
            >
              <div className="flex h-full w-full items-center justify-center p-4">
                {src ? (
                  <img
                    src={src}
                    alt={fileName}
                    draggable={false}
                    className="max-h-full max-w-full select-none object-contain"
                    style={{ transform: `rotate(${rotation}deg)` }}
                  />
                ) : null}
              </div>
            </TransformComponent>
          </div>
        </div>
      )}
    </TransformWrapper>
  );
}

function MarkdownPreview({ content }: { content: string }) {
  return (
    <div className="terminal-scroll h-full overflow-auto bg-background/60 p-4 text-sm leading-6">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        skipHtml
        components={{
          a: ({ children, href }) => (
            <a
              className="text-primary underline underline-offset-2"
              href={href}
              target="_blank"
              rel="noreferrer"
            >
              {children}
            </a>
          ),
          img: ({ alt }) => (
            <span className="rounded bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
              {alt || "image"}
            </span>
          ),
          pre: ({ children }) => (
            <pre className="terminal-scroll my-3 overflow-auto rounded-md border bg-muted/30 p-3 text-xs leading-5">
              {children}
            </pre>
          ),
          code: ({ children }) => (
            <code className="rounded bg-muted/40 px-1 py-0.5 font-mono text-xs">
              {children}
            </code>
          ),
          table: ({ children }) => (
            <div className="terminal-scroll my-3 overflow-auto">
              <table className="w-full border-collapse text-left text-xs">
                {children}
              </table>
            </div>
          ),
          th: ({ children }) => (
            <th className="border px-2 py-1 font-medium">{children}</th>
          ),
          td: ({ children }) => (
            <td className="border px-2 py-1">{children}</td>
          ),
        }}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}

function normalizeCsvCell(value: unknown) {
  return value == null ? "" : String(value).trim();
}

function isPlainDecimal(value: string) {
  return /^[+-]?(?:\d+\.?\d*|\.\d+)$/.test(value);
}

function compareCsvCells(
  leftValue: unknown,
  rightValue: unknown,
  direction: CsvSortDirection,
) {
  const left = normalizeCsvCell(leftValue);
  const right = normalizeCsvCell(rightValue);
  const leftEmpty = left.length === 0;
  const rightEmpty = right.length === 0;

  if (leftEmpty && rightEmpty) return 0;
  if (leftEmpty) return 1;
  if (rightEmpty) return -1;

  const directionMultiplier = direction === "asc" ? 1 : -1;
  if (isPlainDecimal(left) && isPlainDecimal(right)) {
    const numericCompare = Number(left) - Number(right);
    if (numericCompare !== 0) return numericCompare * directionMultiplier;
  }

  return (
    left.localeCompare(right, undefined, {
      numeric: true,
      sensitivity: "base",
    }) * directionMultiplier
  );
}

function CsvPreview({
  content,
  extension,
}: {
  content: string;
  extension: string;
}) {
  const { t } = useTranslation();
  const [firstRowHeader, setFirstRowHeader] = useState(true);
  const [sortColumnIndex, setSortColumnIndex] = useState<number | null>(null);
  const [sortDirection, setSortDirection] = useState<CsvSortDirection>("asc");
  const scrollParentRef = useRef<HTMLDivElement | null>(null);
  const parsedRows = useMemo(() => {
    const result = Papa.parse<string[]>(content, {
      delimiter: extension === "tsv" ? "\t" : "",
      skipEmptyLines: false,
    });
    return result.data.filter((row) =>
      row.some((cell) => String(cell ?? "").length > 0),
    );
  }, [content, extension]);

  const headers = useMemo(() => {
    const first = parsedRows[0] ?? [];
    const columnCount = Math.max(
      ...parsedRows.map((row) => row.length),
      first.length,
      1,
    );
    if (firstRowHeader && first.length > 0) {
      return Array.from(
        { length: columnCount },
        (_, index) => first[index] || `#${index + 1}`,
      );
    }
    return Array.from({ length: columnCount }, (_, index) => `#${index + 1}`);
  }, [firstRowHeader, parsedRows]);

  const rows = useMemo(
    () => (firstRowHeader ? parsedRows.slice(1) : parsedRows),
    [firstRowHeader, parsedRows],
  );
  const sortedRows = useMemo(() => {
    if (sortColumnIndex == null) return rows;

    return rows
      .map((row, index) => ({ row, index }))
      .sort((left, right) => {
        const cellCompare = compareCsvCells(
          left.row[sortColumnIndex],
          right.row[sortColumnIndex],
          sortDirection,
        );
        return cellCompare || left.index - right.index;
      })
      .map((entry) => entry.row);
  }, [rows, sortColumnIndex, sortDirection]);
  const columns = headers.map((label, columnIndex) => ({
    id: `column-${columnIndex}-${label}`,
    label,
    index: columnIndex,
  }));
  const gridTemplateColumns = `repeat(${headers.length}, minmax(120px, 1fr))`;
  const rowVirtualizer = useVirtualizer({
    count: sortedRows.length,
    getScrollElement: () => scrollParentRef.current,
    estimateSize: () => CSV_ROW_HEIGHT,
    overscan: 8,
  });

  useEffect(() => {
    if (sortColumnIndex != null && sortColumnIndex >= headers.length) {
      setSortColumnIndex(null);
    }
  }, [headers.length, sortColumnIndex]);

  const toggleSort = (columnIndex: number) => {
    if (sortColumnIndex !== columnIndex) {
      setSortColumnIndex(columnIndex);
      setSortDirection("asc");
      return;
    }

    if (sortDirection === "asc") {
      setSortDirection("desc");
      return;
    }

    setSortColumnIndex(null);
    setSortDirection("asc");
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <PreviewToolbar>
        <label className="flex items-center gap-2 text-xs text-muted-foreground">
          <Checkbox
            checked={firstRowHeader}
            onCheckedChange={(checked) => setFirstRowHeader(checked === true)}
          />
          {t("filePreview.firstRowHeader")}
        </label>
        <span className="ml-auto text-xs text-muted-foreground">
          {t("filePreview.csvRows", { count: sortedRows.length })}
        </span>
      </PreviewToolbar>
      <div
        ref={scrollParentRef}
        className="terminal-scroll min-h-0 flex-1 overflow-auto bg-background/60"
      >
        <div className="min-w-max">
          <table
            className="sticky top-0 z-10 grid border-b bg-muted text-xs font-medium"
            style={{ gridTemplateColumns }}
          >
            <thead className="contents">
              <tr className="contents">
                {columns.map((column) => {
                  const isSorted = sortColumnIndex === column.index;
                  const nextSortLabel = !isSorted
                    ? t("filePreview.sortAscending")
                    : sortDirection === "asc"
                      ? t("filePreview.sortDescending")
                      : t("filePreview.clearSort");
                  const sortedLabel =
                    isSorted && sortDirection === "asc"
                      ? t("filePreview.sortedAscending")
                      : isSorted
                        ? t("filePreview.sortedDescending")
                        : undefined;

                  return (
                    <th
                      key={column.id}
                      scope="col"
                      aria-sort={
                        isSorted
                          ? sortDirection === "asc"
                            ? "ascending"
                            : "descending"
                          : "none"
                      }
                      className={cn(
                        "group min-w-0 border-r p-0 text-left",
                        isSorted && "bg-primary/10 text-primary",
                      )}
                    >
                      <button
                        type="button"
                        className="flex h-full w-full min-w-0 items-center gap-1.5 px-2 py-2 text-left outline-none transition-colors hover:bg-muted-foreground/10 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/50"
                        title={`${nextSortLabel}: ${column.label}`}
                        aria-label={`${nextSortLabel}: ${column.label}`}
                        onClick={() => toggleSort(column.index)}
                      >
                        <span className="min-w-0 flex-1 truncate">
                          {column.label}
                        </span>
                        {isSorted && sortDirection === "asc" ? (
                          <MdKeyboardArrowUp
                            className="h-4 w-4 shrink-0"
                            aria-label={sortedLabel}
                          />
                        ) : isSorted ? (
                          <MdKeyboardArrowDown
                            className="h-4 w-4 shrink-0"
                            aria-label={sortedLabel}
                          />
                        ) : (
                          <MdUnfoldMore
                            className="h-4 w-4 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-60"
                            aria-hidden="true"
                          />
                        )}
                      </button>
                    </th>
                  );
                })}
              </tr>
            </thead>
          </table>
          <div
            className="relative"
            style={{
              height: rowVirtualizer.getTotalSize(),
            }}
          >
            {rowVirtualizer.getVirtualItems().map((virtualRow) => {
              const item = sortedRows[virtualRow.index] ?? [];
              return (
                <div
                  key={virtualRow.key}
                  className="absolute left-0 top-0 grid w-full border-b text-xs"
                  style={{
                    height: virtualRow.size,
                    transform: `translateY(${virtualRow.start}px)`,
                    gridTemplateColumns,
                  }}
                >
                  {columns.map((column) => (
                    <div
                      key={column.id}
                      className="truncate border-r px-2 py-1.5"
                    >
                      {item[column.index] ?? ""}
                    </div>
                  ))}
                </div>
              );
            })}
          </div>
        </div>
      </div>
    </div>
  );
}

function JsonPreview({ content }: { content: string }) {
  const { t } = useTranslation();
  const formatted = useMemo(() => {
    try {
      return {
        content: JSON.stringify(JSON.parse(content), null, 2),
        error: "",
      };
    } catch (error) {
      return {
        content,
        error: getErrorMessage(error) || t("filePreview.jsonParseFailed"),
      };
    }
  }, [content, t]);

  return (
    <div className="flex h-full min-h-0 flex-col">
      {formatted.error && (
        <div className="border-b bg-destructive/10 px-3 py-2 text-xs text-destructive">
          {formatted.error}
        </div>
      )}
      <TextPreview content={formatted.content} language="json" />
    </div>
  );
}

function TextPreview({
  content,
  fileName,
  language,
}: {
  content: string;
  fileName?: string;
  language?: string;
}) {
  return (
    <ReadOnlyCodeMirror
      content={content}
      language={language ?? languageFromFilename(fileName ?? "")}
    />
  );
}

function ReadOnlyCodeMirror({
  content,
  language,
}: {
  content: string;
  language: string;
}) {
  const parentRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const parent = parentRef.current;
    if (!parent) return;

    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: content,
        extensions: codeMirrorFileViewExtensions(language, { editable: false }),
      }),
    });

    return () => view.destroy();
  }, [content, language]);

  return (
    <div
      ref={parentRef}
      className="h-full min-h-0 bg-background/60"
      style={{ fontSize: "13px" }}
    />
  );
}

function PdfPreview({ file }: { file: RemoteBinaryFile }) {
  const { t } = useTranslation();
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const renderTaskRef = useRef<RenderTask | null>(null);
  const [pdfApi, setPdfApi] = useState<PdfJsApi | null>(null);
  const [documentProxy, setDocumentProxy] = useState<PDFDocumentProxy | null>(
    null,
  );
  const [pageNumber, setPageNumber] = useState(1);
  const [scale, setScale] = useState(1);
  const [error, setError] = useState("");

  useEffect(() => {
    let cancelled = false;
    let loadingTask: ReturnType<PdfJsApi["getDocument"]> | null = null;
    setPdfApi(null);
    setDocumentProxy(null);
    setPageNumber(1);
    setError("");

    const bytes = binaryFileBytes(file);
    Promise.all([
      import("pdfjs-dist"),
      import("pdfjs-dist/build/pdf.worker.mjs?url"),
    ])
      .then(([pdfjs, worker]) => {
        if (cancelled) return;
        pdfjs.GlobalWorkerOptions.workerSrc = worker.default;
        setPdfApi(pdfjs);
        loadingTask = pdfjs.getDocument({ data: bytes });
        return loadingTask.promise;
      })
      .then((pdf) => {
        if (!cancelled && pdf) setDocumentProxy(pdf);
      })
      .catch((loadError) => {
        if (!cancelled)
          setError(getErrorMessage(loadError) || String(loadError));
      });

    return () => {
      cancelled = true;
      loadingTask?.destroy();
    };
  }, [file]);

  useEffect(() => {
    if (!pdfApi || !documentProxy || !canvasRef.current) return;
    let cancelled = false;
    const canvas = canvasRef.current;

    documentProxy
      .getPage(pageNumber)
      .then((page) => {
        if (cancelled) return;
        renderTaskRef.current?.cancel();
        const viewport = page.getViewport({ scale });
        const context = canvas.getContext("2d");
        if (!context) return;
        canvas.width = Math.floor(viewport.width);
        canvas.height = Math.floor(viewport.height);
        const renderTask = page.render({
          canvas,
          canvasContext: context,
          viewport,
        });
        renderTaskRef.current = renderTask;
        return renderTask.promise;
      })
      .catch((renderError) => {
        if (!cancelled && renderError?.name !== "RenderingCancelledException") {
          setError(getErrorMessage(renderError) || String(renderError));
        }
      });

    return () => {
      cancelled = true;
      renderTaskRef.current?.cancel();
    };
  }, [documentProxy, pageNumber, pdfApi, scale]);

  if (error) return <PreviewError message={error} />;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <PreviewToolbar>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-7 w-7"
          disabled={!documentProxy || pageNumber <= 1}
          onClick={() => setPageNumber((value) => Math.max(1, value - 1))}
        >
          <MdChevronLeft className="h-4 w-4" />
        </Button>
        <span className="text-xs text-muted-foreground">
          {documentProxy
            ? t("filePreview.pdfPage", {
                page: pageNumber,
                total: documentProxy.numPages,
              })
            : t("filePreview.loading")}
        </span>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-7 w-7"
          disabled={!documentProxy || pageNumber >= documentProxy.numPages}
          onClick={() =>
            setPageNumber((value) =>
              Math.min(documentProxy?.numPages ?? value, value + 1),
            )
          }
        >
          <MdChevronRight className="h-4 w-4" />
        </Button>
        <span className="mx-1 h-4 w-px bg-border" />
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-7 w-7"
          onClick={() => setScale((value) => Math.max(0.5, value - 0.25))}
        >
          <MdZoomOut className="h-4 w-4" />
        </Button>
        <span className="min-w-12 text-center text-xs text-muted-foreground">
          {Math.round(scale * 100)}%
        </span>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-7 w-7"
          onClick={() => setScale((value) => Math.min(3, value + 0.25))}
        >
          <MdZoomIn className="h-4 w-4" />
        </Button>
      </PreviewToolbar>
      <div className="terminal-scroll flex min-h-0 flex-1 justify-center overflow-auto bg-background/60 p-4">
        <canvas
          ref={canvasRef}
          className={cn("h-fit max-w-none bg-white shadow-sm")}
        />
      </div>
    </div>
  );
}

function PreviewToolbar({ children }: { children: ReactNode }) {
  return (
    <div className="flex h-10 shrink-0 items-center gap-1 border-b bg-background/80 px-2">
      {children}
    </div>
  );
}
