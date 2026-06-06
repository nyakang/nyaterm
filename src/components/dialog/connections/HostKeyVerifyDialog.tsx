import { ShieldAlert, ShieldQuestion } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { invoke } from "@/lib/invoke";
import { logger } from "@/lib/logger";

export interface HostKeyVerifyRequest {
  requestId: string;
  host: string;
  port: number;
  keyType: string;
  fingerprint: string;
  isKeyChanged: boolean;
  targetWindowLabel?: string | null;
}

interface HostKeyVerifyDialogProps {
  request: HostKeyVerifyRequest | null;
  onDone: () => void;
}

export function HostKeyVerifyDialog({ request, onDone }: HostKeyVerifyDialogProps) {
  const { t } = useTranslation();
  const [submitting, setSubmitting] = useState(false);

  const handleAccept = async () => {
    if (!request || submitting) return;
    setSubmitting(true);
    try {
      await invoke("respond_host_key_verify", {
        requestId: request.requestId,
        accepted: true,
      });
      logger.info({
        domain: "security.flow",
        event: "host_key.user_accepted",
        message: "User accepted host key",
        ids: { request_id: request.requestId },
      });
    } catch (error) {
      logger.error({
        domain: "security.flow",
        event: "host_key.accept_failed",
        message: "Failed to send host key acceptance",
        ids: { request_id: request.requestId },
        error,
      });
    }
    setSubmitting(false);
    onDone();
  };

  const handleReject = async () => {
    if (!request) return;
    setSubmitting(true);
    try {
      await invoke("respond_host_key_verify", {
        requestId: request.requestId,
        accepted: false,
      });
      logger.info({
        domain: "security.flow",
        event: "host_key.user_rejected",
        message: "User rejected host key",
        ids: { request_id: request.requestId },
      });
    } catch (error) {
      logger.error({
        domain: "security.flow",
        event: "host_key.reject_failed",
        message: "Failed to send host key rejection",
        ids: { request_id: request.requestId },
        error,
      });
    }
    setSubmitting(false);
    onDone();
  };

  return (
    <Dialog
      open={!!request}
      onOpenChange={(open) => {
        if (!open) void handleReject();
      }}
    >
      <DialogContent className="max-w-sm">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2 text-sm">
            {request?.isKeyChanged ? (
              <ShieldAlert className="h-4 w-4 text-destructive" />
            ) : (
              <ShieldQuestion className="h-4 w-4 text-yellow-500" />
            )}
            {t("settings.hostKeyVerifyTitle")}
          </DialogTitle>
          <DialogDescription className="text-xs">
            {request?.isKeyChanged
              ? t("settings.hostKeyVerifyChanged")
              : t("settings.hostKeyVerifyNew")}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-2 py-2 text-xs overflow-hidden">
          <div className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1">
            <span className="text-muted-foreground">{t("settings.hostKeyVerifyHost")}</span>
            <span className="font-mono truncate" title={`${request?.host}:${request?.port}`}>
              {request?.host}:{request?.port}
            </span>
            <span className="text-muted-foreground">{t("settings.hostKeyVerifyKeyType")}</span>
            <span className="font-mono">{request?.keyType}</span>
            <span className="text-muted-foreground">{t("settings.hostKeyVerifyFingerprint")}</span>
            <span className="break-all font-mono select-all">{request?.fingerprint}</span>
          </div>

          {request?.isKeyChanged && (
            <div className="rounded-md border border-destructive/50 bg-destructive/10 p-2 text-destructive text-[0.6875rem]">
              {t("settings.hostKeyVerifyWarning")}
            </div>
          )}
        </div>

        <DialogFooter className="gap-2 sm:gap-0">
          <Button
            variant="ghost"
            size="sm"
            className="text-xs"
            onClick={() => void handleReject()}
            disabled={submitting}
          >
            {t("settings.hostKeyVerifyReject")}
          </Button>
          <Button
            size="sm"
            className="text-xs"
            variant={request?.isKeyChanged ? "destructive" : "default"}
            onClick={() => void handleAccept()}
            disabled={submitting}
          >
            {t("settings.hostKeyVerifyAccept")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
