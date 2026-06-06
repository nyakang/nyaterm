import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";
import {
  bounceTopModalWindow,
  getOpenModalChildWindowLabels,
  isOwnedModalChildLabel,
  prepareForModalChildClose,
  syncMainWindowModalState,
} from "@/lib/windowManager";

export function useModalChildWindows() {
  const [modalChildWindowLabels, setModalChildWindowLabels] = useState<Set<string>>(
    () => new Set(),
  );
  const closingLabelsRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    const refreshOpenModalChildWindows = async () => {
      const labels = await getOpenModalChildWindowLabels().catch(() => []);
      setModalChildWindowLabels(
        new Set(labels.filter((label) => !closingLabelsRef.current.has(label))),
      );
      await syncMainWindowModalState().catch(() => {});
    };

    void refreshOpenModalChildWindows();

    const unsubs = [
      listen<{ label: string }>("child-window-opened", ({ payload }) => {
        if (!isOwnedModalChildLabel(payload.label)) return;
        closingLabelsRef.current.delete(payload.label);
        setModalChildWindowLabels((labels) => {
          const nextLabels = new Set(labels);
          nextLabels.add(payload.label);
          return nextLabels;
        });
        void refreshOpenModalChildWindows();
      }),
      listen<{ label: string }>("child-window-closed", ({ payload }) => {
        if (!isOwnedModalChildLabel(payload.label)) return;
        closingLabelsRef.current.add(payload.label);
        setModalChildWindowLabels((labels) => {
          const nextLabels = new Set(labels);
          nextLabels.delete(payload.label);
          return nextLabels;
        });
        void prepareForModalChildClose(payload.label);
        window.setTimeout(() => {
          closingLabelsRef.current.delete(payload.label);
          void refreshOpenModalChildWindows();
        }, 250);
      }),
    ];

    return () => {
      unsubs.forEach((promise) => {
        promise.then((unsub) => unsub());
      });
    };
  }, []);

  const modalChildWindowCount = modalChildWindowLabels.size;

  useEffect(() => {
    let unlistenFocusChanged: (() => void) | undefined;

    import("@tauri-apps/api/window").then(({ getCurrentWindow }) => {
      getCurrentWindow()
        .onFocusChanged(({ payload: focused }) => {
          if (!focused || modalChildWindowCount === 0) return;
          void syncMainWindowModalState();
          void bounceTopModalWindow();
        })
        .then((unlisten) => {
          unlistenFocusChanged = unlisten;
        })
        .catch(() => {});
    });

    return () => {
      unlistenFocusChanged?.();
    };
  }, [modalChildWindowCount]);

  return modalChildWindowCount;
}
