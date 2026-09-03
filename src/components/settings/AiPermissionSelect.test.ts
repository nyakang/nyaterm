import { describe, expect, it } from "vitest";
import { AI_PERMISSION_MODES, requiresFullAccessConfirmation } from "./AiPermissionSelect";

describe("AI permission modes", () => {
  it("exposes the four permission levels in increasing order", () => {
    expect(AI_PERMISSION_MODES).toEqual(["observer", "confirm", "auto", "full_access"]);
  });

  it("only requires confirmation when entering full access", () => {
    expect(requiresFullAccessConfirmation("confirm", "full_access")).toBe(true);
    expect(requiresFullAccessConfirmation("auto", "full_access")).toBe(true);
    expect(requiresFullAccessConfirmation("full_access", "full_access")).toBe(false);
    expect(requiresFullAccessConfirmation("full_access", "confirm")).toBe(false);
  });
});
