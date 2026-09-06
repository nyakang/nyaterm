/**
 * Web shim for `@tauri-apps/api/webview`.
 *
 * `getCurrentWebview().onDragDropEvent` is bridged to real HTML5 drag events
 * so the existing terminal / file-explorer drop hooks keep working. Tauri
 * delivers OS file paths; browsers only expose `File` objects, so the shim
 * stages dropped files via the same upload staging endpoint the dialog shim
 * uses and reports `__web_upload__/<staged>` marker paths.
 */

import { stageFileForUpload } from "../rpcClient";
import type { UnlistenFn } from "../eventBus";

export type { UnlistenFn };

export type DragDropEvent =
  | { type: "enter"; paths: string[]; position: { x: number; y: number } }
  | { type: "over"; position: { x: number; y: number } }
  | { type: "drop"; paths: string[]; position: { x: number; y: number } }
  | { type: "leave" };

type DragDropHandler = (event: { payload: DragDropEvent }) => void;

let bridgeInstalled = false;
const dragHandlers = new Set<DragDropHandler>();
let lastOverAt = 0;

async function stageAll(fileList: FileList | File[]): Promise<string[]> {
  const files = Array.from(fileList);
  const staged: string[] = [];
  for (const file of files) {
    try {
      staged.push(await stageFileForUpload(file));
    } catch (error) {
      console.error("[nyaterm-web] failed to stage dropped file", file.name, error);
    }
  }
  return staged;
}

function installBridge(): void {
  if (bridgeInstalled || typeof document === "undefined") return;
  bridgeInstalled = true;

  let depth = 0;

  document.addEventListener("dragenter", (event) => {
    if (!event.dataTransfer?.types.includes("Files")) return;
    depth += 1;
    const position = { x: event.clientX, y: event.clientY };
    for (const handler of dragHandlers) {
      handler({ payload: { type: "enter", paths: [], position } });
    }
  });

  document.addEventListener("dragover", (event) => {
    if (!event.dataTransfer?.types.includes("Files")) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = "copy";
    const now = Date.now();
    if (now - lastOverAt < 100) return;
    lastOverAt = now;
    const position = { x: event.clientX, y: event.clientY };
    for (const handler of dragHandlers) {
      handler({ payload: { type: "over", position } });
    }
  });

  document.addEventListener("dragleave", (event) => {
    if (!event.relatedTarget) {
      depth = Math.max(0, depth - 1);
      if (depth === 0) {
        for (const handler of dragHandlers) {
          handler({ payload: { type: "leave" } });
        }
      }
    }
  });

  document.addEventListener("drop", (event) => {
    if (!event.dataTransfer?.types.includes("Files")) return;
    event.preventDefault();
    depth = 0;
    const position = { x: event.clientX, y: event.clientY };
    const files = event.dataTransfer.files;
    for (const handler of dragHandlers) {
      handler({ payload: { type: "over", position } });
    }
    // Staging is asynchronous; the drop event reports staged marker paths
    // once uploads finish.
    void stageAll(files).then((paths) => {
      if (paths.length === 0) return;
      for (const handler of dragHandlers) {
        handler({ payload: { type: "drop", paths, position } });
      }
    });
  });
}

function currentWebview(): {
  label: string;
  onDragDropEvent: (handler: DragDropHandler) => Promise<UnlistenFn>;
  setZoom: (scaleFactor: number) => Promise<void>;
} {
  const params = new URLSearchParams(window.location.search);
  return {
    label: params.get("window") ?? "web",
    onDragDropEvent: (handler: DragDropHandler) => {
      installBridge();
      dragHandlers.add(handler);
      return Promise.resolve(() => {
        dragHandlers.delete(handler);
      });
    },
    setZoom: async (_scaleFactor: number) => {
      // Browser zoom is user-controlled; ignore programmatic zoom.
    },
  };
}

export function getCurrentWebview() {
  return currentWebview();
}
