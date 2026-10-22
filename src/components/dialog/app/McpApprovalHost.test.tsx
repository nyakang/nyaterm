import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ComponentProps } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { McpApprovalRequest } from "@/types/global";
import { McpApprovalHost } from "./McpApprovalHost";

type EventHandler = (event: { payload: unknown }) => void;

const mocks = vi.hoisted(() => ({
  listeners: new Map<string, EventHandler>(),
  invoke: vi.fn(),
  error: vi.fn(),
  onOpenChange: null as ((open: boolean) => void) | null,
}));

vi.mock("@/lib/backend/api", () => ({
  listen: vi.fn(async (event: string, handler: EventHandler) => {
    mocks.listeners.set(event, handler);
    return () => {
      if (mocks.listeners.get(event) === handler) mocks.listeners.delete(event);
    };
  }),
}));
vi.mock("@/lib/invoke", () => ({ invoke: mocks.invoke }));
vi.mock("sonner", () => ({ toast: { error: mocks.error } }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => key,
    i18n: { language: "en" },
  }),
}));
vi.mock("@/components/ui/alert-dialog", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/components/ui/alert-dialog")>();
  return {
    ...actual,
    AlertDialog: (props: ComponentProps<typeof actual.AlertDialog>) => {
      mocks.onOpenChange = props.onOpenChange ?? null;
      return <actual.AlertDialog {...props} />;
    },
  };
});

function emit(event: string, payload: unknown) {
  const handler = mocks.listeners.get(event);
  expect(handler).toBeDefined();
  act(() => handler?.({ payload }));
}

function queueRequest(requestId: string) {
  emit("mcp-approval-request", {
    requestId,
    client: `client-${requestId}`,
    capability: "execute_command",
    connectionName: `connection-${requestId}`,
    parameterSummary: `parameters-${requestId}`,
    risk: "medium",
  } satisfies McpApprovalRequest);
}

function deferred() {
  let resolve!: () => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<void>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  mocks.listeners.clear();
  mocks.invoke.mockReset().mockResolvedValue(undefined);
  mocks.error.mockReset();
  mocks.onOpenChange = null;
});

describe("McpApprovalHost", () => {
  it("processes queued requests FIFO with each selected approval mode and closes after the last", async () => {
    const first = deferred();
    mocks.invoke.mockReturnValueOnce(first.promise);
    render(<McpApprovalHost />);
    queueRequest("one");

    fireEvent.click(screen.getByRole("button", { name: "ai.externalMcpAllowSession" }));
    expect(mocks.invoke).toHaveBeenCalledExactlyOnceWith("respond_external_mcp_approval", {
      requestId: "one",
      decision: "allow_session",
    });
    expect(screen.getByText("client-one")).toBeTruthy();
    expect(screen.getByRole("alertdialog")).toBeTruthy();

    queueRequest("two");
    queueRequest("three");
    expect(screen.queryByText("client-two")).toBeNull();

    await act(async () => first.resolve());
    expect(screen.getByText("client-two")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "ai.externalMcpAllowOnce" }));

    await waitFor(() => expect(screen.getByText("client-three")).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "ai.externalMcpDeny" }));

    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(mocks.invoke.mock.calls).toEqual([
      ["respond_external_mcp_approval", { requestId: "one", decision: "allow_session" }],
      ["respond_external_mcp_approval", { requestId: "two", decision: "allow_once" }],
      ["respond_external_mcp_approval", { requestId: "three", decision: "deny" }],
    ]);
  });

  it("keeps the modal and its request visible while pending and ignores duplicate clicks or close", async () => {
    const pending = deferred();
    mocks.invoke.mockReturnValueOnce(pending.promise);
    render(<McpApprovalHost />);
    queueRequest("one");

    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "ai.externalMcpAllowSession" }));
      fireEvent.click(screen.getByRole("button", { name: "ai.externalMcpAllowOnce" }));
      mocks.onOpenChange?.(false);
    });
    expect(mocks.invoke).toHaveBeenCalledOnce();
    expect(screen.getByText("client-one")).toBeTruthy();
    expect(screen.getByRole("alertdialog")).toBeTruthy();
    for (const name of [
      "ai.externalMcpDeny",
      "ai.externalMcpAllowOnce",
      "ai.externalMcpAllowSession",
    ]) {
      expect((screen.getByRole("button", { name }) as HTMLButtonElement).disabled).toBe(true);
    }

    await act(async () => pending.resolve());
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(mocks.invoke).toHaveBeenCalledExactlyOnceWith("respond_external_mcp_approval", {
      requestId: "one",
      decision: "allow_session",
    });
  });

  it("treats closing an unanswered prompt as one denial", async () => {
    render(<McpApprovalHost />);
    queueRequest("one");
    act(() => mocks.onOpenChange?.(false));

    expect(mocks.invoke).toHaveBeenCalledExactlyOnceWith("respond_external_mcp_approval", {
      requestId: "one",
      decision: "deny",
    });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  it("reports a failed response and lets the next request be answered", async () => {
    const first = deferred();
    mocks.invoke.mockReturnValueOnce(first.promise);
    render(<McpApprovalHost />);
    queueRequest("one");
    queueRequest("two");
    fireEvent.click(screen.getByRole("button", { name: "ai.externalMcpAllowOnce" }));

    await act(async () => first.reject(new Error("Approval expired")));
    expect(mocks.error).toHaveBeenCalledWith("Approval expired");
    expect(screen.getByText("client-two")).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: "ai.externalMcpDeny" }) as HTMLButtonElement).disabled,
    ).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "ai.externalMcpDeny" }));

    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(mocks.invoke.mock.calls).toEqual([
      ["respond_external_mcp_approval", { requestId: "one", decision: "allow_once" }],
      ["respond_external_mcp_approval", { requestId: "two", decision: "deny" }],
    ]);
  });

  it.each([
    "resolved",
    "rejected",
  ] as const)("does not remove a new request after locking when an old response is %s late", async (outcome) => {
    const first = deferred();
    mocks.invoke.mockReturnValueOnce(first.promise);
    render(<McpApprovalHost />);
    queueRequest("old");
    fireEvent.click(screen.getByRole("button", { name: "ai.externalMcpAllowSession" }));

    emit("app-lock-state-changed", { locked: true });
    expect(screen.queryByRole("alertdialog")).toBeNull();
    queueRequest("new");
    expect(screen.getByText("client-new")).toBeTruthy();

    await act(async () => {
      if (outcome === "resolved") first.resolve();
      else first.reject(new Error("No longer pending"));
    });
    expect(screen.getByText("client-new")).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: "ai.externalMcpAllowOnce" }) as HTMLButtonElement)
        .disabled,
    ).toBe(false);

    fireEvent.click(screen.getByRole("button", { name: "ai.externalMcpAllowOnce" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(mocks.invoke.mock.calls).toEqual([
      ["respond_external_mcp_approval", { requestId: "old", decision: "allow_session" }],
      ["respond_external_mcp_approval", { requestId: "new", decision: "allow_once" }],
    ]);
    expect(mocks.error).toHaveBeenCalledTimes(outcome === "rejected" ? 1 : 0);
  });

  it("drops a request resolved from the system notification without denying it or the next one", async () => {
    render(<McpApprovalHost />);
    queueRequest("one");
    queueRequest("two");

    emit("mcp-approval-resolved", { requestId: "two", decision: "allow_once" });
    expect(screen.getByText("client-one")).toBeTruthy();
    expect(mocks.invoke).not.toHaveBeenCalled();

    act(() => mocks.onOpenChange?.(false));
    expect(mocks.invoke).toHaveBeenCalledExactlyOnceWith("respond_external_mcp_approval", {
      requestId: "one",
      decision: "deny",
    });

    emit("mcp-approval-resolved", { requestId: "one", decision: "deny" });
    act(() => mocks.onOpenChange?.(false));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(mocks.invoke).toHaveBeenCalledOnce();
  });
});
