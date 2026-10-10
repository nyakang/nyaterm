import { describe, expect, it } from "vitest";
import type { AIInlineMention } from "./AIReferenceComposer";
import {
  buildReferenceClipboardPayload,
  parseReferenceClipboardPayload,
  serializeReferenceClipboardPayload,
} from "./referenceClipboard";

const mentions: AIInlineMention[] = [
  {
    id: "file:one",
    kind: "file",
    label: "reference.py",
    title: "root@source.example.test:2222:/home/test/reference.py",
    start: 0,
    end: 13,
    sessionIds: ["source"],
    fileReference: {
      id: "file:one",
      name: "reference.py",
      path: "/home/test/reference.py",
      backend: "remote",
      terminalSessionId: "source",
      host: "root@source.example.test:2222",
      content: "SECRET_BODY",
      sizeBytes: 11,
    },
  },
  {
    id: "host:target",
    kind: "host",
    label: "jump.example.test",
    title: "root@jump.example.test:2200",
    start: 23,
    end: 41,
    sessionIds: ["target"],
  },
];

describe("reference clipboard protocol", () => {
  it("round-trips text and source metadata without copying file contents", () => {
    const text = "@reference.py analysis @jump.example.test";
    const payload = buildReferenceClipboardPayload(text, mentions, 0, text.length);
    expect(payload?.text).toBe(text);
    expect(payload?.references.map((reference) => reference.kind)).toEqual(["file", "host"]);
    expect(serializeReferenceClipboardPayload(payload!)).not.toContain("SECRET_BODY");
    expect(parseReferenceClipboardPayload(serializeReferenceClipboardPayload(payload!))).toEqual(
      payload,
    );
  });

  it("preserves metadata ranges for labels containing spaces", () => {
    const mention: AIInlineMention = {
      ...mentions[0],
      label: "my config.yaml",
      title: "root@source.example.test:2222:/home/test/my config.yaml",
      start: 0,
      end: 15,
    };
    const text = "@my config.yaml review";
    const payload = buildReferenceClipboardPayload(text, [mention], 0, text.length);
    expect(payload?.references[0]).toMatchObject({
      label: "my config.yaml",
      start: 0,
      end: 15,
    });
  });

  it("copies only fully selected references", () => {
    const text = "@reference.py analysis @jump.example.test";
    const payload = buildReferenceClipboardPayload(text, mentions, 0, text.length);
    expect(payload?.references).toHaveLength(2);
    const partial = buildReferenceClipboardPayload(text, mentions, 1, text.length);
    expect(partial?.references).toHaveLength(1);
    expect(partial?.references[0]?.kind).toBe("host");
  });

  it("rejects foreign or malformed clipboard payloads", () => {
    expect(parseReferenceClipboardPayload("not json")).toBeNull();
    expect(
      parseReferenceClipboardPayload(
        JSON.stringify({ type: "other", version: 1, text: "@x", references: [] }),
      ),
    ).toBeNull();
  });
});
