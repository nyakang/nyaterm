import { logger } from "@/lib/logger";

export interface NativeFileDropEventPayload {
  kind: "enter" | "over" | "leave" | "drop";
  paths: string[];
  position: {
    x: number;
    y: number;
  };
}

export const EXTERNAL_FILE_DROP_MESSAGE_KIND = "external-file-drop";
export const DRAG_EVENT_CAPTURE_OPTIONS = true;

type WebView2Bridge = {
  postMessageWithAdditionalObjects: (
    message: unknown,
    additionalObjects: ArrayLike<unknown>,
  ) => void;
};

type DataTransferItemWithFileSystemHandle = DataTransferItem & {
  getAsFileSystemHandle?: () => Promise<unknown>;
};

declare global {
  interface Window {
    chrome?: {
      webview?: WebView2Bridge;
    };
  }
}

export function isDropPositionInsideElement(
  position: { x: number; y: number },
  element: HTMLElement | null,
) {
  if (!element) {
    return false;
  }

  const rect = element.getBoundingClientRect();
  return (
    position.x >= rect.left &&
    position.x <= rect.right &&
    position.y >= rect.top &&
    position.y <= rect.bottom
  );
}

export function isDropPositionTopmostWithinElement(
  position: { x: number; y: number },
  element: HTMLElement | null,
) {
  if (!isDropPositionInsideElement(position, element) || !element) {
    return false;
  }

  const elementFromPoint = element.ownerDocument.elementFromPoint;
  if (typeof elementFromPoint !== "function") {
    return false;
  }

  const topmostElement = elementFromPoint.call(element.ownerDocument, position.x, position.y);
  return (
    topmostElement === element || (topmostElement !== null && element.contains(topmostElement))
  );
}

export function isExternalFileDragEvent(event: DragEvent) {
  const dataTransfer = event.dataTransfer;
  if (!dataTransfer) {
    return false;
  }

  if (Array.from(dataTransfer.types ?? []).includes("Files")) {
    return true;
  }

  if (dataTransfer.files.length > 0) {
    return true;
  }

  return Array.from(dataTransfer.items ?? []).some((item) => item.kind === "file");
}

export function getDragEventPosition(event: DragEvent) {
  return {
    x: event.clientX,
    y: event.clientY,
  };
}

export function getExternalFileDropBridge() {
  return window.chrome?.webview;
}

export function createExternalFileDropBridgeMessage(position: { x: number; y: number }) {
  return JSON.stringify({
    kind: EXTERNAL_FILE_DROP_MESSAGE_KIND,
    position,
  });
}

export async function collectExternalDropAdditionalObjects(dataTransfer: DataTransfer | null) {
  if (!dataTransfer) {
    return [];
  }

  const fileItems = Array.from(dataTransfer.items ?? []).filter((item) => item.kind === "file");
  if (fileItems.length === 0 && dataTransfer.files.length > 0) {
    return Array.from(dataTransfer.files);
  }

  const additionalObjects: unknown[] = [];
  for (const item of fileItems) {
    const file = item.getAsFile();
    if (file) {
      additionalObjects.push(file);
      continue;
    }

    const itemWithHandle = item as DataTransferItemWithFileSystemHandle;
    if (typeof itemWithHandle.getAsFileSystemHandle === "function") {
      try {
        const handle = await itemWithHandle.getAsFileSystemHandle();
        if (handle) {
          additionalObjects.push(handle);
        }
      } catch {
        // Fall back to File objects if the runtime cannot expose FileSystemHandle.
      }
    }
  }

  return additionalObjects;
}

export function logExternalDropBridgeFailure(
  event: string,
  message: string,
  error: unknown,
  data?: Record<string, unknown>,
) {
  logger.error({
    domain: "ui.error",
    event,
    message,
    data,
    error,
  });
}
