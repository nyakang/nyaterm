import { useState } from "react";
import { useTranslation } from "react-i18next";
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
import { SelectItem } from "@/components/ui/select";
import type { AIPermissionMode } from "@/types/global";
import { SettingSelect } from "./SettingFormItems";

export const AI_PERMISSION_MODES = [
  "observer",
  "confirm",
  "auto",
  "full_access",
] as const satisfies readonly AIPermissionMode[];

const PERMISSION_MODE_LABEL_KEYS: Record<AIPermissionMode, string> = {
  observer: "ai.permissionObserver",
  confirm: "ai.permissionConfirm",
  auto: "ai.permissionAuto",
  full_access: "ai.permissionFullAccess",
};

const PERMISSION_MODE_DESCRIPTION_KEYS: Record<AIPermissionMode, string> = {
  observer: "ai.permissionObserverDesc",
  confirm: "ai.permissionConfirmDesc",
  auto: "ai.permissionAutoDesc",
  full_access: "ai.permissionFullAccessDesc",
};

export function requiresFullAccessConfirmation(current: AIPermissionMode, next: AIPermissionMode) {
  return current !== "full_access" && next === "full_access";
}

interface AiPermissionSelectProps {
  value: AIPermissionMode;
  targetLabel: string;
  onValueChange: (value: AIPermissionMode) => void;
}

export function AiPermissionSelect({ value, targetLabel, onValueChange }: AiPermissionSelectProps) {
  const { t } = useTranslation();
  const [confirmingFullAccess, setConfirmingFullAccess] = useState(false);

  const handleValueChange = (nextValue: string) => {
    const next = nextValue as AIPermissionMode;
    if (requiresFullAccessConfirmation(value, next)) {
      setConfirmingFullAccess(true);
      return;
    }
    onValueChange(next);
  };

  const enableFullAccess = () => {
    setConfirmingFullAccess(false);
    onValueChange("full_access");
  };

  return (
    <>
      <SettingSelect
        label={t("ai.permissionMode")}
        desc={t(PERMISSION_MODE_DESCRIPTION_KEYS[value])}
        value={value}
        onValueChange={handleValueChange}
      >
        {AI_PERMISSION_MODES.map((mode) => (
          <SelectItem key={mode} value={mode}>
            {t(PERMISSION_MODE_LABEL_KEYS[mode])}
          </SelectItem>
        ))}
      </SettingSelect>

      <AlertDialog open={confirmingFullAccess} onOpenChange={setConfirmingFullAccess}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>{t("ai.fullAccessConfirmTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("ai.fullAccessConfirmDesc", { target: targetLabel })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction variant="destructive" onClick={enableFullAccess}>
              {t("ai.enableFullAccess")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
