import {
  type ClipboardEvent,
  type CompositionEvent,
  forwardRef,
  type KeyboardEvent,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import type { AIFileReference } from "@/types/global";
import { ReferenceTooltip } from "./ReferenceTooltip";
import {
  buildReferenceClipboardPayload,
  readReferenceClipboard,
  selectionOffsets,
  writeReferenceClipboard,
} from "./referenceClipboard";

export interface AIInlineMention {
  id: string;
  kind: "host" | "session" | "file";
  label: string;
  title: string;
  start: number;
  end: number;
  sessionIds?: string[];
  fileReference?: AIFileReference;
}

export interface AIReferenceComposerHandle {
  focusAt: (offset: number) => void;
  focus: () => void;
}

interface AIReferenceComposerProps {
  value: string;
  mentions: AIInlineMention[];
  placeholder: string;
  disabled: boolean;
  className?: string;
  onChange: (value: string, mentions: AIInlineMention[]) => void;
  onMentionQuery: (query: string, range: { start: number; end: number } | null) => void;
  onKeyDown: (event: KeyboardEvent<HTMLDivElement>) => void;
  onCompositionStart: (event: CompositionEvent<HTMLDivElement>) => void;
  onCompositionEnd: (event: CompositionEvent<HTMLDivElement>) => void;
  onPasteReferences?: (
    event: ClipboardEvent<HTMLDivElement>,
    text: string,
    selection: { start: number; end: number },
    payload: ReturnType<typeof readReferenceClipboard>,
  ) => boolean;
}

interface ParsedEditor {
  value: string;
  mentions: AIInlineMention[];
}

function parseEditor(root: HTMLDivElement, mentions: AIInlineMention[]): ParsedEditor {
  const mentionsById = new Map(mentions.map((mention) => [mention.id, mention]));
  let value = "";
  const parsedMentions: AIInlineMention[] = [];

  const appendNode = (node: Node, isRoot = false) => {
    if (node.nodeType === Node.TEXT_NODE) {
      value += node.textContent ?? "";
      return;
    }
    if (!(node instanceof HTMLElement)) return;

    const mentionId = node.dataset.aiReferenceId;
    if (mentionId) {
      const mention = mentionsById.get(mentionId);
      if (!mention) return;
      const start = value.length;
      value += `@${mention.label}`;
      parsedMentions.push({ ...mention, start, end: value.length });
      return;
    }
    if (node.tagName === "BR") {
      value += "\n";
      return;
    }

    const isBlock = !isRoot && (node.tagName === "DIV" || node.tagName === "P");
    if (isBlock && value.length > 0 && !value.endsWith("\n")) value += "\n";
    for (const child of node.childNodes) appendNode(child);
  };

  for (const child of root.childNodes) appendNode(child);
  value = value.replace(/\n+$/, (suffix) => suffix.slice(0, 1));
  return {
    value,
    mentions: parsedMentions.filter((mention) => mention.end <= value.length),
  };
}

function editorMatches(root: HTMLDivElement, value: string, mentions: AIInlineMention[]) {
  const parsed = parseEditor(root, mentions);
  return (
    parsed.value === value &&
    parsed.mentions.length === mentions.length &&
    parsed.mentions.every(
      (mention, index) =>
        mention.id === mentions[index]?.id &&
        mention.start === mentions[index]?.start &&
        mention.end === mentions[index]?.end,
    )
  );
}

function renderEditor(root: HTMLDivElement, value: string, mentions: AIInlineMention[]) {
  const fragment = document.createDocumentFragment();
  let cursor = 0;
  const sortedMentions = [...mentions].sort((left, right) => left.start - right.start);

  for (const mention of sortedMentions) {
    if (mention.start < cursor || mention.end > value.length) continue;
    fragment.append(document.createTextNode(value.slice(cursor, mention.start)));
    const token = document.createElement("span");
    token.dataset.aiReferenceId = mention.id;
    token.setAttribute("contenteditable", "false");
    token.className =
      "relative mx-0.5 inline-flex cursor-default items-center rounded border border-primary/30 bg-primary/10 px-1 font-medium text-primary align-baseline";
    token.setAttribute("role", "note");
    token.setAttribute("aria-label", mention.title);
    token.textContent = value.slice(mention.start, mention.end);
    fragment.append(token);
    cursor = mention.end;
  }

  fragment.append(document.createTextNode(value.slice(cursor)));
  root.replaceChildren(fragment);
}

function positionAtOffset(root: HTMLDivElement, requestedOffset: number) {
  let remaining = Math.max(0, requestedOffset);
  const visit = (node: Node): { node: Node; offset: number } | null => {
    if (node.nodeType === Node.TEXT_NODE) {
      const length = node.textContent?.length ?? 0;
      if (remaining <= length) return { node, offset: remaining };
      remaining -= length;
      return null;
    }
    if (!(node instanceof HTMLElement)) return null;
    if (node.dataset.aiReferenceId) {
      const parent = node.parentNode;
      if (!parent) return null;
      const index = Array.prototype.indexOf.call(parent.childNodes, node) as number;
      const length = node.textContent?.length ?? 0;
      if (remaining <= length) return { node: parent, offset: index + (remaining > 0 ? 1 : 0) };
      remaining -= length;
      return null;
    }
    if (node.tagName === "BR") {
      const parent = node.parentNode;
      if (!parent) return null;
      const index = Array.prototype.indexOf.call(parent.childNodes, node) as number;
      if (remaining <= 1) return { node: parent, offset: index + (remaining > 0 ? 1 : 0) };
      remaining -= 1;
      return null;
    }
    for (const child of node.childNodes) {
      const result = visit(child);
      if (result) return result;
    }
    return null;
  };

  return visit(root) ?? { node: root, offset: root.childNodes.length };
}

function insertTextAtCaret(root: HTMLDivElement, text: string) {
  const selection = window.getSelection();
  if (!selection?.rangeCount || !root.contains(selection.anchorNode)) return;
  const range = selection.getRangeAt(0);
  range.deleteContents();
  const textNode = document.createTextNode(text);
  range.insertNode(textNode);
  range.setStartAfter(textNode);
  range.collapse(true);
  selection.removeAllRanges();
  selection.addRange(range);
}

function setCaret(root: HTMLDivElement, offset: number) {
  root.focus();
  const position = positionAtOffset(root, offset);
  const range = document.createRange();
  range.setStart(position.node, position.offset);
  range.collapse(true);
  const selection = window.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
}

function caretOffset(root: HTMLDivElement) {
  const selection = window.getSelection();
  if (!selection?.rangeCount) return 0;
  const range = selection.getRangeAt(0).cloneRange();
  range.selectNodeContents(root);
  range.setEnd(selection.anchorNode ?? root, selection.anchorOffset);
  return range.toString().length;
}

function removeAdjacentMention(root: HTMLDivElement, direction: "backward" | "forward") {
  const selection = window.getSelection();
  if (!selection?.isCollapsed || !selection.rangeCount) return false;
  const range = selection.getRangeAt(0);
  let node: Node | null = range.startContainer;
  let offset = range.startOffset;

  if (node.nodeType === Node.TEXT_NODE) {
    const textLength = node.textContent?.length ?? 0;
    if (
      (direction === "backward" && offset !== 0) ||
      (direction === "forward" && offset !== textLength)
    ) {
      return false;
    }
    const parent = node.parentNode;
    if (!parent) return false;
    const index = Array.prototype.indexOf.call(parent.childNodes, node) as number;
    node = parent;
    offset = index + (direction === "forward" ? 1 : 0);
  }

  while (node) {
    const siblingIndex = direction === "backward" ? offset - 1 : offset;
    const sibling = node.childNodes[siblingIndex];
    if (sibling instanceof HTMLElement && sibling.dataset.aiReferenceId) {
      const oldOffset = caretOffset(root);
      const tokenLength = sibling.textContent?.length ?? 0;
      sibling.remove();
      setCaret(root, direction === "backward" ? oldOffset - tokenLength : oldOffset);
      return true;
    }
    if (sibling || node === root) return false;
    const nextParent: Node | null = node.parentNode;
    if (!nextParent) return false;
    offset = Array.prototype.indexOf.call(nextParent.childNodes, node) as number;
    node = nextParent;
  }
  return false;
}

export const AIReferenceComposer = forwardRef<AIReferenceComposerHandle, AIReferenceComposerProps>(
  function AIReferenceComposer(
    {
      value,
      mentions,
      placeholder,
      disabled,
      className,
      onChange,
      onMentionQuery,
      onKeyDown,
      onCompositionStart,
      onCompositionEnd,
      onPasteReferences,
    },
    forwardedRef,
  ) {
    const editorRef = useRef<HTMLDivElement | null>(null);
    const mentionsRef = useRef(mentions);
    mentionsRef.current = mentions;
    const tooltipTimerRef = useRef<number | null>(null);
    const tooltipAnchorRef = useRef<HTMLElement | null>(null);
    const [hovered, setHovered] = useState<{ element: HTMLElement; text: string } | null>(null);
    const clearTooltip = useCallback(() => {
      if (tooltipTimerRef.current !== null) window.clearTimeout(tooltipTimerRef.current);
      tooltipTimerRef.current = null;
      tooltipAnchorRef.current = null;
      setHovered(null);
    }, []);
    // biome-ignore lint/correctness/useExhaustiveDependencies: 输入或引用变化时取消旧 DOM 节点的悬浮提示。
    useEffect(() => {
      clearTooltip();
      return () => {
        if (tooltipTimerRef.current !== null) window.clearTimeout(tooltipTimerRef.current);
      };
    }, [clearTooltip, value, mentions]);

    useImperativeHandle(
      forwardedRef,
      () => ({
        focusAt: (offset) => {
          const editor = editorRef.current;
          if (editor) setCaret(editor, offset);
        },
        focus: () => editorRef.current?.focus(),
      }),
      [],
    );

    useLayoutEffect(() => {
      const editor = editorRef.current;
      if (editor && !editorMatches(editor, value, mentions)) {
        const hadFocus = document.activeElement === editor;
        const oldOffset = hadFocus ? caretOffset(editor) : value.length;
        renderEditor(editor, value, mentions);
        if (hadFocus) setCaret(editor, Math.min(oldOffset, value.length));
      }
    }, [mentions, value]);

    const emitChange = () => {
      const editor = editorRef.current;
      if (!editor) return;
      const parsed = parseEditor(editor, mentionsRef.current);
      onChange(parsed.value, parsed.mentions);

      const selection = window.getSelection();
      if (!selection?.rangeCount || !editor.contains(selection.anchorNode)) {
        onMentionQuery("", null);
        return;
      }
      const prefixRange = document.createRange();
      prefixRange.selectNodeContents(editor);
      prefixRange.setEnd(selection.anchorNode ?? editor, selection.anchorOffset);
      const beforeCaret = prefixRange.toString();
      const match = beforeCaret.match(/@(\S*)$/);
      if (!match) {
        onMentionQuery("", null);
        return;
      }
      const end = beforeCaret.length;
      onMentionQuery(match[1], { start: end - match[0].length, end });
    };

    const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
      if (event.key === "Enter" && event.shiftKey) {
        event.preventDefault();
        insertTextAtCaret(event.currentTarget, "\n");
        emitChange();
        return;
      }
      if (event.key === "Backspace" && removeAdjacentMention(event.currentTarget, "backward")) {
        event.preventDefault();
        emitChange();
        return;
      }
      if (event.key === "Delete" && removeAdjacentMention(event.currentTarget, "forward")) {
        event.preventDefault();
        emitChange();
        return;
      }
      onKeyDown(event);
    };

    const handleCopy = (event: ClipboardEvent<HTMLDivElement>) => {
      const selection = selectionOffsets(event.currentTarget);
      if (!selection || selection.start === selection.end) return;
      const parsed = parseEditor(event.currentTarget, mentionsRef.current);
      const payload = buildReferenceClipboardPayload(
        parsed.value,
        parsed.mentions,
        selection.start,
        selection.end,
      );
      if (payload) writeReferenceClipboard(event, payload, payload.text);
    };

    const handlePaste = (event: ClipboardEvent<HTMLDivElement>) => {
      const selection = selectionOffsets(event.currentTarget);
      const text = event.clipboardData.getData("text/plain");
      if (selection && onPasteReferences?.(event, text, selection, readReferenceClipboard(event)))
        return;
      event.preventDefault();
      if (!selection) return;
      insertTextAtCaret(event.currentTarget, text);
      emitChange();
    };

    return (
      <div className="relative">
        {!value ? (
          <div className="pointer-events-none absolute left-2 top-2 text-xs text-muted-foreground">
            {placeholder}
          </div>
        ) : null}
        {/* biome-ignore lint/a11y/useSemanticElements: 富文本编辑器需要支持输入框内的原子引用。 */}
        <div
          ref={editorRef}
          contentEditable={!disabled}
          suppressContentEditableWarning
          role="textbox"
          tabIndex={disabled ? -1 : 0}
          aria-label={placeholder}
          aria-multiline="true"
          aria-disabled={disabled}
          className={className}
          onInput={emitChange}
          onPointerMove={(event) => {
            const element =
              event.target instanceof Element
                ? event.target.closest<HTMLElement>("[data-ai-reference-id]")
                : null;
            if (
              event.pointerType === "touch" ||
              !element ||
              !event.currentTarget.contains(element)
            ) {
              clearTooltip();
              return;
            }
            if (tooltipAnchorRef.current === element) return;
            clearTooltip();
            const mention = mentionsRef.current.find(
              (item) => item.id === element.dataset.aiReferenceId,
            );
            if (!mention) return;
            tooltipAnchorRef.current = element;
            tooltipTimerRef.current = window.setTimeout(() => {
              tooltipTimerRef.current = null;
              if (element.isConnected) setHovered({ element, text: mention.title });
            }, 250);
          }}
          onPointerLeave={clearTooltip}
          onScroll={clearTooltip}
          onKeyDown={(event) => {
            clearTooltip();
            handleKeyDown(event);
          }}
          onKeyUp={emitChange}
          onClick={emitChange}
          onCopy={handleCopy}
          onPaste={handlePaste}
          onCompositionStart={onCompositionStart}
          onCompositionEnd={(event) => {
            onCompositionEnd(event);
            emitChange();
          }}
        />
        {hovered?.element.isConnected
          ? createPortal(
              <ReferenceTooltip
                text={hovered.text}
                open
                onOpenChange={(open) => {
                  if (!open) clearTooltip();
                }}
              >
                <span aria-hidden="true" className="pointer-events-none absolute inset-0" />
              </ReferenceTooltip>,
              hovered.element,
            )
          : null}
      </div>
    );
  },
);
