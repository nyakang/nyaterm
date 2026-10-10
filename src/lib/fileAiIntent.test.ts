import { describe, expect, it } from "vitest";
import type { AIFileReference, AICustomActionConfig } from "@/types/global";
import { buildFileAIAssistantIntent } from "./fileAiIntent";

const action: AICustomActionConfig = {
  id: "review",
  name: "Review file",
  prompt: "Review this file",
  enabled: true,
};

const fileReference: AIFileReference = {
  id: "file:remote:session:/home/test/reference.py",
  name: "reference.py",
  path: "/home/test/reference.py",
  backend: "remote",
  terminalSessionId: "session",
  connectionId: "connection",
  host: "root@source.example.test:2222",
  sizeBytes: 12,
  mimeType: "text/plain",
  content: "print('test')",
};

describe("file AI intent", () => {
  it("uses legacy selected text for Web Ask without a file reference", () => {
    expect(
      buildFileAIAssistantIntent({
        runtime: "web",
        action,
        content: fileReference.content,
        filePath: fileReference.path,
        fileSize: 12,
        fileReference,
      }),
    ).toEqual({
      action: "custom_file_action",
      userInput: "Review this file",
      selectedText: "print('test')",
      metadata: {
        actionId: "review",
        actionName: "Review file",
        filePath: "/home/test/reference.py",
        fileSize: 12,
      },
    });
  });

  it("uses the structured file reference on Desktop", () => {
    expect(
      buildFileAIAssistantIntent({
        runtime: "desktop",
        action,
        content: fileReference.content,
        filePath: fileReference.path,
        fileSize: 12,
        fileReference,
      }),
    ).toEqual({
      action: "custom_file_action",
      userInput: "Review this file",
      fileReference,
      metadata: { actionId: "review", actionName: "Review file" },
    });
  });
});
