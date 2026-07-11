import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdAdd, MdDelete, MdExpandLess, MdExpandMore, MdFileUpload } from "react-icons/md";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { useApp } from "@/context/AppContext";
import { useTheme } from "@/context/ThemeContext";
import { getBuiltinRules, hexLuminance } from "@/lib/keywordHighlightPresets";
import type { KeywordHighlightRule } from "@/types/global";
import { KeywordHighlightImportDialog } from "../dialog/terminal/KeywordHighlightImportDialog";
import {
  SettingInput,
  SettingNumberInput,
  SettingRow,
  SettingSection,
  SettingSwitch,
} from "./SettingFormItems";

const DEFAULT_ACTION_LINK_MATCHERS = {
  ipv4: true,
  archive: true,
  host_port: true,
} as const;

export function TerminalTab() {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings, updateUi } = useApp();
  const { terminalTheme } = useTheme();
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [showImportDialog, setShowImportDialog] = useState(false);

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

        <SettingNumberInput
          label={t("settings.keepAliveInterval")}
          desc={t("settings.keepAliveIntervalDesc")}
          min={0}
          max={600}
          step={5}
          value={appSettings.terminal.keep_alive_interval}
          controlClassName="max-w-sm"
          onChange={(v) =>
            updateAppSettings({
              terminal: { ...appSettings.terminal, keep_alive_interval: v || 0 },
            })
          }
        />

        <SettingNumberInput
          label={t("settings.sshChannelWindowSize")}
          desc={t("settings.sshChannelWindowSizeDesc")}
          min={1}
          max={64}
          step={1}
          value={appSettings.terminal.ssh_channel_window_size_mb ?? 4}
          controlClassName="max-w-sm"
          onChange={(v) =>
            updateAppSettings({
              terminal: {
                ...appSettings.terminal,
                ssh_channel_window_size_mb: Math.max(1, v || 4),
              },
            })
          }
        />

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
          <SettingRow
            label={t("settings.showTimestampMilliseconds")}
            desc={t("settings.showTimestampMillisecondsDesc")}
          >
            <SettingSwitch
              checked={appSettings.terminal.show_timestamp_milliseconds ?? false}
              onChange={(v) =>
                updateAppSettings({
                  terminal: { ...appSettings.terminal, show_timestamp_milliseconds: v },
                })
              }
            />
          </SettingRow>
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
