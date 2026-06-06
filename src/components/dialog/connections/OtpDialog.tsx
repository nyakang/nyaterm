import { useCallback, useEffect, useRef, useState } from "react";
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
import { InputOTP, InputOTPGroup, InputOTPSlot } from "@/components/ui/input-otp";
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
  prompts: OtpPrompt[];
  otpEntryId?: string;
  targetWindowLabel?: string | null;
}

interface OtpDialogProps {
  request: OtpRequest | null;
  onDone: () => void;
}

function getOtpSlotClassName(length: number) {
  if (length >= 9) return "h-8 flex-1 basis-0 min-w-0 text-xs";
  if (length >= 7) return "h-8 w-8 text-xs sm:h-9 sm:w-9 sm:text-sm";
  return undefined;
}

export function OtpDialog({ request, onDone }: OtpDialogProps) {
  const { t } = useTranslation();
  const [responses, setResponses] = useState<string[]>([]);
  const [codeLengths, setCodeLengths] = useState<number[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const firstInputRef = useRef<HTMLInputElement>(null);

  const handleCodeChange = useCallback((code: string) => {
    setCodeLengths((prev) => {
      if (prev[0] === code.length) return prev;
      const next = [...prev];
      next[0] = code.length;
      return next;
    });
  }, []);

  useEffect(() => {
    if (request) {
      setResponses(request.prompts.map(() => ""));
      setCodeLengths(request.prompts.map(() => 6));
      setSubmitting(false);
    }
  }, [request]);

  useEffect(() => {
    if (request) {
      const timer = setTimeout(() => firstInputRef.current?.focus(), 100);
      return () => clearTimeout(timer);
    }
  }, [request]);

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
    onDone();
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
    onDone();
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

    setCodeLengths((prev) => {
      if ((prev[0] ?? 0) >= code.length) return prev;
      const nextLengths = [...prev];
      nextLengths[0] = code.length;
      return nextLengths;
    });
  };

  return (
    <Dialog
      open={!!request}
      onOpenChange={(open) => {
        if (!open) void handleCancel();
      }}
    >
      <DialogContent className="max-w-sm overflow-x-hidden" onKeyDown={handleKeyDown}>
        <DialogHeader>
          <DialogTitle className="text-sm">{t("otp.title")}</DialogTitle>
          <DialogDescription className="text-xs">
            {t("otp.description", { name: request?.connectionName })}
          </DialogDescription>
        </DialogHeader>

        <div className="min-w-0 space-y-3 py-2">
          {request?.prompts.map((p, promptIndex) => {
            const codeLength = codeLengths[promptIndex] ?? 6;
            const slotClassName = getOtpSlotClassName(codeLength);

            return (
              <div
                key={`${request.requestId}-${p.prompt}-${p.echo ? "echo" : "masked"}`}
                className="min-w-0"
              >
                <Label className="text-[0.6875rem] text-muted-foreground">
                  {p.prompt.replace(/:\s*$/, "")}
                </Label>
                <InputOTP
                  ref={promptIndex === 0 ? firstInputRef : undefined}
                  maxLength={codeLength}
                  value={responses[promptIndex] ?? ""}
                  onChange={(val) => {
                    const next = [...responses];
                    next[promptIndex] = val;
                    setResponses(next);
                  }}
                  containerClassName="mt-1 w-full min-w-0"
                >
                  <InputOTPGroup className={codeLength >= 9 ? "w-full" : undefined}>
                    {Array.from({ length: codeLength }, (_, index) => ({
                      index,
                      key: `slot-${index}`,
                    })).map((slot) => (
                      <InputOTPSlot key={slot.key} index={slot.index} className={slotClassName} />
                    ))}
                  </InputOTPGroup>
                </InputOTP>
              </div>
            );
          })}

          {request?.otpEntryId && (
            <OtpCodePanel
              otpEntryId={request.otpEntryId}
              onSendToInput={handleSendToInput}
              onCodeChange={handleCodeChange}
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
