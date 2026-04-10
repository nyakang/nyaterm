import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { OtpCodePanel } from "@/components/panel/OtpCodePanel";
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

export interface OtpPrompt {
  prompt: string;
  echo: boolean;
}

export interface OtpRequest {
  requestId: string;
  connectionName: string;
  prompts: OtpPrompt[];
  otpEntryId?: string;
}

interface OtpDialogProps {
  request: OtpRequest | null;
  onDone: () => void;
}

export function OtpDialog({ request, onDone }: OtpDialogProps) {
  const { t } = useTranslation();
  const [responses, setResponses] = useState<string[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const firstInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (request) {
      setResponses(request.prompts.map(() => ""));
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
    } catch {
      /* backend handles the error */
    }
    onDone();
  };

  const handleCancel = async () => {
    if (!request) return;
    try {
      await invoke("cancel_otp_request", { requestId: request.requestId });
    } catch {
      /* ignore */
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
  };

  return (
    <Dialog
      open={!!request}
      onOpenChange={(open) => {
        if (!open) void handleCancel();
      }}
    >
      <DialogContent className="max-w-sm" onKeyDown={handleKeyDown}>
        <DialogHeader>
          <DialogTitle className="text-sm">{t("otp.title")}</DialogTitle>
          <DialogDescription className="text-xs">
            {t("otp.description", { name: request?.connectionName })}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-3 py-2">
          {request?.prompts.map((p, promptIndex) => (
            <div key={`${request.requestId}-${p.prompt}-${p.echo ? "echo" : "masked"}`}>
              <Label className="text-[0.6875rem] text-muted-foreground">
                {p.prompt.replace(/:\s*$/, "")}
              </Label>
              <Input
                ref={promptIndex === 0 ? firstInputRef : undefined}
                className="mt-1 text-xs h-8"
                type={p.echo ? "text" : "password"}
                autoComplete="one-time-code"
                value={responses[promptIndex] ?? ""}
                onChange={(e) => {
                  const next = [...responses];
                  next[promptIndex] = e.target.value;
                  setResponses(next);
                }}
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
