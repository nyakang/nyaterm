import { Eye, EyeOff, Shield } from "lucide-react";
import { useEffect, useRef, useState } from "react";
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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { invoke } from "@/lib/invoke";
import { logger } from "@/lib/logger";

export interface DockerSudoPasswordRequest {
  requestId: string;
  sessionId: string;
  sessionName: string;
  targetWindowLabel?: string | null;
}

interface DockerSudoPasswordDialogProps {
  request: DockerSudoPasswordRequest | null;
  onDone: (requestId: string) => void;
}

export default function DockerSudoPasswordDialog({
  request,
  onDone,
}: DockerSudoPasswordDialogProps) {
  const { t } = useTranslation();
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!request) return;
    setPassword("");
    setShowPassword(false);
    setSubmitting(false);
    const timer = window.setTimeout(() => inputRef.current?.focus(), 100);
    return () => window.clearTimeout(timer);
  }, [request]);

  const handleSubmit = async () => {
    if (!request || !password || submitting) return;
    setSubmitting(true);
    try {
      await invoke("submit_docker_sudo_password", {
        requestId: request.requestId,
        password,
      });
    } catch (error) {
      logger.error({
        domain: "security.flow",
        event: "docker_sudo.response_submit_failed",
        message: "Failed to submit Docker sudo password",
        ids: { request_id: request.requestId, session_id: request.sessionId },
        error,
      });
    }
    onDone(request.requestId);
  };

  const handleCancel = async () => {
    if (!request) return;
    try {
      await invoke("cancel_docker_sudo_password", { requestId: request.requestId });
    } catch (error) {
      logger.error({
        domain: "security.flow",
        event: "docker_sudo.request_cancel_failed",
        message: "Failed to cancel Docker sudo password request",
        ids: { request_id: request.requestId, session_id: request.sessionId },
        error,
      });
    }
    onDone(request.requestId);
  };

  return (
    <Dialog
      disablePointerDismissal
      open={!!request}
      onOpenChange={(open) => {
        if (!open) void handleCancel();
      }}
    >
      <DialogContent
        className="w-[min(24rem,calc(100vw-2rem))] max-w-none"
        onKeyDown={(event) => {
          if (event.key === "Enter" && password) void handleSubmit();
        }}
      >
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2 text-sm">
            <Shield className="h-4 w-4" />
            {t("dockerSudo.title")}
          </DialogTitle>
          <DialogDescription className="text-xs">
            {t("dockerSudo.description", { name: request?.sessionName ?? "" })}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-2 py-2">
          <Label className="text-xs">{t("dockerSudo.password")}</Label>
          <div className="relative">
            <Input
              ref={inputRef}
              type={showPassword ? "text" : "password"}
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              autoComplete="off"
              autoCapitalize="none"
              spellCheck={false}
              className="h-9 pr-9 text-sm"
            />
            <button
              type="button"
              className="absolute right-2 top-1/2 -translate-y-1/2 rounded-sm p-1 text-muted-foreground hover:text-foreground"
              onClick={() => setShowPassword((value) => !value)}
              title={showPassword ? t("dialog.hidePassword") : t("dialog.showPassword")}
            >
              {showPassword ? <EyeOff className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}
            </button>
          </div>
          <div className="text-xs leading-relaxed text-muted-foreground">
            {t("dockerSudo.hint")}
          </div>
        </div>

        <DialogFooter className="gap-2 sm:gap-0">
          <Button
            variant="ghost"
            size="sm"
            className="text-xs"
            onClick={() => void handleCancel()}
            disabled={submitting}
          >
            {t("common.cancel")}
          </Button>
          <Button
            size="sm"
            className="text-xs"
            onClick={() => void handleSubmit()}
            disabled={!password || submitting}
          >
            {t("dockerSudo.submit")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
