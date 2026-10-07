import { fireEvent, render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { RdpSessionPane, Tab, TerminalSessionPane, VncSessionPane } from "@/types/global";
import PaneWorkspace from "./PaneWorkspace";

const { rdpPaneHostMock, vncPaneHostMock, xTerminalMock } = vi.hoisted(() => ({
  rdpPaneHostMock: vi.fn(),
  vncPaneHostMock: vi.fn(),
  xTerminalMock: vi.fn(),
}));

vi.mock("@/context/AppContext", () => ({
  useApp: () => ({
    tabs: [],
    syncGroups: [],
    broadcastToAll: false,
    isLocked: true,
    setSyncGroups: vi.fn(),
  }),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => vi.fn()),
}));

vi.mock("@/components/rdp/RdpPaneHost", () => ({
  default: (props: unknown) => {
    rdpPaneHostMock(props);
    return <div data-testid="rdp-pane-host" />;
  },
}));

vi.mock("@/components/vnc/VncPaneHost", () => ({
  default: (props: unknown) => {
    vncPaneHostMock(props);
    return <div data-testid="vnc-pane-host" />;
  },
}));

vi.mock("./XTerminal", () => ({
  default: (props: unknown) => {
    xTerminalMock(props);
    return <div data-testid="x-terminal" />;
  },
}));

describe("PaneWorkspace RDP routing", () => {
  beforeEach(() => {
    rdpPaneHostMock.mockReset();
    vncPaneHostMock.mockReset();
    xTerminalMock.mockReset();
  });

  it("routes RDP leaves to RdpPaneHost with active and visible state", () => {
    const onActivatePane = vi.fn();
    const onConnectionError = vi.fn();
    const tab = tabWithRoot(rdpPane(), "rdp-pane");

    const view = render(
      <PaneWorkspace
        tab={tab}
        visible
        onActivatePane={onActivatePane}
        onUpdateSplitRatio={vi.fn()}
        onConnectionError={onConnectionError}
      />,
    );

    expect(rdpPaneHostMock).toHaveBeenCalledWith(
      expect.objectContaining({
        pane: tab.root,
        active: true,
        visible: true,
      }),
    );
    expect(xTerminalMock).not.toHaveBeenCalled();

    const rdpHost = view.getByTestId("rdp-pane-host");
    fireEvent.mouseDown(rdpHost.parentElement as HTMLElement);
    expect(onActivatePane).toHaveBeenCalledWith("rdp-pane");

    const rdpProps = rdpPaneHostMock.mock.lastCall?.[0] as {
      onConnectionError: (sessionId: string, error: string) => void;
    };
    expect(rdpProps).not.toHaveProperty("onDisconnectedCloseRequested");
    rdpProps.onConnectionError("rdp-session", "RDP failed");
    expect(onConnectionError).toHaveBeenCalledWith(
      "tab-1",
      "rdp-pane",
      "rdp-session",
      "RDP failed",
    );
  });

  it("keeps terminal leaves on XTerminal in a mixed split tree", () => {
    const onUpdateSplitRatio = vi.fn();
    const tab = tabWithRoot(
      {
        id: "split-1",
        kind: "split",
        direction: "vertical",
        ratio: 0.4,
        first: terminalPane(),
        second: rdpPane(),
      },
      "terminal-pane",
    );

    const view = render(
      <PaneWorkspace
        tab={tab}
        visible
        onActivatePane={vi.fn()}
        onUpdateSplitRatio={onUpdateSplitRatio}
      />,
    );

    expect(xTerminalMock).toHaveBeenCalledWith(
      expect.objectContaining({
        sessionId: "terminal-session",
        sessionType: "SSH",
        active: true,
        appLocked: true,
        visible: true,
      }),
    );
    expect(rdpPaneHostMock).toHaveBeenCalledWith(
      expect.objectContaining({
        pane: expect.objectContaining({ id: "rdp-pane", paneKind: "remote-desktop" }),
        active: false,
        visible: true,
      }),
    );
    expect(view.getAllByTestId(/(?:x-terminal|rdp-pane-host)/)).toHaveLength(2);
  });

  it("keeps inactive panes mounted but hidden in pane focus mode", () => {
    const tab = tabWithRoot(
      {
        id: "split-1",
        kind: "split",
        direction: "vertical",
        ratio: 0.4,
        first: terminalPane(),
        second: rdpPane(),
      },
      "rdp-pane",
    );

    const view = render(
      <PaneWorkspace
        tab={tab}
        visible
        paneFocusMode
        onActivatePane={vi.fn()}
        onUpdateSplitRatio={vi.fn()}
      />,
    );

    expect(view.getByTestId("x-terminal")).not.toBeNull();
    expect(view.getByTestId("rdp-pane-host")).not.toBeNull();
    expect(xTerminalMock).toHaveBeenCalledWith(
      expect.objectContaining({ visible: false, active: false }),
    );
    expect(rdpPaneHostMock).toHaveBeenCalledWith(
      expect.objectContaining({ visible: true, active: true }),
    );
  });

  it("routes VNC leaves to VncPaneHost without invoking RDP or terminal hosts", () => {
    const tab = tabWithRoot(vncPane(), "vnc-pane");
    const view = render(
      <PaneWorkspace tab={tab} visible onActivatePane={vi.fn()} onUpdateSplitRatio={vi.fn()} />,
    );

    expect(view.getByTestId("vnc-pane-host")).not.toBeNull();
    expect(vncPaneHostMock).toHaveBeenCalledWith(
      expect.objectContaining({
        pane: tab.root,
        active: true,
        visible: true,
      }),
    );
    expect(rdpPaneHostMock).not.toHaveBeenCalled();
    expect(xTerminalMock).not.toHaveBeenCalled();
  });

  it("does not mark an active RDP leaf active while its tab is hidden", () => {
    const tab = tabWithRoot(rdpPane(), "rdp-pane");
    const view = render(
      <PaneWorkspace
        tab={tab}
        visible={false}
        onActivatePane={vi.fn()}
        onUpdateSplitRatio={vi.fn()}
      />,
    );

    expect((view.container.firstElementChild as HTMLElement).style.display).toBe("none");
    expect(rdpPaneHostMock).toHaveBeenCalledWith(
      expect.objectContaining({ active: false, visible: false }),
    );
  });

  it("renders the SFTP-only placeholder without mounting XTerminal", () => {
    const pane = { ...terminalPane(), sshRuntimeMode: "sftp" as const };
    const tab = tabWithRoot(pane, pane.id);
    const view = render(
      <PaneWorkspace tab={tab} visible onActivatePane={vi.fn()} onUpdateSplitRatio={vi.fn()} />,
    );

    expect(view.getByTestId("sftp-only-placeholder")).not.toBeNull();
    expect(xTerminalMock).not.toHaveBeenCalled();
  });

  it("uses effective SessionInfo mode for Standard-to-SFTP fallback", () => {
    const pane = terminalPane();
    const tab = tabWithRoot(pane, pane.id);
    const view = render(
      <PaneWorkspace
        tab={tab}
        visible
        sessionInfoById={
          new Map([[pane.sessionId, { id: pane.sessionId, ssh_runtime_mode: "sftp" } as never]])
        }
        onActivatePane={vi.fn()}
        onUpdateSplitRatio={vi.fn()}
      />,
    );

    expect(view.getByTestId("sftp-only-placeholder")).not.toBeNull();
    expect(xTerminalMock).not.toHaveBeenCalled();
  });

  it("clears fallback SFTP presentation when a reconnect creates a standard session", () => {
    const pane = terminalPane();
    const tab = tabWithRoot(pane, pane.id);
    const view = render(
      <PaneWorkspace
        tab={tab}
        visible
        sessionInfoById={
          new Map([[pane.sessionId, { id: pane.sessionId, ssh_runtime_mode: "sftp" } as never]])
        }
        onActivatePane={vi.fn()}
        onUpdateSplitRatio={vi.fn()}
      />,
    );

    const reconnectedPane = { ...pane, sessionId: "ssh-session-2" };
    view.rerender(
      <PaneWorkspace
        tab={tabWithRoot(reconnectedPane, reconnectedPane.id)}
        visible
        sessionInfoById={
          new Map([
            [
              reconnectedPane.sessionId,
              { id: reconnectedPane.sessionId, ssh_runtime_mode: "standard" } as never,
            ],
          ])
        }
        onActivatePane={vi.fn()}
        onUpdateSplitRatio={vi.fn()}
      />,
    );

    expect(view.queryByTestId("sftp-only-placeholder")).toBeNull();
    expect(xTerminalMock).toHaveBeenCalledWith(
      expect.objectContaining({ sessionId: reconnectedPane.sessionId }),
    );
  });

  it("updates XTerminal runtime mode when the same SSH pane reconnects as Mosh", () => {
    const pane = terminalPane();
    const tab = tabWithRoot(pane, pane.id);
    const view = render(
      <PaneWorkspace
        tab={tab}
        visible
        sessionInfoById={
          new Map([[pane.sessionId, { id: pane.sessionId, ssh_runtime_mode: "standard" } as never]])
        }
        onActivatePane={vi.fn()}
        onUpdateSplitRatio={vi.fn()}
      />,
    );

    expect(xTerminalMock).toHaveBeenLastCalledWith(
      expect.objectContaining({
        sessionId: pane.sessionId,
        connectionId: pane.connectionId,
        sshRuntimeMode: "standard",
      }),
    );

    const reconnectedPane = { ...pane, sessionId: "mosh-session-2" };
    view.rerender(
      <PaneWorkspace
        tab={tabWithRoot(reconnectedPane, reconnectedPane.id)}
        visible
        sessionInfoById={new Map([[reconnectedPane.sessionId, { id: reconnectedPane.sessionId } as never]])}
        onActivatePane={vi.fn()}
        onUpdateSplitRatio={vi.fn()}
      />,
    );

    expect(xTerminalMock).toHaveBeenLastCalledWith(
      expect.objectContaining({
        sessionId: reconnectedPane.sessionId,
        connectionId: pane.connectionId,
        sshRuntimeMode: undefined,
      }),
    );
  });

  it("updates XTerminal runtime mode when the same Mosh pane reconnects as SSH", () => {
    const pane = terminalPane();
    const tab = tabWithRoot(pane, pane.id);
    const view = render(
      <PaneWorkspace
        tab={tab}
        visible
        sessionInfoById={new Map([[pane.sessionId, { id: pane.sessionId } as never]])}
        onActivatePane={vi.fn()}
        onUpdateSplitRatio={vi.fn()}
      />,
    );

    expect(xTerminalMock).toHaveBeenLastCalledWith(
      expect.objectContaining({ sshRuntimeMode: undefined }),
    );

    const reconnectedPane = { ...pane, sessionId: "ssh-session-2" };
    view.rerender(
      <PaneWorkspace
        tab={tabWithRoot(reconnectedPane, reconnectedPane.id)}
        visible
        sessionInfoById={
          new Map([
            [
              reconnectedPane.sessionId,
              { id: reconnectedPane.sessionId, ssh_runtime_mode: "standard" } as never,
            ],
          ])
        }
        onActivatePane={vi.fn()}
        onUpdateSplitRatio={vi.fn()}
      />,
    );

    expect(xTerminalMock).toHaveBeenLastCalledWith(
      expect.objectContaining({
        sessionId: reconnectedPane.sessionId,
        connectionId: pane.connectionId,
        sshRuntimeMode: "standard",
      }),
    );
  });
});

function tabWithRoot(root: Tab["root"], activePaneId: string): Tab {
  return {
    id: "tab-1",
    persistOrder: 0,
    activePaneId,
    root,
  };
}

function rdpPane(): RdpSessionPane {
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
  };
}

function vncPane(): VncSessionPane {
  return {
    id: "vnc-pane",
    kind: "leaf",
    paneKind: "remote-desktop",
    sessionId: "vnc-session",
    name: "VNC Desktop",
    type: "VNC",
    connectionId: "vnc-connection",
    display: { scaleMode: "fit" },
  };
}

function terminalPane(): TerminalSessionPane {
  return {
    id: "terminal-pane",
    kind: "leaf",
    paneKind: "terminal",
    sessionId: "terminal-session",
    name: "SSH Terminal",
    type: "SSH",
    connectionId: "ssh-connection",
  };
}
