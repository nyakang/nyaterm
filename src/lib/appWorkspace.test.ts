import { describe, expect, it } from "vitest";
import type { SavedConnection, SessionInfo, SessionPane, UiConfig } from "@/types/global";
import {
  canCreateSessionFromPane,
  canMultiplexSshPane,
  canUseStartupCommandForPane,
  canUseFloatingPanel,
  clearUnavailableFloatingPanels,
  cloneDefaultActivityBarLayout,
  getHiddenActivityItemsForSide,
  getItemSide,
  getSideOpenPanels,
  getVisibleActivityIds,
  hasVisibleActivityBarItems,
  hideActivityBarItem,
  isActivityBarItemVisible,
  isActivityItemAvailable,
  mergeVisibleReorder,
  moveFloatingPanelSide,
  normalizePanelOpenMode,
  reduceFloatingPanelSelect,
  resetActivityBarLayout,
  showActivityBarItem,
  toggleActivityBarItemVisibility,
} from "./appWorkspace";

const basePane = {
  id: "pane-1",
  kind: "leaf",
  paneKind: "terminal",
  sessionId: "session-1",
  name: "Session",
  connecting: false,
} as const;

function pane(overrides: Partial<SessionPane>): SessionPane {
  return {
    ...basePane,
    type: "SSH",
    ...overrides,
  } as SessionPane;
}

describe("canCreateSessionFromPane", () => {
  it("allows temporary panes with matching protocols", () => {
    expect(
      canCreateSessionFromPane(
        pane({
          type: "SSH",
          temporaryConfig: {
            protocol: "ssh",
            runtime_mode: "standard",
            name: "root@example.com:22",
            host: "example.com",
            port: 22,
            username: "root",
            auth: { type: "password", password: "secret" },
            backspace_mode: "del",
            x11_forwarding: false,
            x11_display: "",
            proxy: null,
            proxy_jump: null,
            post_login: null,
          },
        }),
      ),
    ).toBe(true);

    expect(
      canCreateSessionFromPane(
        pane({
          type: "Telnet",
          temporaryConfig: {
            protocol: "telnet",
            name: "telnet://example.com:23",
            host: "example.com",
            port: 23,
          },
        }),
      ),
    ).toBe(true);

    expect(
      canCreateSessionFromPane(
        pane({
          type: "Serial",
          temporaryConfig: {
            protocol: "serial",
            name: "COM3 @ 115200",
            portName: "COM3",
            baudRate: 115200,
          },
        }),
      ),
    ).toBe(true);
  });

  it("rejects panes without a saved connection or matching temporary config", () => {
    expect(canCreateSessionFromPane(pane({ type: "SSH" }))).toBe(false);
    expect(
      canCreateSessionFromPane(
        pane({
          type: "SSH",
          temporaryConfig: {
            protocol: "telnet",
            name: "telnet://example.com:23",
            host: "example.com",
            port: 23,
          },
        }),
      ),
    ).toBe(false);
  });

  it("still allows local panes without connection metadata", () => {
    expect(canCreateSessionFromPane(pane({ type: "Local" }))).toBe(true);
  });
});

describe("SSH transport session action capabilities", () => {
  const standardSsh = {
    id: "ssh-standard",
    name: "Standard SSH",
    type: "ssh",
    ssh_transport: "ssh",
  } as SavedConnection;
  const mosh = {
    id: "ssh-mosh",
    name: "Mosh",
    type: "ssh",
    ssh_transport: "mosh",
  } as SavedConnection;
  const connections = [standardSsh, mosh];
  const runtimeSessions = (
    transport: "ssh" | "mosh",
  ): Map<string, SessionInfo> =>
    new Map([
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

  it("blocks russh-only and startup-command actions for saved Mosh panes", () => {
    const moshPane = pane({ type: "SSH", connectionId: mosh.id });

    expect(canCreateSessionFromPane(moshPane)).toBe(true);
    expect(canMultiplexSshPane(moshPane, runtimeSessions("mosh"))).toBe(false);
    expect(canUseStartupCommandForPane(moshPane, connections)).toBe(false);
  });

  it("keeps ordinary SSH and temporary SSH actions available", () => {
    const sshPane = pane({ type: "SSH", connectionId: standardSsh.id });
    const temporarySshPane = pane({
      type: "SSH",
      temporaryConfig: {
        protocol: "ssh",
        runtime_mode: "standard",
        name: "root@example.com:22",
        host: "example.com",
        port: 22,
        username: "root",
        auth: { type: "password", password: "secret" },
        backspace_mode: "del",
        x11_forwarding: false,
        x11_display: "",
        proxy: null,
        proxy_jump: null,
        post_login: null,
      },
    });

    expect(canMultiplexSshPane(sshPane, runtimeSessions("ssh"))).toBe(true);
    expect(canUseStartupCommandForPane(sshPane, connections)).toBe(true);
    expect(canMultiplexSshPane(temporarySshPane, runtimeSessions("ssh"))).toBe(true);
    expect(canUseStartupCommandForPane(temporarySshPane, connections)).toBe(true);
  });

  it("keeps multiplex capability tied to the running source session when saved transport changes", () => {
    const sourcePane = pane({ type: "SSH", connectionId: standardSsh.id });

    expect(
      canMultiplexSshPane(sourcePane, runtimeSessions("mosh")),
    ).toBe(false);
    expect(
      canMultiplexSshPane(sourcePane, runtimeSessions("ssh")),
    ).toBe(true);
  });
});

function uiConfig(overrides: Partial<UiConfig> = {}): UiConfig {
  return {
    activity_bar_layout: cloneDefaultActivityBarLayout(),
    panel_open_mode: "docked",
    active_left_panel: null,
    active_right_panel: null,
    left_open_panels: [],
    right_open_panels: [],
    show_notes_panel: true,
    show_remote_stats: true,
    show_gpu_monitor: true,
    show_ascend_npu_monitor: true,
    show_process_manager: true,
    show_docker_manager: true,
    ...overrides,
  } as UiConfig;
}

describe("activity bar visibility state", () => {
  it("does not keep a side visible when it only contains hidden items", () => {
    expect(
      hasVisibleActivityBarItems({
        items: [],
        bottomItems: [],
        hiddenItems: ["fileExplorer"],
      }),
    ).toBe(false);
    expect(hasVisibleActivityBarItems({ items: ["savedConnections"] })).toBe(true);
    expect(hasVisibleActivityBarItems({ items: [], bottomItems: ["settings"] })).toBe(true);
  });

  it("hides, shows, and toggles icons without changing feature availability", () => {
    const ui = uiConfig({
      show_gpu_monitor: true,
      activity_bar_layout: hideActivityBarItem(cloneDefaultActivityBarLayout(), "gpuMonitor"),
    });

    expect(isActivityItemAvailable("gpuMonitor", ui)).toBe(true);
    expect(isActivityBarItemVisible("gpuMonitor", ui)).toBe(false);
    expect(showActivityBarItem(ui.activity_bar_layout, "gpuMonitor").hidden_items).toEqual([]);
    expect(
      toggleActivityBarItemVisibility(ui.activity_bar_layout, "gpuMonitor").hidden_items,
    ).toEqual([]);
    expect(
      toggleActivityBarItemVisibility(cloneDefaultActivityBarLayout(), "gpuMonitor").hidden_items,
    ).toEqual(["gpuMonitor"]);
  });

  it("filters activity bar ids by hidden state while open-panel state can remain active", () => {
    const layout = hideActivityBarItem(cloneDefaultActivityBarLayout(), "aiAssistant");
    const ui = uiConfig({ activity_bar_layout: layout, active_right_panel: "aiAssistant" });

    expect(getVisibleActivityIds(["savedConnections", "aiAssistant"], ui)).toEqual([
      "savedConnections",
    ]);
    expect(getSideOpenPanels(ui, "right", false)).toEqual(["aiAssistant"]);
  });

  it("keeps hidden disabled feature state until the feature is available again", () => {
    const layout = hideActivityBarItem(cloneDefaultActivityBarLayout(), "gpuMonitor");
    const disabled = uiConfig({ activity_bar_layout: layout, show_gpu_monitor: false });
    const enabled = uiConfig({ activity_bar_layout: layout, show_gpu_monitor: true });

    expect(isActivityItemAvailable("gpuMonitor", disabled)).toBe(false);
    expect(isActivityBarItemVisible("gpuMonitor", disabled)).toBe(false);
    expect(isActivityBarItemVisible("gpuMonitor", enabled)).toBe(false);
    expect(enabled.activity_bar_layout.hidden_items).toEqual(["gpuMonitor"]);
  });

  it("merges visible reorders without moving or restoring hidden items", () => {
    const layout = {
      ...cloneDefaultActivityBarLayout(),
      left_top: ["A", "B", "C", "D"],
      hidden_items: ["B"],
    };
    const ui = uiConfig({ activity_bar_layout: layout });

    expect(mergeVisibleReorder(layout.left_top, ["A", "D", "C"], ui)).toEqual([
      "A",
      "B",
      "D",
      "C",
    ]);
  });

  it("resolves hidden items by their layout side", () => {
    const layout = {
      ...cloneDefaultActivityBarLayout(),
      left_bottom: [...cloneDefaultActivityBarLayout().left_bottom, "aiAssistant"],
      right_top: cloneDefaultActivityBarLayout().right_top.filter((id) => id !== "aiAssistant"),
      hidden_items: ["aiAssistant"],
    };
    const ui = uiConfig({ activity_bar_layout: layout });

    expect(getItemSide("aiAssistant", layout)).toBe("left");
    expect(getHiddenActivityItemsForSide(ui, "left")).toEqual(["aiAssistant"]);
    expect(getHiddenActivityItemsForSide(ui, "right")).toEqual([]);
  });

  it("resets layout order, hidden items, and label visibility", () => {
    const layout = resetActivityBarLayout();

    expect(layout.hidden_items).toEqual([]);
    expect(layout.show_labels).toBe(false);
    expect(layout.left_top).toEqual(["fileExplorer", "notes", "network", "securityAuth"]);
  });
});

describe("floating panel state", () => {
  it("normalizes unknown panel open modes to docked", () => {
    expect(normalizePanelOpenMode("floating")).toBe("floating");
    expect(normalizePanelOpenMode("docked")).toBe("docked");
    expect(normalizePanelOpenMode("sideways")).toBe("docked");
    expect(normalizePanelOpenMode(null)).toBe("docked");
    expect(normalizePanelOpenMode(undefined)).toBe("docked");
  });

  it("only treats real side panels as floating-capable", () => {
    expect(canUseFloatingPanel("recording")).toBe(true);
    expect(canUseFloatingPanel("aiAssistant")).toBe(true);
    expect(canUseFloatingPanel("settings")).toBe(false);
    expect(canUseFloatingPanel("lock")).toBe(false);
    expect(canUseFloatingPanel("quickCmdBar")).toBe(false);
    expect(canUseFloatingPanel("serialSend")).toBe(false);
  });

  it("toggles the same floating panel and replaces another panel on the same side", () => {
    const empty = { left: null, right: null };
    const opened = reduceFloatingPanelSelect(empty, "fileExplorer", "left");
    expect(opened).toEqual({ left: "fileExplorer", right: null });
    expect(reduceFloatingPanelSelect(opened, "notes", "left")).toEqual({
      left: "notes",
      right: null,
    });
    expect(reduceFloatingPanelSelect(opened, "fileExplorer", "left")).toEqual({
      left: null,
      right: null,
    });
  });

  it("keeps left and right floating panels independent", () => {
    const state = reduceFloatingPanelSelect(
      { left: "fileExplorer", right: null },
      "aiAssistant",
      "right",
    );

    expect(state).toEqual({ left: "fileExplorer", right: "aiAssistant" });
  });

  it("moves a floating panel to a new side and replaces the target side", () => {
    expect(
      moveFloatingPanelSide(
        { left: "fileExplorer", right: "aiAssistant" },
        "aiAssistant",
        "left",
      ),
    ).toEqual({ left: "aiAssistant", right: null });
  });

  it("closes floating panels that become unavailable without treating hidden items as unavailable", () => {
    const layout = hideActivityBarItem(cloneDefaultActivityBarLayout(), "aiAssistant");
    const ui = uiConfig({
      activity_bar_layout: layout,
      show_gpu_monitor: false,
    });

    expect(
      clearUnavailableFloatingPanels(
        { left: "gpuMonitor", right: "aiAssistant" },
        ui,
      ),
    ).toEqual({ left: null, right: "aiAssistant" });
  });
});
