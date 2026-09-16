import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SavedAccount, SavedConnection } from "@/types/global";
import NewSessionPage from "./NewSessionPage";

const {
  closeMock,
  emitMock,
  invokeMock,
  rdpFormMock,
  serialFormMock,
  sshFormMock,
  telnetFormMock,
  translateMock,
} = vi.hoisted(() => ({
  closeMock: vi.fn(),
  emitMock: vi.fn(),
  invokeMock: vi.fn(),
  rdpFormMock: vi.fn(),
  serialFormMock: vi.fn(),
  sshFormMock: vi.fn(),
  telnetFormMock: vi.fn(),
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
vi.mock("@/components/sessions/SerialForm", () => ({
  SerialForm: (props: Record<string, unknown>) => {
    serialFormMock(props);
    return (
      <button
        type="button"
        onClick={() =>
          (props.setModemUploadProtocol as ((value: "xmodem") => void) | undefined)?.("xmodem")
        }
      >
        choose-xmodem
      </button>
    );
  },
}));
vi.mock("@/components/sessions/SshForm", () => ({
  SshForm: (props: Record<string, unknown>) => {
    sshFormMock(props);
    return (
      <>
        <button
          type="button"
          onClick={() => {
            (props.setAccountId as (value: string) => void)("account-1");
            (props.setPasswordSource as (value: string) => void)("account");
            (props.setPassword as (value: string) => void)("");
            (props.setHasPassword as (value: boolean) => void)(false);
          }}
        >
          choose-account
        </button>
        <button type="button" onClick={() => (props.setAccountId as (value: string) => void)("")}>
          choose-manual
        </button>
        <button
          type="button"
          onClick={() => {
            (props.setPasswordSource as (value: string) => void)("direct");
            (props.setPassword as (value: string) => void)("");
            (props.setHasPassword as (value: boolean) => void)(false);
          }}
        >
          choose-empty-direct-password
        </button>
        <button
          type="button"
          onClick={() => {
            (props.setAuthType as (value: string) => void)("key");
            (props.setKeyId as (value: string) => void)("key-1");
          }}
        >
          choose-key
        </button>
      </>
    );
  },
}));
vi.mock("@/components/sessions/TelnetForm", () => ({
  TelnetForm: (props: Record<string, unknown>) => {
    telnetFormMock(props);
    return (
      <button
        type="button"
        onClick={() => {
          (props.setPasswordSource as (value: string) => void)("direct");
          (props.setPassword as (value: string) => void)("");
          (props.setHasPassword as (value: boolean) => void)(false);
        }}
      >
        choose-empty-telnet-password
      </button>
    );
  },
}));
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

const account: SavedAccount = {
  id: "account-1",
  name: "Production",
  username: "admin",
  has_password: true,
};

const sshConnection: SavedConnection = {
  id: "ssh-1",
  name: "SSH server",
  type: "ssh",
  host: "ssh.example.com",
  port: 22,
  username: "root",
  auth: { mode: "password", account_id: account.id },
};

const manualSshConnection: SavedConnection = {
  ...sshConnection,
  id: "ssh-manual",
  name: "Manual SSH server",
  auth: { mode: "password", has_password: true },
};

const legacySshConnection: SavedConnection = {
  ...sshConnection,
  id: "ssh-legacy",
  name: "Legacy SSH server",
  auth: { mode: "password", password_id: account.id },
};

const telnetConnection: SavedConnection = {
  id: "telnet-1",
  name: "Telnet server",
  type: "telnet",
  host: "telnet.example.com",
  port: 23,
  username: "fallback",
  auth: { mode: "password", account_id: account.id },
};

const rdpConnection: SavedConnection = {
  id: "rdp-1",
  name: "RDP desktop",
  type: "rdp",
  host: "rdp.example.com",
  port: 3389,
  username: "Administrator",
  tags: ["production"],
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

const serialConnection: SavedConnection = {
  id: "serial-1",
  name: "Board",
  type: "serial",
  port_name: "COM3",
  baud_rate: 115200,
  data_bits: 8,
  parity: "none",
  stop_bits: "1",
  backspace_mode: "ctrl_h",
  modem_upload_protocol: "ymodem",
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
        case "get_saved_passwords":
          return Promise.resolve([account]);
        case "get_saved_connections":
          return Promise.resolve([
            rdpConnection,
            serialConnection,
            jumpHost,
            sshConnection,
            manualSshConnection,
            legacySshConnection,
            telnetConnection,
          ]);
        case "list_serial_ports":
          return Promise.resolve(["COM3"]);
        case "save_connection":
          return Promise.resolve(rdpConnection.id);
        default:
          return Promise.reject(new Error(`Unexpected command: ${command}`));
      }
    });
    rdpFormMock.mockReset();
    serialFormMock.mockReset();
    sshFormMock.mockReset();
    telnetFormMock.mockReset();
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

  it("loads, edits, and saves connection tags", async () => {
    render(<NewSessionPage />);

    expect(await screen.findByText("production")).not.toBeNull();
    const tagInput = screen.getByRole("combobox", { name: "dialog.tagsPlaceholder" });
    fireEvent.change(tagInput, { target: { value: " gpu " } });
    fireEvent.keyDown(tagInput, { key: "Enter" });
    fireEvent.click(screen.getAllByRole("button", { name: "dialog.removeTag" })[0]);
    fireEvent.click(screen.getByRole("button", { name: "dialog.save" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "save_connection",
        expect.objectContaining({
          connection: expect.objectContaining({ tags: ["gpu"] }),
        }),
      );
    });
  });

  it("restores and saves the Serial modem upload protocol", async () => {
    window.history.replaceState({}, "", `/?edit=${serialConnection.id}`);
    render(<NewSessionPage />);

    await waitFor(() => {
      expect(serialFormMock).toHaveBeenLastCalledWith(
        expect.objectContaining({ modemUploadProtocol: "ymodem" }),
      );
    });

    fireEvent.click(screen.getByRole("button", { name: "choose-xmodem" }));
    await waitFor(() => {
      expect(serialFormMock).toHaveBeenLastCalledWith(
        expect.objectContaining({ modemUploadProtocol: "xmodem" }),
      );
    });

    fireEvent.click(screen.getByRole("button", { name: "dialog.save" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "save_connection",
        expect.objectContaining({
          connection: expect.objectContaining({
            type: "serial",
            modem_upload_protocol: "xmodem",
          }),
        }),
      );
    });
  });

  it("saves an account reference without copying its username", async () => {
    window.history.replaceState({}, "", `/?edit=${manualSshConnection.id}`);
    render(<NewSessionPage />);

    await waitFor(() => expect(sshFormMock).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "choose-account" }));
    fireEvent.click(screen.getByRole("button", { name: "dialog.save" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "save_connection",
        expect.objectContaining({
          connection: expect.objectContaining({
            type: "ssh",
            username: "root",
            auth: expect.objectContaining({
              account_id: account.id,
              password_source: "account",
              password_id: "",
              password: "",
            }),
          }),
        }),
      );
    });
  });

  it("keeps the account username but disables its password for an empty direct password", async () => {
    window.history.replaceState({}, "", `/?edit=${sshConnection.id}`);
    render(<NewSessionPage />);

    await waitFor(() => {
      expect(sshFormMock).toHaveBeenLastCalledWith(
        expect.objectContaining({ accountId: account.id, passwordSource: "account" }),
      );
    });
    fireEvent.click(screen.getByRole("button", { name: "choose-empty-direct-password" }));
    fireEvent.click(screen.getByRole("button", { name: "dialog.save" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "save_connection",
        expect.objectContaining({
          connection: expect.objectContaining({
            username: "root",
            auth: expect.objectContaining({
              account_id: account.id,
              password_source: "connection",
              password: "",
            }),
          }),
        }),
      );
    });
  });

  it("keeps a Telnet account reference while saving an empty direct password", async () => {
    window.history.replaceState({}, "", `/?edit=${telnetConnection.id}`);
    render(<NewSessionPage />);

    await waitFor(() => {
      expect(telnetFormMock).toHaveBeenLastCalledWith(
        expect.objectContaining({ accountId: account.id, passwordSource: "account" }),
      );
    });
    fireEvent.click(screen.getByRole("button", { name: "choose-empty-telnet-password" }));
    fireEvent.click(screen.getByRole("button", { name: "dialog.save" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "save_connection",
        expect.objectContaining({
          connection: expect.objectContaining({
            type: "telnet",
            username: "fallback",
            auth: expect.objectContaining({
              account_id: account.id,
              password_source: "connection",
              password: "",
            }),
          }),
        }),
      );
    });
  });

  it("clears only the account reference when switching to Manual", async () => {
    window.history.replaceState({}, "", `/?edit=${sshConnection.id}`);
    render(<NewSessionPage />);

    await waitFor(() => {
      expect(sshFormMock).toHaveBeenLastCalledWith(
        expect.objectContaining({ accountId: account.id, username: "root" }),
      );
    });
    fireEvent.click(screen.getByRole("button", { name: "choose-manual" }));
    fireEvent.click(screen.getByRole("button", { name: "dialog.save" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "save_connection",
        expect.objectContaining({
          connection: expect.objectContaining({
            username: "root",
            auth: expect.objectContaining({ account_id: "" }),
          }),
        }),
      );
    });
  });

  it("upgrades a legacy password_id reference when saving", async () => {
    window.history.replaceState({}, "", `/?edit=${legacySshConnection.id}`);
    render(<NewSessionPage />);

    await waitFor(() => {
      expect(sshFormMock).toHaveBeenLastCalledWith(
        expect.objectContaining({ accountId: account.id }),
      );
    });
    fireEvent.click(screen.getByRole("button", { name: "dialog.save" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "save_connection",
        expect.objectContaining({
          connection: expect.objectContaining({
            auth: expect.objectContaining({
              account_id: account.id,
              password_id: "",
            }),
          }),
        }),
      );
    });
  });

  it("keeps the account reference when changing SSH authentication to key", async () => {
    window.history.replaceState({}, "", `/?edit=${sshConnection.id}`);
    render(<NewSessionPage />);

    await waitFor(() => expect(sshFormMock).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "choose-key" }));
    fireEvent.click(screen.getByRole("button", { name: "dialog.save" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "save_connection",
        expect.objectContaining({
          connection: expect.objectContaining({
            auth: expect.objectContaining({ mode: "key", account_id: account.id }),
          }),
        }),
      );
    });
  });

  it("saves an edited connection when Enter is pressed in a single-line input", async () => {
    render(<NewSessionPage />);

    const nameInput = await screen.findByDisplayValue(rdpConnection.name);
    invokeMock.mockClear();

    fireEvent.keyDown(nameInput, { key: "Enter" });

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "save_connection",
        expect.objectContaining({
          connection: expect.objectContaining({ id: rdpConnection.id }),
        }),
      );
      expect(closeMock).toHaveBeenCalledTimes(1);
    });
  });

  it("does not save from a textarea or while Enter is composing text", async () => {
    render(<NewSessionPage />);

    const nameInput = await screen.findByDisplayValue(rdpConnection.name);
    const description = screen.getByPlaceholderText("dialog.descriptionPlaceholder");
    invokeMock.mockClear();

    fireEvent.keyDown(description, { key: "Enter" });
    fireEvent.keyDown(nameInput, { key: "Enter", isComposing: true });

    expect(invokeMock).not.toHaveBeenCalledWith("save_connection", expect.anything());
    expect(closeMock).not.toHaveBeenCalled();
  });
});
