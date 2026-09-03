import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import type { McpApprovalRequest } from "@/types/global";

type Decision = "deny" | "allow_once" | "allow_session";

export function McpApprovalHost() {
  const { t } = useTranslation();
  const [requests, setRequests] = useState<McpApprovalRequest[]>([]);
  const current = requests[0] ?? null;

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let unlistenLock: (() => void) | undefined;
    void listen<McpApprovalRequest>("mcp-approval-request", (event) => {
      if (!disposed) setRequests((items) => [...items, event.payload]);
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });
    void listen<{ locked: boolean }>("app-lock-state-changed", (event) => {
      if (!disposed && event.payload.locked) setRequests([]);
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenLock = dispose;
    });
    return () => {
      disposed = true;
      unlisten?.();
      unlistenLock?.();
    };
  }, []);

  const respond = useCallback(
    async (decision: Decision) => {
      const request = requests[0];
      if (!request) return;
      setRequests((items) => items.slice(1));
      try {
        await invoke("respond_external_mcp_approval", {
          requestId: request.requestId,
          decision,
        });
      } catch (error) {
        toast.error(getErrorMessage(error));
      }
    },
    [requests],
  );

  return (
    <AlertDialog
      open={current !== null}
      onOpenChange={(open) => !open && void respond("deny")}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            {t("ai.externalMcpApprovalTitle")}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {t("ai.externalMcpApprovalDesc")}
          </AlertDialogDescription>
        </AlertDialogHeader>
        {current ? (
          <div className="space-y-2 rounded-md border border-border/70 bg-muted/20 p-3 text-sm">
            <div>
              <span className="text-muted-foreground">
                {t("ai.externalMcpClient")}:
              </span>{" "}
              {current.client}
            </div>
            <div>
              <span className="text-muted-foreground">
                {t("ai.externalMcpCapability")}:
              </span>{" "}
              {current.capability}
            </div>
            <div>
              <span className="text-muted-foreground">
                {t("ai.externalMcpTarget")}:
              </span>{" "}
              {current.connectionName ??
                current.connectionId ??
                current.sessionName ??
                current.sessionId ??
                "-"}
            </div>
            <div>
              <span className="text-muted-foreground">
                {t("ai.externalMcpRisk")}:
              </span>{" "}
              {current.risk}
            </div>
            <pre className="max-h-40 overflow-auto whitespace-pre-wrap break-all rounded bg-background p-2 text-xs">
              {current.parameterSummary}
            </pre>
          </div>
        ) : null}
        <AlertDialogFooter>
          <Button variant="outline" onClick={() => void respond("deny")}>
            {t("ai.externalMcpDeny")}
          </Button>
          <Button variant="outline" onClick={() => void respond("allow_once")}>
            {t("ai.externalMcpAllowOnce")}
          </Button>
          <Button onClick={() => void respond("allow_session")}>
            {t("ai.externalMcpAllowSession")}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
