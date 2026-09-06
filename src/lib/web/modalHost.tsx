/**
 * Web-mode child-window host.
 *
 * Desktop child windows (settings, new-session, quick-command, auto-upload,
 * file editor/preview, notes) are separate OS windows booted via
 * `?window=<type>` URLs. In web mode they render as in-document modals
 * instead:
 *
 * - `shims/core.ts` intercepts the `open_child_window` /
 *   `close_child_window` commands and routes them here
 * - the modal reuses the real `ChildWindowRouter` page components inside
 *   `ChildAppProvider`, so pages run unmodified
 * - the child-window ready handshake (`child-window-lifecycle` events with
 *   the ready token) is replayed locally so `windowManager.openChildWindow`
 *   resolves exactly as it does on the desktop
 * - closing fires the fake window's `tauri://destroyed` callbacks, which
 *   triggers the same cleanup path the desktop uses
 */

import { Component, lazy, Suspense, useEffect, useRef, useState, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";

import { ChildAppProvider } from "@/context/ChildAppProvider";
import ErrorBoundary from "@/components/ErrorBoundary";
import { ThemeProvider } from "@/context/ThemeContext";
import { Toaster } from "@/components/ui/sonner";
import { emit } from "./shims/event";
import { WebviewWindow } from "./shims/webviewWindow";
import {
  hasCloseSubscriber,
  registerModal,
  unregisterModal,
} from "./windowContext";

const MODAL_OPEN_EVENT = "nyaterm:web-modal-open";
const MODAL_CLOSE_EVENT = "nyaterm:web-modal-close";
const MODAL_CLOSE_REQUESTED_EVENT = "nyaterm:web-modal-close-requested";
const MODAL_MOUNTED_EVENT = "nyaterm:web-modal-mounted";
const READY_TOKEN_PARAM = "readyToken";

interface OpenModalDetail {
  label: string;
  title?: string;
  url?: string;
  kind?: string;
  width?: number;
  height?: number;
}

const SettingsPage = lazy(() => import("@/pages/SettingsPage"));
const NewSessionPage = lazy(() => import("@/pages/NewSessionPage"));
const QuickCommandPage = lazy(() => import("@/pages/QuickCommandPage"));
const ProxyPage = lazy(() => import("@/pages/ProxyPage"));
const TunnelPage = lazy(() => import("@/pages/TunnelPage"));
const AutoUploadPage = lazy(() => import("@/pages/FileUploadPage"));
const RemoteFileEditorPage = lazy(() => import("@/pages/RemoteFileEditorPage"));
const FilePreviewPage = lazy(() => import("@/pages/FilePreviewPage"));
const NoteEditorPage = lazy(() => import("@/pages/NoteEditorPage"));

const PAGES: Record<string, React.ComponentType> = {
  settings: SettingsPage,
  "new-session": NewSessionPage,
  "quick-command": QuickCommandPage,
  proxy: ProxyPage,
  tunnel: TunnelPage,
  "auto-upload": AutoUploadPage,
  "file-editor": RemoteFileEditorPage,
  "file-preview": FilePreviewPage,
  "note-editor": NoteEditorPage,
};

function windowTypeFromUrl(url?: string): string {
  if (!url) return "";
  try {
    const query = url.slice(url.indexOf("?"));
    return new URLSearchParams(query).get("window") ?? "";
  } catch {
    return "";
  }
}

function readyTokenFromUrl(url?: string): string | undefined {
  if (!url) return undefined;
  try {
    const query = url.slice(url.indexOf("?"));
    return new URLSearchParams(query).get(READY_TOKEN_PARAM) ?? undefined;
  } catch {
    return undefined;
  }
}

interface ModalEntry {
  label: string;
  title: string;
  windowType: string;
  width?: number;
  height?: number;
}

function ModalPage({ entry }: { entry: ModalEntry }) {
  const Page = PAGES[entry.windowType];
  if (!Page) {
    return (
      <div className="flex h-full items-center justify-center text-muted-foreground">
        Unknown window type: {entry.windowType}
      </div>
    );
  }
  // Mirrors the desktop child-window boot tree in main.tsx:
  // ErrorBoundary > ChildAppProvider > ThemeProvider > page + Toaster.
  return (
    <ModalErrorBoundary>
      <ErrorBoundary>
        <ChildAppProvider>
          <ThemeProvider>
            <Suspense
              fallback={
                <div className="flex h-full items-center justify-center">
                  <span className="size-5 animate-spin rounded-full border-2 border-primary border-t-transparent" />
                </div>
              }
            >
              <Page />
              <Toaster />
            </Suspense>
          </ThemeProvider>
        </ChildAppProvider>
      </ErrorBoundary>
    </ModalErrorBoundary>
  );
}

/** A render error inside a modal must not unmount the whole modal host
 *  (React unmounts the root when no boundary catches). Show an in-place
 *  error card and log the RAW error to the console for debugging. */
class ModalErrorBoundary extends Component<
  { children: ReactNode },
  { error: Error | null }
> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, _info: React.ErrorInfo) {
    console.error("[nyaterm-web] modal render error:", error.message, "\n", error.stack);
  }

  render() {
    if (this.state.error) {
      return (
        <div className="flex h-full flex-col items-center justify-center gap-3 p-6 text-center">
          <div className="text-sm font-semibold text-destructive">
            {this.state.error.name}: {this.state.error.message}
          </div>
          <button
            type="button"
            className="rounded-md border border-border px-3 py-1.5 text-xs text-foreground hover:bg-muted"
            onClick={() => this.setState({ error: null })}
          >
            Retry
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}

function ModalHostApp() {
  const [modals, setModals] = useState<ModalEntry[]>([]);
  const modalsRef = useRef<ModalEntry[]>([]);
  modalsRef.current = modals;
  useEffect(() => {
    const closeModal = (label: string) => {
      setModals((prev) => prev.filter((modal) => modal.label !== label));
      const handle = fakeWindows.get(label);
      handle?.fireDestroyed();
      fakeWindows.delete(label);
      unregisterModal(label);
    };

    // Register the opener callback before draining the queue so early
    // open requests (dispatched before React committed this effect) apply.
    applyOpenModal = (detail: OpenModalDetail) => {
      const windowType = windowTypeFromUrl(detail.url);
      const token = readyTokenFromUrl(detail.url);
      const fakeWindow = new WebviewWindow(detail.label);
      fakeWindows.set(detail.label, fakeWindow);
      registerModal({ label: detail.label, window: fakeWindow });
      setModals((prev) => {
        if (prev.some((modal) => modal.label === detail.label)) return prev;
        return [
          ...prev,
          {
            label: detail.label,
            title: detail.title ?? detail.label,
            windowType,
            width: detail.width,
            height: detail.height,
          },
        ];
      });
      // Replay the desktop child-window ready handshake for the opener, then
      // resolve the intercepted open_child_window invoke.
      void emit("child-window-lifecycle", {
        label: detail.label,
        token,
        phase: "load-started",
      });
      void emit("child-window-lifecycle", {
        label: detail.label,
        token,
        phase: "shell-ready",
      });
      window.dispatchEvent(
        new CustomEvent(MODAL_MOUNTED_EVENT, {
          detail: { label: detail.label, kind: "opened" },
        }),
      );
    };
    hostReady = true;
    for (const detail of queuedOpens.splice(0)) {
      applyOpenModal(detail);
    }

    const closeListener = (event: Event) => {
      const detail = (event as CustomEvent<{ label: string }>).detail;
      if (detail?.label) closeModal(detail.label);
    };

    const closeRequestedListener = (event: Event) => {
      const detail = (event as CustomEvent<{ label: string }>).detail;
      if (!detail?.label) return;
      if (!hasCloseSubscriber(detail.label)) {
        // Nobody handles close-requested (e.g. the page defers to its own
        // controls); close directly so the chrome button still works.
        window.setTimeout(() => {
          window.dispatchEvent(
            new CustomEvent(MODAL_CLOSE_EVENT, { detail: { label: detail.label } }),
          );
        }, 0);
      }
    };

    const keyListener = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      const stack = modalsRef.current;
      const top = stack[stack.length - 1];
      if (!top) return;
      window.dispatchEvent(
        new CustomEvent(MODAL_CLOSE_REQUESTED_EVENT, { detail: { label: top.label } }),
      );
    };

    window.addEventListener(MODAL_OPEN_EVENT, openListener);
    window.addEventListener(MODAL_CLOSE_EVENT, closeListener);
    window.addEventListener(MODAL_CLOSE_REQUESTED_EVENT, closeRequestedListener);
    window.addEventListener("keydown", keyListener);
    return () => {
      window.removeEventListener(MODAL_OPEN_EVENT, openListener);
      window.removeEventListener(MODAL_CLOSE_EVENT, closeListener);
      window.removeEventListener(MODAL_CLOSE_REQUESTED_EVENT, closeRequestedListener);
      window.removeEventListener("keydown", keyListener);
    };
  }, []);

  const topLabel = modals[modals.length - 1]?.label;

  return (
    <>
      {modals.map((modal) => (
        <div
          key={modal.label}
          className="fixed inset-0 z-[1000] flex items-center justify-center"
          style={{ background: "rgba(8, 10, 14, 0.55)" }}
        >
          <div
            className="flex flex-col overflow-hidden rounded-xl border border-border bg-background shadow-2xl"
            style={{
              width: `min(${modal.width ?? 860}px, 94vw)`,
              height: `min(${modal.height ?? 640}px, 92vh)`,
              outline: topLabel === modal.label ? "1px solid var(--primary, #2f81f7)" : "none",
            }}
          >
            {/* No chrome title bar: child pages render their own headers and
                close controls, matching the desktop window layout. Escape and
                the backdrop replay the close-requested flow. */}
            <div className="min-h-0 flex-1 overflow-hidden">
              <ModalPage entry={modal} />
            </div>
          </div>
        </div>
      ))}
    </>
  );
}

const fakeWindows = new Map<string, WebviewWindow>();

let hostRoot: Root | null = null;
let hostReady = false;
let applyOpenModal: ((detail: OpenModalDetail) => void) | null = null;
const queuedOpens: OpenModalDetail[] = [];

function ensureHostMounted(): void {
  if (hostRoot) return;
  const container = document.createElement("div");
  container.setAttribute("data-nyaterm-web-modal-host", "");
  document.body.append(container);
  hostRoot = createRoot(container);
  hostRoot.render(<ModalHostApp />);
}

function openModalEntry(detail: OpenModalDetail): void {
  if (hostReady && applyOpenModal) {
    applyOpenModal(detail);
  } else {
    // React has not committed the host yet; queue and drain on mount.
    queuedOpens.push(detail);
  }
}

function openListener(event: Event): void {
  const detail = (event as CustomEvent<OpenModalDetail>).detail;
  if (detail?.label) openModalEntry(detail);
}

export function openWebModal(options: OpenModalDetail): Promise<unknown> {
  ensureHostMounted();
  return new Promise((resolve) => {
    const detailLabel = options.label;
    const listener = (event: Event) => {
      const detail = (event as CustomEvent<{ label: string; kind: string }>).detail;
      if (detail.label !== detailLabel) return;
      if (detail.kind === "opened") {
        window.removeEventListener(MODAL_MOUNTED_EVENT, listener);
        resolve(undefined);
      }
    };
    window.addEventListener(MODAL_MOUNTED_EVENT, listener);
    openModalEntry(options);
  });
}

export function handleWebWindowCommand(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<unknown> {
  if (cmd === "open_child_window") {
    const options = (args?.options ?? {}) as OpenModalDetail;
    return openWebModal(options);
  }
  if (cmd === "close_child_window") {
    const label = String(args?.label ?? "");
    if (label) {
      window.dispatchEvent(new CustomEvent(MODAL_CLOSE_EVENT, { detail: { label } }));
    }
    return Promise.resolve(undefined);
  }
  return Promise.resolve(undefined);
}
