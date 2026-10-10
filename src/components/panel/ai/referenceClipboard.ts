import type { AIFileAttachment } from "@/types/global";
import type { AIInlineMention } from "./AIReferenceComposer";

export const AI_REFERENCE_CLIPBOARD_MIME = "application/x-nyaterm-ai-reference+json";
const AI_REFERENCE_CLIPBOARD_VERSION = 1;

export interface AIClipboardReference {
  id: string;
  kind: "host" | "session" | "file";
  label: string;
  title: string;
  start: number;
  end: number;
  sessionIds: string[];
  file?: Pick<
    AIFileAttachment,
    "path" | "backend" | "host" | "connectionId" | "terminalSessionId" | "sizeBytes"
  >;
}

export interface AIReferenceClipboardPayload {
  type: "nyaterm-ai-reference";
  version: 1;
  text: string;
  references: AIClipboardReference[];
}

export function buildReferenceClipboardPayload(
  text: string,
  mentions: AIInlineMention[],
  selectionStart = 0,
  selectionEnd = text.length,
): AIReferenceClipboardPayload | null {
  const references = mentions
    .filter((mention) => mention.start >= selectionStart && mention.end <= selectionEnd)
    .map((mention) => ({
      id: mention.id,
      kind: mention.kind,
      label: mention.label,
      title: mention.title,
      start: mention.start - selectionStart,
      end: mention.end - selectionStart,
      sessionIds: mention.sessionIds ?? [],
      file: mention.fileReference
        ? {
            path: mention.fileReference.path,
            backend: mention.fileReference.backend,
            host: mention.fileReference.host,
            connectionId: mention.fileReference.connectionId,
            terminalSessionId: mention.fileReference.terminalSessionId,
            sizeBytes: mention.fileReference.sizeBytes,
          }
        : undefined,
    }));
  if (references.length === 0) return null;
  return {
    type: "nyaterm-ai-reference",
    version: AI_REFERENCE_CLIPBOARD_VERSION,
    text: text.slice(selectionStart, selectionEnd),
    references,
  };
}

export function serializeReferenceClipboardPayload(payload: AIReferenceClipboardPayload) {
  return JSON.stringify(payload);
}

export function parseReferenceClipboardPayload(value: string | null | undefined) {
  if (!value) return null;
  try {
    const payload = JSON.parse(value) as Partial<AIReferenceClipboardPayload>;
    if (
      payload.type !== "nyaterm-ai-reference" ||
      payload.version !== AI_REFERENCE_CLIPBOARD_VERSION ||
      typeof payload.text !== "string" ||
      !Array.isArray(payload.references)
    ) {
      return null;
    }
    return payload as AIReferenceClipboardPayload;
  } catch {
    return null;
  }
}

export type ReferenceClipboardEvent = {
  preventDefault: () => void;
  clipboardData: DataTransfer;
};

export function writeReferenceClipboard(
  event: ReferenceClipboardEvent,
  payload: AIReferenceClipboardPayload | null,
  plainText: string,
) {
  if (!payload) return false;
  event.preventDefault();
  event.clipboardData.setData("text/plain", plainText);
  event.clipboardData.setData(
    AI_REFERENCE_CLIPBOARD_MIME,
    serializeReferenceClipboardPayload(payload),
  );
  return true;
}

export function readReferenceClipboard(event: Pick<ReferenceClipboardEvent, "clipboardData">) {
  return parseReferenceClipboardPayload(event.clipboardData.getData(AI_REFERENCE_CLIPBOARD_MIME));
}

export function selectionOffsets(root: HTMLElement) {
  const selection = window.getSelection();
  if (!selection?.rangeCount || !selection.anchorNode || !selection.focusNode) return null;
  if (!root.contains(selection.anchorNode) || !root.contains(selection.focusNode)) return null;
  const offset = (node: Node, point: number) => {
    const range = document.createRange();
    range.selectNodeContents(root);
    range.setEnd(node, point);
    return range.toString().length;
  };
  const anchor = offset(selection.anchorNode, selection.anchorOffset);
  const focus = offset(selection.focusNode, selection.focusOffset);
  return {
    start: Math.min(anchor, focus),
    end: Math.max(anchor, focus),
  };
}
