import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { OtpCodePanel } from "@/components/panel/security-auth/OtpCodePanel";
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

export interface OtpPrompt {
  prompt: string;
  echo: boolean;
}

export interface OtpRequest {
  requestId: string;
  connectionName: string;
  name?: string | null;
  instructions?: string | null;
  round?: number;
  prompts: OtpPrompt[];
  otpEntryId?: string;
  targetWindowLabel?: string | null;
}

interface OtpDialogProps {
  request: OtpRequest | null;
  onDone: (requestId: string) => void;
}

export function OtpDialog({ request, onDone }: OtpDialogProps) {
  const { t } = useTranslation();
  const [responses, setResponses] = useState<string[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const firstInputRef = useRef<HTMLInputElement>(null);
  const requestId = request?.requestId;
  const prompts = request?.prompts;
  const challengeTitle = request?.name?.trim();
  const challengeInstructions = request?.instructions?.trim();

  useEffect(() => {
    if (prompts) {
      setResponses(prompts.map(() => ""));
      setSubmitting(false);
    }
  }, [prompts]);

  useEffect(() => {
    if (requestId) {
      const timer = setTimeout(() => firstInputRef.current?.focus(), 100);
      return () => clearTimeout(timer);
    }
  }, [requestId]);

  const handleSubmit = async () => {
    if (!request || submitting) return;
    setSubmitting(true);
    try {
      await invoke("submit_otp_response", {
        requestId: request.requestId,
        responses,
      });
      logger.info({
        domain: "security.flow",
        event: "otp.response_submitted",
        message: "Submitted OTP response",
        ids: { request_id: request.requestId },
        data: { prompt_count: responses.length },
      });
    } catch (error) {
      logger.error({
        domain: "security.flow",
        event: "otp.response_submit_failed",
        message: "Failed to submit OTP response",
        ids: { request_id: request.requestId },
        error,
      });
    }
    onDone(request.requestId);
  };

  const handleCancel = async () => {
    if (!request) return;
    try {
      await invoke("cancel_otp_request", { requestId: request.requestId });
      logger.info({
        domain: "security.flow",
        event: "otp.request_cancelled",
        message: "Cancelled OTP request",
        ids: { request_id: request.requestId },
      });
    } catch (error) {
      logger.error({
        domain: "security.flow",
        event: "otp.request_cancel_failed",
        message: "Failed to cancel OTP request",
        ids: { request_id: request.requestId },
        error,
      });
    }
    onDone(request.requestId);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && !submitting) {
      void handleSubmit();
    }
  };

  const handleSendToInput = (code: string) => {
    if (!request) return;
    const next = [...responses];
    next[0] = code;
    setResponses(next);
  };

  return (
    <Dialog
      disablePointerDismissal
      open={!!request}
      onOpenChange={(open) => {
        if (!open) void handleCancel();
      }}
    >
      <DialogContent className="max-w-sm overflow-x-hidden" onKeyDown={handleKeyDown}>
        <DialogHeader>
          <DialogTitle className="text-sm">
            {request?.round && request.round > 1
              ? t("otp.titleWithRound", { round: request.round })
              : t("otp.title")}
          </DialogTitle>
          <DialogDescription className="text-xs">
            {t("otp.description", { name: request?.connectionName })}
          </DialogDescription>
        </DialogHeader>

        <div className="min-w-0 space-y-3 py-2">
          {(challengeTitle || challengeInstructions) && (
            <div className="min-w-0 rounded-md border bg-muted/30 px-3 py-2">
              {challengeTitle && (
                <div className="break-words text-xs font-medium text-foreground">
                  {challengeTitle}
                </div>
              )}
              {challengeInstructions && (
                <div className="mt-1 whitespace-pre-wrap break-words text-xs leading-relaxed text-muted-foreground">
                  {challengeInstructions}
                </div>
              )}
            </div>
          )}

          {request?.prompts.map((p, promptIndex) => (
            <div
              key={`${request.requestId}-${p.prompt}-${p.echo ? "echo" : "masked"}`}
              className="min-w-0"
            >
              <Label className="text-[0.6875rem] text-muted-foreground">
                {p.prompt.replace(/:\s*$/, "")}
              </Label>
              <Input
                ref={promptIndex === 0 ? firstInputRef : undefined}
                type={p.echo ? "text" : "password"}
                value={responses[promptIndex] ?? ""}
                onChange={(event) => {
                  const next = [...responses];
                  next[promptIndex] = event.target.value;
                  setResponses(next);
                }}
                autoComplete="off"
                autoCapitalize="none"
                spellCheck={false}
                className="mt-1 h-9 text-sm"
              />
            </div>
          ))}

          {request?.otpEntryId && (
            <OtpCodePanel
              otpEntryId={request.otpEntryId}
              onSendToInput={handleSendToInput}
              variant="dialog"
            />
          )}
        </div>

        <DialogFooter className="gap-2 sm:gap-0">
          <Button
            variant="ghost"
            size="sm"
            className="text-xs"
            onClick={() => void handleCancel()}
            disabled={submitting}
          >
            {t("otp.cancel")}
          </Button>
          <Button
            size="sm"
            className="text-xs"
            onClick={() => void handleSubmit()}
            disabled={submitting}
          >
            {t("otp.submit")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
