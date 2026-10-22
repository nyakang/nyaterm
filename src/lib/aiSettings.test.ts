import { describe, expect, it } from "vitest";
import {
  BUILTIN_PROVIDERS,
  DEFAULT_AI_SETTINGS,
  localizeDefaultAiAction,
} from "./aiSettings";

describe("aiSettings Ollama defaults", () => {
  it("uses the Ollama native API root instead of the OpenAI-compatible v1 path", () => {
    expect(BUILTIN_PROVIDERS.ollama?.defaultBaseUrl).toBe("http://localhost:11434/");

    const profile = DEFAULT_AI_SETTINGS.provider_profiles.find((item) => item.id === "ollama");
    const credential = DEFAULT_AI_SETTINGS.provider_credentials.find(
      (item) => item.id === "ollama",
    );

    expect(profile?.base_url).toBe("http://localhost:11434/");
    expect(credential?.base_url).toBe("http://localhost:11434/");
  });
});

describe("External MCP defaults", () => {
  it("starts disabled with confirm-scoped persistent storage", () => {
    expect(DEFAULT_AI_SETTINGS.external_mcp).toEqual({
      enabled: false,
      permission_mode: "confirm",
      session_scope: "current_window",
      terminal_display_mode: "inline",
      system_notification: true,
    });
  });

  describe("default AI action localization", () => {
    const translate = (key: string) => `[${key}]`;

    it("localizes built-in terminal and file actions", () => {
      expect(
        localizeDefaultAiAction(
          {
            id: "explain-selected",
            name: "解释选中内容",
            prompt: "请解释终端中选中的内容，指出含义、可能原因和下一步建议。",
            enabled: true,
          },
          translate,
        ),
      ).toEqual({
        id: "explain-selected",
        name: "[ai.defaultActions.explainSelectedName]",
        prompt: "[ai.defaultActions.explainSelectedPrompt]",
        enabled: true,
      });

      expect(
        localizeDefaultAiAction(
          {
            id: "summarize-file",
            name: "总结文件",
            prompt: "请总结选中文件的主要内容、关键风险和建议操作。",
            enabled: true,
          },
          translate,
        ),
      ).toEqual({
        id: "summarize-file",
        name: "[ai.defaultActions.summarizeFileName]",
        prompt: "[ai.defaultActions.summarizeFilePrompt]",
        enabled: true,
      });
    });

    it("preserves custom actions and edited built-in fields", () => {
      const customAction = {
        id: "my-action",
        name: "My action",
        prompt: "My prompt",
        enabled: true,
      };
      expect(localizeDefaultAiAction(customAction, translate)).toBe(customAction);

      expect(
        localizeDefaultAiAction(
          {
            id: "explain-selected",
            name: "My custom name",
            prompt: "请解释终端中选中的内容，指出含义、可能原因和下一步建议。",
            enabled: true,
          },
          translate,
        ),
      ).toEqual({
        id: "explain-selected",
        name: "My custom name",
        prompt: "[ai.defaultActions.explainSelectedPrompt]",
        enabled: true,
      });
    });
  });
});
