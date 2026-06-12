import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdLock, MdLockOpen } from "react-icons/md";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
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
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useApp } from "@/context/AppContext";
import { invoke } from "@/lib/invoke";
import { openSettings } from "@/lib/windowManager";

interface SecretUnlockFooterProps {
  unlocked: boolean;
  onLock: () => void;
  onUnlocked: () => void;
  showTrigger?: boolean;
  unlockRequestNonce?: number;
}

export function SecretUnlockFooter({
  unlocked,
  onLock,
  onUnlocked,
  showTrigger = true,
  unlockRequestNonce = 0,
}: SecretUnlockFooterProps) {
  const { t } = useTranslation();
  const { appSettings } = useApp();
  const inputRef = useRef<HTMLInputElement | null>(null);
  const handledUnlockRequestNonceRef = useRef(unlockRequestNonce);
  const [unlockOpen, setUnlockOpen] = useState(false);
  const [masterAlertOpen, setMasterAlertOpen] = useState(false);
  const [password, setPassword] = useState("");
  const [error, setError] = useState(false);
  const [verifying, setVerifying] = useState(false);

  const hasMasterPassword = Boolean(appSettings.security.master_password);

  const resetUnlockDialog = useCallback(() => {
    setPassword("");
    setError(false);
    setVerifying(false);
  }, []);

  const handleRequestUnlock = useCallback(() => {
    if (!hasMasterPassword) {
      setMasterAlertOpen(true);
      return;
    }
    resetUnlockDialog();
    setUnlockOpen(true);
    window.setTimeout(() => inputRef.current?.focus(), 0);
  }, [hasMasterPassword, resetUnlockDialog]);

  useEffect(() => {
    if (
      !unlockRequestNonce ||
      unlocked ||
      handledUnlockRequestNonceRef.current === unlockRequestNonce
    ) {
      return;
    }

    handledUnlockRequestNonceRef.current = unlockRequestNonce;
    handleRequestUnlock();
  }, [handleRequestUnlock, unlocked, unlockRequestNonce]);

  const handleUnlock = async () => {
    if (!password || verifying) return;
    setVerifying(true);
    try {
      const ok = await invoke<boolean>("verify_master_password", { password });
      if (!ok) {
        setError(true);
        setPassword("");
        inputRef.current?.focus();
        return;
      }
      setUnlockOpen(false);
      resetUnlockDialog();
      onUnlocked();
    } catch {
      setError(true);
      setPassword("");
      inputRef.current?.focus();
    } finally {
      setVerifying(false);
    }
  };

  return (
    <>
      {showTrigger ? (
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              className="group flex h-10 w-full shrink-0 cursor-pointer items-center justify-between gap-3 border-t border-[var(--df-border)] bg-primary/10 px-3 text-xs text-foreground transition-colors hover:bg-primary/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/45"
              onClick={unlocked ? onLock : handleRequestUnlock}
            >
              <span className="flex min-w-0 items-center gap-2">
                <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md border border-primary/25 bg-background/70 text-primary transition-colors group-hover:border-primary/45">
                  {unlocked ? (
                    <MdLock className="h-[0.875rem] w-[0.875rem]" />
                  ) : (
                    <MdLockOpen className="h-[0.875rem] w-[0.875rem]" />
                  )}
                </span>
                <span className="truncate">
                  {unlocked ? t("secretUnlock.unlockedTitle") : t("secretUnlock.lockedTitle")}
                </span>
              </span>
              <span className="shrink-0 font-medium text-primary">
                {unlocked ? t("secretUnlock.lockAction") : t("secretUnlock.unlockAction")}
              </span>
            </button>
          </TooltipTrigger>
          <TooltipContent side="top">
            {unlocked ? t("secretUnlock.unlockedDesc") : t("secretUnlock.lockedDesc")}
          </TooltipContent>
        </Tooltip>
      ) : null}

      <Dialog
        open={unlockOpen}
        onOpenChange={(open) => {
          setUnlockOpen(open);
          if (!open) resetUnlockDialog();
        }}
      >
        <DialogContent showCloseButton={false} className="max-w-xs">
          <DialogHeader>
            <DialogTitle className="text-sm">{t("secretUnlock.unlockTitle")}</DialogTitle>
            <DialogDescription className="sr-only">
              {t("secretUnlock.unlockTitle")}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-2">
            <Input
              ref={inputRef}
              type="password"
              value={password}
              placeholder={t("secretUnlock.masterPasswordPlaceholder")}
              className="h-8 text-xs"
              disabled={verifying}
              onChange={(event) => {
                setPassword(event.target.value);
                setError(false);
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  void handleUnlock();
                }
              }}
            />
            {error ? (
              <div className="text-[0.6875rem] text-destructive">
                {t("secretUnlock.wrongPassword")}
              </div>
            ) : null}
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setUnlockOpen(false)}>
              {t("common.cancel")}
            </Button>
            <Button disabled={!password || verifying} onClick={() => void handleUnlock()}>
              {t("secretUnlock.unlock")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <AlertDialog open={masterAlertOpen} onOpenChange={setMasterAlertOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("settings.masterPasswordRequired")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("settings.masterPasswordRequiredDesc")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                setMasterAlertOpen(false);
                openSettings("security");
              }}
            >
              {t("settings.security")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
