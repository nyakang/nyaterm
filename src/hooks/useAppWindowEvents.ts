import { listen } from "@/lib/backend/api";
import { type Dispatch, type SetStateAction, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import type { HostKeyVerifyRequest } from "@/components/dialog/connections/HostKeyVerifyDialog";
import type { OtpRequest } from "@/components/dialog/connections/OtpDialog";
import type { VncServerKeyVerifyRequest } from "@/components/dialog/connections/VncServerKeyVerifyDialog";
import type { RdpCertificateVerifyRequest } from "@/components/dialog/connections/RdpCertificateVerifyDialog";
import type { SshAgentAuthRequest } from "@/components/dialog/connections/SshAgentAuthDialog";
import type { SshAuthRequest } from "@/components/dialog/connections/SshAuthDialog";
import type { DockerSudoPasswordRequest } from "@/components/dialog/docker/DockerSudoPasswordDialog";
import type { AppContextType } from "@/context/AppContext";
import { focusTerminalSession } from "@/lib/appSessionFactory";
import { invoke } from "@/lib/invoke";
import { insertTabIntoLeaf, type TerminalWindowNode } from "@/lib/tabWindows";
import { setBackendTransferDuplicatePrompt } from "@/lib/transferDuplicatePrompt";
import { updateTmuxState } from "@/lib/tmux/store";
import type { TmuxSessionState } from "@/lib/tmux/types";
import { eventTargetsCurrentWindow } from "@/lib/windowManager";
import type { AppSettings, CloudConflictPreview, WorkspaceSessionType } from "@/types/global";
import type { SessionConnectAfterEditPayload } from "./useConnectAfterEdit";
import type { useSecurityPromptQueue } from "./useSecurityPromptQueue";

interface AppWindowEventsOptions
  extends Pick<AppContextType, "addTab" | "applyTmuxState" | "replaceAppSettings">,
    Pick<
      ReturnType<typeof useSecurityPromptQueue>,
      "queueSecurityPrompt" | "removeSecurityPrompt"
    > {
  setTerminalWindows: Dispatch<SetStateAction<TerminalWindowNode | null>>;
  setDockerSudoPasswordRequest: Dispatch<SetStateAction<DockerSudoPasswordRequest | null>>;
  setVncServerKeyRequests: Dispatch<SetStateAction<VncServerKeyVerifyRequest[]>>;
  setRdpCertificateRequests: Dispatch<SetStateAction<RdpCertificateVerifyRequest[]>>;
  handleConnectAfterEdit: (payload: SessionConnectAfterEditPayload) => Promise<void>;
  handleOpenPanel: (panelId: "syncBackupHistory") => void;
}

export function useAppWindowEvents({
  addTab,
  applyTmuxState,
  replaceAppSettings,
  setTerminalWindows,
  queueSecurityPrompt,
  removeSecurityPrompt,
  setDockerSudoPasswordRequest,
  setRdpCertificateRequests,
  setVncServerKeyRequests,
  handleConnectAfterEdit,
  handleOpenPanel,
}: AppWindowEventsOptions) {
  const { t } = useTranslation();
  const lastCloudConflictRevisionRef = useRef<string | null>(null);

  // Cross-window event listeners
  useEffect(() => {
    const unsubs: Promise<() => void>[] = [];

    unsubs.push(
      listen<AppSettings>("settings-changed", () => {
        invoke<AppSettings>("get_app_settings").then((cfg) => {
          replaceAppSettings(cfg);
        });
      }),
    );

    unsubs.push(
      listen<{
        sessionId: string;
        name: string;
        type: WorkspaceSessionType;
        targetLeafId?: string;
        anchorTabId?: string | null;
        targetWindowLabel?: string | null;
      }>("session-created", (event) => {
        const {
          sessionId,
          name: sessionName,
          type,
          targetLeafId,
          anchorTabId,
          targetWindowLabel,
        } = event.payload;
        if (!eventTargetsCurrentWindow(targetWindowLabel)) return;
        const tabId = addTab(
          sessionId,
          sessionName,
          type,
          undefined,
          undefined,
          anchorTabId ? { afterTabId: anchorTabId } : undefined,
        );
        if (targetLeafId) {
          setTerminalWindows((current) =>
            current
              ? insertTabIntoLeaf(current, targetLeafId, tabId, {
                  afterTabId: anchorTabId,
                  activeTabId: tabId,
                })
              : current,
          );
        }
        focusTerminalSession(sessionId);
      }),
    );

    unsubs.push(
      listen<OtpRequest>("otp-request", (event) => {
        if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel)) return;
        queueSecurityPrompt({
          kind: "otp",
          request: event.payload,
        });
      }),
    );

    unsubs.push(
      listen<SshAuthRequest>("ssh-auth-request", (event) => {
        if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel)) return;
        queueSecurityPrompt({
          kind: "ssh-auth",
          request: event.payload,
        });
      }),
    );

    unsubs.push(
      listen<SshAgentAuthRequest>("ssh-agent-auth-pending", (event) => {
        if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel)) return;
        queueSecurityPrompt({
          kind: "ssh-agent",
          request: event.payload,
        });
      }),
    );
    unsubs.push(
      listen<SshAgentAuthRequest>("ssh-agent-auth-failed", (event) => {
        if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel)) return;
        queueSecurityPrompt({
          kind: "ssh-agent",
          request: event.payload,
        });
      }),
    );
    unsubs.push(
      listen<{ requestId: string }>("ssh-agent-auth-resolved", (event) => {
        removeSecurityPrompt(event.payload.requestId);
      }),
    );
    unsubs.push(
      listen<{ requestId: string }>("host-key-verify-resolved", (event) => {
        removeSecurityPrompt(event.payload.requestId);
      }),
    );
    unsubs.push(
      listen<{ requestId: string }>("security-prompt-resolved", (event) => {
        removeSecurityPrompt(event.payload.requestId);
      }),
    );

    unsubs.push(
      listen<DockerSudoPasswordRequest>("docker-sudo-password-request", (event) => {
        if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel)) return;
        setDockerSudoPasswordRequest(event.payload);
      }),
    );

    unsubs.push(
      listen<HostKeyVerifyRequest>("host-key-verify", (event) => {
        if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel)) return;
        queueSecurityPrompt({
          kind: "host-key",
          request: event.payload,
        });
      }),
    );

    unsubs.push(
      listen<RdpCertificateVerifyRequest>("rdp-certificate-verify", (event) => {
        if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel)) return;
        setRdpCertificateRequests((current) => {
          if (current.some((item) => item.requestId === event.payload.requestId)) return current;
          return [...current, event.payload];
        });
      }),
    );

    unsubs.push(
      listen<{ requestId: string }>("vnc-server-key-verify-resolved", (event) => {
        setVncServerKeyRequests((current) =>
          current.filter((item) => item.requestId !== event.payload.requestId),
        );
      }),
    );

    unsubs.push(
      listen<VncServerKeyVerifyRequest>("vnc-server-key-verify", (event) => {
        if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel)) return;
        setVncServerKeyRequests((current) => {
          if (current.some((item) => item.requestId === event.payload.requestId)) return current;
          return [...current, event.payload];
        });
      }),
    );

    unsubs.push(
      listen<{
        requestId: string;
        sessionId: string;
        remotePath: string;
        fileName: string;
        isDirectory: boolean;
        targetWindowLabel?: string | null;
      }>("transfer-duplicate-request", (event) => {
        if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel)) return;
        setBackendTransferDuplicatePrompt({
          requestId: event.payload.requestId,
          sessionId: event.payload.sessionId,
          remotePath: event.payload.remotePath,
          fileName: event.payload.fileName,
          isDirectory: event.payload.isDirectory,
          targetWindowLabel: event.payload.targetWindowLabel,
          respondViaBackend: true,
        });
      }),
    );

    unsubs.push(
      listen<SessionConnectAfterEditPayload>("session-connect-after-edit", (event) => {
        if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel)) return;
        return handleConnectAfterEdit(event.payload);
      }),
    );

    unsubs.push(
      listen<TmuxSessionState>("tmux-session-state", (event) => {
        updateTmuxState(event.payload);
        applyTmuxState(event.payload);
      }),
    );

    unsubs.push(
      listen<{ message?: string }>("tmux-command-error", (event) => {
        const message = event.payload?.message;
        if (message) toast.error(t("tmux.commandError", { message }));
      }),
    );

    return () => {
      unsubs.forEach((p) => {
        p.then((unsub) => unsub());
      });
    };
  }, [
    addTab,
    applyTmuxState,
    replaceAppSettings,
    t,
    setTerminalWindows,
    queueSecurityPrompt,
    removeSecurityPrompt,
    setDockerSudoPasswordRequest,
    setRdpCertificateRequests,
    setVncServerKeyRequests,
    handleConnectAfterEdit,
  ]);

  useEffect(() => {
    const unlisten = listen<CloudConflictPreview | null>("cloud-sync-conflict", (event) => {
      const conflict = event.payload;
      if (!conflict) return;
      if (lastCloudConflictRevisionRef.current === conflict.remote_revision) {
        return;
      }

      lastCloudConflictRevisionRef.current = conflict.remote_revision;
      toast.error(conflict.message);
      handleOpenPanel("syncBackupHistory");
    });

    return () => {
      unlisten.then((dispose) => dispose());
    };
  }, [handleOpenPanel]);
}
