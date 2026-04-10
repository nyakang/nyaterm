import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import type { OtpCodeResult } from "@/types/global";

interface UseOtpCodeOptions {
  autoStart?: boolean;
  otpEntryId: string;
  otpType?: string | null;
  period?: number;
}

interface UseOtpCodeResult {
  code: string;
  copied: boolean;
  copyCode: () => void;
  fetchCode: () => Promise<void>;
  hasCode: boolean;
  isHotp: boolean;
  isTotp: boolean;
  loading: boolean;
  progressPercent: number;
  remaining: number;
}

const COPY_FEEDBACK_MS = 2000;
const AUTO_FETCH_RESULT_TTL_MS = 1000;
const pendingAutoRequests = new Map<string, Promise<OtpCodeResult>>();
const recentAutoResults = new Map<string, { expiresAt: number; result: OtpCodeResult }>();

async function invokeOtpCode(otpEntryId: string) {
  return invoke<OtpCodeResult>("generate_otp_code", { id: otpEntryId });
}

async function requestOtpCodeWithAutoCache(otpEntryId: string, cacheKey: string) {
  const now = Date.now();
  const recent = recentAutoResults.get(cacheKey);
  if (recent && recent.expiresAt > now) {
    return recent.result;
  }

  const pending = pendingAutoRequests.get(cacheKey);
  if (pending) {
    return pending;
  }

  const request = invokeOtpCode(otpEntryId)
    .then((result) => {
      recentAutoResults.set(cacheKey, {
        expiresAt: Date.now() + AUTO_FETCH_RESULT_TTL_MS,
        result,
      });
      return result;
    })
    .finally(() => {
      pendingAutoRequests.delete(cacheKey);
    });

  pendingAutoRequests.set(cacheKey, request);
  return request;
}

export function useOtpCode({
  autoStart = true,
  otpEntryId,
  otpType,
  period = 30,
}: UseOtpCodeOptions): UseOtpCodeResult {
  const [code, setCode] = useState("");
  const [copied, setCopied] = useState(false);
  const [loading, setLoading] = useState(false);
  const [remaining, setRemaining] = useState(0);
  const requestRef = useRef(0);
  const copyTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const normalizedType = otpType?.toLowerCase();
  const normalizedPeriod = period > 0 ? period : 30;
  const isHotp = normalizedType === "hotp";
  const isTotp = normalizedType === "totp";
  const resetKey = `${otpEntryId}:${normalizedType ?? ""}:${normalizedPeriod}`;
  const autoRequestKey = isTotp ? resetKey : "";

  const clearCopyTimer = useCallback(() => {
    if (copyTimerRef.current) {
      clearTimeout(copyTimerRef.current);
      copyTimerRef.current = null;
    }
  }, []);

  const fetchCodeInternal = useCallback(
    async (useAutoCache = false) => {
      if (!otpEntryId || !normalizedType) return;

      const requestId = ++requestRef.current;
      setLoading(true);

      try {
        const result =
          useAutoCache && autoRequestKey
            ? await requestOtpCodeWithAutoCache(otpEntryId, autoRequestKey)
            : await invokeOtpCode(otpEntryId);
        if (requestRef.current !== requestId) return;

        setCode(result.code);
        setRemaining(isTotp ? result.remainingSeconds : 0);
      } catch {
        if (requestRef.current !== requestId) return;
        setCode("");
        setRemaining(0);
      } finally {
        if (requestRef.current === requestId) {
          setLoading(false);
        }
      }
    },
    [autoRequestKey, isTotp, normalizedType, otpEntryId],
  );

  const fetchCode = useCallback(async () => {
    await fetchCodeInternal(false);
  }, [fetchCodeInternal]);

  const copyCode = useCallback(() => {
    if (!code) return;

    navigator.clipboard.writeText(code).catch(() => {});
    setCopied(true);
    clearCopyTimer();
    copyTimerRef.current = setTimeout(() => {
      setCopied(false);
      copyTimerRef.current = null;
    }, COPY_FEEDBACK_MS);
  }, [clearCopyTimer, code]);

  useEffect(() => {
    void resetKey;
    requestRef.current += 1;
    setCode("");
    setCopied(false);
    setLoading(false);
    setRemaining(0);
    clearCopyTimer();
  }, [clearCopyTimer, resetKey]);

  useEffect(() => {
    if (!isTotp || !autoStart || loading || code) return;
    void fetchCodeInternal(true);
  }, [autoStart, code, fetchCodeInternal, isTotp, loading]);

  useEffect(() => {
    if (!isTotp || !autoStart || remaining <= 0) return;

    const timer = setTimeout(() => {
      setRemaining((prev) => {
        if (prev <= 1) {
          void fetchCode();
          return 0;
        }
        return prev - 1;
      });
    }, 1000);

    return () => clearTimeout(timer);
  }, [autoStart, fetchCode, isTotp, remaining]);

  useEffect(
    () => () => {
      requestRef.current += 1;
      clearCopyTimer();
    },
    [clearCopyTimer],
  );

  return {
    code,
    copied,
    copyCode,
    fetchCode,
    hasCode: Boolean(code),
    isHotp,
    isTotp,
    loading,
    progressPercent: isTotp ? Math.max(0, Math.min(100, (remaining / normalizedPeriod) * 100)) : 0,
    remaining,
  };
}
