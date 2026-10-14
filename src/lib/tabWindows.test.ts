import { describe, expect, it } from "vitest";
import {
  createTerminalWindowLeaf,
  insertTabAfterInLeaf,
  moveTabBetweenLeaves,
  restoreTerminalWindowLayout,
  serializeTerminalWindowLayout,
  splitTerminalWindowForTab,
} from "./tabWindows";
import {
  createFileDocumentPane,
  createSessionPane,
  createWorkspaceTab,
  insertTabAfter,
  restoreTabFromPersistence,
  serializeTabsForPersistence,
} from "./workspaceTabs";

describe("terminal window persistence", () => {
  it("moves a single tab across split leaves without losing the target pane", () => {
    const layout = {
      kind: "split" as const,
      direction: "vertical" as const,
      ratio: 0.5,
      id: "split-1",
      first: createTerminalWindowLeaf(["tab-a"], "tab-a"),
      second: {
        ...createTerminalWindowLeaf(["tab-b"], "tab-b"),
        id: "leaf-b",
      },
    };

    const moved = moveTabBetweenLeaves(layout, "tab-a", "leaf-b", 1);

    expect(moved).toEqual({
      ...layout,
      first: {
        ...layout.first,
        tabIds: [],
        activeTabId: null,
      },
      second: {
        ...layout.second,
        tabIds: ["tab-b", "tab-a"],
        activeTabId: "tab-a",
      },
    });
  });

  it("indexes window tabs against the restorable tab list", () => {
    const fileTab = createWorkspaceTab(
      createFileDocumentPane({
        sessionId: "session-ssh",
        name: "notes.md",
        type: "SSH",
        connectionId: "ssh-1",
        backend: "remote",
        path: "/srv/notes.md",
        file: { content: "notes", size: 5, mtime: 42, contentHash: "hash-notes" },
      }),
      0,
    );
    const terminalTab = createWorkspaceTab(
      createSessionPane("Host", "SSH", "ssh-1", {
        sessionId: "session-ssh",
      }),
      1,
    );
    const layout = createTerminalWindowLeaf([fileTab.id, terminalTab.id], terminalTab.id);

    expect(serializeTerminalWindowLayout(layout, [fileTab, terminalTab])).toEqual({
      kind: "leaf",
      tab_indexes: [0],
      active_tab_index: 0,
    });
  });

  it("preserves a horizontal split after inserting a tab in the middle and restoring", () => {
    const [a, b, c, d] = ["A", "B", "C", "D"].map((name, index) =>
      createWorkspaceTab(
        createSessionPane(name, "SSH", `ssh-${name}`, { sessionId: `session-${name}` }),
        index,
      ),
    );
    const tabs = insertTabAfter([a, b, c], a.id, d);
    const split = splitTerminalWindowForTab(
      createTerminalWindowLeaf([a.id, b.id, c.id], c.id),
      b.id,
      "horizontal",
    );
    const layout = insertTabAfterInLeaf(split, a.id, d.id, d.id);

    const openTabs = serializeTabsForPersistence(tabs);
    const savedLayout = serializeTerminalWindowLayout(layout, tabs);

    expect(openTabs.map((tab) => tab.connection_id)).toEqual(["ssh-A", "ssh-B", "ssh-C", "ssh-D"]);
    expect(savedLayout).toEqual({
      kind: "split",
      direction: "horizontal",
      ratio: 0.5,
      first: { kind: "leaf", tab_indexes: [0, 3, 2], active_tab_index: 3 },
      second: { kind: "leaf", tab_indexes: [1], active_tab_index: 1 },
    });
    expect(tabs.map((tab) => tab.id)).toEqual([a.id, d.id, b.id, c.id]);

    const restoredTabs = openTabs.flatMap((tab, index) => {
      const restored = restoreTabFromPersistence(tab, index);
      return restored ? [restored] : [];
    });
    expect(restoredTabs).toHaveLength(4);
    const [restoredA, restoredB, restoredC, restoredD] = restoredTabs;

    expect(restoreTerminalWindowLayout(savedLayout, restoredTabs)).toMatchObject({
      kind: "split",
      direction: "horizontal",
      ratio: 0.5,
      first: {
        kind: "leaf",
        tabIds: [restoredA.id, restoredD.id, restoredC.id],
        activeTabId: restoredD.id,
      },
      second: {
        kind: "leaf",
        tabIds: [restoredB.id],
        activeTabId: restoredB.id,
      },
    });
  });
});
