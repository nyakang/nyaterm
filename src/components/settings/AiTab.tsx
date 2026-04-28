import { useTranslation } from "react-i18next";
import { MdAdd, MdDelete } from "react-icons/md";
import { Button } from "@/components/ui/button";
import { SelectItem } from "@/components/ui/select";
import { useApp } from "@/context/AppContext";
import type { AIProviderKind, AIProviderProfile, AISettings } from "@/types/global";
import {
  SettingFieldGrid,
  SettingInput,
  SettingNumberInput,
  SettingRow,
  SettingSection,
  SettingSelect,
  SettingSwitch,
} from "./SettingFormItems";

const PROVIDERS: Array<{ value: AIProviderKind; label: string }> = [
  { value: "openai", label: "OpenAI" },
  { value: "anthropic", label: "Anthropic" },
  { value: "gemini", label: "Gemini" },
  { value: "deepseek", label: "DeepSeek" },
  { value: "groq", label: "Groq" },
  { value: "ollama", label: "Ollama" },
  { value: "openai_compatible", label: "OpenAI Compatible" },
];

function newProfile(): AIProviderProfile {
  return {
    id: `custom-${crypto.randomUUID()}`,
    name: "Custom Provider",
    provider_kind: "openai_compatible",
    model: "gpt-4o-mini",
    base_url: "",
    api_key: "",
    enabled: true,
  };
}

export function AiTab() {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings } = useApp();
  const ai = appSettings.ai;

  const update = (patch: Partial<AISettings>) => updateAppSettings({ ai: { ...ai, ...patch } });

  const updateProfile = (id: string, patch: Partial<AIProviderProfile>) => {
    const provider_profiles = ai.provider_profiles.map((profile) =>
      profile.id === id ? { ...profile, ...patch } : profile,
    );
    update({ provider_profiles });
  };

  const addProfile = () => {
    const profile = newProfile();
    update({
      provider_profiles: [...ai.provider_profiles, profile],
      active_profile_id: profile.id,
    });
  };

  const removeProfile = (id: string) => {
    const provider_profiles = ai.provider_profiles.filter((profile) => profile.id !== id);
    update({
      provider_profiles,
      active_profile_id:
        ai.active_profile_id === id ? (provider_profiles[0]?.id ?? "") : ai.active_profile_id,
    });
  };

  return (
    <div className="space-y-5">
      <SettingSection title={t("ai.settings")}>
        <SettingRow label={t("ai.enabled")}>
          <SettingSwitch checked={ai.enabled} onChange={(enabled) => update({ enabled })} />
        </SettingRow>
        <SettingRow label={t("ai.redaction")}>
          <SettingSwitch
            checked={ai.redaction_enabled}
            onChange={(redaction_enabled) => update({ redaction_enabled })}
          />
        </SettingRow>
        <SettingRow label={t("ai.riskCheck")}>
          <SettingSwitch
            checked={ai.risk_check_enabled}
            onChange={(risk_check_enabled) => update({ risk_check_enabled })}
          />
        </SettingRow>
        <SettingRow label={t("ai.allowSave")}>
          <SettingSwitch
            checked={ai.allow_save_command}
            onChange={(allow_save_command) => update({ allow_save_command })}
          />
        </SettingRow>
        <SettingRow label={t("ai.recordHistory")}>
          <SettingSwitch
            checked={ai.record_history}
            onChange={(record_history) => update({ record_history })}
          />
        </SettingRow>
        <SettingFieldGrid>
          <SettingNumberInput
            label={t("ai.contextLineLimit")}
            min={50}
            max={500}
            step={50}
            value={ai.context_line_limit}
            onChange={(context_line_limit) => update({ context_line_limit })}
          />
          <SettingNumberInput
            label={t("ai.timeoutMs")}
            min={5000}
            max={300000}
            step={1000}
            value={ai.timeout_ms}
            onChange={(timeout_ms) => update({ timeout_ms })}
          />
          <SettingNumberInput
            label={t("ai.maxOutputTokens")}
            min={256}
            max={8192}
            step={128}
            value={ai.max_output_tokens}
            onChange={(max_output_tokens) => update({ max_output_tokens })}
          />
        </SettingFieldGrid>
      </SettingSection>

      <SettingSection
        title={t("ai.providerProfiles")}
        action={
          <Button size="sm" variant="outline" onClick={addProfile}>
            <MdAdd />
            {t("common.add")}
          </Button>
        }
        contentClassName="space-y-4"
      >
        <SettingSelect
          label={t("ai.activeProfile")}
          value={ai.active_profile_id || ai.provider_profiles[0]?.id || ""}
          onValueChange={(active_profile_id) => update({ active_profile_id })}
        >
          {ai.provider_profiles.map((profile) => (
            <SelectItem key={profile.id} value={profile.id}>
              {profile.name}
            </SelectItem>
          ))}
        </SettingSelect>

        {ai.provider_profiles.map((profile) => (
          <div key={profile.id} className="rounded-xl border border-border/70 bg-background/75 p-4">
            <div className="mb-4 flex items-center justify-between gap-3">
              <div className="min-w-0 truncate text-sm font-medium">{profile.name}</div>
              <div className="flex items-center gap-2">
                <SettingSwitch
                  checked={profile.enabled}
                  onChange={(enabled) => updateProfile(profile.id, { enabled })}
                />
                <Button
                  size="icon-xs"
                  variant="ghost"
                  disabled={ai.provider_profiles.length <= 1}
                  onClick={() => removeProfile(profile.id)}
                >
                  <MdDelete />
                </Button>
              </div>
            </div>
            <SettingFieldGrid>
              <SettingInput
                label={t("ai.profileName")}
                value={profile.name}
                onChange={(event) => updateProfile(profile.id, { name: event.target.value })}
              />
              <SettingSelect
                label={t("ai.providerKind")}
                value={profile.provider_kind}
                onValueChange={(provider_kind) =>
                  updateProfile(profile.id, { provider_kind: provider_kind as AIProviderKind })
                }
              >
                {PROVIDERS.map((provider) => (
                  <SelectItem key={provider.value} value={provider.value}>
                    {provider.label}
                  </SelectItem>
                ))}
              </SettingSelect>
              <SettingInput
                label={t("ai.model")}
                value={profile.model}
                onChange={(event) => updateProfile(profile.id, { model: event.target.value })}
              />
              <SettingInput
                label={t("ai.baseUrl")}
                placeholder="https://api.openai.com/v1/"
                value={profile.base_url ?? ""}
                onChange={(event) => updateProfile(profile.id, { base_url: event.target.value })}
              />
              <SettingInput
                label={t("settings.apiKey")}
                type="password"
                placeholder={profile.api_key === "__SET__" ? "__SET__" : "sk-..."}
                value={profile.api_key ?? ""}
                onChange={(event) => updateProfile(profile.id, { api_key: event.target.value })}
              />
            </SettingFieldGrid>
          </div>
        ))}
      </SettingSection>
    </div>
  );
}
