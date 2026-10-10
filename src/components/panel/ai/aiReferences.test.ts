import { describe, expect, it } from "vitest";
import { createFileDocumentPane, createSessionPane } from "@/lib/workspaceTabs";
import type { SavedConnection, SessionInfo } from "@/types/global";
import { buildAIReferenceGroups } from "./aiReferences";

function session(id: string, connectionId: string, startedAt: string): SessionInfo {
  return {
    id,
    name: id,
    session_type: "SSH",
    started_at: startedAt,
    connection_id: connectionId,
    connected: true,
  } as SessionInfo;
}

const sshConnection = {
  id: "connection-a",
  name: "Production",
  type: "ssh",
  host: "prod.example.test",
  port: 22,
  username: "root",
} as SavedConnection;

describe("AI reference tree", () => {
  it("groups live sessions and open files as peers and sorts by start/open time", () => {
    const oldSession = createSessionPane("older window", "SSH", sshConnection.id, {
      sessionId: "session-old",
    });
    const newSession = createSessionPane("newer window", "SSH", sshConnection.id, {
      sessionId: "session-new",
    });
    const olderFile = {
      ...createFileDocumentPane({
        sessionId: "session-old",
        name: "older.txt",
        type: "SSH",
        connectionId: sshConnection.id,
        backend: "remote",
        path: "/tmp/older.txt",
        file: { content: "old", size: 3, mtime: 999, contentHash: "old" },
      }),
      openedAt: "2026-01-01T00:00:00.000Z",
    };
    const newerFile = {
      ...createFileDocumentPane({
        sessionId: "session-new",
        name: "newer.txt",
        type: "SSH",
        connectionId: sshConnection.id,
        backend: "remote",
        path: "/tmp/newer.txt",
        file: { content: "new", size: 3, mtime: 1, contentHash: "new" },
      }),
      openedAt: "2026-02-01T00:00:00.000Z",
    };

    const [group] = buildAIReferenceGroups(
      [oldSession, olderFile, newSession, newerFile],
      [
        session("session-old", sshConnection.id, "2026-01-02T00:00:00.000Z"),
        session("session-new", sshConnection.id, "2026-02-02T00:00:00.000Z"),
      ],
      [sshConnection],
    );

    expect(group.sessions).toEqual(["session-new", "session-old"]);
    expect(group.children.map((option) => [option.kind, option.label])).toEqual([
      ["session", "newer window"],
      ["file", "newer.txt"],
      ["session", "older window"],
      ["file", "older.txt"],
    ]);
    expect(group.children[1]?.title).toBe("root@prod.example.test:22:/tmp/newer.txt");
  });

  it("keeps different SSH users on the same endpoint in separate groups", () => {
    const deployConnection = {
      ...sshConnection,
      id: "connection-deploy",
      name: "Deploy",
      username: "deploy",
    } as SavedConnection;
    const rootPane = createSessionPane("root", "SSH", sshConnection.id, {
      sessionId: "root-session",
    });
    const deployPane = createSessionPane("deploy", "SSH", deployConnection.id, {
      sessionId: "deploy-session",
    });
    const groups = buildAIReferenceGroups(
      [rootPane, deployPane],
      [
        session("root-session", sshConnection.id, "2026-01-01T00:00:00.000Z"),
        session("deploy-session", deployConnection.id, "2026-01-02T00:00:00.000Z"),
      ],
      [sshConnection, deployConnection],
    );
    expect(groups.map((group) => group.id)).toEqual(
      expect.arrayContaining(["ssh:root@prod.example.test:22", "ssh:deploy@prod.example.test:22"]),
    );
    expect(groups).toHaveLength(2);
  });

  it("does not include files in the host's session selection", () => {
    const pane = createSessionPane("window", "SSH", sshConnection.id, {
      sessionId: "session-only",
    });
    const file = {
      ...createFileDocumentPane({
        sessionId: "session-only",
        name: "secret.txt",
        type: "SSH",
        connectionId: sshConnection.id,
        backend: "remote",
        path: "/etc/secret.txt",
        file: { content: "secret", size: 6, mtime: 1, contentHash: "hash" },
      }),
      openedAt: "2026-01-01T00:00:00.000Z",
    };
    const [group] = buildAIReferenceGroups(
      [pane, file],
      [session("session-only", sshConnection.id, "2026-02-02T00:00:00.000Z")],
      [sshConnection],
    );

    expect(group.sessions).toEqual(["session-only"]);
    expect(group.children.map((option) => option.kind)).toEqual(["session", "file"]);
  });
});
