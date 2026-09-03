import { useCallback, useEffect, useRef } from "react";
import {
  decreaseFileEditorFontSize,
  increaseFileEditorFontSize,
} from "@/lib/fileEditorFontSize";
import type { AppSettings } from "@/types/global";

type UpdateAppSettings = (
  updates: Partial<AppSettings> | ((prev: AppSettings) => Partial<AppSettings>),
) => void;

const CTRL_WHEEL_ZOOM_THROTTLE_MS = 50;
const FILE_EDITOR_ROOT_SELECTOR = '[data-file-editor-root="true"]';

function isElement(value: EventTarget | null): value is Element {
  return value instanceof Element;
}

function eventTargetIsInsideFileEditorRoot(event: WheelEvent) {
  const pathContainsFileEditorRoot = event.composedPath().some((target) => {
    if (!isElement(target)) return false;
    return target.matches(FILE_EDITOR_ROOT_SELECTOR);
  });
  if (pathContainsFileEditorRoot) return true;

  const target = event.target;
  return isElement(target) && target.closest(FILE_EDITOR_ROOT_SELECTOR) !== null;
}

export function useFileEditorZoom(updateAppSettings: UpdateAppSettings) {
  const lastCtrlWheelZoomAtRef = useRef(0);

  const handleZoomIn = useCallback(() => {
    updateAppSettings((prev) => ({
      transfer: {
        ...prev.transfer,
        internal_editor_font_size: increaseFileEditorFontSize(
          prev.transfer.internal_editor_font_size,
        ),
      },
    }));
  }, [updateAppSettings]);

  const handleZoomOut = useCallback(() => {
    updateAppSettings((prev) => ({
      transfer: {
        ...prev.transfer,
        internal_editor_font_size: decreaseFileEditorFontSize(
          prev.transfer.internal_editor_font_size,
        ),
      },
    }));
  }, [updateAppSettings]);

  useEffect(() => {
    const handleCtrlWheelZoom = (event: WheelEvent) => {
      if (!event.ctrlKey && !event.metaKey) return;
      if (event.deltaY === 0) return;
      if (!eventTargetIsInsideFileEditorRoot(event)) return;

      event.preventDefault();
      const now = Date.now();
      if (now - lastCtrlWheelZoomAtRef.current < CTRL_WHEEL_ZOOM_THROTTLE_MS) return;
      lastCtrlWheelZoomAtRef.current = now;

      if (event.deltaY < 0) {
        handleZoomIn();
      } else {
        handleZoomOut();
      }
    };

    window.addEventListener("wheel", handleCtrlWheelZoom, { passive: false, capture: true });
    return () => {
      window.removeEventListener("wheel", handleCtrlWheelZoom, true);
    };
  }, [handleZoomIn, handleZoomOut]);

  return {
    handleZoomIn,
    handleZoomOut,
  };
}
