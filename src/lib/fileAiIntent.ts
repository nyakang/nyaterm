import type { AIOpenIntent } from "@/lib/aiEvents";
import type { AIFileReference, AICustomActionConfig } from "@/types/global";

export function buildFileAIAssistantIntent({
  runtime,
  action,
  content,
  filePath,
  fileSize,
  fileReference,
}: {
  runtime: "desktop" | "web";
  action: AICustomActionConfig;
  content: string;
  filePath: string;
  fileSize: number;
  fileReference: AIFileReference;
}): Omit<AIOpenIntent, "id"> {
  return {
    action: "custom_file_action",
    userInput: action.prompt,
    ...(runtime === "web"
      ? {
          selectedText: content,
          metadata: {
            actionId: action.id,
            actionName: action.name,
            filePath,
            fileSize,
          },
        }
      : {
          fileReference,
          metadata: {
            actionId: action.id,
            actionName: action.name,
          },
        }),
  };
}
