import { fireEvent, render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { RdpSessionPane, Tab, TerminalSessionPane, VncSessionPane } from "@/types/global";
import PaneWorkspace from "./PaneWorkspace";

const { rdpPaneHostMock, vncPaneHostMock, xTerminalMock, appState } = vi.hoisted(() => ({
  appState: { tabs: [] as Tab[], setActivePane: vi.fn() },
  rdpPaneHostMock: vi.fn(),
  vncPaneHostMock: vi.fn(),
  xTerminalMock: vi.fn(),
}));

vi.mock("@/context/AppContext", () => ({
  useApp: () => ({
    tabs: appState.tabs,
    setActivePane: appState.setActivePane,
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

vi.mock("./TmuxWindowStrip", () => ({
  default: ({ controlSessionId }: { controlSessionId: string }) => (
    <div data-testid={`tmux-strip-${controlSessionId}`} />
  ),
}));

vi.mock("./XTerminal", () => ({
  default: (props: unknown) => {
    xTerminalMock(props);
    const terminal = props as { sessionId: string; active: boolean; visible: boolean };
    return (
      <div
        data-testid="x-terminal"
        data-session={terminal.sessionId}
        data-active={String(terminal.active)}
        data-visible={String(terminal.visible)}
      />
    );
  },
}));

describe("PaneWorkspace RDP routing", () => {
  beforeEach(() => {
    rdpPaneHostMock.mockReset();
    vncPaneHostMock.mockReset();
    xTerminalMock.mockReset();
    appState.tabs = [];
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

describe("PaneWorkspace tmux leaf integration", () => {
  function setup(twoControls = false) {
    const ssh = terminalPane();
    const sibling: TerminalSessionPane = {
      ...terminalPane(),
      id: "sibling",
      sessionId: "sibling-session",
      type: twoControls ? "SSH" : "Local",
    };
    const host = tabWithRoot(
      {
        id: "split",
        kind: "split",
        direction: "vertical",
        ratio: 0.4,
        first: ssh,
        second: sibling,
      },
      ssh.id,
    );
    const virtual: TerminalSessionPane = {
      ...ssh,
      id: "virtual",
      sessionId: "virtual-session",
      tmux: { controlSessionId: ssh.sessionId, paneId: "%0" },
    };
    const child: Tab = {
      ...tabWithRoot(virtual, virtual.id),
      id: "child",
      tmux: {
        controlSessionId: ssh.sessionId,
        windowId: "@0",
        windowName: "bash",
        windowIndex: 0,
        active: true,
      },
    };
    const props = {
      tab: host,
      visible: true,
      onActivatePane: vi.fn(),
      onActivatePaneInTab: vi.fn(),
      onUpdateSplitRatio: vi.fn(),
    };
    return { ssh, sibling, host, child, props };
  }

  it("keeps the sibling visible and the shell mounted across attach/detach", () => {
    const { ssh, sibling, host, child, props } = setup();
    appState.tabs = [host];
    const view = render(<PaneWorkspace {...props} />);
    const shell = view.container.querySelector(`[data-session="${ssh.sessionId}"]`)!;
    const peer = view.container.querySelector(`[data-session="${sibling.sessionId}"]`)!;
    appState.tabs = [host, child];
    view.rerender(<PaneWorkspace {...props} tab={{ ...host }} />);
    expect(view.container.querySelector(`[data-session="${ssh.sessionId}"]`)).toBe(shell);
    expect(shell.getAttribute("data-visible")).toBe("false");
    expect(view.container.querySelector(`[data-session="${sibling.sessionId}"]`)).toBe(peer);
    expect(peer.getAttribute("data-visible")).toBe("true");
    const pane = view.container.querySelector('[data-session="virtual-session"]')!;
    expect(pane.getAttribute("data-visible")).toBe("true");
    fireEvent.mouseDown(pane);
    expect(props.onActivatePaneInTab).toHaveBeenCalledWith(child.id, child.activePaneId);
    fireEvent.mouseDown(peer);
    expect(props.onActivatePane).toHaveBeenLastCalledWith(sibling.id);
    appState.tabs = [host];
    view.rerender(<PaneWorkspace {...props} tab={{ ...host }} />);
    expect(view.container.querySelector(`[data-session="${ssh.sessionId}"]`)).toBe(shell);
    expect(shell.getAttribute("data-visible")).toBe("true");
    expect(view.queryByTestId(`tmux-strip-${ssh.sessionId}`)).toBeNull();
    view.unmount();
  });

  it("keeps both remote window terminals mounted when switching windows", () => {
    const { host, child, props } = setup();
    const secondPane = { ...terminalPane(), id: "remote-second", sessionId: "remote-second-session", tmux: { controlSessionId: "terminal-session", paneId: "%2" } };
    const second: Tab = { ...child, id: "window-second", root: secondPane, activePaneId: secondPane.id, tmux: { ...child.tmux!, windowId: "@1", windowIndex: 1, active: false } };
    appState.tabs = [host, child, second];
    const view = render(<PaneWorkspace {...props} />);
    const firstTerminal = view.container.querySelector('[data-session="virtual-session"]')!;
    const secondTerminal = view.container.querySelector('[data-session="remote-second-session"]')!;
    expect(firstTerminal.getAttribute("data-visible")).toBe("true");
    expect(secondTerminal.getAttribute("data-visible")).toBe("false");
    appState.tabs = [host, { ...child, tmux: { ...child.tmux!, active: false } }, { ...second, tmux: { ...second.tmux!, active: true } }];
    view.rerender(<PaneWorkspace {...props} tab={{ ...host }} />);
    expect(view.container.querySelector('[data-session="virtual-session"]')).toBe(firstTerminal);
    expect(view.container.querySelector('[data-session="remote-second-session"]')).toBe(secondTerminal);
    expect(firstTerminal.getAttribute("data-visible")).toBe("false");
    expect(secondTerminal.getAttribute("data-visible")).toBe("true");
    view.rerender(<PaneWorkspace {...props} visible={false} />);
    expect(secondTerminal.getAttribute("data-visible")).toBe("false");
    view.unmount();
    appState.tabs = [];
  });

  it("gates tmux focus by the host leaf and supports independent controls", () => {
    const { ssh, sibling, host, child, props } = setup(true);
    const secondPane = {
      ...sibling,
      id: "second-virtual",
      sessionId: "second-virtual-session",
      tmux: { controlSessionId: sibling.sessionId, paneId: "%1" },
    };
    const second: Tab = {
      ...child,
      id: "second-child",
      root: secondPane,
      activePaneId: secondPane.id,
      tmux: { ...child.tmux!, controlSessionId: sibling.sessionId },
    };
    appState.tabs = [host, child, second];
    const view = render(<PaneWorkspace {...props} />);
    expect(
      view.container.querySelector('[data-session="virtual-session"]')?.getAttribute("data-active"),
    ).toBe("true");
    expect(
      view.container
        .querySelector('[data-session="second-virtual-session"]')
        ?.getAttribute("data-active"),
    ).toBe("false");
    view.rerender(<PaneWorkspace {...props} tab={{ ...host, activePaneId: sibling.id }} />);
    expect(
      view.container.querySelector('[data-session="virtual-session"]')?.getAttribute("data-active"),
    ).toBe("false");
    expect(
      view.container
        .querySelector('[data-session="second-virtual-session"]')
        ?.getAttribute("data-active"),
    ).toBe("true");
    view.rerender(<PaneWorkspace {...props} paneFocusMode />);
    expect(
      view.container
        .querySelector('[data-session="virtual-session"]')
        ?.getAttribute("data-visible"),
    ).toBe("true");
    expect(
      view.container
        .querySelector('[data-session="second-virtual-session"]')
        ?.getAttribute("data-visible"),
    ).toBe("false");
    appState.tabs = [host, second];
    view.rerender(<PaneWorkspace {...props} tab={{ ...host }} />);
    expect(view.queryByTestId(`tmux-strip-${ssh.sessionId}`)).toBeNull();
    expect(view.getByTestId(`tmux-strip-${sibling.sessionId}`)).not.toBeNull();
    view.unmount();
    appState.tabs = [];
  });
});
