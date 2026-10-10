import type { ClipboardEvent, ReactNode } from "react";
import type { AIMessage } from "@/types/global";
import { ReferenceTooltip } from "./ReferenceTooltip";
import {
  buildReferenceClipboardPayload,
  selectionOffsets,
  writeReferenceClipboard,
} from "./referenceClipboard";

export function AIUserMessageContent({ message }: { message: AIMessage }) {
  const nodes: ReactNode[] = [];
  const spans: Array<{ id: string; start: number; end: number; title: string; kind: string }> = [];
  let legacyCursor = 0;
  const references = message.references?.length ? message.references : (message.attachments ?? []);
  for (const reference of references) {
    const token = `@${reference.name}`;
    // 新记录使用精确范围，避免普通文字和同名文件被错配；老文件附件维持兼容。
    const start =
      reference.start ?? (reference.kind ? -1 : message.content.indexOf(token, legacyCursor));
    const end = reference.end ?? start + token.length;
    if (start < 0 || message.content.slice(start, end) !== token) continue;
    legacyCursor = end;
    spans.push({
      id: reference.id,
      start,
      end,
      kind: reference.kind ?? "file",
      title:
        reference.title ??
        `${reference.host ? `${reference.host}:` : ""}${reference.path ?? reference.name}`,
    });
  }

  let cursor = 0;
  for (const span of spans.sort((left, right) => left.start - right.start)) {
    if (span.start < cursor) continue;
    nodes.push(message.content.slice(cursor, span.start));
    nodes.push(
      <ReferenceTooltip key={`${span.id}:${span.start}`} text={span.title}>
        <span
          role="note"
          aria-label={span.title}
          data-ai-reference-kind={span.kind}
          className="rounded border border-primary/25 bg-primary/10 px-0.5 font-medium text-primary"
        >
          {message.content.slice(span.start, span.end)}
        </span>
      </ReferenceTooltip>,
    );
    cursor = span.end;
  }
  nodes.push(message.content.slice(cursor));
  const handleCopy = (event: ClipboardEvent<HTMLSpanElement>) => {
    const selection = selectionOffsets(event.currentTarget);
    if (!selection || selection.start === selection.end) return;
    const payload = buildReferenceClipboardPayload(
      message.content,
      references.map((reference) => ({
        id: reference.id,
        kind: reference.kind ?? "file",
        label: reference.name,
        title:
          reference.title ??
          `${reference.host ? `${reference.host}:` : ""}${reference.path ?? reference.name}`,
        start: reference.start ?? 0,
        end: reference.end ?? 0,
        sessionIds:
          reference.sessionIds ??
          (reference.terminalSessionId ? [reference.terminalSessionId] : []),
        fileReference:
          reference.kind === "file" &&
          reference.path &&
          reference.backend &&
          reference.terminalSessionId
            ? {
                ...reference,
                path: reference.path,
                backend: reference.backend,
                terminalSessionId: reference.terminalSessionId,
                content: "",
              }
            : undefined,
      })),
      selection.start,
      selection.end,
    );
    if (payload) writeReferenceClipboard(event, payload, payload.text);
  };
  return (
    <span onCopy={handleCopy} className="contents">
      {nodes}
    </span>
  );
}
