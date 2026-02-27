import { useTranslation } from "react-i18next";
import { SelectItem } from "@/components/ui/select";
import { useApp } from "../../../context/AppContext";
import { SettingInput, SettingSelect } from "./SettingFormItems";

export function TranslationTab() {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings } = useApp();

  return (
    <div className="space-y-4">
      <SettingSelect
        label={t("settings.translationProvider", "Translation Provider")}
        desc={t(
          "settings.translationProviderDesc",
          "Select the API provider for translating terminal output.",
        )}
        value={appSettings.translation.provider || "none"}
        onValueChange={(v) =>
          updateAppSettings({
            translation: { ...appSettings.translation, provider: v === "none" ? "" : v },
          })
        }
      >
        <SelectItem value="none">{t("settings.translationDisabled", "Disabled")}</SelectItem>
        <SelectItem value="openai">OpenAI</SelectItem>
        <SelectItem value="deepl">DeepL</SelectItem>
      </SettingSelect>

      {appSettings.translation.provider !== "" && (
        <SettingInput
          label={t("settings.translationApiKey", "API Key")}
          desc={t(
            "settings.translationApiKeyDesc",
            "Enter the API key for your chosen translation provider.",
          )}
          type="password"
          placeholder="sk-..."
          value={appSettings.translation.api_key}
          onChange={(e) =>
            updateAppSettings({
              translation: { ...appSettings.translation, api_key: e.target.value },
            })
          }
        />
      )}
    </div>
  );
}
