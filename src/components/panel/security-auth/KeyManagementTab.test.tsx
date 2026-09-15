import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SshKey } from "@/types/global";
import { KeyManagementTab } from "./KeyManagementTab";

const { invokeMock, toastSuccessMock, writeClipboardTextMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  toastSuccessMock: vi.fn(),
  writeClipboardTextMock: vi.fn(),
}));

vi.mock("@/lib/invoke", () => ({ invoke: invokeMock }));
vi.mock("@/lib/clipboard", () => ({ writeClipboardText: writeClipboardTextMock }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock("sonner", () => ({
  toast: { success: toastSuccessMock, error: vi.fn() },
}));
vi.mock("./SecretUnlockFooter", () => ({ SecretUnlockFooter: () => null }));
vi.mock("@/components/dialog/security-auth/KeyDeleteDialog", () => ({ KeyDeleteDialog: () => null }));
vi.mock("@/components/dialog/security-auth/KeyEditorDialog", () => ({ KeyEditorDialog: () => null }));
vi.mock("@/components/dialog/security-auth/PrivateKeyViewDialog", () => ({
  PrivateKeyViewDialog: () => null,
}));

const key: SshKey = {
  id: "key-1",
  name: "Production key",
  has_key_data: true,
};
const publicKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITest production";

describe("KeyManagementTab", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    writeClipboardTextMock.mockReset();
    toastSuccessMock.mockReset();
    writeClipboardTextMock.mockResolvedValue(undefined);
    invokeMock.mockImplementation((command: string) => {
      switch (command) {
        case "get_ssh_keys":
          return Promise.resolve([key]);
        case "get_ssh_key_public_key":
          return Promise.resolve(publicKey);
        default:
          return Promise.reject(new Error(`Unexpected command: ${command}`));
      }
    });
  });

  it("copies a public key while secrets remain locked", async () => {
    render(<KeyManagementTab />);

    await screen.findByText(key.name);
    fireEvent.click(screen.getByRole("button", { name: "settings.copyPublicKey" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("get_ssh_key_public_key", { id: key.id });
      expect(writeClipboardTextMock).toHaveBeenCalledWith(publicKey);
    });
    expect(toastSuccessMock).toHaveBeenCalledWith("settings.copyPublicKeySuccess");

    fireEvent.click(screen.getByRole("button", { name: "settings.viewPrivateKey" }));
    expect(invokeMock).not.toHaveBeenCalledWith("get_ssh_key_private_key", { id: key.id });
  });
});
