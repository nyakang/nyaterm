import { describe, expect, it } from "vitest";
import { createFileDocumentPane, createSessionPane } from "@/lib/workspaceTabs";
import type { AIFileReference, SavedConnection } from "@/types/global";
import { buildAIReferenceGroups } from "./aiReferences";
import {
  type AyaDraftMention,
  buildAyaContext,
  buildAyaTarget,
  resolveAyaPanes,
} from "./ayaReferences";

const jump: SavedConnection = {
  id: "jump",
  type: "ssh",
  name: "jump.example.test",
  host: "jump.example.test",
  port: 2200,
  username: "root",
};
const source: SavedConnection = {
  id: "source",
  type: "ssh",
  name: "source.example.test",
  host: "source.example.test",
  port: 2222,
  username: "root",
  network: { proxy_jump_id: jump.id },
};
const sourcePane = createSessionPane("source.example.test", "SSH", source.id, {
  sessionId: "source-session",
});
const jumpPane = createSessionPane("jump.example.test", "SSH", jump.id, {
  sessionId: "jump-session",
});
const file: AIFileReference = {
  id: "file:python",
  name: "reference.py",
  backend: "remote",
  path: "/home/test/reference.py",
  terminalSessionId: sourcePane.sessionId,
  host: "root@source.example.test:2222",
  content: "print('source')",
  sizeBytes: 15,
};
const fileMention: AyaDraftMention = {
  id: file.id,
  kind: "file",
  label: file.name,
  title: `${file.host}:${file.path}`,
  start: 0,
  end: 13,
  sessionIds: [sourcePane.sessionId],
  fileReference: file,
};
const hostMention: AyaDraftMention = {
  id: "host:jump",
  kind: "host",
  label: jump.name,
  title: "root@jump.example.test:2200",
  start: 14,
  end: 32,
  sessionIds: [jumpPane.sessionId],
};

describe("AyaAgent reference contract", () => {
  it("keeps file source and explicitly referenced host execution scope separate", () => {
    const result = resolveAyaPanes([sourcePane, jumpPane], jumpPane, [fileMention, hostMention]);
    expect(result.executionIds).toEqual([jumpPane.sessionId]);
    expect(result.panes.map((pane) => pane.sessionId)).toEqual([
      jumpPane.sessionId,
      sourcePane.sessionId,
    ]);
    const context = buildAyaContext([fileMention, hostMention], [file], result.executionIds, 3);
    expect(context.files[0]).toMatchObject({
      sourceSessionId: "source-session",
      sourceEndpoint: "root@source.example.test:2222",
      content: file.content,
    });
    expect(context.references.map((reference) => [reference.kind, reference.start])).toEqual([
      ["file", 3],
      ["host", 17],
    ]);
    expect(JSON.stringify(context.references)).not.toContain(file.content);
  });

  it("deduplicates file snapshots while preserving repeated file references", () => {
    const duplicateMention = { ...fileMention, start: 20, end: 39 };
    const context = buildAyaContext(
      [fileMention, duplicateMention],
      [file, file],
      [sourcePane.sessionId],
      0,
    );
    expect(context.references).toHaveLength(2);
    expect(context.files).toHaveLength(1);
    expect(context.files[0]?.referenceId).toBe(file.id);
  });

  it("keeps the active terminal as execution target for file-only references", () => {
    const result = resolveAyaPanes([sourcePane, jumpPane], jumpPane, [fileMention]);
    expect(result.executionIds).toEqual([jumpPane.sessionId]);
    expect(result.panes.map((pane) => pane.sessionId)).toEqual([
      jumpPane.sessionId,
      sourcePane.sessionId,
    ]);
  });

  it("displays the real endpoint and labels ProxyJump separately from the alias", () => {
    const target = buildAyaTarget(sourcePane, [sourcePane, jumpPane], [source, jump]);
    expect(target.host).toBe("source.example.test");
    expect(target.label).toBe("root@source.example.test:2222 (via root@jump.example.test:2200)");
  });

  it("inherits temporary SSH identity for open file panes", () => {
    const temporary = createSessionPane("SSH alias", "SSH", undefined, {
      sessionId: "temporary",
      temporaryConfig: {
        protocol: "ssh",
        name: "SSH alias",
        host: "source.example.test",
        port: 2222,
        username: "root",
      } as SessionPaneConfig,
    });
    const document = createFileDocumentPane({
      sessionId: "temporary",
      name: file.name,
      type: "SSH",
      backend: "remote",
      path: file.path,
      file: { content: file.content, size: 15, mtime: 0, contentHash: "hash" },
    });
    const groups = buildAIReferenceGroups([temporary, document], [], [], true);
    expect(groups[0].id).toBe("ssh:root@source.example.test:2222");
    expect(groups[0].children[0].title).toBe(`root@source.example.test:2222:${file.path}`);
  });

  it("reports a closed/reconnected source instead of silently rebinding to the active host", () => {
    const result = resolveAyaPanes([jumpPane], jumpPane, [fileMention]);
    expect(result.missingIds).toEqual([sourcePane.sessionId]);
  });
});

type SessionPaneConfig = NonNullable<typeof sourcePane.temporaryConfig>;
