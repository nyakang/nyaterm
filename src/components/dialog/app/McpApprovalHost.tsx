import { listen } from "@/lib/backend/api";
import { runtime } from "@/lib/backend/runtime";
import { useCallback, useEffect, useRef, useState } from "react";
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
import { logger } from "@/lib/logger";
import type { McpApprovalRequest } from "@/types/global";

type Decision = "deny" | "allow_once" | "allow_session";

const NOTIFICATION_ACTION_TYPE_ID = "mcp-approval";

export function McpApprovalHost() {
  const { t, i18n } = useTranslation();
  const [requests, setRequests] = useState<McpApprovalRequest[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const respondingRef = useRef(false);
  const requestsRef = useRef(requests);
  const resolvedIdsRef = useRef(new Set<string>());
  const suppressDismissRef = useRef(false);
  requestsRef.current = requests;
  const current = requests[0] ?? null;

  useEffect(() => {
    if (runtime !== "desktop") return;
    let cancelled = false;
    void import("@tauri-apps/plugin-notification")
      .then(({ registerActionTypes }) => {
        if (cancelled) return;
        return registerActionTypes([
          {
            id: NOTIFICATION_ACTION_TYPE_ID,
            actions: [
              { id: "open", title: t("ai.externalMcpNotificationOpen") },
              { id: "allow_once", title: t("ai.externalMcpAllowOnce") },
              { id: "deny", title: t("ai.externalMcpDeny") },
            ],
          },
        ]);
      })
      .catch((error) => {
        logger.warn({
          domain: "app.lifecycle",
          event: "mcp_approval.notification_actions",
          message: "Failed to register MCP approval notification actions",
          data: { title: t("ai.externalMcpNotificationTitle") },
          error,
        });
      });
    return () => {
      cancelled = true;
    };
  }, [i18n.language, t]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let unlistenResolved: (() => void) | undefined;
    let unlistenLock: (() => void) | undefined;
    void listen<McpApprovalRequest>("mcp-approval-request", (event) => {
      if (!disposed) setRequests((items) => [...items, event.payload]);
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });
    void listen<{ requestId: string; decision: string }>("mcp-approval-resolved", (event) => {
      if (disposed) return;
      const requestId = event.payload.requestId;
      resolvedIdsRef.current.add(requestId);
      setRequests((items) => {
        const next = items.filter((item) => item.requestId !== requestId);
        if (items[0]?.requestId === requestId && next.length === 0) {
          suppressDismissRef.current = true;
        }
        return next;
      });
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenResolved = dispose;
    });
    void listen<{ locked: boolean }>("app-lock-state-changed", (event) => {
      if (!disposed && event.payload.locked) {
        resolvedIdsRef.current.clear();
        suppressDismissRef.current = false;
        setRequests([]);
      }
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenLock = dispose;
    });
    return () => {
      disposed = true;
      unlisten?.();
      unlistenResolved?.();
      unlistenLock?.();
    };
  }, []);

  const respond = useCallback(
    async (decision: Decision) => {
      const request = requests[0];
      if (!request || respondingRef.current) return;
      respondingRef.current = true;
      setSubmitting(true);
      try {
        await invoke("respond_external_mcp_approval", {
          requestId: request.requestId,
          decision,
        });
      } catch (error) {
        toast.error(getErrorMessage(error));
      } finally {
        setRequests((items) => items.filter((item) => item.requestId !== request.requestId));
        respondingRef.current = false;
        setSubmitting(false);
      }
    },
    [requests],
  );

  return (
    <AlertDialog
      open={current !== null}
      onOpenChange={(open) => {
        if (open) return;
        if (suppressDismissRef.current) {
          suppressDismissRef.current = false;
          return;
        }
        const requestId = requestsRef.current[0]?.requestId;
        if (!requestId || resolvedIdsRef.current.has(requestId)) return;
        void respond("deny");
      }}
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
          <Button variant="outline" disabled={submitting} onClick={() => void respond("deny")}>
            {t("ai.externalMcpDeny")}
          </Button>
          <Button variant="outline" disabled={submitting} onClick={() => void respond("allow_once")}>
            {t("ai.externalMcpAllowOnce")}
          </Button>
          <Button disabled={submitting} onClick={() => void respond("allow_session")}>
            {t("ai.externalMcpAllowSession")}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
