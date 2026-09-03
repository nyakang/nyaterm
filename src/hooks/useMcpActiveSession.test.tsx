import { renderHook, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { useMcpActiveSession } from "./useMcpActiveSession";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@/lib/invoke", () => ({ invoke: mocks.invoke }));

beforeEach(() => {
  vi.clearAllMocks();
  mocks.invoke.mockResolvedValue(undefined);
});

it("reports active session changes and clearing to the MCP host", async () => {
  const { rerender } = renderHook(({ sessionId }) => useMcpActiveSession(sessionId), {
    initialProps: { sessionId: "session-a" as string | null },
  });

  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenLastCalledWith("report_mcp_active_session", {
      sessionId: "session-a",
    }),
  );

  rerender({ sessionId: "session-b" });
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenLastCalledWith("report_mcp_active_session", {
      sessionId: "session-b",
    }),
  );

  rerender({ sessionId: null });
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenLastCalledWith("report_mcp_active_session", {
      sessionId: null,
    }),
  );
});
