import { describe, expect, it } from "vitest";
import type { RestorableTab } from "@/types/global";
import type { TemporaryLinkConfig } from "@/types/temporaryConnection";
import {
  collectSessionPanes,
  createFileDocumentPane,
  createSessionPane,
  createWorkspaceTab,
  findOpenFileDocument,
  getActiveSessionTabDisplayName,
  getReleasedSessionIds,
  getSessionRowDisplayName,
  getTabDisplayName,
  replaceSessionReferences,
  restoreTabFromPersistence,
  serializeTabsForPersistence,
  splitSessionPane,
  updateSessionPane,
} from "./workspaceTabs";

describe("workspaceTabs file documents", () => {
  const file = (path: string, sessionId = "session-ssh") =>
    createFileDocumentPane({
      sessionId,
      name: path.split("/").pop() || path,
      type: "SSH",
      connectionId: "ssh-1",
      backend: "remote",
      path,
      file: {
        content: `content:${path}`,
        size: 12,
        mtime: 42,
        contentHash: `hash:${path}`,
      },
    });

  it("finds an already open file by backend, session and exact path", () => {
    const existing = createWorkspaceTab(file("/srv/README"), 0);

    expect(
      findOpenFileDocument([existing], {
        backend: "remote",
        sessionId: "session-ssh",
        path: "/srv/README",
      }),
    ).toMatchObject({ tabId: existing.id, paneId: existing.root.id });
    expect(
      findOpenFileDocument([existing], {
        backend: "remote",
        sessionId: "session-ssh",
        path: "/srv/readme",
      }),
    ).toBeNull();
  });

  it("only releases a shared session after its final pane reference is removed", () => {
    const terminal = createWorkspaceTab(
      createSessionPane("Host", "SSH", "ssh-1", { sessionId: "session-ssh" }),
      0,
    );
    const document = createWorkspaceTab(file("/srv/notes.md"), 1);

    expect(getReleasedSessionIds([terminal, document], [document])).toEqual([]);
    expect(getReleasedSessionIds([terminal, document], [terminal])).toEqual([]);
    expect(getReleasedSessionIds([terminal, document], [])).toEqual([
      "session-ssh",
    ]);
  });

  it("moves file dependencies to the new id when a session reconnects", () => {
    const terminalPane = createSessionPane("Host", "SSH", "ssh-1", {
      sessionId: "session-old",
    });
    const filePane = file("/srv/notes.md", "session-old");
    const root = {
      id: "split-reconnect",
      kind: "split" as const,
      direction: "vertical" as const,
      ratio: 0.5,
      first: terminalPane,
      second: filePane,
    };

    const updated = replaceSessionReferences(
      root,
      "session-old",
      "session-new",
    );

    expect(collectSessionPanes(updated).map((pane) => pane.sessionId)).toEqual([
      "session-new",
      "session-new",
    ]);
  });

  it("does not persist file-only tabs and collapses file leaves out of split tabs", () => {
    const terminalPane = createSessionPane("Host", "SSH", "ssh-1", {
      id: "pane-terminal",
      sessionId: "session-ssh",
    });
    const filePane = file("/srv/notes.md");
    const mixed = createWorkspaceTab(terminalPane, 1);
    mixed.root = {
      id: "split-1",
      kind: "split",
      direction: "vertical",
      ratio: 0.5,
      first: terminalPane,
      second: filePane,
    };
    mixed.activePaneId = filePane.id;

    const serialized = serializeTabsForPersistence([
      createWorkspaceTab(file("/srv/only.md"), 0),
      mixed,
    ]);

    expect(serialized).toHaveLength(1);
    expect(serialized[0].root).toMatchObject({
      kind: "leaf",
      pane_kind: "terminal",
      title: "Host",
    });
    expect(serialized[0].active_pane_id).toBe("pane-terminal");
  });

  it("ignores legacy persisted file leaves instead of restoring them as terminals", () => {
    const restored = restoreTabFromPersistence(
      {
        title: "notes.md",
        session_type: "SSH",
        root: {
          kind: "leaf",
          pane_kind: "file",
          title: "notes.md",
          session_type: "SSH",
          connection_id: "ssh-1",
        },
      } as RestorableTab,
      0,
    );

    expect(restored).toBeNull();
  });
});

describe("workspaceTabs remote desktop persistence", () => {
  it("keeps legacy terminal tabs restorable without pane_kind", () => {
    const restored = restoreTabFromPersistence(
      {
        title: "Legacy SSH",
        session_type: "SSH",
        connection_id: "ssh-1",
      } as RestorableTab,
      0,
    );

    expect(restored).not.toBeNull();
    const leaf = restored?.root.kind === "leaf" ? restored.root : null;
    expect(leaf?.paneKind).toBe("terminal");
    expect(leaf?.type).toBe("SSH");
    expect(leaf?.connectionId).toBe("ssh-1");
  });

  it("restores legacy RDP tabs without pane_kind as graphical leaves", () => {
    const restored = restoreTabFromPersistence(
      {
        title: "Legacy Windows",
        session_type: "rdp",
        connection_id: "rdp-legacy",
      } as RestorableTab,
      0,
    );

    const leaf = restored?.root.kind === "leaf" ? restored.root : null;
    expect(leaf).toMatchObject({
      paneKind: "remote-desktop",
      type: "RDP",
      connectionId: "rdp-legacy",
      connecting: true,
    });
  });

  it("serializes and restores RDP panes as graphical leaves", () => {
    const pane = createSessionPane("Windows", "RDP", "rdp-1", {
      id: "pane-rdp",
      sessionId: "session-rdp",
    });
    const [serialized] = serializeTabsForPersistence([
      createWorkspaceTab(pane, 0),
    ]);

    expect(serialized.root?.kind).toBe("leaf");
    if (serialized.root?.kind === "leaf") {
      expect(serialized.root.pane_kind).toBe("remote-desktop");
      expect(serialized.root.session_type).toBe("RDP");
    }

    const restored = restoreTabFromPersistence(serialized, 0);
    const leaf = restored?.root.kind === "leaf" ? restored.root : null;
    if (leaf?.paneKind !== "remote-desktop") throw new Error("expected RDP leaf");
    expect(leaf?.type).toBe("RDP");
    expect(leaf?.display).toMatchObject({
      remoteWidth: 1920,
      remoteHeight: 1080,
      scaleMode: "fit",
    });
  });

  it("round-trips mixed terminal and RDP split trees", () => {
    const terminal = createSessionPane("Linux", "SSH", "ssh-1", {
      id: "pane-ssh",
      sessionId: "session-ssh",
    });
    const rdp = createSessionPane("Windows", "RDP", "rdp-1", {
      id: "pane-rdp",
      sessionId: "session-rdp",
    });
    const tab = createWorkspaceTab(terminal, 0);
    tab.root = splitSessionPane(tab.root, terminal.id, "vertical", rdp);
    tab.activePaneId = rdp.id;

    const [serialized] = serializeTabsForPersistence([tab]);
    expect(serialized.root).toMatchObject({
      kind: "split",
      direction: "vertical",
      first: {
        kind: "leaf",
        pane_kind: "terminal",
        session_type: "SSH",
      },
      second: {
        kind: "leaf",
        pane_kind: "remote-desktop",
        session_type: "RDP",
      },
    });

    const restored = restoreTabFromPersistence(serialized, 0);
    expect(restored?.activePaneId).toBe("pane-rdp");
    expect(restored && collectSessionPanes(restored.root)).toMatchObject([
      { id: "pane-ssh", paneKind: "terminal", type: "SSH" },
      { id: "pane-rdp", paneKind: "remote-desktop", type: "RDP" },
    ]);
  });

  it("drops invalid leaves while preserving valid siblings in mixed persisted trees", () => {
    const restored = restoreTabFromPersistence(
      {
        title: "Mixed",
        session_type: "RDP",
        active_pane_id: "invalid-rdp",
        root: {
          id: "split-mixed",
          kind: "split",
          direction: "horizontal",
          ratio: 0.5,
          first: {
            id: "invalid-rdp",
            kind: "leaf",
            pane_kind: "rdp",
            title: "Invalid RDP",
            session_type: "SSH",
          },
          second: {
            id: "valid-ssh",
            kind: "leaf",
            title: "Valid SSH",
            session_type: "SSH",
            connection_id: "ssh-1",
          },
        },
      } as RestorableTab,
      0,
    );

    expect(restored?.activePaneId).toBe("valid-ssh");
    expect(restored?.root).toMatchObject({
      id: "valid-ssh",
      kind: "leaf",
      paneKind: "terminal",
      type: "SSH",
      connectionId: "ssh-1",
    });
  });

  it("restores legacy persisted pane_kind rdp and writes the generalized kind on save", () => {
    const restored = restoreTabFromPersistence(
      {
        title: "Legacy Windows",
        session_type: "RDP",
        root: {
          kind: "leaf",
          pane_kind: "rdp",
          title: "Legacy Windows",
          session_type: "RDP",
          connection_id: "rdp-legacy",
        },
      } as RestorableTab,
      0,
    );

    const leaf = restored?.root.kind === "leaf" ? restored.root : null;
    expect(leaf?.paneKind).toBe("remote-desktop");
    const serialized = restored ? serializeTabsForPersistence([restored])[0] : null;
    expect(serialized?.root?.kind === "leaf" && serialized.root.pane_kind).toBe("remote-desktop");
  });

  it("round-trips VNC display metadata as a remote desktop leaf", () => {
    const pane = createSessionPane("VNC Desktop", "VNC", "vnc-1", {
      id: "pane-vnc",
      sessionId: "session-vnc",
      display: {
        remoteWidth: 1366,
        remoteHeight: 768,
        scaleMode: "stretch",
        viewOnly: true,
        clipboardEnabled: false,
      },
    });
    const [serialized] = serializeTabsForPersistence([createWorkspaceTab(pane, 0)]);

    expect(serialized.root).toMatchObject({
      kind: "leaf",
      pane_kind: "remote-desktop",
      session_type: "VNC",
      display: {
        remoteWidth: 1366,
        remoteHeight: 768,
        scaleMode: "stretch",
        viewOnly: true,
        clipboardEnabled: false,
      },
    });
    const restored = restoreTabFromPersistence(serialized, 0);
    expect(restored?.root).toMatchObject({
      paneKind: "remote-desktop",
      type: "VNC",
      display: {
        remoteWidth: 1366,
        remoteHeight: 768,
        scaleMode: "stretch",
        viewOnly: true,
        clipboardEnabled: false,
      },
    });
  });

  it("keeps paneKind consistent when pane type changes", () => {
    const terminal = createSessionPane("Terminal", "SSH", "ssh-1", { id: "pane-1" });
    const remote = updateSessionPane(terminal, terminal.id, { type: "VNC" });
    expect(remote).toMatchObject({ paneKind: "remote-desktop", type: "VNC" });

    const terminalAgain = updateSessionPane(remote, terminal.id, { type: "Local" });
    expect(terminalAgain).toMatchObject({ paneKind: "terminal", type: "Local" });
    expect(terminalAgain).not.toHaveProperty("display");
  });

  it("rejects mismatched RDP pane kind and terminal session type", () => {
    const restored = restoreTabFromPersistence(
      {
        title: "Invalid",
        session_type: "SSH",
        root: {
          kind: "leaf",
          pane_kind: "rdp",
          title: "Invalid",
          session_type: "SSH",
        },
      } as RestorableTab,
      0,
    );

    expect(restored).toBeNull();
  });
});

describe("workspaceTabs temporary session metadata", () => {
  it("keeps temporaryConfig on runtime panes without persisting it", () => {
    const temporaryConfig: TemporaryLinkConfig = {
      protocol: "telnet",
      name: "telnet://example.com:23",
      host: "example.com",
      port: 23,
    };
    const pane = createSessionPane("Temporary Telnet", "Telnet", undefined, {
      temporaryConfig,
    });

    expect(pane.temporaryConfig).toBe(temporaryConfig);

    const [serialized] = serializeTabsForPersistence([createWorkspaceTab(pane, 0)]);
    expect(JSON.stringify(serialized)).not.toContain("temporaryConfig");
    expect(serialized.root).toMatchObject({
      kind: "leaf",
      session_type: "Telnet",
    });
    expect(serialized.root?.kind === "leaf" && serialized.root.connection_id).toBeUndefined();
  });
});

describe("workspaceTabs dynamic titles", () => {
  const localTab = () =>
    createWorkspaceTab(createSessionPane("Local Terminal", "Local"), 0);

  it("prefers custom name over a resolved dynamic title", () => {
    const tab = { ...localTab(), customName: "pinned" };
    expect(getTabDisplayName(tab, "vim — /home/user")).toBe("pinned");
  });

  it("uses the active session's dynamic title through the shared resolver", () => {
    const tab = localTab();
    const activeSessionId =
      tab.root.kind === "leaf" ? tab.root.sessionId : null;
    expect(
      getActiveSessionTabDisplayName(tab, (sessionId) =>
        sessionId === activeSessionId ? "cargo build" : null,
      ),
    ).toBe("cargo build");
  });

  it("falls back to the static name when no resolved title exists", () => {
    expect(getTabDisplayName(localTab(), null)).toBe("Local Terminal");
    expect(getTabDisplayName(localTab(), "   ")).toBe("Local Terminal");
  });

  it("uses each enabled session's own title in session-identity rows", () => {
    const first = createSessionPane("First", "Local", undefined, {
      id: "pane-first",
      sessionId: "session-first",
    });
    const second = createSessionPane("Second", "Local", undefined, {
      id: "pane-second",
      sessionId: "session-second",
    });
    const tab = createWorkspaceTab(first, 0);
    tab.root = splitSessionPane(tab.root, first.id, "vertical", second);
    tab.activePaneId = first.id;

    expect(
      getSessionRowDisplayName(tab, "Second", {
        applicationTitle: null,
        cwd: null,
        effectiveTitle: "~/second-project",
        enabled: true,
      }),
    ).toBe("~/second-project");
    expect(
      getSessionRowDisplayName(tab, "Second", {
        applicationTitle: null,
        cwd: null,
        effectiveTitle: null,
        enabled: false,
      }),
    ).toBe("First");
  });

  it("does not persist dynamic title policy in workspace tabs", () => {
    const [serialized] = serializeTabsForPersistence([localTab()]);
    expect(serialized).not.toHaveProperty("dynamic_title");
  });

  it("ignores a legacy per-tab dynamic title field when restoring", () => {
    const restored = restoreTabFromPersistence(
      {
        title: "Local Terminal",
        session_type: "Local",
        dynamic_title: true,
        root: {
          kind: "leaf",
          title: "Local Terminal",
          session_type: "Local",
        },
      } as RestorableTab & { dynamic_title?: boolean },
      0,
    );
    expect(restored).not.toHaveProperty("dynamicTitle");
  });
});
