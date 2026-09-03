import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@/lib/invoke";
import type { FileProperties } from "@/types/global";
import PropertiesDialog, {
  type PropertiesDialogData,
} from "./PropertiesDialog";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => key,
  }),
}));

vi.mock("@/lib/invoke", () => ({ invoke: vi.fn() }));
vi.mock("sonner", () => ({
  toast: { error: vi.fn(), success: vi.fn(), info: vi.fn() },
}));

const symlinkProperties: FileProperties = {
  name: "current",
  is_dir: true,
  is_symlink: true,
  symlink_target: "releases/v2",
  size: 11,
  permissions: "lrwxrwxrwx",
  owner: "root",
  group: "root",
  uid: "0",
  gid: "0",
  mtime: 0,
  atime: 0,
};

const remoteData: PropertiesDialogData = {
  sessionId: "session-1",
  backend: "remote",
  fullPath: "/opt/app/current",
  rawPathToken: "raw-current",
  name: "current",
  is_dir: true,
};

describe("PropertiesDialog symlink target", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("shows the original target and identifies links before directories", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(symlinkProperties);

    render(<PropertiesDialog data={remoteData} onClose={vi.fn()} />);

    const targetInput = await screen.findByRole("textbox", {
      name: "fileExplorer.symlinkTarget",
    });
    expect((targetInput as HTMLInputElement).value).toBe("releases/v2");
    expect(screen.getByText("fileExplorer.symbolicLink")).not.toBeNull();
    expect(screen.queryByText("fileExplorer.folder")).toBeNull();
  });

  it("preserves target whitespace and saves it before attribute changes", async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce(symlinkProperties)
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce(undefined);
    const onClose = vi.fn();
    const onSuccess = vi.fn();

    render(
      <PropertiesDialog
        data={remoteData}
        onClose={onClose}
        onSuccess={onSuccess}
      />,
    );

    fireEvent.change(
      await screen.findByRole("textbox", {
        name: "fileExplorer.symlinkTarget",
      }),
      { target: { value: " ../releases/v3 " } },
    );
    fireEvent.change(screen.getAllByDisplayValue("root")[0], {
      target: { value: "deploy" },
    });
    fireEvent.click(screen.getByRole("button", { name: "dialog.save" }));

    await waitFor(() => expect(invoke).toHaveBeenCalledTimes(3));
    expect(invoke).toHaveBeenNthCalledWith(2, "update_remote_symlink_target", {
      sessionId: "session-1",
      path: "/opt/app/current",
      rawPathToken: "raw-current",
      targetPath: " ../releases/v3 ",
    });
    expect(invoke).toHaveBeenNthCalledWith(3, "update_remote_file_attributes", {
      sessionId: "session-1",
      path: "/opt/app/current",
      rawPathToken: "raw-current",
      update: {
        mode: null,
        owner: "deploy",
        group: null,
        recursive: false,
      },
    });
    expect(onSuccess).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("rejects blank targets without sending an update", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(symlinkProperties);
    const { toast } = await import("sonner");

    render(<PropertiesDialog data={remoteData} onClose={vi.fn()} />);

    fireEvent.change(
      await screen.findByRole("textbox", {
        name: "fileExplorer.symlinkTarget",
      }),
      { target: { value: "   " } },
    );
    fireEvent.click(screen.getByRole("button", { name: "dialog.save" }));

    expect(toast.error).toHaveBeenCalledWith(
      "fileExplorer.symlinkTargetRequired",
    );
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("closes without an update when nothing changed", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(symlinkProperties);
    const onClose = vi.fn();

    render(<PropertiesDialog data={remoteData} onClose={onClose} />);

    await screen.findByRole("textbox", { name: "fileExplorer.symlinkTarget" });
    fireEvent.click(screen.getByRole("button", { name: "dialog.save" }));

    expect(invoke).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("does not expose target editing for local links or regular remote files", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(symlinkProperties);
    const { unmount } = render(
      <PropertiesDialog
        data={{ ...remoteData, backend: "local" }}
        onClose={vi.fn()}
      />,
    );

    await screen.findByText("fileExplorer.symbolicLink");
    expect(
      screen.queryByRole("textbox", { name: "fileExplorer.symlinkTarget" }),
    ).toBeNull();
    unmount();

    vi.mocked(invoke).mockResolvedValueOnce({
      ...symlinkProperties,
      is_dir: false,
      is_symlink: false,
      symlink_target: null,
    });
    render(
      <PropertiesDialog
        data={{ ...remoteData, name: "config", is_dir: false }}
        onClose={vi.fn()}
      />,
    );

    await screen.findByText("fileExplorer.file");
    expect(
      screen.queryByRole("textbox", { name: "fileExplorer.symlinkTarget" }),
    ).toBeNull();
  });
});
