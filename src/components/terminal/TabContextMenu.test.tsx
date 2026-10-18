import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SavedConnection, SessionInfo, SessionPane, Tab } from "@/types/global";
import TabContextMenu from "./TabContextMenu";

let savedConnections: SavedConnection[] = [];

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

vi.mock("@/context/AppContext", () => ({
  useApp: () => ({
    savedConnections,
    updateTab: vi.fn(),
  }),
}));

vi.mock("@/lib/aiEvents", () => ({ openAIAssistant: vi.fn() }));

describe("TabContextMenu SSH transport action gating", () => {
  beforeEach(() => {
    savedConnections = [];
  });

  it("keeps duplicate/reconnect but hides startup-command and multiplex actions for Mosh", async () => {
    savedConnections = [connection("mosh")];
    renderMenu(tab("ssh-1"), runtimeSessions("mosh"));

    fireEvent.contextMenu(screen.getByTestId("tab-target"));

    await waitFor(() => expect(screen.getByText("tabCtx.duplicate")).not.toBeNull());
    expect(screen.getByText("tabCtx.reconnect")).not.toBeNull();
    expect(screen.queryByText("tabCtx.duplicateWithCommand")).toBeNull();
    expect(screen.queryByText("tabCtx.sshAdvanced")).toBeNull();
  });

  it("keeps startup-command and multiplex actions for ordinary SSH", async () => {
    savedConnections = [connection("ssh")];
    renderMenu(tab("ssh-1"), runtimeSessions("ssh"));

    fireEvent.contextMenu(screen.getByTestId("tab-target"));

    await waitFor(() => expect(screen.getByText("tabCtx.duplicateWithCommand")).not.toBeNull());
    expect(screen.getByText("tabCtx.sshAdvanced")).not.toBeNull();
  });

  it("keeps multiplex hidden for a running Mosh source after saved config changes to SSH", async () => {
    savedConnections = [connection("ssh")];
    renderMenu(tab("ssh-1"), runtimeSessions("mosh"));

    fireEvent.contextMenu(screen.getByTestId("tab-target"));

    await waitFor(() => expect(screen.getByText("tabCtx.duplicate")).not.toBeNull());
    expect(screen.queryByText("tabCtx.sshAdvanced")).toBeNull();
  });

  it("keeps multiplex available for a running SSH source after saved config changes to Mosh", async () => {
    savedConnections = [connection("mosh")];
    renderMenu(tab("ssh-1"), runtimeSessions("ssh"));

    fireEvent.contextMenu(screen.getByTestId("tab-target"));

    await waitFor(() => expect(screen.getByText("tabCtx.sshAdvanced")).not.toBeNull());
    expect(screen.queryByText("tabCtx.duplicateWithCommand")).toBeNull();
  });
});

function connection(transport: "ssh" | "mosh"): SavedConnection {
  return {
    id: "ssh-1",
    name: transport,
    type: "ssh",
    ssh_transport: transport,
  } as SavedConnection;
}

function tab(connectionId: string): Tab {
  const root = {
    id: "pane-1",
    kind: "leaf",
    paneKind: "terminal",
    sessionId: "session-1",
    name: "SSH",
    type: "SSH",
    connectionId,
    connecting: false,
  } as SessionPane;
  return {
    id: "tab-1",
    persistOrder: 0,
    activePaneId: root.id,
    root,
  };
}

function runtimeSessions(transport: "ssh" | "mosh"): Map<string, SessionInfo> {
  return new Map([
    [
      "session-1",
      {
        id: "session-1",
        name: transport,
        session_type: "SSH",
        started_at: "",
        connected: true,
        ai_execution_profile: "auto",
        injection_active: false,
        dynamic_title_enabled: false,
        dynamic_title_integration_active: false,
        remote_file_browser_enabled: transport === "ssh",
        remote_stats_enabled: transport === "ssh",
        ssh_runtime_mode: transport === "ssh" ? "standard" : null,
      } as SessionInfo,
    ],
  ]);
}

function renderMenu(targetTab: Tab, sessionInfoById: Map<string, SessionInfo>) {
  const noop = vi.fn();
  return render(
    <TabContextMenu
      tab={targetTab}
      sessionInfoById={sessionInfoById}
      tabs={[targetTab]}
      onDuplicateSession={noop}
      onMultiplexSshSession={noop}
      onDuplicateSessionWithCommand={noop}
      onMultiplexSshSessionWithCommand={noop}
      onReconnectSession={noop}
      onDisconnectSession={noop}
      onSplitSession={noop}
      onCloseSession={noop}
      onCloseAll={noop}
      onCloseInactive={noop}
      onCloseRight={noop}
      onSessionInfo={noop}
      onActivateTab={noop}
      canCopyIp={false}
      onRenameTab={noop}
      onCopyTabName={noop}
      onCopyServerIp={noop}
    >
      <button type="button" data-testid="tab-target">
        tab
      </button>
    </TabContextMenu>,
  );
}
