import type { AIAction, AIFileReference } from "@/types/global";

export const AI_OPEN_EVENT = "nyaterm:ai-open";
export const AI_ERROR_DETECTED_EVENT = "nyaterm:ai-error-detected";

export interface AIOpenIntent {
  id: string;
  action: AIAction;
  userInput?: string;
  selectedText?: string;
  fileReference?: AIFileReference;
  metadata?: Record<string, unknown>;
}

export interface AIErrorDetectedDetail {
  sessionId: string;
  output: string;
}

export function openAIAssistant(intent: Omit<AIOpenIntent, "id">) {
  window.dispatchEvent(
    new CustomEvent<AIOpenIntent>(AI_OPEN_EVENT, {
      detail: {
        id: `ai-intent-${Date.now()}-${Math.random().toString(36).slice(2)}`,
        ...intent,
      },
    }),
  );
}

export function emitAIErrorDetected(detail: AIErrorDetectedDetail) {
  window.dispatchEvent(new CustomEvent<AIErrorDetectedDetail>(AI_ERROR_DETECTED_EVENT, { detail }));
}
