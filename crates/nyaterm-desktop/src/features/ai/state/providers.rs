use super::AiFeatureState;
use gpui::RenderImage;
use nyaterm_core::ai::provider_settings::{
    DEFAULT_MODEL_REASONING_EFFORTS, MODEL_REASONING_EFFORTS, available_provider_name,
    is_builtin_provider, model_belongs_to_provider, provider_default_url, provider_label,
};
use nyaterm_core::ai::{
    AiApiFormat, AiModelConfigItem, AiModelDiscovery, AiModelReasoningEffort, AiModelSource,
    AiProviderCredential, AiProviderKind,
};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::features) enum ConnectionStatus {
    #[default]
    Idle,
    Testing,
    Success,
    Error,
}

#[derive(Clone, Default)]
pub(in crate::features) struct ProviderSettingsView {
    pub tasks: Vec<std::sync::Arc<crate::ai_runtime::AiTask>>,
    pub selected_id: Option<String>,
    pub choices_open: bool,
    pub advanced_open: bool,
    pub editing_model: Option<String>,
    pub statuses: HashMap<String, ConnectionStatus>,
    pub model_statuses: HashMap<String, ConnectionStatus>,
    pub model_order: Vec<String>,
    pub reveal_generation: u64,
    pub revealed_id: Option<String>,
    pub revealed_header_id: Option<(String, String)>,
    pub message: Option<String>,
    pub generation: u64,
    pub revision: u64,
    pub icons: Arc<HashMap<String, Arc<RenderImage>>>,
}

impl PartialEq for ProviderSettingsView {
    fn eq(&self, other: &Self) -> bool {
        self.revision == other.revision && self.generation == other.generation
    }
}

impl AiFeatureState {
    pub(in crate::features) fn provider_view(&self) -> &ProviderSettingsView {
        &self.settings.providers
    }
    pub(in crate::features) fn provider_view_mut(&mut self) -> &mut ProviderSettingsView {
        self.settings.providers.revision = self.settings.providers.revision.wrapping_add(1);
        &mut self.settings.providers
    }
    pub(in crate::features) fn invalidate_provider_jobs(&mut self) {
        let view = self.provider_view_mut();
        for task in &view.tasks {
            task.cancel();
        }
        view.tasks.clear();
        view.generation = view.generation.wrapping_add(1);
        view.statuses.values_mut().for_each(|status| {
            if *status == ConnectionStatus::Testing {
                *status = ConnectionStatus::Idle;
            }
        });
        view.model_statuses.clear();
        view.message = None;
    }
    pub(in crate::features) fn prepare_provider_settings(&mut self) {
        let selected = self.settings.providers.selected_id.clone();
        if !self
            .settings
            .config
            .provider_credentials
            .iter()
            .any(|credential| credential.enabled && Some(&credential.id) == selected.as_ref())
        {
            let id = self
                .settings
                .config
                .provider_credentials
                .iter()
                .find(|credential| credential.enabled)
                .map(|credential| credential.id.clone());
            self.select_settings_provider(id);
        }
        self.refresh_provider_model_order();
    }
    pub(in crate::features) fn select_settings_provider(&mut self, id: Option<String>) {
        self.invalidate_provider_jobs();
        let view = self.provider_view_mut();
        view.selected_id = id;
        view.revealed_id = None;
        view.revealed_header_id = None;
        view.editing_model = None;
        view.choices_open = false;
        view.model_order.clear();
        self.refresh_provider_model_order();
    }
    pub(in crate::features) fn refresh_provider_model_order(&mut self) {
        let Some(credential) =
            self.settings
                .config
                .provider_credentials
                .iter()
                .find(|credential| {
                    Some(&credential.id) == self.settings.providers.selected_id.as_ref()
                })
        else {
            return;
        };
        let mut models: Vec<_> = self
            .settings
            .config
            .models
            .iter()
            .filter(|model| model_belongs_to_provider(model, credential))
            .collect();
        let existing = &self.settings.providers.model_order;
        if models.len() == existing.len() && models.iter().all(|model| existing.contains(&model.id))
        {
            return;
        }
        models.sort_by_key(|model| !model.enabled);
        let ids = models.into_iter().map(|model| model.id.clone()).collect();
        self.provider_view_mut().model_order = ids;
    }
    pub(in crate::features) fn add_settings_provider_preset(
        &mut self,
        kind: AiProviderKind,
    ) -> String {
        self.invalidate_provider_jobs();
        let builtin = self
            .settings
            .config
            .provider_credentials
            .iter()
            .find(|credential| {
                credential.provider_kind == kind
                    && is_builtin_provider(&credential.id)
                    && !credential.enabled
            })
            .cloned();
        let mut credential = builtin.unwrap_or_else(|| AiProviderCredential {
            id: format!("credential-{}", nyaterm_core::uuid()),
            name: String::new(),
            provider_kind: kind.clone(),
            icon_data_url: None,
            api_protocol: None,
            api_format: AiApiFormat::ChatCompletions,
            base_url: None,
            api_key: None,
            extra_headers: Vec::new(),
            enabled: true,
        });
        credential.enabled = true;
        credential.name = available_provider_name(
            if credential.name.trim().is_empty() {
                provider_label(&kind)
            } else {
                credential.name.trim()
            },
            &self.settings.config.provider_credentials,
            &credential.id,
        );
        if credential
            .base_url
            .as_deref()
            .is_none_or(|url| url.is_empty())
        {
            credential.base_url = Some(provider_default_url(&kind).into());
        }
        let id = credential.id.clone();
        if let Some(existing) = self
            .settings
            .config
            .provider_credentials
            .iter_mut()
            .find(|item| item.id == id)
        {
            *existing = credential;
        } else {
            self.settings
                .config
                .provider_credentials
                .insert(0, credential);
        }
        // Seed defaults by brand, then bind newly seeded models to this account.
        let mut seed = nyaterm_core::AiSettings::default();
        super::settings::seed_builtin_ai_models_for_provider(&mut seed, &kind);
        for mut model in seed
            .models
            .into_iter()
            .filter(|model| model.provider_kind.as_ref() == Some(&kind))
        {
            if !is_builtin_provider(&id) {
                model.id = nyaterm_core::ai_model_id_for_credential(&id, &model.name);
                model.credential_id = Some(id.clone());
            }
            model.enabled = false;
            if !self
                .settings
                .config
                .models
                .iter()
                .any(|existing| existing.id == model.id)
            {
                self.settings.config.models.push(model);
            }
        }
        self.select_settings_provider(Some(id.clone()));
        id
    }
    pub(in crate::features) fn reconcile_provider_models(
        &mut self,
        id: &str,
        discoveries: Vec<AiModelDiscovery>,
    ) {
        let Some(credential) = self
            .settings
            .config
            .provider_credentials
            .iter()
            .find(|item| item.id == id)
            .cloned()
        else {
            return;
        };
        let seen: std::collections::HashSet<_> =
            discoveries.iter().map(|model| model.id.clone()).collect();
        self.settings.config.models.retain(|model| {
            !model_belongs_to_provider(model, &credential)
                || model.source == AiModelSource::Manual
                || seen.contains(&model.id)
        });
        for discovery in discoveries {
            if let Some(model) = self
                .settings
                .config
                .models
                .iter_mut()
                .find(|model| model.id == discovery.id)
            {
                model.last_seen_at = Some(nyaterm_core::now_rfc3339());
            } else {
                self.settings.config.models.push(AiModelConfigItem {
                    id: discovery.id,
                    name: discovery.name,
                    backend: Default::default(),
                    provider_kind: discovery.provider_kind,
                    credential_id: discovery.credential_id,
                    enabled: false,
                    source: AiModelSource::RustGenai,
                    last_seen_at: Some(nyaterm_core::now_rfc3339()),
                    supported_reasoning_efforts: None,
                });
            }
        }
        if self.configured_default_model_is_disabled() {
            self.select_first_enabled_model();
        }
        self.refresh_provider_model_order();
    }
    pub(in crate::features) fn remove_settings_model(&mut self, id: &str) {
        self.invalidate_provider_jobs();
        self.settings.config.models.retain(|model| model.id != id);
        if self.configured_default_model_is_disabled() {
            self.select_first_enabled_model();
        }
        self.refresh_provider_model_order();
    }
    pub(in crate::features) fn toggle_model_reasoning(
        &mut self,
        id: &str,
        effort: AiModelReasoningEffort,
    ) {
        self.invalidate_provider_jobs();
        if let Some(model) = self
            .settings
            .config
            .models
            .iter_mut()
            .find(|model| model.id == id)
        {
            let mut values = model
                .supported_reasoning_efforts
                .clone()
                .unwrap_or_else(|| DEFAULT_MODEL_REASONING_EFFORTS.to_vec());
            if values.contains(&effort) {
                values.retain(|value| value != &effort);
            } else {
                values.push(effort);
            }
            model.supported_reasoning_efforts = Some(
                MODEL_REASONING_EFFORTS
                    .into_iter()
                    .filter(|value| values.contains(value))
                    .collect(),
            );
        }
        self.reconcile_selected_reasoning();
    }
    pub(super) fn reconcile_selected_reasoning(&mut self) {
        let model = self
            .settings
            .config
            .models
            .iter()
            .find(|model| Some(&model.id) == self.settings.config.default_model_id.as_ref());
        if !nyaterm_core::ai::provider_settings::model_reasoning_options(model)
            .contains(&self.settings.config.default_reasoning_effort)
        {
            self.settings.config.default_reasoning_effort = nyaterm_core::AiReasoningEffort::Auto;
        }
    }
    pub(in crate::features) fn complete_provider_discoveries(
        &mut self,
        generation: u64,
        results: Vec<(String, Result<Vec<AiModelDiscovery>, String>)>,
        refresh: bool,
    ) -> bool {
        if generation != self.provider_view().generation {
            return false;
        }
        for (id, result) in results {
            match result {
                Ok(models) => {
                    if refresh {
                        self.reconcile_provider_models(&id, models);
                    }
                    self.provider_view_mut()
                        .statuses
                        .insert(id, ConnectionStatus::Success);
                }
                Err(error) => {
                    self.provider_view_mut()
                        .statuses
                        .insert(id, ConnectionStatus::Error);
                    self.provider_view_mut().message =
                        Some(nyaterm_core::sanitize_ai_diagnostic(&error, 500));
                }
            }
        }
        true
    }
    pub(in crate::features) fn settings_config_mut(&mut self) -> &mut nyaterm_core::AiSettings {
        self.invalidate_provider_jobs();
        &mut self.settings.config
    }
}
