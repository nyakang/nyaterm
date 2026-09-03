import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useEffect } from "react";
import { toast } from "sonner";
import { logger } from "@/lib/logger";
import {
  collectExternalDropAdditionalObjects,
  createExternalFileDropBridgeMessage,
  DRAG_EVENT_CAPTURE_OPTIONS,
  getDragEventPosition,
  getExternalFileDropBridge,
  isDropPositionTopmostWithinElement,
  isExternalFileDragEvent,
  type NativeFileDropEventPayload,
} from "@/lib/nativeFileDrop";
import { normalizeDirectoryPath } from "./model";

type CurrentRef<T> = {
  current: T;
};

interface UseExternalFileDropParams {
  activeSessionIdRef: CurrentRef<string | null>;
  canBrowseFilesRef: CurrentRef<boolean>;
  currentPathRef: CurrentRef<string>;
  homeDirRef: CurrentRef<string>;
  listContainerRef: CurrentRef<HTMLDivElement | null>;
  resetExternalDropHover: () => void;
  setIsExternalDropActive: (active: boolean) => void;
  processExternalDropPaths: (
    target: { sessionId: string; remoteDir: string },
    dropPaths: string[],
  ) => void | Promise<void>;
  externalDropPathsRequiredMessage: string;
}

export function useExternalFileDrop({
  activeSessionIdRef,
  canBrowseFilesRef,
  currentPathRef,
  homeDirRef,
  listContainerRef,
  resetExternalDropHover,
  setIsExternalDropActive,
  processExternalDropPaths,
  externalDropPathsRequiredMessage,
}: UseExternalFileDropParams) {
  useEffect(() => {
    const bridge = getExternalFileDropBridge();
    if (!bridge?.postMessageWithAdditionalObjects) {
      return;
    }

    const updateExternalDropState = (event: DragEvent) => {
      if (!isExternalFileDragEvent(event)) {
        return;
      }

      const isOverDropTarget = isDropPositionTopmostWithinElement(
        getDragEventPosition(event),
        listContainerRef.current,
      );

      if (!isOverDropTarget) {
        resetExternalDropHover();
        return;
      }

      event.preventDefault();
      const isActive =
        canBrowseFilesRef.current && !!activeSessionIdRef.current && isOverDropTarget;

      setIsExternalDropActive(isActive);
      if (event.dataTransfer) {
        event.dataTransfer.dropEffect = "copy";
      }
    };

    const handleWindowDragLeave = (event: DragEvent) => {
      if (!isExternalFileDragEvent(event)) {
        return;
      }

      event.preventDefault();

      const leftWindow =
        event.clientX <= 0 ||
        event.clientY <= 0 ||
        event.clientX >= window.innerWidth ||
        event.clientY >= window.innerHeight;

      if (
        leftWindow ||
        !isDropPositionTopmostWithinElement(getDragEventPosition(event), listContainerRef.current)
      ) {
        resetExternalDropHover();
      }
    };

    const handleWindowDrop = (event: DragEvent) => {
      if (!isExternalFileDragEvent(event)) {
        return;
      }

      const dropPosition = getDragEventPosition(event);
      const isOverDropTarget = isDropPositionTopmostWithinElement(
        dropPosition,
        listContainerRef.current,
      );
      resetExternalDropHover();

      const currentSessionId = activeSessionIdRef.current;
      if (!canBrowseFilesRef.current || !currentSessionId || !isOverDropTarget) {
        return;
      }

      event.preventDefault();

      const dataTransfer = event.dataTransfer;
      if (dataTransfer?.files && dataTransfer.files.length > 0) {
        try {
          bridge.postMessageWithAdditionalObjects(
            createExternalFileDropBridgeMessage(dropPosition),
            dataTransfer.files,
          );
        } catch (error) {
          logger.error({
            domain: "ui.error",
            event: "file_explorer.external_drop_filelist_bridge_failed",
            message:
              "Failed to bridge external file drop FileList through WebView2 additional objects",
            ids: { session_id: currentSessionId },
            data: {
              remote_dir:
                normalizeDirectoryPath(currentPathRef.current) || homeDirRef.current || "/",
              file_count: dataTransfer.files.length,
            },
            error,
          });
          toast.error(String(error));
        }
        return;
      }

      void (async () => {
        try {
          const additionalObjects = await collectExternalDropAdditionalObjects(dataTransfer);
          if (additionalObjects.length === 0) {
            logger.warn({
              domain: "ui.error",
              event: "file_explorer.external_drop_objects_unavailable",
              message: "External file drop did not expose any transferable WebView2 objects",
              ids: { session_id: currentSessionId },
              data: {
                remote_dir:
                  normalizeDirectoryPath(currentPathRef.current) || homeDirRef.current || "/",
                item_count: dataTransfer?.items.length ?? 0,
                file_count: dataTransfer?.files.length ?? 0,
              },
            });
            toast.error(externalDropPathsRequiredMessage);
            return;
          }

          bridge.postMessageWithAdditionalObjects(
            createExternalFileDropBridgeMessage(dropPosition),
            additionalObjects,
          );
        } catch (error) {
          logger.error({
            domain: "ui.error",
            event: "file_explorer.external_drop_bridge_failed",
            message: "Failed to bridge external file drop through WebView2 additional objects",
            ids: { session_id: currentSessionId },
            data: {
              remote_dir:
                normalizeDirectoryPath(currentPathRef.current) || homeDirRef.current || "/",
            },
            error,
          });
          toast.error(String(error));
        }
      })();
    };

    const handleWindowBlur = () => {
      resetExternalDropHover();
    };

    window.addEventListener("dragenter", updateExternalDropState, DRAG_EVENT_CAPTURE_OPTIONS);
    window.addEventListener("dragover", updateExternalDropState, DRAG_EVENT_CAPTURE_OPTIONS);
    window.addEventListener("dragleave", handleWindowDragLeave, DRAG_EVENT_CAPTURE_OPTIONS);
    window.addEventListener("drop", handleWindowDrop, DRAG_EVENT_CAPTURE_OPTIONS);
    window.addEventListener("blur", handleWindowBlur);

    return () => {
      resetExternalDropHover();
      window.removeEventListener("dragenter", updateExternalDropState, DRAG_EVENT_CAPTURE_OPTIONS);
      window.removeEventListener("dragover", updateExternalDropState, DRAG_EVENT_CAPTURE_OPTIONS);
      window.removeEventListener("dragleave", handleWindowDragLeave, DRAG_EVENT_CAPTURE_OPTIONS);
      window.removeEventListener("drop", handleWindowDrop, DRAG_EVENT_CAPTURE_OPTIONS);
      window.removeEventListener("blur", handleWindowBlur);
    };
  }, [
    activeSessionIdRef,
    canBrowseFilesRef,
    currentPathRef,
    externalDropPathsRequiredMessage,
    homeDirRef,
    listContainerRef,
    resetExternalDropHover,
    setIsExternalDropActive,
  ]);

  useEffect(() => {
    const bridge = getExternalFileDropBridge();
    if (!bridge?.postMessageWithAdditionalObjects) {
      return;
    }

    let cancelled = false;

    const unlistenPromise = listen<NativeFileDropEventPayload>("external-file-drop", (event) => {
      if (cancelled) {
        return;
      }

      const payload = event.payload;
      if (payload.kind === "leave") {
        resetExternalDropHover();
        return;
      }

      const isOverDropTarget = isDropPositionTopmostWithinElement(
        payload.position,
        listContainerRef.current,
      );
      const currentSessionId = activeSessionIdRef.current;
      const isActive = canBrowseFilesRef.current && !!currentSessionId && isOverDropTarget;

      if (payload.kind === "enter" || payload.kind === "over") {
        setIsExternalDropActive(isActive);
        return;
      }

      if (payload.kind !== "drop") {
        return;
      }

      resetExternalDropHover();

      if (!isActive || !currentSessionId) {
        return;
      }

      const remoteDir = normalizeDirectoryPath(currentPathRef.current) || homeDirRef.current || "/";
      void processExternalDropPaths({ sessionId: currentSessionId, remoteDir }, payload.paths);
    });

    return () => {
      cancelled = true;
      resetExternalDropHover();
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, [
    activeSessionIdRef,
    canBrowseFilesRef,
    currentPathRef,
    homeDirRef,
    listContainerRef,
    processExternalDropPaths,
    resetExternalDropHover,
    setIsExternalDropActive,
  ]);

  useEffect(() => {
    const bridge = getExternalFileDropBridge();
    if (bridge?.postMessageWithAdditionalObjects) {
      return;
    }

    let cancelled = false;

    const handleWindowBlur = () => {
      resetExternalDropHover();
    };

    window.addEventListener("blur", handleWindowBlur);

    const unlistenPromise = getCurrentWebview().onDragDropEvent((event) => {
      if (cancelled) {
        return;
      }

      const payload = event.payload;
      if (payload.type === "leave") {
        resetExternalDropHover();
        return;
      }

      const isOverDropTarget = isDropPositionTopmostWithinElement(
        payload.position,
        listContainerRef.current,
      );
      const isActive =
        canBrowseFilesRef.current && !!activeSessionIdRef.current && isOverDropTarget;

      if (payload.type === "enter" || payload.type === "over") {
        setIsExternalDropActive(isActive);
        return;
      }

      resetExternalDropHover();

      if (!isActive) {
        return;
      }

      const currentSessionId = activeSessionIdRef.current;
      if (!currentSessionId) {
        return;
      }

      const remoteDir = normalizeDirectoryPath(currentPathRef.current) || homeDirRef.current || "/";
      void processExternalDropPaths({ sessionId: currentSessionId, remoteDir }, payload.paths);
    });

    return () => {
      cancelled = true;
      resetExternalDropHover();
      window.removeEventListener("blur", handleWindowBlur);
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, [
    activeSessionIdRef,
    canBrowseFilesRef,
    currentPathRef,
    homeDirRef,
    listContainerRef,
    processExternalDropPaths,
    resetExternalDropHover,
    setIsExternalDropActive,
  ]);
}
