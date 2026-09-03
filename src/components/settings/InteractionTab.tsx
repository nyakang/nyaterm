import { useTranslation } from "react-i18next";
import { SelectItem } from "@/components/ui/select";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useApp } from "@/context/AppContext";
import {
  MAX_COMMAND_SUGGESTION_MAX_CHARS,
  MAX_COMMAND_SUGGESTION_MIN_CHARS,
  MIN_COMMAND_SUGGESTION_MAX_CHARS,
  MIN_COMMAND_SUGGESTION_MIN_CHARS,
  normalizeCommandSuggestionMaxChars,
  normalizeCommandSuggestionMinChars,
  normalizeTabMouseAction,
  normalizeTerminalRightClickAction,
  TAB_MOUSE_ACTION_LABEL_KEYS,
  TAB_MOUSE_ACTIONS,
} from "@/lib/interactionSettings";
import type { InteractionSettings } from "@/types/global";
import {
  SettingInput,
  SettingNumberInput,
  SettingRow,
  SettingSection,
  SettingSelect,
  SettingSwitch,
} from "./SettingFormItems";

const MIN_DUPLICATE_SESSION_COMMAND_DELAY_MS = 0;
const MAX_DUPLICATE_SESSION_COMMAND_DELAY_MS = 60000;

function normalizeDuplicateSessionCommandDelayMs(value: number) {
  if (!Number.isFinite(value)) return 1000;
  return Math.max(
    MIN_DUPLICATE_SESSION_COMMAND_DELAY_MS,
    Math.min(MAX_DUPLICATE_SESSION_COMMAND_DELAY_MS, Math.round(value)),
  );
}

export function InteractionTab() {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings } = useApp();
  const interaction = appSettings.interaction;

  const updateInteraction = (updates: Partial<InteractionSettings>) => {
    updateAppSettings({ interaction: { ...interaction, ...updates } });
  };

  const renderTabMouseActionItems = () =>
    TAB_MOUSE_ACTIONS.map((action) => (
      <SelectItem key={action} value={action}>
        {t(TAB_MOUSE_ACTION_LABEL_KEYS[action])}
      </SelectItem>
    ));

  return (
    <div className="space-y-5">
      <SettingSection
        title={t("settings.interactionClipboardMouse")}
        desc={t("settings.interactionClipboardMouseDesc")}
        contentClassName="space-y-5"
      >
        <SettingRow label={t("settings.copyOnSelect")} desc={t("settings.copyOnSelectDesc")}>
          <SettingSwitch
            checked={interaction.copy_on_select}
            onChange={(v) => updateInteraction({ copy_on_select: v })}
          />
        </SettingRow>

        <SettingRow
          label={t("settings.allowOsc52ClipboardWrite")}
          desc={t("settings.allowOsc52ClipboardWriteDesc")}
        >
          <SettingSwitch
            checked={interaction.allow_osc52_clipboard_write}
            onChange={(v) => updateInteraction({ allow_osc52_clipboard_write: v })}
          />
        </SettingRow>

        <SettingRow
          label={t("settings.terminalRightClickAction")}
          desc={t("settings.terminalRightClickActionDesc")}
        >
          <Tabs
            value={normalizeTerminalRightClickAction(
              interaction.terminal_right_click_action,
            )}
            onValueChange={(value) =>
              updateInteraction({
                terminal_right_click_action: normalizeTerminalRightClickAction(value),
              })
            }
          >
            <TabsList className="grid w-52 grid-cols-3">
              <TabsTrigger value="none">
                {t("settings.terminalRightClickNone")}
              </TabsTrigger>
              <TabsTrigger value="menu">
                {t("settings.terminalRightClickMenu")}
              </TabsTrigger>
              <TabsTrigger value="paste">
                {t("settings.terminalRightClickPaste")}
              </TabsTrigger>
            </TabsList>
          </Tabs>
        </SettingRow>
      </SettingSection>

      <SettingSection
        title={t("settings.interactionTerminalPage")}
        desc={t("settings.interactionTerminalPageDesc")}
        contentClassName="space-y-5"
      >
        <SettingRow
          label={t("settings.terminalZoomEnabled")}
          desc={t("settings.terminalZoomEnabledDesc")}
        >
          <SettingSwitch
            checked={interaction.terminal_zoom_enabled}
            onChange={(v) => updateInteraction({ terminal_zoom_enabled: v })}
          />
        </SettingRow>
      </SettingSection>

      <SettingSection
        title={t("settings.interactionCommandInput")}
        desc={t("settings.interactionCommandInputDesc")}
        contentClassName="space-y-5"
      >
        <SettingRow
          label={t("settings.commandSuggestions")}
          desc={t("settings.commandSuggestionsDesc")}
        >
          <SettingSwitch
            checked={interaction.command_suggestions_enabled}
            onChange={(v) =>
              updateInteraction({
                command_suggestions_enabled: v,
              })
            }
          />
        </SettingRow>

        {interaction.command_suggestions_enabled && (
          <>
            <SettingNumberInput
              label={t("settings.commandSuggestionsMinChars")}
              desc={t("settings.commandSuggestionsMinCharsDesc")}
              value={interaction.command_suggestion_min_chars}
              min={MIN_COMMAND_SUGGESTION_MIN_CHARS}
              max={Math.min(
                MAX_COMMAND_SUGGESTION_MIN_CHARS,
                interaction.command_suggestion_max_chars,
              )}
              step={1}
              controlClassName="max-w-sm"
              onChange={(v) =>
                updateInteraction({
                  command_suggestion_min_chars: normalizeCommandSuggestionMinChars(
                    v,
                    interaction.command_suggestion_max_chars,
                  ),
                })
              }
            />

            <SettingNumberInput
              label={t("settings.commandSuggestionsMaxChars")}
              desc={t("settings.commandSuggestionsMaxCharsDesc")}
              value={interaction.command_suggestion_max_chars}
              min={Math.max(
                MIN_COMMAND_SUGGESTION_MAX_CHARS,
                interaction.command_suggestion_min_chars,
              )}
              max={MAX_COMMAND_SUGGESTION_MAX_CHARS}
              step={1}
              controlClassName="max-w-sm"
              onChange={(v) =>
                updateInteraction({
                  command_suggestion_max_chars: normalizeCommandSuggestionMaxChars(
                    v,
                    interaction.command_suggestion_min_chars,
                  ),
                })
              }
            />
          </>
        )}

        <SettingInput
          label={t("settings.wordSeparators")}
          desc={t("settings.wordSeparatorsDesc")}
          value={interaction.word_separators}
          controlClassName="max-w-2xl"
          onChange={(e) => updateInteraction({ word_separators: e.target.value })}
        />

        <SettingNumberInput
          label={t("settings.duplicateSessionCommandDelay")}
          desc={t("settings.duplicateSessionCommandDelayDesc")}
          value={interaction.duplicate_session_command_delay_ms}
          min={MIN_DUPLICATE_SESSION_COMMAND_DELAY_MS}
          max={MAX_DUPLICATE_SESSION_COMMAND_DELAY_MS}
          step={100}
          controlClassName="max-w-sm"
          onChange={(v) =>
            updateInteraction({
              duplicate_session_command_delay_ms: normalizeDuplicateSessionCommandDelayMs(v),
            })
          }
        />

        <SettingRow label={t("settings.altAsMeta")} desc={t("settings.altAsMetaDesc")}>
          <SettingSwitch
            checked={interaction.alt_as_meta}
            onChange={(v) => updateInteraction({ alt_as_meta: v })}
          />
        </SettingRow>

        <SettingRow
          label={t("settings.imeCompatibility")}
          desc={t("settings.imeCompatibilityDesc")}
        >
          <SettingSwitch
            checked={interaction.ime_compatibility}
            onChange={(v) => updateInteraction({ ime_compatibility: v })}
          />
        </SettingRow>
      </SettingSection>

      <SettingSection
        title={t("settings.tabMouseActions")}
        desc={t("settings.tabMouseActionsDesc")}
        contentClassName="space-y-5"
      >
        <SettingSelect
          label={t("settings.tabDoubleClickAction")}
          desc={t("settings.tabDoubleClickActionDesc")}
          value={normalizeTabMouseAction(interaction.tab_double_click_action)}
          controlClassName="max-w-sm"
          onValueChange={(v) =>
            updateInteraction({ tab_double_click_action: normalizeTabMouseAction(v) })
          }
        >
          {renderTabMouseActionItems()}
        </SettingSelect>

        <SettingSelect
          label={t("settings.tabMiddleClickAction")}
          desc={t("settings.tabMiddleClickActionDesc")}
          value={normalizeTabMouseAction(interaction.tab_middle_click_action)}
          controlClassName="max-w-sm"
          onValueChange={(v) =>
            updateInteraction({ tab_middle_click_action: normalizeTabMouseAction(v) })
          }
        >
          {renderTabMouseActionItems()}
        </SettingSelect>

        <SettingSelect
          label={t("settings.tabRightClickAction")}
          desc={t("settings.tabRightClickActionDesc")}
          value={normalizeTabMouseAction(interaction.tab_right_click_action)}
          controlClassName="max-w-sm"
          onValueChange={(v) =>
            updateInteraction({ tab_right_click_action: normalizeTabMouseAction(v) })
          }
        >
          {renderTabMouseActionItems()}
        </SettingSelect>
      </SettingSection>

      <SettingSection
        title={t("settings.interactionEncoding")}
        desc={t("settings.interactionEncodingDesc")}
        contentClassName="space-y-5"
      >
        <SettingSelect
          label={t("settings.defaultEncoding")}
          desc={t("settings.defaultEncodingDesc")}
          value={interaction.default_encoding}
          controlClassName="max-w-sm"
          onValueChange={(v) => updateInteraction({ default_encoding: v })}
        >
          <SelectItem value="UTF-8">UTF-8</SelectItem>
          <SelectItem value="GBK">GBK</SelectItem>
        </SettingSelect>
      </SettingSection>
    </div>
  );
}
