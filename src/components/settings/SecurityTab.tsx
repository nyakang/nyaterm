import { useState } from "react";
import { useTranslation } from "react-i18next";
import { SelectItem } from "@/components/ui/select";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useApp } from "@/context/AppContext";
import { useSettingsDraft } from "@/context/SettingsDraftContext";
import { NumberInput } from "../ui/number-input";
import {
  SettingInput,
  SettingRow,
  SettingSection,
  SettingSelect,
  SettingSwitch,
} from "./SettingFormItems";

export function SecurityTab() {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings } = useApp();
  const { committedSettings, isSaving } = useSettingsDraft();
  const [newPassword, setNewPassword] = useState("");

  const masterPasswordValue = appSettings.security.master_password;
  const hasStoredMasterPassword =
    committedSettings.security.master_password === "__SET__" || masterPasswordValue === "__SET__";
  const masterPasswordEnabled = appSettings.cloud_sync.enabled || masterPasswordValue !== undefined;
  const masterPasswordSwitchDisabled = appSettings.cloud_sync.enabled || isSaving;

  const handlePasswordChange = (val: string) => {
    setNewPassword(val);
    updateAppSettings({
      security: {
        ...appSettings.security,
        master_password: val || (hasStoredMasterPassword ? "__SET__" : ""),
      },
    });
  };

  const handleMasterPasswordEnabledChange = (enabled: boolean) => {
    setNewPassword("");
    updateAppSettings({
      security: {
        ...appSettings.security,
        master_password: enabled ? (hasStoredMasterPassword ? "__SET__" : "") : undefined,
      },
    });
  };

  return (
    <div className="space-y-5">
      <SettingSection title={t("settings.masterPasswordSection")} contentClassName="space-y-5">
        <SettingRow
          label={t("settings.masterPasswordSwitch")}
          desc={t("settings.masterPasswordSwitchDesc")}
        >
          {appSettings.cloud_sync.enabled ? (
            <Tooltip>
              <TooltipTrigger asChild>
                <div className="inline-flex">
                  <SettingSwitch
                    checked={masterPasswordEnabled}
                    disabled={masterPasswordSwitchDisabled}
                    onChange={handleMasterPasswordEnabledChange}
                  />
                </div>
              </TooltipTrigger>
              <TooltipContent side="top">
                {t("settings.masterPasswordLockedByCloudSync")}
              </TooltipContent>
            </Tooltip>
          ) : (
            <SettingSwitch
              checked={masterPasswordEnabled}
              disabled={masterPasswordSwitchDisabled}
              onChange={handleMasterPasswordEnabledChange}
            />
          )}
        </SettingRow>

        {hasStoredMasterPassword && !newPassword ? (
          <div className="text-sm text-muted-foreground">{t("settings.masterPasswordIsSet")}</div>
        ) : null}

        <SettingInput
          label={
            hasStoredMasterPassword ? t("settings.masterPasswordNew") : t("settings.masterPassword")
          }
          desc={t("settings.masterPasswordDesc")}
          type="password"
          controlClassName="max-w-lg"
          placeholder={
            hasStoredMasterPassword
              ? t("settings.masterPasswordNewPlaceholder")
              : t("settings.masterPasswordPlaceholder")
          }
          value={newPassword}
          disabled={!masterPasswordEnabled || isSaving}
          onChange={(e) => handlePasswordChange(e.target.value)}
        />
      </SettingSection>

      <SettingSection title={t("settings.sessionSecurity")} contentClassName="space-y-5">
        <SettingRow
          label={t("settings.enableScreenLock")}
          desc={t("settings.enableScreenLockDesc")}
        >
          <SettingSwitch
            checked={appSettings.security.enable_screen_lock}
            onChange={(v) =>
              updateAppSettings({
                security: { ...appSettings.security, enable_screen_lock: v },
              })
            }
          />
        </SettingRow>

        {appSettings.security.enable_screen_lock && (
          <SettingRow
            label={t("settings.idleLockMinutes")}
            desc={t("settings.idleLockMinutesDesc")}
          >
            <div className="flex w-full max-w-xs items-center gap-3 sm:w-auto">
              <NumberInput
                min={0}
                max={1440}
                className="w-full sm:w-32"
                value={appSettings.security.idle_lock_minutes}
                onChange={(v) =>
                  updateAppSettings({
                    security: { ...appSettings.security, idle_lock_minutes: v || 0 },
                  })
                }
              />
              <span className="shrink-0 text-sm text-muted-foreground">{t("common.minutes")}</span>
            </div>
          </SettingRow>
        )}
      </SettingSection>

      <SettingSection>
        <SettingSelect
          label={t("settings.hostKeyPolicy")}
          desc={t("settings.hostKeyPolicyDesc")}
          value={appSettings.security.host_key_policy}
          controlClassName="max-w-sm"
          onValueChange={(v) =>
            updateAppSettings({ security: { ...appSettings.security, host_key_policy: v } })
          }
        >
          <SelectItem value="strict">{t("settings.hostKeyStrict")}</SelectItem>
          <SelectItem value="prompt">{t("settings.hostKeyPrompt")}</SelectItem>
          <SelectItem value="accept">{t("settings.hostKeyAccept")}</SelectItem>
        </SettingSelect>
      </SettingSection>
    </div>
  );
}
