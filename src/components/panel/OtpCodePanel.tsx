import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdCheck, MdContentCopy, MdRefresh } from "react-icons/md";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { useOtpCode } from "@/hooks/useOtpCode";
import type { OtpEntry } from "@/types/global";

interface OtpCodePanelProps {
  className?: string;
  onSendToInput?: (code: string) => void;
  otpEntryId: string;
  otpType?: string;
  period?: number;
  variant?: "dialog" | "list";
}

function formatOtpCode(code: string) {
  return code.match(/.{1,3}/g)?.join(" ") ?? code;
}

function LoadingState({ label }: { label: string }) {
  return (
    <div className="flex min-h-[4.25rem] items-center justify-center gap-2 text-xs text-muted-foreground">
      <MdRefresh className="text-sm animate-spin" />
      <span>{label}</span>
    </div>
  );
}

export function OtpCodePanel({
  className = "",
  onSendToInput,
  otpEntryId,
  otpType,
  period,
  variant = "list",
}: OtpCodePanelProps) {
  const { t } = useTranslation();
  const [entryMeta, setEntryMeta] = useState<Pick<OtpEntry, "otp_type" | "period"> | null>(null);
  const [metaLoading, setMetaLoading] = useState(false);
  const metaRequestRef = useRef(0);

  useEffect(() => {
    if (otpType || !otpEntryId) {
      setEntryMeta(null);
      setMetaLoading(false);
      return;
    }

    const requestId = ++metaRequestRef.current;
    setMetaLoading(true);

    invoke<OtpEntry[]>("get_otp_entries")
      .then((entries) => {
        if (metaRequestRef.current !== requestId) return;
        const match = entries.find((entry) => entry.id === otpEntryId);
        setEntryMeta(match ? { otp_type: match.otp_type, period: match.period } : null);
      })
      .catch(() => {
        if (metaRequestRef.current !== requestId) return;
        setEntryMeta(null);
      })
      .finally(() => {
        if (metaRequestRef.current === requestId) {
          setMetaLoading(false);
        }
      });
  }, [otpEntryId, otpType]);

  const resolvedType = otpType ?? entryMeta?.otp_type;
  const resolvedPeriod = period ?? entryMeta?.period ?? 30;
  const containerClassName = useMemo(
    () =>
      [
        "rounded-md border border-border/70 bg-background/70 px-3 py-3",
        variant === "dialog" ? "space-y-3" : "space-y-2.5",
        className,
      ]
        .filter(Boolean)
        .join(" "),
    [className, variant],
  );

  const {
    code,
    copied,
    copyCode,
    fetchCode,
    hasCode,
    isHotp,
    isTotp,
    loading,
    progressPercent,
    remaining,
  } = useOtpCode({
    autoStart: resolvedType === "totp",
    otpEntryId,
    otpType: resolvedType,
    period: resolvedPeriod,
  });

  if (metaLoading || !resolvedType) {
    return (
      <div className={containerClassName}>
        <LoadingState label={t("otp.loadingCode")} />
      </div>
    );
  }

  const showGenerateButton = isHotp;
  const showProgress = isTotp && hasCode;
  const displayCode = hasCode ? formatOtpCode(code) : "--- ---";
  const statusText = showProgress
    ? t("otp.expiresIn", { seconds: remaining })
    : isHotp
      ? t("otp.hotpHint")
      : t("otp.loadingCode");

  return (
    <div className={containerClassName}>
      <div className="flex items-center justify-between gap-2">
        <Label className="text-[0.6875rem] text-muted-foreground">{t("otp.currentCode")}</Label>
        <span className="rounded-full border border-border/70 px-2 py-0.5 text-[0.5625rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground">
          {resolvedType.toUpperCase()}
        </span>
      </div>

      {loading && !hasCode ? (
        <LoadingState label={t("otp.loadingCode")} />
      ) : isHotp && !hasCode ? (
        <div className="space-y-2">
          <p className="text-xs leading-relaxed text-muted-foreground">{t("otp.hotpHint")}</p>
          <Button
            variant="outline"
            size="sm"
            className="h-8 text-xs"
            onClick={() => void fetchCode()}
            disabled={loading}
          >
            {loading ? (
              <MdRefresh className="mr-1 text-[0.875rem] animate-spin" />
            ) : (
              <MdRefresh className="mr-1 text-[0.875rem]" />
            )}
            {t("otp.generateCode")}
          </Button>
        </div>
      ) : (
        <>
          <div className="flex items-start justify-between gap-3">
            <div className="min-w-0 flex-1">
              <div className="font-mono text-lg font-semibold tracking-[0.3em] text-foreground sm:text-xl">
                {displayCode}
              </div>
              <div className="mt-1 text-[0.6875rem] text-muted-foreground">{statusText}</div>
            </div>

            {showProgress ? (
              <div className="shrink-0 text-right">
                <div className="text-[1.15rem] font-semibold tabular-nums text-primary">
                  {remaining}
                </div>
                <div className="text-[0.625rem] uppercase tracking-[0.16em] text-muted-foreground">
                  s
                </div>
              </div>
            ) : null}
          </div>

          {showProgress ? (
            <div className="h-1.5 overflow-hidden rounded-full bg-muted">
              <div
                className="h-full rounded-full bg-primary transition-all duration-1000 ease-linear"
                style={{ width: `${progressPercent}%` }}
              />
            </div>
          ) : null}

          <div
            className={`flex gap-2 ${variant === "dialog" ? "flex-col sm:flex-row" : "flex-wrap"}`}
          >
            <Button
              variant="outline"
              size="sm"
              className={variant === "dialog" ? "h-8 text-xs" : "h-7 text-xs"}
              onClick={copyCode}
              disabled={!hasCode}
            >
              {copied ? (
                <MdCheck className="mr-1 text-[0.875rem]" />
              ) : (
                <MdContentCopy className="mr-1 text-[0.875rem]" />
              )}
              {copied ? t("otp.copied") : t("otp.copyCode")}
            </Button>

            {showGenerateButton ? (
              <Button
                variant="outline"
                size="sm"
                className={variant === "dialog" ? "h-8 text-xs" : "h-7 text-xs"}
                onClick={() => void fetchCode()}
                disabled={loading}
              >
                {loading ? (
                  <MdRefresh className="mr-1 text-[0.875rem] animate-spin" />
                ) : (
                  <MdRefresh className="mr-1 text-[0.875rem]" />
                )}
                {t("otp.generateCode")}
              </Button>
            ) : null}

            {onSendToInput ? (
              <Button
                size="sm"
                className={variant === "dialog" ? "h-8 text-xs sm:ml-auto" : "h-7 text-xs"}
                onClick={() => onSendToInput(code)}
                disabled={!hasCode}
              >
                {t("otp.sendToInput")}
              </Button>
            ) : null}
          </div>
        </>
      )}
    </div>
  );
}
