import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_CLOUD_SYNC_SETTINGS, DEFAULT_CLOUD_SYNC_STATUS } from "@/lib/cloudSync";
import type { CloudSyncSettings, CloudSyncStatus } from "@/types/global";
import { SyncBackupTab } from "./SyncBackupTab";

const state = vi.hoisted(() => ({
  status: null as CloudSyncStatus | null,
  isDirty: false,
  isSaving: false,
  hasMasterPassword: true,
  invoke: vi.fn(),
  success: vi.fn(),
  error: vi.fn(),
}));

const cloudSettings: CloudSyncSettings = {
  ...DEFAULT_CLOUD_SYNC_SETTINGS,
  enabled: true,
  webdav: { ...DEFAULT_CLOUD_SYNC_SETTINGS.webdav, endpoint: "https://dav.example.test" },
};

vi.mock("@/context/AppContext", () => ({
  useApp: () => ({
    appSettings: {
      cloud_sync: cloudSettings,
      security: { master_password: state.hasMasterPassword ? "configured" : null },
    },
    updateAppSettings: vi.fn(),
  }),
}));
vi.mock("@/context/SettingsDraftContext", () => ({
  useSettingsDraft: () => ({
    committedSettings: {
      cloud_sync: cloudSettings,
      security: { master_password: state.hasMasterPassword ? "configured" : null },
    },
    isDirty: state.isDirty,
    isSaving: state.isSaving,
  }),
}));
vi.mock("@/lib/invoke", () => ({ invoke: state.invoke }));
vi.mock("@/lib/backend/api", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock("sonner", () => ({
  toast: { success: state.success, error: state.error },
}));

function conflictStatus(kind: "content_conflict" | "remote_inconsistent"): CloudSyncStatus {
  return {
    ...DEFAULT_CLOUD_SYNC_STATUS,
    enabled: true,
    state: "conflict",
    conflict: {
      kind,
      provider: "webdav",
      local_payload_hash: "local-hash",
      remote_payload_hash: "remote-hash",
      remote_revision: "remote-revision",
      remote_created_at_ms: 100,
      remote_device_id: "remote-device",
      detected_at_ms: 101,
      recovery_revision: null,
      recovery_payload_hash: null,
      recovery_created_at_ms: null,
      message: "Both local and cloud state changed since last sync",
    },
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  cloudSettings.enabled = true;
  state.status = conflictStatus("content_conflict");
  state.isDirty = false;
  state.isSaving = false;
  state.hasMasterPassword = true;
  state.invoke.mockImplementation(async (command: string) => {
    if (command === "get_cloud_sync_status") return state.status;
    return undefined;
  });
});

describe("SyncBackupTab additive cloud conflict resolution", () => {
  it("offers merge only for content conflicts and invokes the existing resolution command", async () => {
    render(<SyncBackupTab onNavigateSecurity={vi.fn()} />);
    const merge = await screen.findByRole("button", {
      name: "settings.syncMergeConnections",
    });
    expect((merge as HTMLButtonElement).disabled).toBe(false);
    expect(screen.getByText("settings.syncMergeConnectionsHint")).toBeTruthy();
    expect(screen.getByRole("button", { name: "settings.downloadRemoteVersion" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "settings.uploadLocalVersion" })).toBeTruthy();

    fireEvent.click(merge);
    await waitFor(() =>
      expect(state.invoke).toHaveBeenCalledWith("resolve_cloud_sync_conflict", {
        action: "merge_connections",
      }),
    );
    await waitFor(() =>
      expect(state.success).toHaveBeenCalledWith("settings.syncResolveMergeSuccess"),
    );
  });

  it("does not offer merge for inconsistent remote metadata", async () => {
    state.status = conflictStatus("remote_inconsistent");
    render(<SyncBackupTab onNavigateSecurity={vi.fn()} />);
    await screen.findByRole("button", { name: "settings.useCurrentRemoteSnapshot" });
    expect(screen.queryByRole("button", { name: "settings.syncMergeConnections" })).toBeNull();
    expect(screen.queryByText("settings.syncMergeConnectionsHint")).toBeNull();
  });

  it("disables merge while draft settings are unsaved", async () => {
    state.isDirty = true;
    render(<SyncBackupTab onNavigateSecurity={vi.fn()} />);
    const merge = await screen.findByRole("button", {
      name: "settings.syncMergeConnections",
    });
    expect((merge as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(merge);
    expect(state.invoke).not.toHaveBeenCalledWith(
      "resolve_cloud_sync_conflict",
      expect.anything(),
    );
  });

  it("disables merge when cloud sync has been disabled", async () => {
    cloudSettings.enabled = false;
    render(<SyncBackupTab onNavigateSecurity={vi.fn()} />);
    const merge = await screen.findByRole("button", {
      name: "settings.syncMergeConnections",
    });
    expect((merge as HTMLButtonElement).disabled).toBe(true);
  });

  it("disables merge while an earlier merge request is pending", async () => {
    state.invoke.mockImplementation((command: string) => {
      if (command === "get_cloud_sync_status") return Promise.resolve(state.status);
      return new Promise(() => {});
    });
    render(<SyncBackupTab onNavigateSecurity={vi.fn()} />);
    const merge = await screen.findByRole("button", {
      name: "settings.syncMergeConnections",
    });
    fireEvent.click(merge);
    await waitFor(() => expect((merge as HTMLButtonElement).disabled).toBe(true));
    fireEvent.click(merge);
    expect(
      state.invoke.mock.calls.filter(([command]) => command === "resolve_cloud_sync_conflict"),
    ).toHaveLength(1);
  });
});
