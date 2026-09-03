import { describe, expect, it } from "vitest";
import { BUILTIN_PROVIDERS, DEFAULT_AI_SETTINGS } from "./aiSettings";

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
    });
  });
});
