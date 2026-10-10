import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_AI_SETTINGS } from "@/lib/aiSettings";
import { createFileDocumentPane, createWorkspaceTab } from "@/lib/workspaceTabs";
import type {
  AIFileReference,
  AIMessage,
  AISession,
  AISessionScope,
  Group,
  SavedConnection,
  SessionInfo,
  Tab,
  TerminalSessionPane,
} from "@/types/global";
import AIAssistantPanel from "./AIAssistantPanel";

const { invokeMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
}));

let appState: {
  appSettings: {
    ai: typeof DEFAULT_AI_SETTINGS;
    ui: { language: string };
  };
  updateAppSettings: ReturnType<typeof vi.fn>;
  tabs: Tab[];
  savedConnections: SavedConnection[];
  savedGroups: Group[];
};

vi.mock("@/context/AppContext", () => ({
  useApp: () => appState,
}));

vi.mock("@/context/ThemeContext", () => ({
  useTheme: () => ({ theme: { colors: {} } }),
}));

vi.mock("@/lib/invoke", () => ({ invoke: invokeMock }));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(),
  listen: vi.fn(async () => vi.fn()),
}));

vi.mock("react-i18next", async (importOriginal) => {
  const actual = await importOriginal<typeof import("react-i18next")>();
  return {
    ...actual,
    useTranslation: () => ({ t: (key: string) => key }),
  };
});

vi.mock("@/components/dialog/ai/AIAssistantDialogs", () => ({
  AIAssistantDialogs: () => null,
}));

vi.mock("./ModelCombobox", () => ({
  ModelCombobox: () => null,
}));

vi.mock("./utils", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./utils")>();
  return { ...actual, buildPrismThemeFromColors: () => ({}) };
});

describe("AIAssistantPanel history scope ownership", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    appState = {
      appSettings: {
        ai: { ...DEFAULT_AI_SETTINGS },
        ui: { language: "zh-CN" },
      },
      updateAppSettings: vi.fn(),
      tabs: [],
      savedConnections: [],
      savedGroups: [],
    };
  });

  it("passes only the selected connection metadata and its effective runtime profile to chat", async () => {
    const pane = terminalPane("selected-session");
    appState.tabs = [tabWithPane(pane)];
    appState.savedGroups = [{ id: "prod", name: "Production", sort_order: 0 }];
    appState.savedConnections = [
      {
        id: "connection-1",
        type: "ssh",
        name: "API",
        host: "api.example",
        port: 2222,
        username: "ops",
        description: "Selected API server",
        tags: ["prod"],
        group_id: "prod",
        auth: { mode: "password", password: "do-not-send" },
      },
      {
        id: "other",
        type: "ssh",
        name: "Other",
        description: "unselected connection",
      },
    ];
    appState.appSettings.ai = {
      ...DEFAULT_AI_SETTINGS,
      enabled: true,
      default_model_id: "test-model",
      models: [
        {
          id: "test-model",
          name: "Test model",
          provider_kind: "openai",
          enabled: true,
          source: "manual",
        },
      ],
    };
    invokeMock.mockImplementation((command: string) => {
      switch (command) {
        case "get_ai_sessions":
          return Promise.resolve([]);
        case "list_sessions":
          return Promise.resolve([
            {
              id: pane.sessionId,
              session_type: "SSH",
              ai_execution_profile: "posix",
            },
          ]);
        case "get_terminal_cwd":
          return Promise.resolve("/srv/api");
        case "start_ai_chat_stream":
          return Promise.resolve({ sessionId: "new-chat" });
        case "append_ai_audit":
          return Promise.resolve(null);
        default:
          return Promise.reject(new Error(`Unexpected command: ${command}`));
      }
    });
    render(
      <AIAssistantPanel
        activePane={pane}
        intent={{
          id: "metadata-intent",
          action: "generate_command",
          userInput: "inspect server",
        }}
      />,
    );
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("start_ai_chat_stream", expect.anything()),
    );
    const request = invokeMock.mock.calls.find(
      ([command]) => command === "start_ai_chat_stream",
    )?.[1].request;
    expect(request.context).toMatchObject({
      connectionName: "API",
      host: "api.example",
      port: 2222,
      description: "Selected API server",
      tags: ["prod"],
      groupPath: ["Production"],
      executionProfile: "posix",
      cwd: "/srv/api",
    });
    expect(request.targetContexts).toHaveLength(1);
    expect(request.targetContexts[0].context).toEqual(request.context);
    expect(JSON.stringify(request)).not.toContain("do-not-send");
    expect(JSON.stringify(request)).not.toContain("unselected connection");
    expect(
      invokeMock.mock.calls.filter(([command]) => command === "list_sessions").length,
    ).toBeGreaterThanOrEqual(1);
  });

  it("allows a history session to move after its terminal reconnects with a new session id", async () => {
    appState.appSettings.ai.default_mode = "agent";
    appState.appSettings.ai.default_agent_kind = "nyaterm";
    const oldPane = terminalPane("old-session");
    const newPane = terminalPane("new-session");
    let historySession = aiSession("ai-session", oldPane.sessionId);
    let messageLoadCount = 0;

    invokeMock.mockImplementation((command: string, args?: Record<string, unknown>) => {
      switch (command) {
        case "get_ai_sessions":
          return Promise.resolve([historySession]);
        case "get_ai_messages": {
          messageLoadCount += 1;
          return Promise.resolve([
            aiMessage(
              historySession.id,
              messageLoadCount === 1 ? "before reconnect" : "after reconnect",
            ),
          ]);
        }
        case "rebind_ai_session":
          historySession = {
            ...historySession,
            scope: args?.ownerScope as AISessionScope,
          };
          return Promise.resolve(historySession);
        default:
          return Promise.reject(new Error(`Unexpected command: ${command}`));
      }
    });

    appState.tabs = [tabWithPane(oldPane)];
    const view = render(<AIAssistantPanel activePane={oldPane} intent={null} />);

    openHistory(view.container);
    fireEvent.click(await historySessionButton(historySession.title));
    await screen.findByText("before reconnect");
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("rebind_ai_session", {
        sessionId: historySession.id,
        ownerScope: {
          type: "workspace",
          targetId: "main",
          connectionIds: [],
          label: "AI Assistant",
        },
      });
    });

    appState.tabs = [tabWithPane(newPane)];
    view.rerender(<AIAssistantPanel activePane={newPane} intent={null} />);

    openHistory(view.container);
    const continuedSessionButton = await historySessionButton(historySession.title);
    expect(continuedSessionButton.disabled).toBe(false);
    expect(screen.queryByText("ai.historyInUse")).toBeNull();
    expect(screen.queryByText("ai.historyMoveToCurrent")).toBeNull();
    fireEvent.click(continuedSessionButton);
    await screen.findByText("after reconnect");
    expect(messageLoadCount).toBe(2);
    expect(
      invokeMock.mock.calls.filter(([command]) => command === "rebind_ai_session"),
    ).toHaveLength(1);
  });

  it("allows a workspace AI history session after its original terminal is gone", async () => {
    appState.appSettings.ai.default_mode = "agent";
    appState.appSettings.ai.default_agent_kind = "nyaterm";
    const owningPane = terminalPane("owning-session", {
      connectError: "connection lost",
    });
    const currentPane = terminalPane("current-session", {
      id: "current-pane",
      connectionId: "current-connection",
      name: "Current terminal",
    });
    const historySession = aiSession("shared-ai-session", owningPane.sessionId);

    invokeMock.mockImplementation((command: string) => {
      switch (command) {
        case "get_ai_sessions":
          return Promise.resolve([historySession]);
        case "get_ai_messages":
          return Promise.resolve([]);
        case "rebind_ai_session":
          historySession.scope = {
            type: "workspace",
            targetId: "main",
            connectionIds: [],
            label: "AI Assistant",
          };
          return Promise.resolve(historySession);
        default:
          return Promise.reject(new Error(`Unexpected command: ${command}`));
      }
    });

    appState.tabs = [tabWithPane(owningPane, 0), tabWithPane(currentPane, 1)];
    const view = render(<AIAssistantPanel activePane={owningPane} intent={null} />);

    openHistory(view.container);
    fireEvent.click(await historySessionButton(historySession.title));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("get_ai_messages", {
        sessionId: historySession.id,
      });
    });

    view.rerender(<AIAssistantPanel activePane={currentPane} intent={null} />);
    openHistory(view.container);

    const continuedSessionButton = await historySessionButton(historySession.title);
    expect(continuedSessionButton.disabled).toBe(false);
    expect(screen.queryByText("ai.historyInUse")).toBeNull();
    expect(screen.queryByText("ai.historyMoveToCurrent")).toBeNull();
  });

  it("keeps history locked while its AI stream is still running after the terminal closes", async () => {
    const owningPane = terminalPane("stream-owner");
    const currentPane = terminalPane("current-session", {
      id: "current-pane",
      connectionId: "current-connection",
      name: "Current terminal",
    });
    const historySession = aiSession("stream-ai-session", owningPane.sessionId);

    appState.appSettings.ai = {
      ...DEFAULT_AI_SETTINGS,
      enabled: true,
      default_model_id: "test-model",
      models: [
        {
          id: "test-model",
          name: "Test model",
          provider_kind: "openai",
          enabled: true,
          source: "manual",
        },
      ],
    };

    invokeMock.mockImplementation((command: string) => {
      switch (command) {
        case "get_ai_sessions":
          return Promise.resolve([historySession]);
        case "start_ai_chat_stream":
          return Promise.resolve({ sessionId: historySession.id });
        case "append_ai_audit":
          return Promise.resolve(null);
        default:
          return Promise.reject(new Error(`Unexpected command: ${command}`));
      }
    });

    appState.tabs = [tabWithPane(owningPane)];
    const intent = {
      id: "stream-intent",
      action: "generate_command" as const,
      userInput: "keep streaming",
    };
    const view = render(<AIAssistantPanel activePane={owningPane} intent={intent} />);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "start_ai_chat_stream",
        expect.objectContaining({
          request: expect.objectContaining({ sessionId: null }),
        }),
      );
    });

    appState.tabs = [tabWithPane(currentPane)];
    view.rerender(<AIAssistantPanel activePane={currentPane} intent={intent} />);
    openHistory(view.container);

    const lockedSessionButton = await historySessionButton(historySession.title);
    expect(lockedSessionButton.disabled).toBe(true);
    expect(screen.getByText("ai.historyInUse")).not.toBeNull();
    expect(invokeMock).not.toHaveBeenCalledWith("rebind_ai_session", expect.anything());
  });

  it("keeps one AI conversation when the active terminal session changes", async () => {
    appState.appSettings.ai = {
      ...DEFAULT_AI_SETTINGS,
      enabled: true,
      default_model_id: "test-model",
      default_mode: "agent",
      default_agent_kind: "nyaterm",
      models: [
        {
          id: "test-model",
          name: "Test model",
          provider_kind: "openai",
          enabled: true,
          source: "manual",
        },
      ],
    };
    const firstPane = terminalPane("session-a", { id: "pane-a", name: "Terminal A" });
    const secondPane = terminalPane("session-b", { id: "pane-b", name: "Terminal B" });
    invokeMock.mockImplementation((command: string) => {
      switch (command) {
        case "get_ai_sessions":
        case "list_sessions":
          return Promise.resolve([]);
        case "get_terminal_cwd":
          return Promise.resolve("/");
        case "start_ai_chat_stream":
          return Promise.resolve({ sessionId: "stable-ai-session" });
        case "rebind_ai_session":
          return Promise.resolve({ id: "stable-ai-session" });
        case "append_ai_audit":
          return Promise.resolve(null);
        default:
          return Promise.reject(new Error(`Unexpected command: ${command}`));
      }
    });
    appState.tabs = [tabWithPane(firstPane)];
    const view = render(
      <AIAssistantPanel
        activePane={firstPane}
        intent={{ id: "first-turn", action: "generate_command", userInput: "first turn" }}
      />,
    );
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "start_ai_chat_stream",
        expect.objectContaining({
          request: expect.objectContaining({ sessionId: null }),
        }),
      );
    });

    appState.tabs = [tabWithPane(secondPane)];
    view.rerender(
      <AIAssistantPanel
        activePane={secondPane}
        intent={{ id: "second-turn", action: "generate_command", userInput: "second turn" }}
      />,
    );
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "rebind_ai_session",
        expect.objectContaining({
          sessionId: "stable-ai-session",
          ownerScope: {
            type: "workspace",
            targetId: "main",
            connectionIds: [],
            label: "AI Assistant",
          },
        }),
      );
      expect(invokeMock).toHaveBeenCalledWith(
        "start_ai_chat_stream",
        expect.objectContaining({
          request: expect.objectContaining({ sessionId: "stable-ai-session" }),
        }),
      );
    });
    view.unmount();
  });
});

describe("AIAssistantPanel file references", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    appState = {
      appSettings: {
        ai: {
          ...DEFAULT_AI_SETTINGS,
          enabled: true,
          default_model_id: "test-model",
          models: [
            {
              id: "test-model",
              name: "Test model",
              provider_kind: "openai",
              enabled: true,
              source: "manual",
            },
          ],
        },
        ui: { language: "zh-CN" },
      },
      updateAppSettings: vi.fn(),
      tabs: [],
      savedConnections: [],
      savedGroups: [],
    };
  });

  it("keeps external plain-text @ tokens as ordinary text", async () => {
    const pane = terminalPane("plain-paste-session");
    appState.tabs = [tabWithPane(pane)];
    invokeMock.mockImplementation((command: string) => {
      if (command === "get_ai_sessions") return Promise.resolve([]);
      if (command === "list_sessions")
        return Promise.resolve([
          {
            id: pane.sessionId,
            name: pane.name,
            session_type: pane.type,
            started_at: "2026-01-01T00:00:00Z",
            connection_id: pane.connectionId,
            connected: true,
          } as SessionInfo,
        ]);
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });

    const view = render(<AIAssistantPanel activePane={pane} intent={null} />);
    const editor = screen.getByRole("textbox");
    editor.focus();
    const range = document.createRange();
    range.selectNodeContents(editor);
    range.collapse(false);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
    const pastedText = "admin@prod @my config.yaml";
    fireEvent.paste(editor, {
      clipboardData: {
        getData: (type: string) => (type === "text/plain" ? pastedText : ""),
      },
    });

    await waitFor(() => expect(editor.textContent).toContain(pastedText));
    expect(editor.querySelector("[data-ai-reference-id]")).toBeNull();
    view.unmount();
  });

  it("stages a file AI action and sends labeled content plus source metadata", async () => {
    const pane = terminalPane("file-session");
    appState.tabs = [tabWithPane(pane)];
    const fileReference: AIFileReference = {
      id: "file:remote:file-session:/etc/nginx.conf",
      name: "nginx.conf",
      path: "/etc/nginx.conf",
      backend: "remote",
      terminalSessionId: pane.sessionId,
      connectionId: pane.connectionId ?? null,
      host: "root@prod.example.test:22",
      sizeBytes: 11,
      mimeType: "text/plain",
      content: "worker_processes auto;",
    };

    invokeMock.mockImplementation((command: string) => {
      switch (command) {
        case "get_ai_sessions":
          return Promise.resolve([]);
        case "list_sessions":
          return Promise.resolve([
            {
              id: pane.sessionId,
              name: pane.name,
              session_type: "SSH",
              started_at: "2026-01-01T00:00:00Z",
              connection_id: pane.connectionId,
              connected: true,
            } as SessionInfo,
          ]);
        case "start_ai_chat_stream":
          return Promise.resolve({ sessionId: "file-ai-session" });
        case "append_ai_audit":
          return Promise.resolve(null);
        default:
          return Promise.reject(new Error(`Unexpected command: ${command}`));
      }
    });

    const view = render(
      <AIAssistantPanel
        activePane={pane}
        intent={{
          id: "file-action-intent",
          action: "custom_file_action",
          userInput: "Review this file",
          fileReference,
        }}
      />,
    );

    const editor = await screen.findByRole("textbox");
    await waitFor(() => expect(editor.textContent).toContain("@nginx.conf"));
    const token = editor.querySelector(
      '[data-ai-reference-id="file:remote:file-session:/etc/nginx.conf"]',
    );
    expect(token?.getAttribute("aria-label")).toBe("root@prod.example.test:22:/etc/nginx.conf");

    fireEvent.keyDown(editor, { key: "Enter", shiftKey: false });
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith(
        "start_ai_chat_stream",
        expect.objectContaining({
          request: expect.objectContaining({
            action: "custom_file_action",
            userInput: "Review this file @nginx.conf",
            attachments: [
              expect.objectContaining({
                name: "nginx.conf",
                path: "/etc/nginx.conf",
                host: "root@prod.example.test:22",
              }),
            ],
            context: expect.objectContaining({
              selectedText: expect.stringContaining(
                "ai.referencedFile: root@prod.example.test:22:/etc/nginx.conf",
              ),
            }),
          }),
        }),
      ),
    );
    const call = invokeMock.mock.calls.find(([command]) => command === "start_ai_chat_stream");
    expect(call?.[1]?.request.context).not.toHaveProperty("ayaContext");
    view.unmount();
  });

  it("disables a disconnected source host and its open file references", async () => {
    appState.appSettings.ai.default_mode = "agent";
    appState.appSettings.ai.default_agent_kind = "nyaterm";
    const source = terminalPane("disconnected-session", {
      id: "disconnected-pane",
      connectError: "connection lost",
    });
    const file = createFileDocumentPane({
      sessionId: source.sessionId,
      connectionId: source.connectionId,
      type: "SSH",
      name: "offline.py",
      backend: "remote",
      path: "/home/demo/offline.py",
      file: { content: "print('offline')", size: 16, mtime: 1, contentHash: "offline" },
    });
    appState.tabs = [tabWithPane(source), createWorkspaceTab(file, 1)];
    appState.savedConnections = [
      {
        id: "connection-1",
        name: "Offline host",
        type: "ssh",
        host: "offline.example.test",
        port: 22,
        username: "root",
      },
    ];
    invokeMock.mockImplementation((command: string) => {
      if (command === "list_sessions")
        return Promise.resolve([
          {
            id: source.sessionId,
            name: source.name,
            session_type: source.type,
            connected: false,
            started_at: "2026-01-01T00:00:00Z",
          },
        ]);
      if (command === "get_ai_sessions") return Promise.resolve([]);
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });

    const view = render(<AIAssistantPanel activePane={file} intent={null} />);
    const editor = screen.getByRole("textbox");
    editor.append(document.createTextNode("@offline.py"));
    const range = document.createRange();
    range.selectNodeContents(editor);
    range.collapse(false);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
    fireEvent.input(editor);

    const fileOption = await waitFor(() => {
      const button = view.container.querySelector<HTMLButtonElement>(
        '[data-reference-option^="file:"]',
      );
      if (!button) throw new Error("Missing disconnected file option");
      return button;
    });
    expect(fileOption.disabled).toBe(true);
    const hostOption = view.container.querySelector<HTMLButtonElement>(
      '[data-reference-host-group="ssh:root@offline.example.test:22"]',
    );
    expect(hostOption?.disabled).toBe(true);
  });

  it("keeps a referenced file on its own source host and persists an explicit host token in AyaAgent", async () => {
    appState.appSettings.ai.default_mode = "agent";
    appState.appSettings.ai.default_agent_kind = "nyaterm";
    const source = terminalPane("source-session", {
      id: "source-pane",
      name: "source.example.test",
      connectionId: "source-connection",
    });
    const host = terminalPane("host-session", {
      id: "host-pane",
      name: "jump.example.test",
      connectionId: "host-connection",
    });
    appState.tabs = [tabWithPane(source), tabWithPane(host, 1)];
    appState.savedConnections = [
      {
        id: "source-connection",
        name: "source.example.test",
        type: "ssh",
        host: "source.example.test",
        port: 2222,
        username: "root",
        network: { proxy_jump_id: "host-connection" },
      },
      {
        id: "host-connection",
        name: "jump.example.test",
        type: "ssh",
        host: "jump.example.test",
        port: 2200,
        username: "root",
      },
    ];
    invokeMock.mockImplementation((command: string) => {
      if (command === "list_sessions")
        return Promise.resolve(
          [source, host].map((pane) => ({
            id: pane.sessionId,
            name: pane.name,
            session_type: pane.type,
            connected: true,
            started_at: "2026-01-01T00:00:00Z",
          })),
        );
      if (command === "get_ai_sessions") return Promise.resolve([]);
      if (command === "get_terminal_cwd") return Promise.resolve("/home/test");
      if (command === "start_ai_chat_stream") return Promise.resolve({ sessionId: "aya-session" });
      if (command === "append_ai_audit") return Promise.resolve(null);
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });
    const view = render(
      <AIAssistantPanel
        activePane={host}
        intent={{
          id: "aya-file-intent",
          action: "custom_file_action",
          userInput: "Which files?",
          fileReference: {
            id: "python",
            name: "reference.py",
            path: "/home/test/reference.py",
            backend: "remote",
            host: "root@source.example.test:2222",
            terminalSessionId: source.sessionId,
            content: "print('source only')",
            sizeBytes: 20,
          },
        }}
      />,
    );
    const editor = screen.getByRole("textbox");
    await waitFor(() => expect(editor.textContent).toContain("@reference.py"));
    editor.append(document.createTextNode("@"));
    const range = document.createRange();
    range.selectNodeContents(editor);
    range.collapse(false);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
    fireEvent.input(editor);
    const button = await waitFor(() => {
      const root = view.container.querySelector<HTMLButtonElement>(
        'button[data-reference-host-group="ssh:root@jump.example.test:2200"]',
      );
      if (!root) throw new Error("Host root was not rendered");
      return root;
    });
    fireEvent.click(button);
    await waitFor(() => expect(editor.textContent).toContain("@jump.example.test"));
    fireEvent.keyDown(editor, { key: "Enter" });
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith(
        "start_ai_chat_stream",
        expect.objectContaining({
          request: expect.objectContaining({
            defaultTargetSessionId: host.sessionId,
            context: expect.objectContaining({
              ayaContext: expect.objectContaining({
                executionTargetSessionIds: [host.sessionId],
                files: [
                  expect.objectContaining({
                    sourceSessionId: source.sessionId,
                    sourceEndpoint: "root@source.example.test:2222",
                    content: "print('source only')",
                  }),
                ],
              }),
            }),
            references: [
              expect.objectContaining({ kind: "file", name: "reference.py" }),
              expect.objectContaining({ kind: "host", name: "jump.example.test" }),
            ],
            targets: expect.arrayContaining([
              expect.objectContaining({
                terminalSessionId: source.sessionId,
                host: "source.example.test",
                label: "root@source.example.test:2222 (via root@jump.example.test:2200)",
              }),
            ]),
          }),
        }),
      ),
    );
    expect(view.container.querySelector('[data-ai-reference-kind="host"]')?.textContent).toBe(
      "@jump.example.test",
    );
    expect(
      view.container.querySelector('[data-ai-reference-kind="file"]')?.getAttribute("aria-label"),
    ).toBe("root@source.example.test:2222:/home/test/reference.py");
  });
  it("keeps root, session, and file references as independent tree selections", async () => {
    appState.appSettings.ai.default_mode = "agent";
    appState.appSettings.ai.default_agent_kind = "nyaterm";
    const a = terminalPane("session-a", { id: "pane-a", name: "Shared host" });
    const b = terminalPane("session-b", { id: "pane-b", name: "Shared host" });
    const file = createFileDocumentPane({
      sessionId: a.sessionId,
      connectionId: a.connectionId,
      type: "SSH",
      name: "keep.py",
      backend: "remote",
      path: "/keep.py",
      file: { content: "keep", size: 4, mtime: 1, contentHash: "hash" },
    });
    appState.tabs = [tabWithPane(a), tabWithPane(b, 1), createWorkspaceTab(file, 2)];
    appState.savedConnections = [
      {
        id: "connection-1",
        name: "Shared host",
        type: "ssh",
        host: "server",
        port: 22,
        username: "ops",
      },
    ];
    invokeMock.mockImplementation((command: string) => {
      if (command === "list_sessions")
        return Promise.resolve(
          [a, b].map((pane, index) => ({
            id: pane.sessionId,
            name: pane.name,
            connected: true,
            started_at: `2026-01-0${index + 1}T00:00:00Z`,
          })),
        );
      if (command === "get_ai_sessions") return Promise.resolve([]);
      if (command === "get_terminal_cwd") return Promise.resolve("/");
      if (command === "start_ai_chat_stream")
        return Promise.resolve({ sessionId: "narrowed-chat" });
      if (command === "append_ai_audit") return Promise.resolve(null);
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });
    const view = render(
      <AIAssistantPanel
        activePane={a}
        intent={{
          id: "keep-file",
          action: "custom_file_action",
          userInput: "Inspect",
          fileReference: {
            id: "keep-file",
            name: file.name,
            backend: "remote",
            path: file.file.path,
            terminalSessionId: a.sessionId,
            host: "ops@server:22",
            content: "keep",
            sizeBytes: 4,
          },
        }}
      />,
    );
    const editor = screen.getByRole("textbox");
    await waitFor(() => expect(editor.textContent).toContain("@keep.py"));
    const openMentions = (query = "") => {
      editor.append(document.createTextNode(`@${query}`));
      const range = document.createRange();
      range.selectNodeContents(editor);
      range.collapse(false);
      window.getSelection()?.removeAllRanges();
      window.getSelection()?.addRange(range);
      fireEvent.input(editor);
    };
    openMentions();
    const root = await waitFor(() => {
      const button = view.container.querySelector<HTMLButtonElement>(
        '[data-reference-host-group="ssh:ops@server:22"]',
      );
      if (!button) throw new Error("Missing host root");
      return button;
    });
    expect(root.getAttribute("aria-label")).toContain("ai.referenceHostScope");
    expect(root.hasAttribute("title")).toBe(false);
    fireEvent.click(root);
    await waitFor(() =>
      expect(
        editor.querySelector('[data-ai-reference-id="host:ssh:ops@server:22"]'),
      ).not.toBeNull(),
    );
    openMentions();
    const duplicateRoot = await waitFor(() => {
      const button = view.container.querySelector<HTMLButtonElement>(
        '[data-reference-host-group="ssh:ops@server:22"]',
      );
      if (!button) throw new Error("Missing duplicate host root");
      return button;
    });
    fireEvent.click(duplicateRoot);
    await waitFor(() =>
      expect(
        editor.querySelectorAll('[data-ai-reference-id="host:ssh:ops@server:22"]'),
      ).toHaveLength(2),
    );
    openMentions();
    const selectedRoot = await waitFor(() => {
      const button = view.container.querySelector<HTMLButtonElement>(
        '[data-reference-host-group="ssh:ops@server:22"]',
      );
      if (!button) throw new Error("Missing selected host root");
      return button;
    });
    const expand =
      selectedRoot.parentElement?.querySelector<HTMLButtonElement>("button[aria-expanded]");
    if (!expand) throw new Error("Missing expansion control");
    fireEvent.click(expand);
    const child = await waitFor(() => {
      const button = view.container.querySelector<HTMLButtonElement>(
        '[data-reference-option="session:session-b"]',
      );
      if (!button) throw new Error("Missing session child");
      return button;
    });
    expect(child.textContent).toContain("ai.referenceSessionStartedAt");
    expect(child.getAttribute("data-reference-depth")).toBe("1");
    const fileChild = view.container.querySelector('[data-reference-option^="file:"]');
    expect(fileChild?.getAttribute("data-reference-depth")).toBe("2");
    openMentions("server");
    const collapse = await waitFor(() => {
      const button = view.container.querySelector<HTMLButtonElement>(
        '[data-reference-host-group="ssh:ops@server:22"] + button[aria-expanded], button[aria-label="ai.collapseReferenceGroup"]',
      );
      if (!button) throw new Error("Missing collapse control");
      return button;
    });
    fireEvent.click(collapse);
    await waitFor(() =>
      expect(
        view.container.querySelector('[data-reference-option="session:session-b"]'),
      ).toBeNull(),
    );
    expect(collapse.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(collapse);
    const narrowedChild = await waitFor(() => {
      const button = view.container.querySelector<HTMLButtonElement>(
        '[data-reference-option="session:session-b"]',
      );
      if (!button) throw new Error("Missing session child after re-expanding");
      return button;
    });
    fireEvent.click(narrowedChild);
    await waitFor(() =>
      expect(editor.querySelector('[data-ai-reference-id="session:session-b"]')).not.toBeNull(),
    );
    expect(editor.querySelectorAll('[data-ai-reference-id="host:ssh:ops@server:22"]')).toHaveLength(
      2,
    );
    expect(editor.querySelector('[data-ai-reference-id="keep-file"]')).not.toBeNull();
    fireEvent.keyDown(editor, { key: "Enter" });
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith(
        "start_ai_chat_stream",
        expect.objectContaining({
          request: expect.objectContaining({
            context: expect.objectContaining({
              ayaContext: expect.objectContaining({
                executionTargetSessionIds: expect.arrayContaining([a.sessionId, b.sessionId]),
                files: [expect.objectContaining({ content: "keep", sourceSessionId: a.sessionId })],
              }),
            }),
            references: expect.arrayContaining([
              expect.objectContaining({ kind: "file" }),
              expect.objectContaining({ kind: "host" }),
              expect.objectContaining({ kind: "session", sessionIds: [b.sessionId] }),
            ]),
          }),
        }),
      ),
    );
  });
});

function openHistory(container: HTMLElement) {
  const button = container.querySelector<HTMLButtonElement>(
    'button[aria-expanded]:not([aria-label]):not([role="combobox"])',
  );
  if (!button) throw new Error("History button not found");
  fireEvent.click(button);
}

async function historySessionButton(title: string) {
  const titleElement = await screen.findByText(title);
  const button = titleElement.closest("button");
  if (!(button instanceof HTMLButtonElement)) throw new Error("History session button not found");
  return button;
}

function terminalPane(
  sessionId: string,
  overrides: Partial<TerminalSessionPane> = {},
): TerminalSessionPane {
  return {
    id: "terminal-pane",
    kind: "leaf",
    paneKind: "terminal",
    sessionId,
    name: "SSH terminal",
    type: "SSH",
    connectionId: "connection-1",
    ...overrides,
  };
}

function tabWithPane(pane: TerminalSessionPane, persistOrder = 0): Tab {
  return {
    id: `tab-${pane.id}`,
    persistOrder,
    activePaneId: pane.id,
    root: pane,
  };
}

function aiSession(id: string, targetId: string): AISession {
  return {
    id,
    title: "Reconnect chat",
    createdAt: "2026-09-07T10:00:00Z",
    updatedAt: "2026-09-07T10:05:00Z",
    connectionId: "connection-1",
    scope: {
      type: "terminal",
      targetId,
      connectionIds: ["connection-1"],
      label: "SSH terminal",
    },
  };
}

function aiMessage(sessionId: string, content: string): AIMessage {
  return {
    id: `message-${content}`,
    sessionId,
    role: "user",
    content,
    createdAt: "2026-09-07T10:01:00Z",
  };
}
