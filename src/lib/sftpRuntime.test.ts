import { describe, expect, it, vi } from "vitest";
import type { SavedConnection } from "@/types/global";
import { canOpenSavedConnectionWithSftp, openSavedConnectionWithSftp } from "./sftpRuntime";

describe("canOpenSavedConnectionWithSftp", () => {
  it("allows SSH connections when SFTP is defaulted or enabled", () => {
    expect(canOpenSavedConnectionWithSftp(connection("ssh"))).toBe(true);
    expect(
      canOpenSavedConnectionWithSftp({
        ...connection("ssh"),
        sftp: { enabled: true } as SavedConnection["sftp"],
      }),
    ).toBe(true);
  });

  it("hides the action when SFTP is explicitly disabled", () => {
    expect(
      canOpenSavedConnectionWithSftp({
        ...connection("ssh"),
        sftp: { enabled: false } as SavedConnection["sftp"],
      }),
    ).toBe(false);
  });

  it("hides the action for Mosh transport", () => {
    expect(
      canOpenSavedConnectionWithSftp({
        ...connection("ssh"),
        ssh_transport: "mosh",
      }),
    ).toBe(false);
  });

  it("hides the action for non-SSH connections", () => {
    expect(canOpenSavedConnectionWithSftp(connection("telnet"))).toBe(false);
  });

  it("opens only the connection supplied by the context-menu action", () => {
    const selected = connection("ssh");
    const other = { ...connection("ssh"), id: "ssh-2" };
    const onOpen = vi.fn();

    expect(openSavedConnectionWithSftp(selected, onOpen)).toBe(true);
    expect(onOpen).toHaveBeenCalledOnce();
    expect(onOpen).toHaveBeenCalledWith(selected);
    expect(onOpen).not.toHaveBeenCalledWith(other);
  });

  it("does not invoke the action for an ineligible connection", () => {
    const onOpen = vi.fn();

    expect(openSavedConnectionWithSftp(connection("telnet"), onOpen)).toBe(false);
    expect(onOpen).not.toHaveBeenCalled();
  });
});

function connection(type: SavedConnection["type"]): SavedConnection {
  return { id: `${type}-1`, name: type, type } as SavedConnection;
}
