import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { installWebviewReloadGuard } from "@/lib/webviewReloadGuard";
import type { RdpSessionPane } from "@/types/global";
import RdpPaneHost from "./RdpPaneHost";

const { invokeMock, listenMock, listeners } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  listenMock: vi.fn(),
  listeners: new Map<string, (event: { payload: unknown }) => void>(),
}));

vi.mock("@/lib/invoke", () => ({
  invoke: invokeMock,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: listenMock,
}));

vi.mock("@tauri-apps/api/core", () => ({
  Channel: class Channel<T> {
    onmessage: (message: T) => void;

    constructor(onmessage: (message: T) => void) {
      this.onmessage = onmessage;
    }
  },
}));

describe("RdpPaneHost", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(undefined);
    listenMock.mockReset();
    listeners.clear();
    listenMock.mockImplementation(
      (eventName: string, handler: (event: { payload: unknown }) => void) => {
        listeners.set(eventName, handler);
        return Promise.resolve(vi.fn());
      },
    );
  });

  it("attaches the frame channel and subscribes to session-scoped events", async () => {
    render(<RdpPaneHost pane={rdpPane()} active visible />);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("rdp_attach_frame_channel", {
        sessionId: "rdp-session",
        frameChannel: expect.any(Object),
      });
    });
    expect(listenMock).toHaveBeenCalledWith("rdp-state-rdp-session", expect.any(Function));
    expect(listenMock).toHaveBeenCalledWith("rdp-pointer-rdp-session", expect.any(Function));
  });

  it("forwards state failures using the existing callback payload", async () => {
    const onConnectionError = vi.fn();
    render(<RdpPaneHost pane={rdpPane()} active visible onConnectionError={onConnectionError} />);

    await waitFor(() => expect(listeners.has("rdp-state-rdp-session")).toBe(true));
    act(() => {
      listeners.get("rdp-state-rdp-session")?.({
        payload: {
          sessionId: "rdp-session",
          state: "failed",
          message: "certificate rejected",
        },
      });
    });

    expect(screen.getByText("certificate rejected")).not.toBeNull();
    expect(onConnectionError).toHaveBeenCalledWith("rdp-session", "certificate rejected");
  });

  it("sends the existing physical keyboard wire payload after activation", async () => {
    render(<RdpPaneHost pane={rdpPane()} active visible />);

    await waitFor(() => expect(listeners.has("rdp-state-rdp-session")).toBe(true));
    act(() => {
      listeners.get("rdp-state-rdp-session")?.({
        payload: { sessionId: "rdp-session", state: "active" },
      });
    });

    invokeMock.mockClear();
    const inputRoot = document.querySelector('[data-rdp-input-root="true"]');
    if (!(inputRoot instanceof HTMLElement)) throw new Error("expected RDP input root");
    fireEvent.keyDown(inputRoot, { code: "ControlLeft", key: "Control" });
    fireEvent.keyUp(inputRoot, { code: "ControlLeft", key: "Control" });

    expect(invokeMock).toHaveBeenCalledWith("rdp_input_batch", {
      sessionId: "rdp-session",
      events: [
        {
          type: "key-down",
          scanCode: 0x1d,
          extended: false,
          repeat: false,
        },
      ],
    });
    expect(invokeMock).toHaveBeenCalledWith("rdp_input_batch", {
      sessionId: "rdp-session",
      events: [
        {
          type: "key-up",
          scanCode: 0x1d,
          extended: false,
          repeat: false,
        },
      ],
    });
  });

  it("forwards Ctrl+R through the WebView guard as an RDP physical key", async () => {
    installWebviewReloadGuard();
    render(<RdpPaneHost pane={rdpPane()} active visible />);

    await waitFor(() => expect(listeners.has("rdp-state-rdp-session")).toBe(true));
    act(() => {
      listeners.get("rdp-state-rdp-session")?.({
        payload: { sessionId: "rdp-session", state: "active" },
      });
    });

    invokeMock.mockClear();
    const inputRoot = document.querySelector('[data-rdp-input-root="true"]');
    if (!(inputRoot instanceof HTMLElement)) throw new Error("expected RDP input root");

    fireEvent.keyDown(inputRoot, { code: "ControlLeft", key: "Control", ctrlKey: true });
    fireEvent.keyDown(inputRoot, { code: "KeyR", key: "r", ctrlKey: true });
    fireEvent.keyUp(inputRoot, { code: "KeyR", key: "r", ctrlKey: true });
    fireEvent.keyUp(inputRoot, { code: "ControlLeft", key: "Control" });

    expect(invokeMock).toHaveBeenCalledWith("rdp_input_batch", {
      sessionId: "rdp-session",
      events: [{ type: "key-down", scanCode: 0x13, extended: false, repeat: false }],
    });
    expect(invokeMock).toHaveBeenCalledWith("rdp_input_batch", {
      sessionId: "rdp-session",
      events: [{ type: "key-up", scanCode: 0x13, extended: false, repeat: false }],
    });
  });

  it("does not render the RDP hover information bar", () => {
    render(<RdpPaneHost pane={rdpPane()} active visible />);

    expect(screen.queryByText("Windows Desktop")).toBeNull();
    expect(screen.queryByText("1920x1080")).toBeNull();
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });
});

function rdpPane(overrides: Partial<RdpSessionPane> = {}): RdpSessionPane {
  return {
    id: "rdp-pane",
    kind: "leaf",
    paneKind: "remote-desktop",
    sessionId: "rdp-session",
    name: "Windows Desktop",
    type: "RDP",
    connectionId: "rdp-connection",
    display: {
      remoteWidth: 1920,
      remoteHeight: 1080,
      scaleMode: "fit",
    },
    ...overrides,
  };
}
