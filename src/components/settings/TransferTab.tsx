import { downloadDir } from "@tauri-apps/api/path";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdFolderOpen } from "react-icons/md";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { SelectItem } from "@/components/ui/select";
import { useApp } from "@/context/AppContext";
import {
  SettingFieldGrid,
  SettingInput,
  SettingNumberInput,
  SettingRow,
  SettingSection,
  SettingSelect,
  SettingSwitch,
} from "./SettingFormItems";

function PathPickerInput({
  label,
  desc,
  value,
  placeholder,
  onChange,
  directory = true,
  filters,
}: {
  label: string;
  desc?: string;
  value: string;
  placeholder?: string;
  onChange: (v: string) => void;
  directory?: boolean;
  filters?: { name: string; extensions: string[] }[];
}) {
  const { t } = useTranslation();

  const handleBrowse = async () => {
    const selected = await openDialog({
      directory,
      filters: directory ? undefined : filters,
    });
    if (selected && typeof selected === "string") {
      onChange(selected);
    }
  };

  return (
    <div className="space-y-3">
      <div className="min-w-0">
        <Label className="text-sm font-medium leading-5">{label}</Label>
        {desc && <p className="mt-1 text-xs leading-5 text-muted-foreground">{desc}</p>}
      </div>
      <div className="flex max-w-2xl flex-col gap-2 sm:flex-row">
        <Input
          className="flex-1 text-sm"
          placeholder={placeholder}
          value={value}
          onChange={(e) => onChange(e.target.value)}
        />
        <Button
          variant="outline"
          size="sm"
          className="w-full shrink-0 gap-1.5 sm:w-auto"
          onClick={handleBrowse}
        >
          <MdFolderOpen className="text-sm" />
          {t("settings.browse")}
        </Button>
      </div>
    </div>
  );
}

export function TransferTab() {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings } = useApp();
  const transfer = appSettings.transfer;
  const [defaultDownloadDir, setDefaultDownloadDir] = useState("");

  useEffect(() => {
    downloadDir()
      .then(setDefaultDownloadDir)
      .catch(() => {});
  }, []);

  const update = (patch: Partial<typeof transfer>) =>
    updateAppSettings({ transfer: { ...transfer, ...patch } });
  const recordingMemoryLimitMiB = Math.max(
    1,
    Math.round((transfer.recording_memory_limit_bytes || 5 * 1024 * 1024) / (1024 * 1024)),
  );

  return (
    <div className="space-y-5">
      <SettingSection contentClassName="space-y-5">
        <PathPickerInput
          label={t("settings.downloadPath")}
          desc={t("settings.downloadPathDesc")}
          value={transfer.download_path}
          placeholder={defaultDownloadDir}
          onChange={(v) => update({ download_path: v })}
        />

        <SettingRow label={t("settings.askSaveLocation")} desc={t("settings.askSaveLocationDesc")}>
          <SettingSwitch
            checked={transfer.ask_save_location}
            onChange={(v) => update({ ask_save_location: v })}
          />
        </SettingRow>

        <SettingSelect
          label={t("settings.duplicateStrategy")}
          desc={t("settings.duplicateStrategyDesc")}
          value={transfer.duplicate_strategy}
          controlClassName="max-w-sm"
          onValueChange={(v) => update({ duplicate_strategy: v })}
        >
          <SelectItem value="overwrite">{t("settings.strategyOverwrite")}</SelectItem>
          <SelectItem value="skip">{t("settings.strategySkip")}</SelectItem>
          <SelectItem value="rename">{t("settings.strategyRename")}</SelectItem>
          <SelectItem value="ask">{t("settings.strategyAsk")}</SelectItem>
        </SettingSelect>

        <SettingSelect
          label={t("settings.editorType")}
          desc={t("settings.editorTypeDesc")}
          value={transfer.editor_type || "external"}
          controlClassName="max-w-sm"
          onValueChange={(v) => update({ editor_type: v as "external" | "internal" })}
        >
          <SelectItem value="external">{t("settings.editorTypeExternal")}</SelectItem>
          <SelectItem value="internal">{t("settings.editorTypeInternal")}</SelectItem>
        </SettingSelect>

        {(transfer.editor_type || "external") === "external" && (
          <PathPickerInput
            label={t("settings.defaultEditor")}
            desc={t("settings.defaultEditorDesc")}
            value={transfer.default_editor}
            placeholder={t("settings.defaultEditorDesc")}
            onChange={(v) => update({ default_editor: v })}
            directory={false}
            filters={[
              { name: "Executable", extensions: ["exe", "cmd", "bat", "com", "app", "sh", ""] },
            ]}
          />
        )}
      </SettingSection>

      <SettingSection
        title={t("settings.recordingSettings")}
        desc={t("settings.recordingSettingsDesc")}
        contentClassName="space-y-5"
      >
        <PathPickerInput
          label={t("settings.recordingPath")}
          desc={t("settings.recordingPathDesc")}
          value={transfer.recording_path}
          placeholder={defaultDownloadDir}
          onChange={(v) => update({ recording_path: v })}
        />

        <SettingRow
          label={t("settings.recordingAutoStart")}
          desc={t("settings.recordingAutoStartDesc")}
        >
          <SettingSwitch
            checked={transfer.recording_auto_start}
            onChange={(v) => update({ recording_auto_start: v })}
          />
        </SettingRow>

        <SettingRow
          label={t("settings.recordingIncludeIoLabels")}
          desc={t("settings.recordingIncludeIoLabelsDesc")}
        >
          <SettingSwitch
            checked={transfer.recording_include_io_labels}
            onChange={(v) => update({ recording_include_io_labels: v })}
          />
        </SettingRow>

        <SettingRow
          label={t("settings.recordingIncludeTimestamps")}
          desc={t("settings.recordingIncludeTimestampsDesc")}
        >
          <SettingSwitch
            checked={transfer.recording_include_timestamps ?? true}
            onChange={(v) => update({ recording_include_timestamps: v })}
          />
        </SettingRow>

        <SettingNumberInput
          label={t("settings.recordingMemoryLimit")}
          desc={t("settings.recordingMemoryLimitDesc")}
          min={1}
          max={100}
          value={recordingMemoryLimitMiB}
          controlClassName="max-w-sm"
          onChange={(v) => update({ recording_memory_limit_bytes: Math.max(1, v) * 1024 * 1024 })}
        />
      </SettingSection>

      <SettingSection contentClassName="space-y-5">
        <SettingFieldGrid>
          <SettingNumberInput
            label={t("settings.downloadConcurrentTasks")}
            desc={t("settings.downloadConcurrentTasksDesc")}
            min={1}
            max={10}
            value={transfer.download_threads}
            controlClassName="max-w-sm"
            onChange={(v) => update({ download_threads: v })}
          />

          <SettingNumberInput
            label={t("settings.uploadConcurrentTasks")}
            desc={t("settings.uploadConcurrentTasksDesc")}
            min={1}
            max={10}
            value={transfer.upload_threads}
            controlClassName="max-w-sm"
            onChange={(v) => update({ upload_threads: v })}
          />

          <SettingNumberInput
            label={t("settings.maxTransferRetries")}
            desc={t("settings.maxTransferRetriesDesc")}
            min={0}
            max={10}
            value={transfer.max_transfer_retries}
            controlClassName="max-w-sm"
            onChange={(v) => update({ max_transfer_retries: v })}
          />

          <SettingNumberInput
            label={t("settings.transferBufferSize")}
            desc={t("settings.transferBufferSizeDesc")}
            min={8}
            max={256}
            step={8}
            value={transfer.transfer_buffer_size}
            controlClassName="max-w-sm"
            onChange={(v) => update({ transfer_buffer_size: v })}
          />

          <SettingInput
            label={t("settings.defaultFilePermissions")}
            desc={t("settings.defaultFilePermissionsDesc")}
            placeholder="644"
            value={transfer.default_file_permissions}
            controlClassName="max-w-sm"
            onChange={(e) => update({ default_file_permissions: e.target.value })}
          />
        </SettingFieldGrid>

        <SettingRow
          label={t("settings.preserveTimestamps")}
          desc={t("settings.preserveTimestampsDesc")}
        >
          <SettingSwitch
            checked={transfer.preserve_timestamps}
            onChange={(v) => update({ preserve_timestamps: v })}
          />
        </SettingRow>

        <SettingRow
          label={t("settings.resumeBrokenTransfer")}
          desc={t("settings.resumeBrokenTransferDesc")}
        >
          <SettingSwitch
            checked={transfer.resume_broken_transfer}
            onChange={(v) => update({ resume_broken_transfer: v })}
          />
        </SettingRow>
      </SettingSection>
    </div>
  );
}
