import { expect, it, vi } from "vitest";
import { NyaScriptSessionOpenLifecycle } from "./nyascriptSessionOpenLifecycle";

const pending = {
  tabId: "tab-1",
  createRequestId: "create-1",
};

it("cancels a pending tab when cancellation arrives before onPending", () => {
  const lifecycle = new NyaScriptSessionOpenLifecycle();
  const cancelPending = vi.fn();

  expect(lifecycle.begin("request-1")).toBe(true);
  expect(lifecycle.cancel("request-1", cancelPending)).toBe("cancelled");
  expect(cancelPending).not.toHaveBeenCalled();

  expect(lifecycle.registerPending("request-1", pending, cancelPending)).toBe(false);
  expect(cancelPending).toHaveBeenCalledOnce();
  expect(cancelPending).toHaveBeenCalledWith(pending);
  expect(lifecycle.markOpened("request-1")).toBe(false);
});

it("preserves an opened terminal when cancellation arrives before the response", () => {
  const lifecycle = new NyaScriptSessionOpenLifecycle();
  const cancelPending = vi.fn();

  expect(lifecycle.begin("request-1")).toBe(true);
  expect(lifecycle.registerPending("request-1", pending, cancelPending)).toBe(true);
  expect(lifecycle.markOpened("request-1")).toBe(true);

  expect(lifecycle.cancel("request-1", cancelPending)).toBe("opened");
  expect(cancelPending).not.toHaveBeenCalled();
});
