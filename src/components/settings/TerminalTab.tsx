import { downloadDir } from "@tauri-apps/api/path";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdAdd,
  MdDelete,
  MdExpandLess,
  MdExpandMore,
  MdFileUpload,
  MdFolderOpen,
} from "react-icons/md";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { SelectItem } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { useApp } from "@/context/AppContext";
import { useTheme } from "@/context/ThemeContext";
import { getBuiltinRules, hexLuminance } from "@/lib/keywordHighlightPresets";
import type { KeywordHighlightRule, SshKeepAliveMode } from "@/types/global";
import { KeywordHighlightImportDialog } from "../dialog/terminal/KeywordHighlightImportDialog";
import {
  SettingInput,
  SettingNumberInput,
  SettingRow,
  SettingSection,
  SettingSelect,
  SettingSwitch,
} from "./SettingFormItems";

const DEFAULT_ACTION_LINK_MATCHERS = {
  ipv4: true,
  archive: true,
  host_port: true,
} as const;

const KEEP_ALIVE_MODE_DESCRIPTION_KEYS: Record<SshKeepAliveMode, string> = {
  compatible: "settings.keepAliveModeCompatibleDescription",
  strict: "settings.keepAliveModeStrictDescription",
  disabled: "settings.keepAliveModeDisabledDescription",
};

const DEFAULT_TIMESTAMP_FORMAT = "[HH:mm:ss]";
const MAX_TIMESTAMP_FORMAT_LENGTH = 64;

function PathPickerInput({
  label,
  desc,
  value,
  placeholder,
  onChange,
}: {
  label: string;
  desc?: string;
  value: string;
  placeholder?: string;
  onChange: (v: string) => void;
}) {
  const { t } = useTranslation();

  const handleBrowse = async () => {
    const selected = await openDialog({ directory: true });
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

function normalizeKeepAliveMode(value: string): SshKeepAliveMode {
  if (value === "strict" || value === "disabled") return value;
  return "compatible";
}

function clampTimestampFormat(value: string): string {
  return Array.from(value).slice(0, MAX_TIMESTAMP_FORMAT_LENGTH).join("");
}

export function TerminalTab() {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings, updateUi } = useApp();
  const { terminalTheme } = useTheme();
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [showImportDialog, setShowImportDialog] = useState(false);
  const [defaultDownloadDir, setDefaultDownloadDir] = useState("");

  const isDark = useMemo(
    () => hexLuminance(terminalTheme.colors.terminal.background) < 0.5,
    [terminalTheme.colors.terminal.background],
  );

  const builtinRules = useMemo(() => getBuiltinRules(isDark), [isDark]);
  const userRules = appSettings.terminal.keyword_highlights ?? [];
  const keywordHighlightingEnabled = appSettings.terminal.keyword_highlights_enabled ?? false;
  const builtinRuleSettings = appSettings.terminal.keyword_highlight_builtin_rules ?? {};
  const actionLinksEnabled = appSettings.terminal.action_links_enabled ?? false;
  const actionLinkMatchers =
    appSettings.terminal.action_links_matchers ?? DEFAULT_ACTION_LINK_MATCHERS;
  const keepAliveMode = normalizeKeepAliveMode(appSettings.terminal.keep_alive_mode);
  const recording = appSettings.recording;
  const recordingMemoryLimitMiB = Math.max(
    1,
    Math.round((recording.memory_limit_bytes || 5 * 1024 * 1024) / (1024 * 1024)),
  );
  const rotationValue =
    recording.rotation?.type === "daily"
      ? "daily"
      : recording.rotation?.type === "size"
        ? "size"
        : "session";

  useEffect(() => {
    downloadDir()
      .then(setDefaultDownloadDir)
      .catch(() => {});
  }, []);

  function updateRecording(patch: Partial<typeof recording>) {
    updateAppSettings({ recording: { ...recording, ...patch } });
  }

  function updateRules(next: KeywordHighlightRule[]) {
    updateAppSettings({ terminal: { ...appSettings.terminal, keyword_highlights: next } });
  }

  function addRule() {
    const id = `kh-${Date.now()}`;
    const next: KeywordHighlightRule = {
      id,
      name: t("settings.keywordHighlightNewRule"),
      patterns: [],
      color_dark: "#79c0ff",
      color_light: "#0969da",
      enabled: true,
    };
    updateRules([...userRules, next]);
    setExpandedId(id);
  }

  function deleteRule(id: string) {
    updateRules(userRules.filter((r) => r.id !== id));
    if (expandedId === id) setExpandedId(null);
  }

  function patchRule(id: string, patch: Partial<KeywordHighlightRule>) {
    updateRules(userRules.map((r) => (r.id === id ? { ...r, ...patch } : r)));
  }

  function patchBuiltinRule(id: string, enabled: boolean) {
    updateAppSettings((prev) => ({
      terminal: {
        ...prev.terminal,
        keyword_highlight_builtin_rules: {
          ...(prev.terminal.keyword_highlight_builtin_rules ?? {}),
          [id]: enabled,
        },
      },
    }));
  }

  const ringClass = isDark ? "ring-white/20" : "ring-black/20";

  return (
    <div className="space-y-5">
      <SettingSection contentClassName="space-y-5">
        <SettingNumberInput
          label={t("settings.scrollbackLines")}
          desc={t("settings.scrollbackLinesDesc")}
          min={100}
          max={100000}
          step={100}
          value={appSettings.terminal.scrollback_lines}
          controlClassName="max-w-sm"
          onChange={(v) =>
            updateAppSettings({
              terminal: { ...appSettings.terminal, scrollback_lines: v || 5000 },
            })
          }
        />

        <SettingSelect
          label={t("settings.keepAliveMode")}
          desc={t(KEEP_ALIVE_MODE_DESCRIPTION_KEYS[keepAliveMode])}
          value={keepAliveMode}
          controlClassName="max-w-sm"
          onValueChange={(value) =>
            updateAppSettings({
              terminal: {
                ...appSettings.terminal,
                keep_alive_mode: normalizeKeepAliveMode(value),
              },
            })
          }
        >
          <SelectItem value="compatible">{t("settings.keepAliveModeCompatible")}</SelectItem>
          <SelectItem value="strict">{t("settings.keepAliveModeStrict")}</SelectItem>
          <SelectItem value="disabled">{t("settings.keepAliveModeDisabled")}</SelectItem>
        </SettingSelect>

        <SettingNumberInput
          label={t("settings.keepAliveInterval")}
          desc={t("settings.keepAliveIntervalDesc")}
          min={0}
          max={600}
          step={5}
          value={appSettings.terminal.keep_alive_interval}
          disabled={keepAliveMode === "disabled"}
          controlClassName="max-w-sm"
          onChange={(v) =>
            updateAppSettings({
              terminal: { ...appSettings.terminal, keep_alive_interval: v || 0 },
            })
          }
        />

        <SettingRow
          label={t("settings.reconnectRestoreCwd")}
          desc={t("settings.reconnectRestoreCwdDesc")}
        >
          <SettingSwitch
            checked={appSettings.terminal.reconnect_restore_cwd ?? false}
            onChange={(v) =>
              updateAppSettings({
                terminal: { ...appSettings.terminal, reconnect_restore_cwd: v },
              })
            }
          />
        </SettingRow>

        <SettingInput
          label={t("settings.x11Display")}
          desc={t("settings.x11DisplayDesc")}
          className="h-8 font-mono text-sm"
          controlClassName="max-w-lg"
          value={appSettings.terminal.x11_display ?? ""}
          placeholder={t("settings.x11DisplayPlaceholder")}
          onChange={(event) =>
            updateAppSettings({
              terminal: { ...appSettings.terminal, x11_display: event.target.value },
            })
          }
        />

        <SettingRow
          label={t("settings.hardwareAcceleration")}
          desc={t("settings.hardwareAccelerationDesc")}
        >
          <SettingSwitch
            checked={appSettings.terminal.hardware_acceleration}
            onChange={(v) =>
              updateAppSettings({ terminal: { ...appSettings.terminal, hardware_acceleration: v } })
            }
          />
        </SettingRow>

        <SettingRow
          label={t("settings.showWorkspacePadding")}
          desc={t("settings.showWorkspacePaddingDesc")}
        >
          <SettingSwitch
            checked={appSettings.terminal.show_workspace_padding ?? false}
            onChange={(v) =>
              updateAppSettings({
                terminal: { ...appSettings.terminal, show_workspace_padding: v },
              })
            }
          />
        </SettingRow>

        <SettingRow label={t("settings.showLineNumbers")} desc={t("settings.showLineNumbersDesc")}>
          <SettingSwitch
            checked={appSettings.terminal.show_line_numbers}
            onChange={(v) =>
              updateAppSettings({ terminal: { ...appSettings.terminal, show_line_numbers: v } })
            }
          />
        </SettingRow>

        <SettingRow label={t("settings.showTimestamps")} desc={t("settings.showTimestampsDesc")}>
          <SettingSwitch
            checked={appSettings.terminal.show_timestamps}
            onChange={(v) =>
              updateAppSettings({ terminal: { ...appSettings.terminal, show_timestamps: v } })
            }
          />
        </SettingRow>

        {appSettings.terminal.show_timestamps && (
          <SettingInput
            label={t("settings.timestampFormat")}
            desc={t("settings.timestampFormatDesc")}
            value={appSettings.terminal.timestamp_format ?? DEFAULT_TIMESTAMP_FORMAT}
            placeholder={DEFAULT_TIMESTAMP_FORMAT}
            controlClassName="max-w-sm"
            className="font-mono"
            maxLength={MAX_TIMESTAMP_FORMAT_LENGTH}
            onChange={(event) =>
              updateAppSettings({
                terminal: {
                  ...appSettings.terminal,
                  timestamp_format: clampTimestampFormat(event.target.value),
                },
              })
            }
            onBlur={(event) => {
              if (event.target.value.trim()) return;
              updateAppSettings({
                terminal: {
                  ...appSettings.terminal,
                  timestamp_format: DEFAULT_TIMESTAMP_FORMAT,
                },
              });
            }}
          />
        )}

        <SettingRow
          label={t("terminal.showMultiLinePasteDialog")}
          desc={t("terminal.showMultiLinePasteDialogDesc")}
        >
          <SettingSwitch
            checked={appSettings.terminal.show_multi_line_paste_dialog ?? true}
            onChange={(v) =>
              updateAppSettings({
                terminal: { ...appSettings.terminal, show_multi_line_paste_dialog: v },
              })
            }
          />
        </SettingRow>

        <SettingRow
          label={t("terminal.pasteImageAsPath")}
          desc={t("terminal.pasteImageAsPathDesc")}
        >
          <SettingSwitch
            checked={appSettings.terminal.paste_image_as_path ?? true}
            onChange={(v) =>
              updateAppSettings({
                terminal: { ...appSettings.terminal, paste_image_as_path: v },
              })
            }
          />
        </SettingRow>

        <SettingRow label={t("settings.showRemoteStats")} desc={t("settings.showRemoteStatsDesc")}>
          <SettingSwitch
            checked={appSettings.ui.show_remote_stats ?? true}
            onChange={(v) => updateUi({ show_remote_stats: v })}
          />
        </SettingRow>

        {appSettings.ui.show_remote_stats && (
          <SettingNumberInput
            label={t("settings.remoteStatsInterval")}
            desc={t("settings.remoteStatsIntervalDesc")}
            min={1}
            max={60}
            step={1}
            value={appSettings.ui.remote_stats_interval ?? 3}
            controlClassName="max-w-sm"
            onChange={(v) => updateUi({ remote_stats_interval: v || 3 })}
          />
        )}

        <SettingRow label={t("settings.showGpuMonitor")} desc={t("settings.showGpuMonitorDesc")}>
          <SettingSwitch
            checked={appSettings.ui.show_gpu_monitor ?? false}
            onChange={(v) => updateUi({ show_gpu_monitor: v })}
          />
        </SettingRow>

        {(appSettings.ui.show_gpu_monitor ?? false) && (
          <SettingNumberInput
            label={t("settings.gpuMonitorInterval")}
            desc={t("settings.gpuMonitorIntervalDesc")}
            min={3}
            max={120}
            step={1}
            value={appSettings.ui.gpu_monitor_interval ?? 3}
            controlClassName="max-w-sm"
            onChange={(v) => updateUi({ gpu_monitor_interval: v || 3 })}
          />
        )}

        <SettingRow
          label={t("settings.showAscendNpuMonitor")}
          desc={t("settings.showAscendNpuMonitorDesc")}
        >
          <SettingSwitch
            checked={appSettings.ui.show_ascend_npu_monitor ?? false}
            onChange={(v) => updateUi({ show_ascend_npu_monitor: v })}
          />
        </SettingRow>

        {(appSettings.ui.show_ascend_npu_monitor ?? false) && (
          <SettingNumberInput
            label={t("settings.ascendNpuMonitorInterval")}
            desc={t("settings.ascendNpuMonitorIntervalDesc")}
            min={3}
            max={120}
            step={1}
            value={appSettings.ui.ascend_npu_monitor_interval ?? 3}
            controlClassName="max-w-sm"
            onChange={(v) => updateUi({ ascend_npu_monitor_interval: v || 3 })}
          />
        )}

        <SettingRow
          label={t("settings.showProcessManager")}
          desc={t("settings.showProcessManagerDesc")}
        >
          <SettingSwitch
            checked={appSettings.ui.show_process_manager ?? false}
            onChange={(v) => updateUi({ show_process_manager: v })}
          />
        </SettingRow>

        {(appSettings.ui.show_process_manager ?? false) && (
          <SettingNumberInput
            label={t("settings.processManagerInterval")}
            desc={t("settings.processManagerIntervalDesc")}
            min={3}
            max={120}
            step={1}
            value={appSettings.ui.process_manager_interval ?? 5}
            controlClassName="max-w-sm"
            onChange={(v) => updateUi({ process_manager_interval: v || 5 })}
          />
        )}

        <SettingRow
          label={t("settings.showDockerManager")}
          desc={t("settings.showDockerManagerDesc")}
        >
          <SettingSwitch
            checked={appSettings.ui.show_docker_manager ?? false}
            onChange={(v) => updateUi({ show_docker_manager: v })}
          />
        </SettingRow>

        {(appSettings.ui.show_docker_manager ?? false) && (
          <SettingNumberInput
            label={t("settings.dockerManagerInterval")}
            desc={t("settings.dockerManagerIntervalDesc")}
            min={3}
            max={120}
            step={1}
            value={appSettings.ui.docker_manager_interval ?? 10}
            controlClassName="max-w-sm"
            onChange={(v) => updateUi({ docker_manager_interval: v || 10 })}
          />
        )}
      </SettingSection>

      <SettingSection
        title={t("settings.recordingSettings")}
        desc={t("settings.recordingSettingsDesc")}
        contentClassName="space-y-5"
      >
        <SettingRow
          label={t("settings.recordingAutoStart")}
          desc={t("settings.recordingAutoStartDesc")}
        >
          <SettingSwitch
            checked={recording.auto_start}
            onChange={(v) => updateRecording({ auto_start: v })}
          />
        </SettingRow>

        <SettingSelect
          label={t("settings.recordingDefaultMode")}
          desc={t("settings.recordingDefaultModeDesc")}
          value={recording.default_mode || "transcript"}
          controlClassName="max-w-sm"
          onValueChange={(v) => updateRecording({ default_mode: v as "transcript" | "raw" })}
        >
          <SelectItem value="transcript">{t("settings.recordingModeTranscript")}</SelectItem>
          <SelectItem value="raw">{t("settings.recordingModeRaw")}</SelectItem>
        </SettingSelect>

        <PathPickerInput
          label={t("settings.recordingPath")}
          desc={t("settings.recordingPathDesc")}
          value={recording.base_path}
          placeholder={defaultDownloadDir}
          onChange={(v) => updateRecording({ base_path: v })}
        />

        <SettingInput
          label={t("settings.recordingPathTemplate")}
          desc={t("settings.recordingPathTemplateDesc")}
          value={recording.path_template}
          controlClassName="max-w-2xl"
          className="font-mono text-xs"
          onChange={(event) => updateRecording({ path_template: event.target.value })}
        />

        <SettingRow
          label={t("settings.recordingIncludeTimestamps")}
          desc={t("settings.recordingIncludeTimestampsDesc")}
        >
          <SettingSwitch
            checked={recording.include_timestamps}
            onChange={(v) => updateRecording({ include_timestamps: v })}
          />
        </SettingRow>

        <SettingRow
          label={t("settings.recordingIncludeIoLabels")}
          desc={t("settings.recordingIncludeIoLabelsDesc")}
        >
          <SettingSwitch
            checked={recording.include_io_labels}
            onChange={(v) => updateRecording({ include_io_labels: v })}
          />
        </SettingRow>

        <SettingRow
          label={t("settings.recordingIncludeMetadata")}
          desc={t("settings.recordingIncludeMetadataDesc")}
        >
          <SettingSwitch
            checked={recording.include_session_metadata}
            onChange={(v) => updateRecording({ include_session_metadata: v })}
          />
        </SettingRow>

        <SettingSelect
          label={t("settings.recordingRotation")}
          desc={t("settings.recordingRotationDesc")}
          value={rotationValue}
          controlClassName="max-w-sm"
          onValueChange={(v) =>
            updateRecording({
              rotation:
                v === "daily"
                  ? { type: "daily" }
                  : v === "size"
                    ? { type: "size", max_bytes: 10 * 1024 * 1024 }
                    : { type: "session" },
            })
          }
        >
          <SelectItem value="session">{t("settings.recordingRotationSession")}</SelectItem>
          <SelectItem value="daily">{t("settings.recordingRotationDaily")}</SelectItem>
          <SelectItem value="size">{t("settings.recordingRotationSize")}</SelectItem>
        </SettingSelect>

        {recording.rotation?.type === "size" && (
          <SettingNumberInput
            label={t("settings.recordingRotationSizeLimit")}
            desc={t("settings.recordingRotationSizeLimitDesc")}
            min={1}
            max={10240}
            value={Math.max(1, Math.round(recording.rotation.max_bytes / (1024 * 1024)))}
            controlClassName="max-w-sm"
            onChange={(v) =>
              updateRecording({
                rotation: { type: "size", max_bytes: Math.max(1, v) * 1024 * 1024 },
              })
            }
          />
        )}

        <SettingSelect
          label={t("settings.recordingExistingFileBehavior")}
          desc={t("settings.recordingExistingFileBehaviorDesc")}
          value={recording.existing_file_behavior || "unique"}
          controlClassName="max-w-sm"
          onValueChange={(v) =>
            updateRecording({ existing_file_behavior: v as "unique" | "append" | "overwrite" })
          }
        >
          <SelectItem value="unique">{t("settings.recordingExistingUnique")}</SelectItem>
          <SelectItem value="append">{t("settings.recordingExistingAppend")}</SelectItem>
          <SelectItem value="overwrite">{t("settings.recordingExistingOverwrite")}</SelectItem>
        </SettingSelect>

        <SettingNumberInput
          label={t("settings.recordingMemoryLimit")}
          desc={t("settings.recordingMemoryLimitDesc")}
          min={1}
          max={100}
          value={recordingMemoryLimitMiB}
          controlClassName="max-w-sm"
          onChange={(v) => updateRecording({ memory_limit_bytes: Math.max(1, v) * 1024 * 1024 })}
        />

        <SettingRow
          label={t("settings.recordingIncludeBinaryTransfers")}
          desc={t("settings.recordingIncludeBinaryTransfersDesc")}
        >
          <SettingSwitch
            checked={recording.include_binary_transfer_payloads}
            onChange={(v) => updateRecording({ include_binary_transfer_payloads: v })}
          />
        </SettingRow>
      </SettingSection>

      <SettingSection contentClassName="space-y-4">
        <SettingRow label={t("settings.actionLinks")} desc={t("settings.actionLinksDesc")}>
          <SettingSwitch
            checked={actionLinksEnabled}
            onChange={(v) =>
              updateAppSettings({
                terminal: { ...appSettings.terminal, action_links_enabled: v },
              })
            }
          />
        </SettingRow>

        <div
          className={`space-y-2 transition-opacity ${
            actionLinksEnabled ? "" : "pointer-events-none opacity-50"
          }`}
        >
          <div className="space-y-1">
            <Label className="text-sm font-medium">{t("settings.actionLinksMatchers")}</Label>
          </div>
          <div className="space-y-2">
            {(
              [
                {
                  key: "ipv4" as const,
                  label: t("settings.actionLinksMatcherIpv4"),
                  example: "192.168.1.1",
                  desc: t("settings.actionLinksMatcherIpv4Desc"),
                },
                {
                  key: "host_port" as const,
                  label: t("settings.actionLinksMatcherHostPort"),
                  example: "localhost:8080",
                  desc: t("settings.actionLinksMatcherHostPortDesc"),
                },
                {
                  key: "archive" as const,
                  label: t("settings.actionLinksMatcherArchive"),
                  example: "backup.tar.gz",
                  desc: t("settings.actionLinksMatcherArchiveDesc"),
                },
              ] as const
            ).map(({ key, label, example, desc }) => (
              <div
                key={key}
                className="flex flex-col gap-3 rounded-lg border border-border/70 bg-background/70 px-3 py-3 sm:flex-row sm:items-center sm:justify-between"
              >
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="text-sm font-medium">{label}</span>
                    <span className="rounded bg-muted px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground/80">
                      {example}
                    </span>
                  </div>
                  <p className="mt-1 text-xs text-muted-foreground">{desc}</p>
                </div>
                <Switch
                  checked={actionLinkMatchers[key]}
                  onCheckedChange={(v) =>
                    updateAppSettings({
                      terminal: {
                        ...appSettings.terminal,
                        action_links_matchers: {
                          ...actionLinkMatchers,
                          [key]: v,
                        },
                      },
                    })
                  }
                />
              </div>
            ))}
          </div>
        </div>
      </SettingSection>

      <SettingSection contentClassName="space-y-4">
        <SettingRow
          label={t("settings.keywordHighlightingExperimental")}
          desc={t("settings.keywordHighlightingExperimentalDesc")}
        >
          <SettingSwitch
            checked={keywordHighlightingEnabled}
            onChange={(v) =>
              updateAppSettings({
                terminal: { ...appSettings.terminal, keyword_highlights_enabled: v },
              })
            }
          />
        </SettingRow>

        <SettingRow
          label={t("settings.keywordHighlightWrappedLines")}
          desc={t("settings.keywordHighlightWrappedLinesDesc")}
        >
          <SettingSwitch
            disabled={!keywordHighlightingEnabled}
            checked={appSettings.terminal.keyword_highlights_across_wrapped_lines ?? false}
            onChange={(v) =>
              updateAppSettings({
                terminal: {
                  ...appSettings.terminal,
                  keyword_highlights_across_wrapped_lines: v,
                },
              })
            }
          />
        </SettingRow>

        <div className="space-y-2">
          <div className="space-y-1">
            <Label className="text-sm font-medium">
              {t("settings.keywordHighlightBuiltinRules")}
            </Label>
            <p className="text-xs text-muted-foreground">
              {t("settings.keywordHighlightBuiltinNote")}
            </p>
          </div>
          <div className="grid gap-2 sm:grid-cols-2">
            {builtinRules.map((rule) => (
              <div
                key={rule.id}
                className="flex min-w-0 items-center justify-between gap-3 rounded-lg border border-border/70 bg-background/70 px-3 py-2.5"
              >
                <div className="flex min-w-0 items-center gap-2">
                  <span
                    className={`h-3 w-3 shrink-0 rounded-full ring-1 ring-inset ${ringClass}`}
                    style={{ backgroundColor: rule.color }}
                  />
                  <span className="min-w-0 truncate text-sm text-muted-foreground">
                    {rule.name}
                  </span>
                </div>
                <Switch
                  checked={builtinRuleSettings[rule.id] ?? true}
                  disabled={!keywordHighlightingEnabled}
                  onCheckedChange={(v) => patchBuiltinRule(rule.id, v)}
                />
              </div>
            ))}
          </div>
        </div>

        <div
          className={`space-y-3 transition-opacity ${
            keywordHighlightingEnabled ? "" : "pointer-events-none opacity-50"
          }`}
        >
          <div className="flex items-center justify-between">
            <Label className="text-sm font-medium">{t("settings.keywordHighlightRules")}</Label>
            <div className="flex items-center gap-1">
              <Button
                variant="ghost"
                size="xs"
                className="text-primary"
                onClick={() => setShowImportDialog(true)}
              >
                <MdFileUpload className="text-[0.875rem]" />
                {t("settings.keywordHighlightImport")}
              </Button>
              <Button variant="ghost" size="xs" className="text-primary" onClick={addRule}>
                <MdAdd className="text-[0.875rem]" />
                {t("common.add")}
              </Button>
            </div>
          </div>

          <div className="space-y-2">
            {userRules.length === 0 && (
              <div className="rounded-xl border border-dashed border-border/70 bg-background/40 px-4 py-8 text-center text-sm text-muted-foreground">
                {t("settings.keywordHighlightNoRules")}
              </div>
            )}

            {userRules.map((rule) => {
              const isOpen = expandedId === rule.id;
              const patternCount = rule.patterns.filter((p) => p.trim()).length;

              return (
                <div
                  key={rule.id}
                  className="overflow-hidden rounded-xl border border-border/70 bg-background/75"
                >
                  <div
                    className="cursor-pointer select-none px-4 py-3 transition-colors hover:bg-accent/40"
                    onClick={() => setExpandedId(isOpen ? null : rule.id)}
                  >
                    <div className="flex flex-col gap-3 sm:flex-row sm:items-center">
                      <div className="flex min-w-0 flex-1 items-center gap-2">
                        <span
                          className={`h-3 w-3 shrink-0 rounded-full ring-1 ring-inset ${ringClass}`}
                          style={{ backgroundColor: isDark ? rule.color_dark : rule.color_light }}
                        />
                        <span className="min-w-0 flex-1 truncate text-sm font-medium">
                          {rule.name}
                        </span>
                      </div>

                      <div className="flex flex-wrap items-center gap-2 sm:justify-end">
                        <span className="shrink-0 text-xs text-muted-foreground">
                          {t("settings.keywordHighlightPatternCount", { count: patternCount })}
                        </span>

                        <Switch
                          checked={rule.enabled}
                          onCheckedChange={(v) => patchRule(rule.id, { enabled: v })}
                          onClick={(e) => e.stopPropagation()}
                        />

                        <Button
                          variant="ghost"
                          size="icon-sm"
                          className="shrink-0 text-destructive hover:bg-destructive/10"
                          title={t("common.delete")}
                          onClick={(e) => {
                            e.stopPropagation();
                            deleteRule(rule.id);
                          }}
                        >
                          <MdDelete className="text-[1rem]" />
                        </Button>

                        {isOpen ? (
                          <MdExpandLess className="shrink-0 text-base text-muted-foreground" />
                        ) : (
                          <MdExpandMore className="shrink-0 text-base text-muted-foreground" />
                        )}
                      </div>
                    </div>
                  </div>

                  {isOpen && (
                    <div
                      className="space-y-4 border-t border-border/70 bg-accent/15 px-4 pb-4 pt-3"
                      onClick={(e) => e.stopPropagation()}
                    >
                      <div className="flex flex-col gap-4 xl:flex-row xl:items-end">
                        <div className="min-w-0 flex-1 space-y-2 xl:max-w-[16rem]">
                          <Label className="text-xs text-muted-foreground">
                            {t("settings.keywordHighlightRuleName")}
                          </Label>
                          <Input
                            className="h-8 text-sm"
                            value={rule.name}
                            placeholder={t("settings.keywordHighlightRuleNamePlaceholder")}
                            onChange={(e) => patchRule(rule.id, { name: e.target.value })}
                          />
                        </div>

                        <div className="grid gap-3 sm:grid-cols-2 xl:flex">
                          {[
                            {
                              field: "color_dark" as const,
                              labelKey: "keywordHighlightDarkPalette",
                              swatchRing: "ring-white/20",
                            },
                            {
                              field: "color_light" as const,
                              labelKey: "keywordHighlightLightPalette",
                              swatchRing: "ring-black/20",
                            },
                          ].map(({ field, labelKey, swatchRing }) => (
                            <div key={field} className="min-w-0 space-y-2">
                              <Label className="block text-xs text-muted-foreground">
                                {t(`settings.${labelKey}`)}
                              </Label>
                              <div className="flex items-center gap-2">
                                <div
                                  className={`relative h-8 w-8 shrink-0 overflow-hidden rounded-md border ring-1 ring-inset ${swatchRing}`}
                                  style={{ backgroundColor: rule[field] }}
                                >
                                  <input
                                    type="color"
                                    className="absolute inset-[-10px] h-[200%] w-[200%] cursor-pointer opacity-0"
                                    value={
                                      rule[field] && rule[field].length === 7
                                        ? rule[field]
                                        : "#000000"
                                    }
                                    onChange={(e) =>
                                      patchRule(rule.id, { [field]: e.target.value })
                                    }
                                  />
                                </div>
                                <Input
                                  className="h-8 w-full font-mono text-xs sm:w-[7.5rem]"
                                  value={rule[field]}
                                  maxLength={7}
                                  placeholder="#rrggbb"
                                  onChange={(e) => {
                                    const v = e.target.value;
                                    if (/^#[0-9a-fA-F]{0,6}$/.test(v)) {
                                      patchRule(rule.id, { [field]: v });
                                    }
                                  }}
                                />
                              </div>
                            </div>
                          ))}
                        </div>
                      </div>

                      <div className="space-y-2">
                        <Label className="text-xs text-muted-foreground">
                          {t("settings.keywordHighlightRulePatterns")}
                        </Label>
                        <Textarea
                          className="min-h-[80px] max-h-[160px] resize-y overflow-y-auto font-mono text-sm"
                          value={rule.patterns.join("\n")}
                          placeholder={t("settings.keywordHighlightRulePatternsPlaceholder")}
                          onChange={(e) =>
                            patchRule(rule.id, { patterns: e.target.value.split("\n") })
                          }
                        />
                      </div>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      </SettingSection>
      <KeywordHighlightImportDialog
        open={showImportDialog}
        onClose={() => setShowImportDialog(false)}
        onImportedRules={(rules) =>
          updateAppSettings((prev) => ({
            terminal: {
              ...prev.terminal,
              keyword_highlights: rules,
            },
          }))
        }
      />
    </div>
  );
}
