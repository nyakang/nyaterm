import { useTranslation } from "react-i18next";
import { SelectItem } from "@/components/ui/select";
import { useApp } from "@/context/AppContext";
import { MOD } from "@/hooks/useGlobalShortcuts";
import { SettingInput, SettingRow, SettingSelect, SettingSwitch } from "./SettingFormItems";
import { KbdGroup, Kbd } from "@/components/ui/kbd";
import React from "react";

export function InteractionTab() {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings } = useApp();

  const shortcutSections = [
    {
      title: t("settings.shortcutCategories.terminal"),
      desc: t("settings.terminalShortcutsDesc"),
      items: [
        { label: t("terminalCtx.copy"), keys: `${MOD}+Shift+C` },
        { label: t("terminalCtx.paste"), keys: `${MOD}+Shift+V` },
        { label: t("terminalCtx.pasteSelectedText"), keys: `${MOD}+Shift+X` },
        { label: t("terminalCtx.find"), keys: `${MOD}+Shift+F` },
        { label: t("terminalCtx.clearScreen"), keys: `${MOD}+Shift+K` },
        { label: t("terminalCtx.selectAll"), keys: `${MOD}+Shift+A` },
      ],
    },
    {
      title: t("settings.shortcutCategories.tab"),
      items: [
        { label: t("settings.shortcutLabels.newSession"), keys: `${MOD}+Shift+N` },
        { label: t("settings.shortcutLabels.newLocalTerminal"), keys: `${MOD}+\`` },
        { label: t("settings.shortcutLabels.closeTab"), keys: `${MOD}+Shift+W` },
        { label: t("settings.shortcutLabels.nextTab"), keys: "Ctrl+Tab" },
        { label: t("settings.shortcutLabels.prevTab"), keys: "Ctrl+Shift+Tab" },
        { label: t("settings.shortcutLabels.switchTab"), keys: `${MOD}+1-9` },
      ],
    },
    {
      title: t("settings.shortcutCategories.view"),
      items: [
        { label: t("settings.shortcutLabels.toggleLeftSidebar"), keys: `${MOD}+Shift+E` },
        { label: t("settings.shortcutLabels.toggleRightSidebar"), keys: `${MOD}+Shift+B` },
        { label: t("settings.shortcutLabels.zoomIn"), keys: `${MOD}+=` },
        { label: t("settings.shortcutLabels.zoomOut"), keys: `${MOD}+-` },
        { label: t("settings.shortcutLabels.resetZoom"), keys: `${MOD}+0` },
        { label: t("settings.shortcutLabels.toggleFullscreen"), keys: "F11" },
      ],
    },
    {
      title: t("settings.shortcutCategories.special"),
      items: [
        { label: t("settings.shortcutLabels.lockScreen"), keys: `${MOD}+Shift+L` },
        { label: t("settings.shortcutLabels.openSettings"), keys: `${MOD}+,` },
      ],
    },
  ];

  return (
    <div className="space-y-6">
      <div className="space-y-4">
        <SettingRow
          label={t("settings.copyOnSelect")}
          desc={t("settings.copyOnSelectDesc")}
        >
          <SettingSwitch
            checked={appSettings.interaction.copy_on_select}
            onChange={(v) =>
              updateAppSettings({ interaction: { ...appSettings.interaction, copy_on_select: v } })
            }
          />
        </SettingRow>

        <SettingRow
          label={t("settings.rightClickPaste")}
          desc={t("settings.rightClickPasteDesc")}
        >
          <SettingSwitch
            checked={appSettings.interaction.right_click_paste}
            onChange={(v) =>
              updateAppSettings({ interaction: { ...appSettings.interaction, right_click_paste: v } })
            }
          />
        </SettingRow>

        <SettingInput
          label={t("settings.wordSeparators")}
          desc={t("settings.wordSeparatorsDesc")}
          value={appSettings.interaction.word_separators}
          onChange={(e) =>
            updateAppSettings({
              interaction: { ...appSettings.interaction, word_separators: e.target.value },
            })
          }
        />

        <SettingSelect
          label={t("settings.defaultEncoding")}
          value={appSettings.interaction.default_encoding}
          onValueChange={(v) =>
            updateAppSettings({ interaction: { ...appSettings.interaction, default_encoding: v } })
          }
        >
          <SelectItem value="UTF-8">UTF-8</SelectItem>
          <SelectItem value="GBK">GBK</SelectItem>
        </SettingSelect>
      </div>

      {shortcutSections.map((section) => (
        <div key={section.title} className="pt-4 border-t border-border/50">
          <div className="mb-3">
            <h3 className="text-sm font-medium">{section.title}</h3>
            {section.desc && <p className="text-xs text-muted-foreground">{section.desc}</p>}
          </div>
          <div className="grid grid-cols-2 gap-x-8 gap-y-2 mt-2">
            {section.items.map((item) => (
              <div key={item.label} className="flex items-center justify-between py-1 px-1">
                <span className="text-sm text-muted-foreground">{item.label}</span>
                <KbdGroup>
                  {item.keys.split("+").map((key, i, arr) => (
                    <React.Fragment key={i}>
                      <Kbd>{key.trim()}</Kbd>
                      {i < arr.length - 1 && <span className="text-muted-foreground">+</span>}
                    </React.Fragment>
                  ))}
                </KbdGroup>
              </div>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}

