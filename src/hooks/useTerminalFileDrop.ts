import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useEffect, useRef } from "react";
import { toast } from "sonner";
import {
  collectExternalDropAdditionalObjects,
  createExternalFileDropBridgeMessage,
  DRAG_EVENT_CAPTURE_OPTIONS,
  getDragEventPosition,
  getExternalFileDropBridge,
  isDropPositionTopmostWithinElement,
  isExternalFileDragEvent,
  logExternalDropBridgeFailure,
  type NativeFileDropEventPayload,
} from "@/lib/nativeFileDrop";
import type { SessionType } from "@/types/global";

interface UseTerminalFileDropParams {
  sessionId: string;
  sessionType: SessionType;
  enabled: boolean;
  containerRef: React.RefObject<HTMLDivElement | null>;
  resetExternalDropHover: () => void;
  setIsExternalDropActive: (active: boolean) => void;
  processDropPaths: (dropPaths: string[]) => void | Promise<void>;
  externalDropPathsRequiredMessage: string;
}

export function useTerminalFileDrop({
  sessionId,
  sessionType,
  enabled,
  containerRef,
  resetExternalDropHover,
  setIsExternalDropActive,
  processDropPaths,
  externalDropPathsRequiredMessage,
}: UseTerminalFileDropParams) {
  const enabledRef = useRef(enabled);
  enabledRef.current = enabled;

  const sessionIdRef = useRef(sessionId);
  sessionIdRef.current = sessionId;

  const sessionTypeRef = useRef(sessionType);
  sessionTypeRef.current = sessionType;

  useEffect(() => {
    if (!enabled) {
      resetExternalDropHover();
    }
  }, [enabled, resetExternalDropHover]);

  useEffect(() => {
    const bridge = getExternalFileDropBridge();
    if (!bridge?.postMessageWithAdditionalObjects) {
      return;
    }

    const updateExternalDropState = (event: DragEvent) => {
      if (!enabledRef.current || !isExternalFileDragEvent(event)) {
        return;
      }

      const isOverDropTarget = isDropPositionTopmostWithinElement(
        getDragEventPosition(event),
        containerRef.current,
      );

      if (!isOverDropTarget) {
        resetExternalDropHover();
        return;
      }

      event.preventDefault();
      setIsExternalDropActive(true);
      if (event.dataTransfer) {
        event.dataTransfer.dropEffect = "copy";
      }
    };

    const handleWindowDragLeave = (event: DragEvent) => {
      if (!enabledRef.current || !isExternalFileDragEvent(event)) {
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
        !isDropPositionTopmostWithinElement(getDragEventPosition(event), containerRef.current)
      ) {
        resetExternalDropHover();
      }
    };

    const handleWindowDrop = (event: DragEvent) => {
      if (!enabledRef.current || !isExternalFileDragEvent(event)) {
        return;
      }

      const dropPosition = getDragEventPosition(event);
      const isOverDropTarget = isDropPositionTopmostWithinElement(
        dropPosition,
        containerRef.current,
      );
      resetExternalDropHover();

      if (!isOverDropTarget) {
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
          logExternalDropBridgeFailure(
            "terminal.external_drop_filelist_bridge_failed",
            "Failed to bridge terminal file drop FileList through WebView2 additional objects",
            error,
            {
              session_id: sessionIdRef.current,
              session_type: sessionTypeRef.current,
              file_count: dataTransfer.files.length,
            },
          );
          toast.error(String(error));
        }
        return;
      }

      void (async () => {
        try {
          const additionalObjects = await collectExternalDropAdditionalObjects(dataTransfer);
          if (additionalObjects.length === 0) {
            toast.error(externalDropPathsRequiredMessage);
            return;
          }

          bridge.postMessageWithAdditionalObjects(
            createExternalFileDropBridgeMessage(dropPosition),
            additionalObjects,
          );
        } catch (error) {
          logExternalDropBridgeFailure(
            "terminal.external_drop_bridge_failed",
            "Failed to bridge terminal file drop through WebView2 additional objects",
            error,
            {
              session_id: sessionIdRef.current,
              session_type: sessionTypeRef.current,
            },
          );
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
    containerRef,
    externalDropPathsRequiredMessage,
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
        containerRef.current,
      );
      const isActive = enabledRef.current && isOverDropTarget;

      if (payload.kind === "enter" || payload.kind === "over") {
        setIsExternalDropActive(isActive);
        return;
      }

      if (payload.kind !== "drop") {
        return;
      }

      resetExternalDropHover();

      if (!isActive) {
        return;
      }

      void processDropPaths(payload.paths);
    });

    return () => {
      cancelled = true;
      resetExternalDropHover();
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, [containerRef, processDropPaths, resetExternalDropHover, setIsExternalDropActive]);

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
        containerRef.current,
      );
      const isActive = enabledRef.current && isOverDropTarget;

      if (payload.type === "enter" || payload.type === "over") {
        setIsExternalDropActive(isActive);
        return;
      }

      resetExternalDropHover();

      if (!isActive) {
        return;
      }

      void processDropPaths(payload.paths);
    });

    return () => {
      cancelled = true;
      resetExternalDropHover();
      window.removeEventListener("blur", handleWindowBlur);
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, [containerRef, processDropPaths, resetExternalDropHover, setIsExternalDropActive]);
}
