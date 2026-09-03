import type { AIStreamEventPayload } from "@/types/global";

export type AIStreamControlEvent = {
  kind: "warning" | "done" | "error" | null;
  terminatesStream: boolean;
};

export function classifyAIStreamControlEvent(
  type: AIStreamEventPayload["type"],
): AIStreamControlEvent {
  switch (type) {
    case "warning":
      return { kind: "warning", terminatesStream: false };
    case "done":
      return { kind: "done", terminatesStream: true };
    case "error":
      return { kind: "error", terminatesStream: true };
    default:
      return { kind: null, terminatesStream: false };
  }
}
