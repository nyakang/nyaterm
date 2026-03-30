import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdAdd, MdDelete, MdExpandLess, MdExpandMore } from "react-icons/md";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { useApp } from "@/context/AppContext";
import { useTheme } from "@/context/ThemeContext";
import { getBuiltinRules, hexLuminance } from "@/lib/keywordHighlightPresets";
import type { KeywordHighlightRule } from "@/types/global";
import { SettingNumberInput, SettingRow, SettingSwitch } from "./SettingFormItems";

export function TerminalTab() {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings, updateUi } = useApp();
  const { theme } = useTheme();
  const [expandedId, setExpandedId] = useState<string | null>(null);

  // Derive dark/light from actual theme background luminance.
  // ThemeContext uses CSS vars and never sets .dark on the document.
  const isDark = useMemo(() => hexLuminance(theme.colors.bg) < 0.5, [theme.colors.bg]);

  const builtinRules = useMemo(() => getBuiltinRules(isDark), [isDark]);
  const userRules = appSettings.terminal.keyword_highlights ?? [];

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

  const ringClass = isDark ? "ring-white/20" : "ring-black/20";

  return (
    <div className="space-y-4">
      <SettingNumberInput
        label={t("settings.scrollbackLines")}
        desc={t("settings.scrollbackLinesDesc")}
        min={100}
        max={100000}
        step={100}
        value={appSettings.terminal.scrollback_lines}
        onChange={(v) =>
          updateAppSettings({ terminal: { ...appSettings.terminal, scrollback_lines: v || 5000 } })
        }
      />

      <SettingNumberInput
        label={t("settings.keepAliveInterval")}
        desc={t("settings.keepAliveIntervalDesc")}
        min={0}
        max={600}
        step={5}
        value={appSettings.terminal.keep_alive_interval}
        onChange={(v) =>
          updateAppSettings({ terminal: { ...appSettings.terminal, keep_alive_interval: v || 0 } })
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
        label={t("settings.showRemoteStats")}
        desc={t("settings.showRemoteStatsDesc")}
      >
        <SettingSwitch
          checked={appSettings.ui.show_remote_stats ?? false}
          onChange={(v) => updateUi({ show_remote_stats: v })}
        />
      </SettingRow>

      {/* ── Keyword Highlighting ──────────────────────────────────────────── */}
      <div className="space-y-3 pt-2 border-t">
        <SettingRow
          label={t("settings.keywordHighlighting")}
          desc={t("settings.keywordHighlightingDesc")}
        >
          <SettingSwitch
            checked={appSettings.terminal.keyword_highlights_enabled ?? true}
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

        {/* ── Built-in rules (read-only preview) ── */}
        <div className="space-y-1">
          <Label className="font-medium text-sm">{t("settings.keywordHighlightBuiltinRules")}</Label>
          <p className="text-xs text-muted-foreground">{t("settings.keywordHighlightBuiltinNote")}</p>
          <div className="border rounded-md overflow-hidden grid grid-cols-2">
            {builtinRules.map((rule, idx) => (
              <div
                key={rule.id}
                className={`flex items-center gap-2 px-3 py-1.5 border-b bg-muted/30 ${idx % 2 === 1 ? "border-l" : ""
                  } ${idx >= builtinRules.length - (builtinRules.length % 2 === 0 ? 2 : 1)
                    ? "border-b-0"
                    : ""
                  }`}
              >
                <span
                  className={`w-3 h-3 rounded-full shrink-0 ring-1 ring-inset ${ringClass}`}
                  style={{ backgroundColor: rule.color }}
                />
                <span className="w-20 shrink-0 text-sm text-muted-foreground">{rule.name}</span>
                <span className="flex-1 text-xs text-muted-foreground/60 font-mono truncate">
                  {rule.patterns.slice(0, 3).join(", ")}
                  {rule.patterns.length > 3 && ` +${rule.patterns.length - 3}`}
                </span>
              </div>
            ))}
          </div>
        </div>

        {/* ── User rules ── */}
        <div className={`space-y-1 transition-opacity ${appSettings.terminal.keyword_highlights_enabled ? "" : "opacity-50 pointer-events-none"}`}>
          <div className="flex items-center justify-between">
            <Label className="font-medium text-sm">{t("settings.keywordHighlightRules")}</Label>
            <Button variant="ghost" size="xs" className="text-primary" onClick={addRule}>
              <MdAdd className="text-[0.875rem]" />
              {t("common.add")}
            </Button>
          </div>

          <div className="border rounded-md overflow-hidden">
            {userRules.length === 0 && (
              <p className="text-center text-xs text-muted-foreground py-5">
                {t("settings.keywordHighlightNoRules")}
              </p>
            )}

            {userRules.map((rule) => {
              const isOpen = expandedId === rule.id;
              const patternCount = rule.patterns.filter((p) => p.trim()).length;

              return (
                <div key={rule.id} className="border-b last:border-0">
                  {/* Collapsed row */}
                  <div
                    className="flex items-center gap-2 px-3 py-2 hover:bg-accent/50 transition-colors cursor-pointer select-none"
                    onClick={() => setExpandedId(isOpen ? null : rule.id)}
                  >
                    <span
                      className={`w-3 h-3 rounded-full shrink-0 ring-1 ring-inset ${ringClass}`}
                      style={{ backgroundColor: isDark ? rule.color_dark : rule.color_light }}
                    />
                    <span className="flex-1 text-sm font-medium truncate">{rule.name}</span>
                    <span className="text-xs text-muted-foreground shrink-0">
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
                      className="text-destructive hover:bg-destructive/10 shrink-0"
                      title={t("common.delete")}
                      onClick={(e) => {
                        e.stopPropagation();
                        deleteRule(rule.id);
                      }}
                    >
                      <MdDelete className="text-[1rem]" />
                    </Button>

                    {isOpen ? (
                      <MdExpandLess className="text-muted-foreground text-base shrink-0" />
                    ) : (
                      <MdExpandMore className="text-muted-foreground text-base shrink-0" />
                    )}
                  </div>

                  {/* Expanded edit form */}
                  {isOpen && (
                    <div
                      className="px-3 pb-3 pt-2 space-y-3 bg-accent/20 border-t"
                      onClick={(e) => e.stopPropagation()}
                    >
                      {/* Rule name + dark/light colors on the same row */}
                      <div className="flex items-end gap-4">
                        <div className="flex-1 space-y-1 min-w-[8rem] max-w-[16rem]">
                          <Label className="text-xs text-muted-foreground">
                            {t("settings.keywordHighlightRuleName")}
                          </Label>
                          <Input
                            className="text-sm h-8"
                            value={rule.name}
                            placeholder={t("settings.keywordHighlightRuleNamePlaceholder")}
                            onChange={(e) => patchRule(rule.id, { name: e.target.value })}
                          />
                        </div>

                        {(
                          [
                            { field: "color_dark" as const, labelKey: "keywordHighlightDarkPalette", swatchRing: "ring-white/20" },
                            { field: "color_light" as const, labelKey: "keywordHighlightLightPalette", swatchRing: "ring-black/20" },
                          ]
                        ).map(({ field, labelKey, swatchRing }) => (
                          <div key={field} className="shrink-0 space-y-1">
                            <Label className="text-xs text-muted-foreground block">
                              {t(`settings.${labelKey}`)}
                            </Label>
                            <div className="flex items-center gap-2">
                              <div
                                className={`relative w-8 h-8 rounded-md border shrink-0 overflow-hidden ring-1 ring-inset ${swatchRing}`}
                                style={{ backgroundColor: rule[field] }}
                              >
                                <input
                                  type="color"
                                  className="absolute inset-[-10px] w-[200%] h-[200%] opacity-0 cursor-pointer"
                                  value={rule[field] && rule[field].length === 7 ? rule[field] : "#000000"}
                                  onChange={(e) => patchRule(rule.id, { [field]: e.target.value })}
                                />
                              </div>
                              <Input
                                className="text-xs h-8 font-mono w-[7.5rem]"
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

                      {/* Patterns */}
                      <div className="space-y-1">
                        <Label className="text-xs text-muted-foreground">
                          {t("settings.keywordHighlightRulePatterns")}
                        </Label>
                        <Textarea
                          className="text-sm font-mono min-h-[80px] max-h-[160px] resize-y overflow-y-auto"
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
      </div>
    </div>
  );
}
