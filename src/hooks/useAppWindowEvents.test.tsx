import { act, renderHook } from "@testing-library/react";
import { StrictMode, useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { VncServerKeyVerifyRequest } from "@/components/dialog/connections/VncServerKeyVerifyDialog";
import type { RdpCertificateVerifyRequest } from "@/components/dialog/connections/RdpCertificateVerifyDialog";
import { createTerminalWindowLeaf, type TerminalWindowNode } from "@/lib/tabWindows";
import { setOwnerMainWindowLabel } from "@/lib/windowManager";
import { useAppWindowEvents } from "./useAppWindowEvents";

const mocks = vi.hoisted(() => ({
  handlers: new Map<string, (event: { payload: unknown }) => unknown>(),
  listen: vi.fn(),
  invoke: vi.fn(),
  focusTerminalSession: vi.fn(),
  setBackendTransferDuplicatePrompt: vi.fn(),
  toastError: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen, emit: vi.fn() }));
vi.mock("@/lib/invoke", () => ({ invoke: mocks.invoke }));
vi.mock("@/lib/appSessionFactory", () => ({ focusTerminalSession: mocks.focusTerminalSession }));
vi.mock("@/lib/transferDuplicatePrompt", () => ({
  setBackendTransferDuplicatePrompt: mocks.setBackendTransferDuplicatePrompt,
}));
vi.mock("sonner", () => ({ toast: { error: mocks.toastError } }));
vi.mock("@/i18n", () => ({ default: { t: (key: string) => key } }));

function eventOptions(): Parameters<typeof useAppWindowEvents>[0] {
  return {
    addTab: vi.fn().mockReturnValue("created-tab"),
    applyTmuxState: vi.fn(),
    replaceAppSettings: vi.fn(),
    setTerminalWindows: vi.fn(),
    queueSecurityPrompt: vi.fn(),
    removeSecurityPrompt: vi.fn(),
    setDockerSudoPasswordRequest: vi.fn(),
    setRdpCertificateRequests: vi.fn(),
    setVncServerKeyRequests: vi.fn(),
    handleConnectAfterEdit: vi.fn().mockResolvedValue(undefined),
    handleOpenPanel: vi.fn(),
  };
}

function renderEvents(options = eventOptions()) {
  const view = renderHook(
    (props) => {
      const [certificates, setCertificates] = useState<RdpCertificateVerifyRequest[]>([]);
      useAppWindowEvents({ ...props, setRdpCertificateRequests: setCertificates });
      return certificates;
    },
    { initialProps: options },
  );
  return { ...view, options };
}

async function receive(name: string, payload: unknown) {
  const handler = mocks.handlers.get(name);
  if (!handler) throw new Error(`Missing listener: ${name}`);
  await act(async () => {
    await handler({ payload });
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  mocks.handlers.clear();
  mocks.listen.mockReset().mockImplementation(async (name, handler) => {
    mocks.handlers.set(name, handler);
    return vi.fn();
  });
  mocks.invoke.mockReset().mockResolvedValue({ ui: { language: "en" } });
  setOwnerMainWindowLabel("main-test");
});

describe("useAppWindowEvents", () => {
  it.each([
    "main-test",
    undefined,
    null,
    "",
  ])("accepts a compatible target %s and inserts the created tab into its split", async (targetWindowLabel) => {
    let layout: TerminalWindowNode | null = createTerminalWindowLeaf(["anchor-tab"], "anchor-tab");
    const leafId = layout.id;
    const options = eventOptions();
    options.setTerminalWindows = vi.fn((update) => {
      layout = typeof update === "function" ? update(layout) : update;
    });
    renderEvents(options);
    await receive("session-created", {
      sessionId: "new-session",
      name: "Host",
      type: "SSH",
      targetWindowLabel,
      targetLeafId: leafId,
      anchorTabId: "anchor-tab",
    });
    expect(options.addTab).toHaveBeenCalledWith(
      "new-session",
      "Host",
      "SSH",
      undefined,
      undefined,
      { afterTabId: "anchor-tab" },
    );
    expect(layout).toMatchObject({
      tabIds: ["anchor-tab", "created-tab"],
      activeTabId: "created-tab",
    });
    expect(mocks.focusTerminalSession).toHaveBeenCalledWith("new-session");
  });

  it.each([
    "session-created",
    "otp-request",
    "ssh-auth-request",
    "ssh-agent-auth-pending",
    "ssh-agent-auth-failed",
    "docker-sudo-password-request",
    "host-key-verify",
    "rdp-certificate-verify",
    "vnc-server-key-verify",
    "transfer-duplicate-request",
    "session-connect-after-edit",
  ])("ignores %s addressed to another window", async (name) => {
    const { result, options } = renderEvents();
    await receive(name, { targetWindowLabel: "main-other", requestId: "request-1" });
    expect(options.addTab).not.toHaveBeenCalled();
    expect(options.queueSecurityPrompt).not.toHaveBeenCalled();
    expect(options.setDockerSudoPasswordRequest).not.toHaveBeenCalled();
    expect(options.setVncServerKeyRequests).not.toHaveBeenCalled();
    expect(options.handleConnectAfterEdit).not.toHaveBeenCalled();
    expect(mocks.focusTerminalSession).not.toHaveBeenCalled();
    expect(mocks.setBackendTransferDuplicatePrompt).not.toHaveBeenCalled();
    expect(result.current).toEqual([]);
  });

  it.each([
    ["otp-request", "otp"],
    ["ssh-auth-request", "ssh-auth"],
    ["ssh-agent-auth-pending", "ssh-agent"],
    ["ssh-agent-auth-failed", "ssh-agent"],
    ["host-key-verify", "host-key"],
  ])("queues %s with its existing prompt kind", async (name, kind) => {
    const { options } = renderEvents();
    const request = { requestId: "auth-1", targetWindowLabel: "main-test" };
    await receive(name, request);
    expect(options.queueSecurityPrompt).toHaveBeenCalledExactlyOnceWith({ kind, request });
  });

  it.each([
    "ssh-agent-auth-resolved",
    "host-key-verify-resolved",
    "security-prompt-resolved",
  ])("removes a resolved prompt on %s without adding a window filter", async (name) => {
    const { options } = renderEvents();
    await receive(name, { requestId: "auth-1", targetWindowLabel: "main-other" });
    expect(options.removeSecurityPrompt).toHaveBeenCalledExactlyOnceWith("auth-1");
  });

  it("routes VNC prompts to the owner, deduplicates, and removes resolved requests", async () => {
    const options = eventOptions();
    let requests: VncServerKeyVerifyRequest[] = [];
    options.setVncServerKeyRequests = vi.fn((update) => {
      requests = typeof update === "function" ? update(requests) : update;
    });
    renderEvents(options);
    const first: VncServerKeyVerifyRequest = {
      requestId: "vnc-1",
      targetWindowLabel: "main-test",
      sessionId: "session-1",
      host: "pi.local",
      port: 5900,
      fingerprint: "key",
      keyBits: 1024,
      knownHostStatus: "unknown",
    };
    const second = { requestId: "vnc-2", targetWindowLabel: "main-test" };
    await receive("vnc-server-key-verify", {
      ...first,
      targetWindowLabel: "main-other",
    });
    expect(requests).toEqual([]);
    await receive("vnc-server-key-verify", first);
    await receive("vnc-server-key-verify", first);
    await receive("vnc-server-key-verify", second);
    expect(requests).toEqual([first, second]);
    await receive("vnc-server-key-verify-resolved", { requestId: "vnc-1" });
    expect(requests).toEqual([second]);
    await receive("vnc-server-key-verify-resolved", { requestId: "vnc-2" });
    expect(requests).toEqual([]);
  });

  it("reloads settings through the existing command", async () => {
    const settings = { ui: { language: "zh-CN" } };
    mocks.invoke.mockResolvedValue(settings);
    const { options } = renderEvents();
    await receive("settings-changed", { stale: "payload" });
    expect(mocks.invoke).toHaveBeenCalledExactlyOnceWith("get_app_settings");
    expect(options.replaceAppSettings).toHaveBeenCalledWith(settings);
  });

  it("uses the current owner window label when routing events", async () => {
    const { options } = renderEvents();
    setOwnerMainWindowLabel("main-updated");
    await receive("session-connect-after-edit", {
      connectionId: "conn-1",
      targetWindowLabel: "main-test",
    });
    expect(options.handleConnectAfterEdit).not.toHaveBeenCalled();
    const payload = {
      connectionId: "conn-1",
      targetWindowLabel: "main-updated",
      sourceTabId: "tab-1",
      sourcePaneId: "pane-1",
    };
    await receive("session-connect-after-edit", payload);
    expect(options.handleConnectAfterEdit).toHaveBeenCalledExactlyOnceWith(payload);
  });

  it("preserves certificate arrival order while deduplicating request IDs", async () => {
    const { result } = renderEvents();
    const certificate: RdpCertificateVerifyRequest = {
      requestId: "cert-1",
      sessionId: "rdp-1",
      host: "host",
      port: 3389,
      fingerprint: "fingerprint",
      knownHostStatus: "unknown",
      targetWindowLabel: "main-test",
    };
    await receive("rdp-certificate-verify", certificate);
    await receive("rdp-certificate-verify", { ...certificate, fingerprint: "duplicate" });
    const second = { ...certificate, requestId: "cert-2" };
    await receive("rdp-certificate-verify", second);
    expect(result.current).toEqual([certificate, second]);
  });

  it("forwards Docker authentication and backend transfer prompts", async () => {
    const { options } = renderEvents();
    const docker = {
      requestId: "sudo-1",
      sessionId: "ssh-1",
      sessionName: "Host",
      targetWindowLabel: "main-test",
    };
    await receive("docker-sudo-password-request", docker);
    expect(options.setDockerSudoPasswordRequest).toHaveBeenCalledWith(docker);
    const transfer = {
      requestId: "transfer-1",
      sessionId: "ssh-1",
      remotePath: "/notes",
      fileName: "notes",
      isDirectory: false,
      targetWindowLabel: "main-test",
    };
    await receive("transfer-duplicate-request", transfer);
    expect(mocks.setBackendTransferDuplicatePrompt).toHaveBeenCalledWith({
      ...transfer,
      respondViaBackend: true,
    });
  });

  it("deduplicates cloud conflicts across subscription updates", async () => {
    const { options, rerender } = renderEvents();
    await receive("cloud-sync-conflict", null);
    expect(mocks.toastError).not.toHaveBeenCalled();
    const conflict = { remote_revision: "revision-1", message: "Conflict" };
    await receive("cloud-sync-conflict", conflict);
    expect(options.handleOpenPanel).toHaveBeenCalledWith("syncBackupHistory");
    const nextOpenPanel = vi.fn();
    rerender({ ...options, handleOpenPanel: nextOpenPanel });
    await receive("cloud-sync-conflict", conflict);
    expect(mocks.toastError).toHaveBeenCalledTimes(1);
    expect(nextOpenPanel).not.toHaveBeenCalled();
    await receive("cloud-sync-conflict", {
      remote_revision: "revision-2",
      message: "New conflict",
    });
    expect(mocks.toastError).toHaveBeenCalledTimes(2);
    expect(nextOpenPanel).toHaveBeenCalledWith("syncBackupHistory");
  });

  it("cleans up every delayed registration exactly once under StrictMode", async () => {
    const registrations: Array<{
      resolve: (dispose: () => void) => void;
      dispose: () => void;
    }> = [];
    mocks.listen.mockImplementation(
      () =>
        new Promise<() => void>((resolve) => {
          registrations.push({ resolve, dispose: vi.fn() });
        }),
    );
    const options = eventOptions();
    const { unmount } = renderHook(() => useAppWindowEvents(options), { wrapper: StrictMode });
    expect(registrations).toHaveLength(38);
    unmount();
    await act(async () => {
      for (const registration of registrations) registration.resolve(registration.dispose);
    });
    for (const registration of registrations) expect(registration.dispose).toHaveBeenCalledOnce();
  });
});
