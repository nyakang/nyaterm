import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { installWebviewReloadGuard } from "./webviewReloadGuard";

beforeAll(() => installWebviewReloadGuard());

afterEach(() => {
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

function dispatchShortcut(target: Element, init: KeyboardEventInit) {
  const received = vi.fn();
  target.addEventListener("keydown", received);
  const event = new KeyboardEvent("keydown", {
    bubbles: true,
    cancelable: true,
    composed: true,
    ...init,
  });
  target.dispatchEvent(event);
  target.removeEventListener("keydown", received);
  return { event, received };
}

function inputTarget(marker?: string, tag = "textarea") {
  const root = document.createElement("div");
  if (marker) root.setAttribute(marker, "true");
  document.body.appendChild(root);
  if (tag === "root") return root;
  const child = document.createElement(tag);
  root.appendChild(child);
  return child;
}

describe("webviewReloadGuard", () => {
  it.each([
    ["RDP root", "data-rdp-input-root", "root"],
    ["RDP textarea", "data-rdp-input-root", "textarea"],
    ["RDP canvas", "data-rdp-input-root", "canvas"],
    ["shared remote desktop root", "data-remote-desktop-input-root", "root"],
    ["VNC canvas", "data-remote-desktop-input-root", "canvas"],
  ])("keeps Ctrl+R available to %s without reloading the WebView", (_label, marker, tag) => {
    const target = inputTarget(marker, tag);
    const { event, received } = dispatchShortcut(target, {
      key: "r",
      code: "KeyR",
      ctrlKey: true,
    });

    expect(event.defaultPrevented).toBe(true);
    expect(received).toHaveBeenCalledOnce();
  });

  it("uses the composed path for an input root inside a shadow tree", () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const shadow = host.attachShadow({ mode: "open" });
    const root = document.createElement("div");
    root.setAttribute("data-remote-desktop-input-root", "true");
    const input = document.createElement("textarea");
    root.appendChild(input);
    shadow.appendChild(root);

    const { event, received } = dispatchShortcut(input, {
      key: "r",
      code: "KeyR",
      ctrlKey: true,
    });

    expect(event.defaultPrevented).toBe(true);
    expect(received).toHaveBeenCalledOnce();
  });

  it("uses closest() when the composed path is unavailable", () => {
    const target = inputTarget("data-rdp-input-root");
    const event = new KeyboardEvent("keydown", {
      key: "r",
      code: "KeyR",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    vi.spyOn(event, "composedPath").mockReturnValue([]);
    const received = vi.fn();
    target.addEventListener("keydown", received);
    target.dispatchEvent(event);

    expect(event.defaultPrevented).toBe(true);
    expect(received).toHaveBeenCalledOnce();
  });

  it.each([
    ["Ctrl+R", { code: "KeyR", ctrlKey: true }],
    ["Meta+R", { code: "KeyR", metaKey: true }],
    ["Ctrl+Shift+R", { code: "KeyR", ctrlKey: true, shiftKey: true }],
    ["F5", { code: "F5" }],
  ])("blocks %s outside remote and terminal inputs", (_label, shortcut) => {
    const { event, received } = dispatchShortcut(inputTarget(), shortcut);
    expect(event.defaultPrevented).toBe(true);
    expect(received).not.toHaveBeenCalled();
  });

  it.each([
    ["Windows", { code: "KeyP", ctrlKey: true }],
    ["Macintosh", { code: "KeyP", metaKey: true }],
  ])("blocks the %s print shortcut in ordinary UI", (userAgent, shortcut) => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue(userAgent);
    const { event, received } = dispatchShortcut(inputTarget(), shortcut);
    expect(event.defaultPrevented).toBe(true);
    expect(received).not.toHaveBeenCalled();
  });

  it("preserves terminal Ctrl+R and only prevents hard reload for Ctrl+Shift+R", () => {
    const target = inputTarget("data-terminal-root");
    const regular = dispatchShortcut(target, { code: "KeyR", ctrlKey: true });
    expect(regular.event.defaultPrevented).toBe(false);
    expect(regular.received).toHaveBeenCalledOnce();

    const hard = dispatchShortcut(target, { code: "KeyR", ctrlKey: true, shiftKey: true });
    expect(hard.event.defaultPrevented).toBe(true);
    expect(hard.received).toHaveBeenCalledOnce();
  });

  it.each([
    ["Ctrl+Alt+R", { code: "KeyR", ctrlKey: true, altKey: true }],
    ["Alt+F5", { code: "F5", altKey: true }],
    ["Ctrl+Shift+P", { code: "KeyP", ctrlKey: true, shiftKey: true }],
  ])("leaves %s unchanged", (_label, shortcut) => {
    const { event, received } = dispatchShortcut(inputTarget(), shortcut);
    expect(event.defaultPrevented).toBe(false);
    expect(received).toHaveBeenCalledOnce();
  });
});
