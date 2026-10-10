import type { AIInlineMention } from "./AIReferenceComposer";
import type { AIReferenceGroup, AIReferenceOption } from "./aiReferences";

export interface ScopedMention extends AIInlineMention {
  sessionIds: string[];
}

export function visibleReferenceChildren(group: AIReferenceGroup): AIReferenceOption[] {
  // 保留单会话节点，让主机 → 会话 → 文件的层级在树中可见。
  return group.children;
}

export interface ReferenceTreeChild {
  option: AIReferenceOption;
  depth: 1 | 2;
}

export function referenceTreeChildren(group: AIReferenceGroup): ReferenceTreeChild[] {
  const children = visibleReferenceChildren(group);
  const sessions = children.filter(
    (option): option is Extract<AIReferenceOption, { kind: "session" }> =>
      option.kind === "session",
  );
  const files = children.filter(
    (option): option is Extract<AIReferenceOption, { kind: "file" }> => option.kind === "file",
  );
  const nestedFileIds = new Set<string>();
  const result: ReferenceTreeChild[] = [];

  for (const session of sessions) {
    result.push({ option: session, depth: 1 });
    for (const file of files) {
      if (!file.sessionIds.some((id) => session.sessionIds.includes(id))) continue;
      result.push({ option: file, depth: 2 });
      nestedFileIds.add(file.id);
    }
  }

  for (const file of files) {
    if (!nestedFileIds.has(file.id)) result.push({ option: file, depth: 1 });
  }
  return result;
}

/** 会话列表只包含当前仍在线的会话；文件保留在树中，但来源断开时不可选。 */
export function referenceOptionAvailable(group: AIReferenceGroup, option: AIReferenceOption) {
  return option.sessionIds.some((sessionId) => group.sessions.includes(sessionId));
}

/** 引用范围变化只编辑对应 token，保持其他提示词、文件引用和 UTF-16 范围不变。 */
export function applyPastedText<T extends ScopedMention>(
  draft: { text: string; mentions: T[] },
  pastedText: string,
  range: { start: number; end: number },
  pastedMentions: T[],
) {
  const removedLength = range.end - range.start;
  const delta = pastedText.length - removedLength;
  const mentions = draft.mentions.flatMap((mention) => {
    if (mention.end <= range.start) return [mention];
    if (mention.start >= range.end) {
      return [{ ...mention, start: mention.start + delta, end: mention.end + delta }];
    }
    return [];
  });
  const next = pastedMentions.map((mention) => ({
    ...mention,
    start: mention.start + range.start,
    end: mention.end + range.start,
  }));
  return {
    text: `${draft.text.slice(0, range.start)}${pastedText}${draft.text.slice(range.end)}`,
    mentions: [...mentions, ...next].sort((left, right) => left.start - right.start),
    caret: range.start + pastedText.length,
  };
}

export function applyReferenceSelection<T extends ScopedMention>(
  draft: { text: string; mentions: T[] },
  option: Omit<T, "start" | "end">,
  range: { start: number; end: number },
) {
  const token = `@${option.label}`;
  const prefix = range.start > 0 && !/\s/.test(draft.text[range.start - 1] ?? "") ? " " : "";
  const suffix = !/\s/.test(draft.text[range.end] ?? "") ? " " : "";
  const inserted = `${prefix}${token}${suffix}`;
  const mention = {
    ...option,
    start: prefix.length,
    end: prefix.length + token.length,
  } as T;
  return applyPastedText(draft, inserted, range, [mention]);
}
