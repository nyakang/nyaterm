import { getCurrentWindow } from "@tauri-apps/api/window";
import { lazy, Suspense, useEffect } from "react";
import { useTranslation } from "react-i18next";
import {
  isModalChildLabel,
  prepareForModalChildClose,
  setOwnerMainWindowLabel,
} from "./lib/windowManager";

const SettingsPage = lazy(() => import("./pages/SettingsPage"));
const NewSessionPage = lazy(() => import("./pages/NewSessionPage"));
const QuickCommandPage = lazy(() => import("./pages/QuickCommandPage"));
const AutoUploadPage = lazy(() => import("./pages/FileUploadPage"));

const PAGES: Record<string, React.ComponentType> = {
  settings: SettingsPage,
  "new-session": NewSessionPage,
  "quick-command": QuickCommandPage,
  "auto-upload": AutoUploadPage,
};

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
    let programmaticClose = false;

    currentWindow.show().catch(() => {});

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

    return () => {
      unlistenCloseRequested?.();
    };
  }, []);

  if (!Page) {
    return (
      <div className="h-screen flex items-center justify-center text-muted-foreground">
        {t("common.unknownWindowType")}: {windowType}
      </div>
    );
  }

  return (
    <Suspense
      fallback={
        <div className="h-screen flex items-center justify-center text-muted-foreground">
          {t("common.loading")}
        </div>
      }
    >
      <Page />
    </Suspense>
  );
}
