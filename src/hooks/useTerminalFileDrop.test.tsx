import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useTerminalFileDrop } from "./useTerminalFileDrop";

const mocks = vi.hoisted(() => ({
  listen: vi.fn(),
  onDragDropEvent: vi.fn(),
  postMessageWithAdditionalObjects: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: mocks.onDragDropEvent }),
}));

type NativeDropListener = (event: {
  payload: {
    kind: "enter" | "over" | "leave" | "drop";
    paths: string[];
    position: { x: number; y: number };
  };
}) => void;

const targetRect = {
  left: 0,
  top: 0,
  right: 300,
  bottom: 300,
  width: 300,
  height: 300,
  x: 0,
  y: 0,
  toJSON: () => ({}),
} as DOMRect;

const originalElementFromPoint = Object.getOwnPropertyDescriptor(document, "elementFromPoint");
const originalChrome = Object.getOwnPropertyDescriptor(window, "chrome");

function createFileDropEvent(x = 100, y = 100) {
  const event = new Event("drop", { bubbles: true, cancelable: true });
  const file = new File(["test"], "single.txt", { type: "text/plain" });
  Object.defineProperties(event, {
    clientX: { value: x },
    clientY: { value: y },
    dataTransfer: {
      value: {
        types: ["Files"],
        files: [file],
        items: [],
        dropEffect: "none",
      },
    },
  });
  return event;
}

function renderTerminalFileDrop(container: HTMLDivElement) {
  const processDropPaths = vi.fn();
  const resetExternalDropHover = vi.fn();
  const setIsExternalDropActive = vi.fn();
  const result = renderHook(() =>
    useTerminalFileDrop({
      sessionId: "session-1",
      sessionType: "SSH",
      enabled: true,
      containerRef: { current: container },
      resetExternalDropHover,
      setIsExternalDropActive,
      processDropPaths,
      externalDropPathsRequiredMessage: "File path required",
    }),
  );

  return {
    ...result,
    processDropPaths,
    resetExternalDropHover,
    setIsExternalDropActive,
  };
}

describe("useTerminalFileDrop visual target ownership", () => {
  let nativeDropListener: NativeDropListener | undefined;

  beforeEach(() => {
    vi.clearAllMocks();
    nativeDropListener = undefined;
    mocks.listen.mockImplementation((eventName: string, listener: NativeDropListener) => {
      if (eventName === "external-file-drop") {
        nativeDropListener = listener;
      }
      return Promise.resolve(vi.fn());
    });
    Object.defineProperty(window, "chrome", {
      configurable: true,
      value: {
        webview: {
          postMessageWithAdditionalObjects: mocks.postMessageWithAdditionalObjects,
        },
      },
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
    document.body.replaceChildren();
    if (originalElementFromPoint) {
      Object.defineProperty(document, "elementFromPoint", originalElementFromPoint);
    } else {
      Reflect.deleteProperty(document, "elementFromPoint");
    }
    if (originalChrome) {
      Object.defineProperty(window, "chrome", originalChrome);
    } else {
      Reflect.deleteProperty(window, "chrome");
    }
  });

  it("does not bridge a browser drop when a floating panel covers the terminal", () => {
    const terminal = document.createElement("div");
    const floatingPanel = document.createElement("aside");
    document.body.append(terminal, floatingPanel);
    vi.spyOn(terminal, "getBoundingClientRect").mockReturnValue(targetRect);
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => floatingPanel),
    });
    const { processDropPaths } = renderTerminalFileDrop(terminal);

    act(() => {
      window.dispatchEvent(createFileDropEvent());
    });

    expect(mocks.postMessageWithAdditionalObjects).not.toHaveBeenCalled();
    expect(processDropPaths).not.toHaveBeenCalled();
  });

  it("bridges a browser drop exactly once when the terminal is topmost", () => {
    const terminal = document.createElement("div");
    const terminalChild = document.createElement("div");
    terminal.append(terminalChild);
    document.body.append(terminal);
    vi.spyOn(terminal, "getBoundingClientRect").mockReturnValue(targetRect);
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => terminalChild),
    });
    renderTerminalFileDrop(terminal);

    const event = createFileDropEvent();
    act(() => {
      window.dispatchEvent(event);
    });

    expect(event.defaultPrevented).toBe(true);
    expect(mocks.postMessageWithAdditionalObjects).toHaveBeenCalledTimes(1);
  });

  it("ignores a native drop response when the terminal is covered", () => {
    const terminal = document.createElement("div");
    const floatingPanel = document.createElement("aside");
    document.body.append(terminal, floatingPanel);
    vi.spyOn(terminal, "getBoundingClientRect").mockReturnValue(targetRect);
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => floatingPanel),
    });
    const { processDropPaths } = renderTerminalFileDrop(terminal);

    act(() => {
      nativeDropListener?.({
        payload: {
          kind: "drop",
          paths: ["C:\\drop\\single.txt"],
          position: { x: 100, y: 100 },
        },
      });
    });

    expect(processDropPaths).not.toHaveBeenCalled();
  });
});
