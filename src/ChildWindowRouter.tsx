import { emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { lazy, type ReactNode, Suspense, useEffect } from "react";
import { useTranslation } from "react-i18next";
import {
  isModalChildLabel,
  prepareForModalChildClose,
  setOwnerMainWindowLabel,
  signalChildWindowReady,
} from "./lib/windowManager";

const SettingsPage = lazy(() => import("./pages/SettingsPage"));
const NewSessionPage = lazy(() => import("./pages/NewSessionPage"));
const QuickCommandPage = lazy(() => import("./pages/QuickCommandPage"));
const ProxyPage = lazy(() => import("./pages/ProxyPage"));
const TunnelPage = lazy(() => import("./pages/TunnelPage"));
const AutoUploadPage = lazy(() => import("./pages/FileUploadPage"));
const FilePreviewPage = lazy(() => import("./pages/FilePreviewPage"));
const NoteEditorPage = lazy(() => import("./pages/NoteEditorPage"));

const PAGES: Record<string, React.ComponentType> = {
  settings: SettingsPage,
  "new-session": NewSessionPage,
  "quick-command": QuickCommandPage,
  proxy: ProxyPage,
  tunnel: TunnelPage,
  "auto-upload": AutoUploadPage,
  "file-preview": FilePreviewPage,
  "note-editor": NoteEditorPage,
};

function ReadyContent({ children }: { children: ReactNode }) {
  useEffect(() => {
    const timeoutId = window.setTimeout(() => {
      void signalChildWindowReady();
    }, 0);

    return () => window.clearTimeout(timeoutId);
  }, []);

  return children;
}

export default function ChildWindowRouter({ windowType }: { windowType: string }) {
  const { t } = useTranslation();
  const Page = PAGES[windowType];

  useEffect(() => {
    const ownerLabel = new URLSearchParams(window.location.search).get("owner");
    if (ownerLabel) {
      setOwnerMainWindowLabel(ownerLabel);
    }
    const currentWindow = getCurrentWindow();
    let unlistenCloseRequested: (() => void) | undefined;
    let unlistenFocusChanged: (() => void) | undefined;
    let programmaticClose = false;
    let lastFocusEmitAt = 0;
    const pageHandlesCloseRequested = windowType === "settings";

    if (!pageHandlesCloseRequested) {
      currentWindow
        .onCloseRequested(async (event) => {
          if (programmaticClose || !isModalChildLabel(currentWindow.label)) return;

          programmaticClose = true;
          event.preventDefault();
          await prepareForModalChildClose(currentWindow.label).catch(() => {});
          await currentWindow.close().catch(() => {
            programmaticClose = false;
          });
        })
        .then((unlisten) => {
          unlistenCloseRequested = unlisten;
        })
        .catch(() => {});
    }

    if (isModalChildLabel(currentWindow.label)) {
      currentWindow
        .onFocusChanged(({ payload: focused }) => {
          if (!focused) return;
          const now = Date.now();
          if (now - lastFocusEmitAt < 100) return;
          lastFocusEmitAt = now;
          emit("modal-child-window-focused", {
            label: currentWindow.label,
            ownerLabel,
          });
        })
        .then((unlisten) => {
          unlistenFocusChanged = unlisten;
        })
        .catch(() => {});
    }

    return () => {
      unlistenCloseRequested?.();
      unlistenFocusChanged?.();
    };
  }, [windowType]);

  if (!Page) {
    return (
      <ReadyContent>
        <div className="h-screen flex items-center justify-center text-muted-foreground">
          {t("common.unknownWindowType")}: {windowType}
        </div>
      </ReadyContent>
    );
  }

  return (
    <Suspense fallback={null}>
      <ReadyContent>
        <Page />
      </ReadyContent>
    </Suspense>
  );
}
