import { describe, expect, it } from "vitest";
import type { AIReferenceGroup, AIReferenceOption } from "./aiReferences";
import type { ScopedMention } from "./referenceSelection";
import {
  applyPastedText,
  applyReferenceSelection,
  referenceOptionAvailable,
  referenceTreeChildren,
  visibleReferenceChildren,
} from "./referenceSelection";

function session(id: string): AIReferenceOption {
  return {
    id: `session:${id}`,
    kind: "session",
    label: "host",
    title: id,
    sessionIds: [id],
    sortTime: 1,
    pane: { id, kind: "leaf", paneKind: "terminal", type: "SSH", name: "host", sessionId: id },
  };
}
const file: AIReferenceOption = {
  id: "file:x",
  kind: "file",
  label: "file.py",
  title: "host:/file.py",
  sessionIds: ["a"],
  sortTime: 2,
  host: "host",
  sizeBytes: 1,
  pane: {
    id: "file-pane",
    kind: "leaf",
    paneKind: "file",
    type: "SSH",
    name: "file.py",
    sessionId: "a",
    file: {
      backend: "remote",
      path: "/file.py",
      initial: { content: "x", size: 1, mtime: 1, contentHash: "hash" },
    },
  },
};
function group(ids: string[], children: AIReferenceOption[]): AIReferenceGroup {
  return {
    id: "group",
    label: "host",
    title: "host:22",
    host: "host",
    connectionIds: [],
    sessions: ids,
    children,
    sortTime: 1,
  };
}
type Mention = ScopedMention & { payload?: { content: string } };
const root = {
  id: "host:group",
  kind: "host" as const,
  label: "host",
  title: "host:22",
  sessionIds: ["a", "b"],
};
const fileMention = {
  id: file.id,
  kind: "file" as const,
  label: file.label,
  title: file.title,
  sessionIds: file.sessionIds,
  payload: { content: "unsaved file contents" },
};
const otherRoot = {
  id: "host:other",
  kind: "host" as const,
  label: "other",
  title: "other:22",
  sessionIds: ["d"],
};
function draft(text: string, mentions: Array<Omit<Mention, "start" | "end">>) {
  let cursor = 0;
  return {
    text,
    mentions: mentions.map((mention) => {
      const start = text.indexOf(`@${mention.label}`, cursor);
      if (start < 0) throw new Error("Missing fixture token");
      const end = start + mention.label.length + 1;
      cursor = end;
      return { ...mention, start, end };
    }),
  };
}
function queryRange(text: string) {
  const start = text.lastIndexOf("@query");
  return { start, end: start + 6 };
}
function assertRanges(result: { text: string; mentions: Mention[] }) {
  for (const mention of result.mentions)
    expect(result.text.slice(mention.start, mention.end)).toBe(`@${mention.label}`);
}

describe("reference tree redundancy and selection scope", () => {
  it("keeps a singleton session and nests its files under the session", () => {
    const item = group(["a"], [session("a"), file]);
    expect(visibleReferenceChildren(item)).toEqual([session("a"), file]);
    expect(referenceTreeChildren(item).map(({ option, depth }) => [option.kind, depth])).toEqual([
      ["session", 1],
      ["file", 2],
    ]);
    expect(item.sessions).toEqual(["a"]);
  });

  it("keeps multiple sessions and places files below their owning session", () => {
    const children = [file, session("b"), session("a")];
    expect(visibleReferenceChildren(group(["b", "a"], children))).toEqual(children);
    expect(
      referenceTreeChildren(group(["b", "a"], children)).map(({ option, depth }) => [
        option.kind,
        depth,
      ]),
    ).toEqual([
      ["session", 1],
      ["session", 1],
      ["file", 2],
    ]);
    expect(visibleReferenceChildren(group([], [file]))).toEqual([file]);
    expect(
      referenceTreeChildren(group([], [file])).map(({ option, depth }) => [option.kind, depth]),
    ).toEqual([["file", 1]]);
  });

  it("does not hide a search result just because only one of several sessions matched", () => {
    const item = group(["a", "b"], [session("b")]);
    expect(visibleReferenceChildren(item)).toEqual([session("b")]);
  });

  it("disables file history when its source session is disconnected", () => {
    expect(referenceOptionAvailable(group(["a"], [session("a"), file]), file)).toBe(true);
    expect(referenceOptionAvailable(group([], [file]), file)).toBe(false);
  });

  it("keeps host, session, file, and prompt references independent", () => {
    const item = draft("🙂 @host explain @file.py @other\nnext @query", [
      root,
      fileMention,
      otherRoot,
    ]);
    const option = {
      id: "session:a",
      kind: "session" as const,
      label: "window-a",
      title: "a",
      sessionIds: ["a"],
    };
    const result = applyReferenceSelection(item, option, queryRange(item.text));
    expect(result.mentions.map((mention) => mention.id)).toEqual([
      root.id,
      file.id,
      otherRoot.id,
      "session:a",
    ]);
    expect(result.mentions.find((mention) => mention.id === file.id)?.payload).toEqual(
      fileMention.payload,
    );
    expect(result.text).toContain("explain @file.py @other\nnext @window-a");
    expect(result.mentions[result.mentions.length - 1]?.sessionIds).toEqual(["a"]);
    assertRanges(result);
  });

  it("keeps existing sessions when a host reference is added", () => {
    const a = {
      id: "session:a",
      kind: "session" as const,
      label: "a",
      title: "a",
      sessionIds: ["a"],
    };
    const b = {
      id: "session:b",
      kind: "session" as const,
      label: "b",
      title: "b",
      sessionIds: ["b"],
    };
    const item = draft("@a @file.py @b keep @other @query", [a, fileMention, b, otherRoot]);
    const result = applyReferenceSelection(item, root, queryRange(item.text));
    expect(result.mentions.map((mention) => mention.id)).toEqual([
      "session:a",
      "file:x",
      "session:b",
      "host:other",
      root.id,
    ]);
    expect(result.mentions[result.mentions.length - 1]?.sessionIds).toEqual(["a", "b"]);
    assertRanges(result);
  });

  it("allows selecting the same root again", () => {
    const item = draft("@host keep @file.py @query", [root, fileMention]);
    const result = applyReferenceSelection(item, root, queryRange(item.text));
    expect(result.mentions.map((mention) => mention.id)).toEqual([root.id, file.id, root.id]);
    expect(result.text).toBe("@host keep @file.py @host ");
    assertRanges(result);
  });

  it.each([
    ["file", "@file.py @query", [fileMention], fileMention],
    ["session", "@host @query", [session("a")], session("a")],
  ] as const)("allows selecting the same %s reference again", (_kind, text, mentions, option) => {
    const item = draft(text, [...mentions]);
    const result = applyReferenceSelection(item, option, queryRange(item.text));
    expect(result.mentions).toHaveLength(2);
    expect(result.mentions.map((mention) => mention.id)).toEqual([option.id, option.id]);
    expect(result.text).not.toContain("@query");
    assertRanges(result);
  });

  it("rebases unique references pasted into the middle of existing prompt text", () => {
    const existing = draft("before @other after", [otherRoot]);
    const pasted = [
      { ...fileMention, start: 0, end: "@file.py".length },
      { ...root, start: "@file.py".length + 1, end: "@file.py @host".length },
    ];
    const result = applyPastedText(existing, "@file.py @host", { start: 7, end: 7 }, pasted);
    expect(result.text).toBe("before @file.py @host@other after");
    expect(result.mentions.map((mention) => mention.id)).toEqual(["file:x", root.id, "host:other"]);
    expect(result.text.slice(result.mentions[0].start, result.mentions[0].end)).toBe("@file.py");
    expect(result.text.slice(result.mentions[1].start, result.mentions[1].end)).toBe("@host");
    assertRanges(result);
  });

  it("handles a mention query before the covered root and rebases later references", () => {
    const item = draft("@query before @host after @file.py", [root, fileMention]);
    const result = applyReferenceSelection(
      item,
      { id: "session:b", kind: "session", label: "b", title: "b", sessionIds: ["b"] },
      queryRange(item.text),
    );
    expect(result.mentions.map((mention) => mention.id)).toEqual(["session:b", root.id, file.id]);
    expect(result.text).toContain("@b before");
    assertRanges(result);
  });
});
