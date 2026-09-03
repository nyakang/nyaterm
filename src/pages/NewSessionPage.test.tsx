import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SavedConnection } from "@/types/global";
import NewSessionPage from "./NewSessionPage";

const { closeMock, emitMock, invokeMock, rdpFormMock, translateMock } = vi.hoisted(() => ({
  closeMock: vi.fn(),
  emitMock: vi.fn(),
  invokeMock: vi.fn(),
  rdpFormMock: vi.fn(),
  translateMock: (key: string, fallback?: unknown) =>
    typeof fallback === "string" ? fallback : key,
}));

vi.mock("@/context/AppContext", () => ({
  useApp: () => ({
    appSettings: {
      recording: {
        auto_start: false,
        default_mode: "transcript",
      },
      ui: {
        show_remote_stats: true,
      },
    },
  }),
}));

vi.mock("@/lib/invoke", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ emit: emitMock }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ close: closeMock }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

vi.mock("@/components/sessions/LocalTerminal", () => ({ LocalTerminal: () => null }));
vi.mock("@/components/sessions/SerialForm", () => ({ SerialForm: () => null }));
vi.mock("@/components/sessions/SshForm", () => ({ SshForm: () => null }));
vi.mock("@/components/sessions/TelnetForm", () => ({ TelnetForm: () => null }));
vi.mock("@/components/sessions/VncForm", () => ({ VncForm: () => null }));
vi.mock("@/components/sessions/RdpForm", () => ({
  RdpForm: (props: Record<string, unknown>) => {
    rdpFormMock(props);
    return null;
  },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: translateMock,
  }),
}));

const jumpHost: SavedConnection = {
  id: "ssh-jump-1",
  name: "SSH jump host",
  type: "ssh",
  host: "jump.example.com",
  port: 22,
  username: "jump-user",
};

const rdpConnection: SavedConnection = {
  id: "rdp-1",
  name: "RDP desktop",
  type: "rdp",
  host: "rdp.example.com",
  port: 3389,
  username: "Administrator",
  auth: { mode: "password" },
  network: {
    proxy_id: "proxy-1",
    proxy_jump_id: jumpHost.id,
  },
  security: {
    use_nla: true,
    certificate_policy: "prompt",
  },
  display: {
    mode: "fit-window",
    width: 1920,
    height: 1080,
    color_depth: 32,
  },
  clipboard: { mode: "text-only" },
  reconnect: { enabled: true, max_attempts: 5 },
};

describe("NewSessionPage", () => {
  beforeEach(() => {
    window.history.replaceState({}, "", `/?edit=${rdpConnection.id}`);
    closeMock.mockReset();
    closeMock.mockResolvedValue(undefined);
    emitMock.mockReset();
    emitMock.mockResolvedValue(undefined);
    invokeMock.mockReset();
    invokeMock.mockImplementation((command: string) => {
      switch (command) {
        case "get_groups":
        case "get_proxies":
        case "get_otp_entries":
        case "get_connection_custom_icons":
          return Promise.resolve([]);
        case "get_saved_connections":
          return Promise.resolve([rdpConnection, jumpHost]);
        case "save_connection":
          return Promise.resolve(rdpConnection.id);
        default:
          return Promise.reject(new Error(`Unexpected command: ${command}`));
      }
    });
    rdpFormMock.mockReset();
  });

  it("restores an RDP jump host and keeps it when saving without changes", async () => {
    render(<NewSessionPage />);

    await waitFor(() => {
      expect(rdpFormMock).toHaveBeenLastCalledWith(
        expect.objectContaining({
          proxyId: "proxy-1",
          jumpHostId: jumpHost.id,
          jumpHostOptions: expect.arrayContaining([
            expect.objectContaining({
              connection: expect.objectContaining({ id: jumpHost.id }),
            }),
          ]),
        }),
      );
    });

    const saveButton = screen.getByRole("button", { name: "dialog.save" });
    expect((saveButton as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(saveButton);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "save_connection",
        expect.objectContaining({
          connection: expect.objectContaining({
            type: "rdp",
            network: {
              proxy_id: "proxy-1",
              proxy_jump_id: jumpHost.id,
            },
          }),
        }),
      );
    });
  });
});
