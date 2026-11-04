use super::ConnectionStatus;
use crate::features::{NyaTermApp, runtime_jobs::await_blocking_job};
use gpui::{AppContext as _, Context, PathPromptOptions, Window};
use nyaterm_core::ai::provider_settings::{
    provider_name_taken, provider_requires_api_key, set_provider_protocol,
};
use nyaterm_core::ai::{AiModelReasoningEffort, AiProviderKind};

impl NyaTermApp {
    fn spawn_ai_provider_job<T: Send + 'static>(
        &mut self,
        future: impl std::future::Future<Output = Result<T, String>> + Send + 'static,
        cx: &mut Context<Self>,
    ) -> tokio::sync::oneshot::Receiver<Result<T, String>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let runtime = cx
            .try_global::<crate::app_shell::DesktopControllerGlobal>()
            .ok_or_else(|| "AI runtime unavailable".to_string())
            .and_then(|global| global.0.read(cx).ai_runtime.clone());
        match runtime {
            Ok(runtime) => {
                let task = runtime.spawn(tokio_util::sync::CancellationToken::new(), async move {
                    let _ = tx.send(future.await);
                });
                self.ai
                    .provider_view_mut()
                    .tasks
                    .push(std::sync::Arc::new(task));
            }
            Err(error) => {
                let _ = tx.send(Err(error));
            }
        }
        rx
    }

    pub(in crate::features) fn select_ai_provider(&mut self, id: String, cx: &mut Context<Self>) {
        self.hide_ai_provider_secret(cx);
        self.ai.select_settings_provider(Some(id.clone()));
        self.ensure_ai_credential_inputs(&id, cx);
        self.load_ai_provider_icons(cx);
        self.request_settings_panel_refresh(cx);
    }
    pub(in crate::features) fn toggle_ai_provider_choices(&mut self, cx: &mut Context<Self>) {
        let open = !self.ai.provider_view().choices_open;
        self.ai.provider_view_mut().choices_open = open;
        self.request_settings_panel_refresh(cx);
    }
    pub(in crate::features) fn toggle_ai_advanced(&mut self, cx: &mut Context<Self>) {
        let open = !self.ai.provider_view().advanced_open;
        self.ai.provider_view_mut().advanced_open = open;
        self.request_settings_panel_refresh(cx);
    }
    pub(in crate::features) fn add_ai_provider_preset(
        &mut self,
        kind: AiProviderKind,
        cx: &mut Context<Self>,
    ) {
        self.hide_ai_provider_secret(cx);
        let id = self.ai.add_settings_provider_preset(kind);
        self.ensure_ai_credential_inputs(&id, cx);
        self.persist_ai_settings_now(cx);
    }
    pub(in crate::features) fn set_ai_provider_protocol(
        &mut self,
        id: String,
        value: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(credential) = self
            .ai
            .settings_config_mut()
            .provider_credentials
            .iter_mut()
            .find(|credential| credential.id == id)
        {
            set_provider_protocol(credential, &value);
        }
        let url = self
            .ai
            .settings_config()
            .provider_credentials
            .iter()
            .find(|credential| credential.id == id)
            .and_then(|credential| credential.base_url.clone())
            .unwrap_or_default();
        self.reset_text_input(&format!("ai.credential.{id}.base-url"), &url, cx);
        self.persist_ai_settings_now(cx);
    }
    pub(in crate::features) fn add_ai_provider_header(
        &mut self,
        id: String,
        cx: &mut Context<Self>,
    ) {
        if self.ai.add_settings_credential_header(&id).is_some() {
            self.ensure_ai_credential_inputs(&id, cx);
            self.request_settings_panel_refresh(cx);
        }
        self.persist_ai_settings_now(cx);
    }
    pub(in crate::features) fn remove_ai_provider_header(
        &mut self,
        id: String,
        header_id: String,
        cx: &mut Context<Self>,
    ) {
        self.hide_ai_provider_secret(cx);
        if self.ai.remove_settings_credential_header(&id, &header_id) {
            self.forget_text_inputs(&format!("ai.credential.{id}.header.{header_id}."));
            self.request_settings_panel_refresh(cx);
        }
        self.persist_ai_settings_now(cx);
    }
    pub(in crate::features) fn remove_ai_provider_confirm(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self
            .ai
            .settings_config()
            .provider_credentials
            .iter()
            .find(|credential| credential.id == id)
            .map(|credential| credential.name.clone())
            .unwrap_or_default();
        self.open_confirm_dialog(
            (
                rust_i18n::t!("ai.deleteProvider").to_string(),
                rust_i18n::t!("ai.deleteProviderConfirm", name = name).to_string(),
                rust_i18n::t!("common.delete").to_string(),
                true,
                move |app, _, cx| {
                    app.hide_ai_provider_secret(cx);
                    app.ai.invalidate_provider_jobs();
                    if nyaterm_core::ai::provider_settings::is_builtin_provider(&id) {
                        app.ai.toggle_settings_credential_enabled(&id);
                    } else {
                        app.ai.remove_settings_credential(&id);
                    }
                    app.ai
                        .cancel_removed_provider_runs(&app.ai.settings_config_cloned());
                    app.ai.prepare_provider_settings();
                    app.persist_ai_settings_now(cx);
                    true
                },
            ),
            window,
            cx,
        );
    }
    pub(in crate::features) fn remove_ai_model(&mut self, id: String, cx: &mut Context<Self>) {
        self.ai.remove_settings_model(&id);
        self.persist_ai_settings_now(cx);
    }
    pub(in crate::features) fn edit_ai_model(&mut self, id: String, cx: &mut Context<Self>) {
        let edit = (self.ai.provider_view().editing_model.as_ref() != Some(&id)).then_some(id);
        self.ai.provider_view_mut().editing_model = edit;
        self.request_settings_panel_refresh(cx);
    }
    pub(in crate::features) fn toggle_ai_model_reasoning(
        &mut self,
        id: String,
        effort: AiModelReasoningEffort,
        cx: &mut Context<Self>,
    ) {
        self.ai.toggle_model_reasoning(&id, effort);
        self.persist_ai_settings_now(cx);
    }
    pub(in crate::features) fn test_ai_provider(
        &mut self,
        all: bool,
        refresh: bool,
        cx: &mut Context<Self>,
    ) {
        let settings = self.ai.pending_settings();
        let ids: Vec<_> = settings
            .provider_credentials
            .iter()
            .filter(|credential| {
                credential.enabled
                    && (all || Some(&credential.id) == self.ai.provider_view().selected_id.as_ref())
            })
            .map(|credential| credential.id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        if self
            .ai
            .provider_view()
            .statuses
            .values()
            .any(|status| *status == ConnectionStatus::Testing)
        {
            return;
        }
        self.ai.invalidate_provider_jobs();
        let generation = self.ai.provider_view().generation;
        for id in &ids {
            self.ai
                .provider_view_mut()
                .statuses
                .insert(id.clone(), ConnectionStatus::Testing);
        }
        self.request_settings_panel_refresh(cx);
        let jobs = self.blocking_jobs.clone();
        let store = self.store_blocking_client();
        let rx = self.spawn_ai_provider_job(
            async move {
                let settings = jobs
                    .submit_task("ai-provider-prepare", move |_| {
                        let current = store
                            .request_fn(nyaterm_store::StoreDomain::Ai, |store| {
                                store.load_ai_settings()
                            })
                            .map_err(|error| error.to_string())?;
                        Ok::<_, String>(nyaterm_core::merge_masked_ai_settings(&current, settings))
                    })
                    .map_err(|error| error.to_string())?
                    .await
                    .map_err(|error| error.to_string())??;
                let cancel = tokio_util::sync::CancellationToken::new();
                let mut results = Vec::new();
                for id in ids {
                    let result = match settings
                        .provider_credentials
                        .iter()
                        .find(|credential| credential.id == id)
                    {
                        Some(credential) => crate::http::ai::discover_provider_models(
                            &settings, credential, &cancel,
                        )
                        .await
                        .map_err(|error| error.to_string()),
                        None => Err("Provider no longer exists".into()),
                    };
                    results.push((id, result));
                }
                Ok(results)
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = rx
                .await
                .unwrap_or_else(|_| Err("AI provider test cancelled".into()));
            let _ = this.update(cx, |app, cx| {
                if app.ai.provider_view().generation != generation {
                    return;
                }
                match result {
                    Ok(results) => {
                        app.ai
                            .complete_provider_discoveries(generation, results, refresh);
                    }
                    Err(error) => {
                        app.ai
                            .provider_view_mut()
                            .statuses
                            .values_mut()
                            .filter(|status| **status == ConnectionStatus::Testing)
                            .for_each(|status| *status = ConnectionStatus::Error);
                        app.ai.provider_view_mut().message = Some(error);
                    }
                }
                if refresh {
                    app.persist_ai_settings_now(cx);
                } else {
                    app.request_settings_panel_refresh(cx);
                }
            });
        })
        .detach();
    }
    pub(in crate::features) fn test_ai_model(&mut self, id: String, cx: &mut Context<Self>) {
        if self
            .ai
            .provider_view()
            .model_statuses
            .values()
            .any(|status| *status == ConnectionStatus::Testing)
        {
            return;
        }
        let generation = self.ai.provider_view().generation;
        self.ai
            .provider_view_mut()
            .model_statuses
            .insert(id.clone(), ConnectionStatus::Testing);
        let settings = self.ai.pending_settings();
        let jobs = self.blocking_jobs.clone();
        let store = self.store_blocking_client();
        self.request_settings_panel_refresh(cx);
        let test_id = id.clone();
        let rx = self.spawn_ai_provider_job(
            async move {
                let settings = jobs
                    .submit_task("ai-model-prepare", move |_| {
                        let current = store
                            .request_fn(nyaterm_store::StoreDomain::Ai, |store| {
                                store.load_ai_settings()
                            })
                            .map_err(|error| error.to_string())?;
                        Ok::<_, String>(nyaterm_core::merge_masked_ai_settings(&current, settings))
                    })
                    .map_err(|error| error.to_string())?
                    .await
                    .map_err(|error| error.to_string())??;
                crate::http::ai::test_model_connection(
                    &settings,
                    &test_id,
                    &tokio_util::sync::CancellationToken::new(),
                )
                .await
                .map_err(|error| error.to_string())
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = rx
                .await
                .unwrap_or_else(|_| Err("AI model test cancelled".into()));
            let _ = this.update(cx, |app, cx| {
                if app.ai.provider_view().generation != generation {
                    return;
                }
                let status = if result.is_ok() {
                    ConnectionStatus::Success
                } else {
                    ConnectionStatus::Error
                };
                app.ai.provider_view_mut().model_statuses.insert(id, status);
                app.ai.provider_view_mut().message = result
                    .err()
                    .map(|error| nyaterm_core::sanitize_ai_diagnostic(&error, 500));
                app.request_settings_panel_refresh(cx);
            });
        })
        .detach();
    }
    pub(in crate::features) fn hide_ai_provider_secret(&mut self, cx: &mut Context<Self>) {
        self.ai.provider_view_mut().reveal_generation =
            self.ai.provider_view().reveal_generation.wrapping_add(1);
        if let Some(id) = self.ai.provider_view().revealed_id.clone() {
            let draft = self
                .ai
                .settings_credential_secret_drafts()
                .get(&id)
                .cloned()
                .unwrap_or_default();
            if let Some(input) = self.existing_text_input(format!("ai.credential.{id}.api-key")) {
                input.update(cx, |input, cx| {
                    input.set_content_silent(&draft, cx);
                    input.set_masked(true, cx);
                });
            }
        }
        if let Some((credential_id, header_id)) = self.ai.provider_view().revealed_header_id.clone()
            && let Some(input) = self.existing_text_input(format!(
                "ai.credential.{}.header.{}.value",
                credential_id, header_id
            ))
        {
            let draft = self
                .ai
                .settings_credential_header_secret_drafts()
                .get(&(credential_id.clone(), header_id.clone()))
                .cloned()
                .unwrap_or_default();
            input.update(cx, |input, cx| {
                input.set_content_silent(&draft, cx);
                input.set_masked(true, cx);
            });
        }
        self.ai.provider_view_mut().revealed_id = None;
        self.ai.provider_view_mut().revealed_header_id = None;
    }
    pub(in crate::features) fn toggle_ai_provider_secret(
        &mut self,
        id: String,
        cx: &mut Context<Self>,
    ) {
        if self.ai.provider_view().revealed_id.as_ref() == Some(&id) {
            self.hide_ai_provider_secret(cx);
            self.request_settings_panel_refresh(cx);
            return;
        }
        self.hide_ai_provider_secret(cx);
        if let Some(draft) = self.ai.settings_credential_secret_drafts().get(&id)
            && !draft.is_empty()
        {
            if let Some(input) = self.existing_text_input(format!("ai.credential.{id}.api-key")) {
                input.update(cx, |input, cx| input.set_masked(false, cx));
            }
            self.ai.provider_view_mut().revealed_id = Some(id);
            self.request_settings_panel_refresh(cx);
            return;
        }
        let generation = self.ai.provider_view().generation;
        let reveal_generation = self.ai.provider_view().reveal_generation;
        let settings = self.ai.pending_settings();
        let store = self.store_blocking_client();
        let jobs = self.blocking_jobs.clone();
        let reveal_id = id.clone();
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(jobs.submit_task("ai-secret-reveal", move |_| {
                let current = store
                    .request_fn(nyaterm_store::StoreDomain::Ai, |store| {
                        store.load_ai_settings()
                    })
                    .map_err(|error| error.to_string())?;
                Ok::<_, String>(
                    nyaterm_core::merge_masked_ai_settings(&current, settings)
                        .provider_credentials
                        .into_iter()
                        .find(|credential| credential.id == reveal_id)
                        .and_then(|credential| credential.api_key)
                        .unwrap_or_default(),
                )
            }))
            .await
            .and_then(|result| result);
            let _ = this.update(cx, |app, cx| {
                if app.ai.provider_view().generation != generation
                    || app.ai.provider_view().reveal_generation != reveal_generation
                    || app.ai.provider_view().selected_id.as_ref() != Some(&id)
                {
                    return;
                }
                if let Ok(secret) = result {
                    if let Some(input) =
                        app.existing_text_input(format!("ai.credential.{id}.api-key"))
                    {
                        input.update(cx, |input, cx| {
                            input.set_content_silent(secret.expose_secret(), cx);
                            input.set_masked(false, cx);
                        });
                    }
                    app.ai.provider_view_mut().revealed_id = Some(id);
                    app.request_settings_panel_refresh(cx);
                }
            });
        })
        .detach();
    }
    pub(in crate::features) fn toggle_ai_provider_header_secret(
        &mut self,
        credential_id: String,
        header_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.ai.provider_view().revealed_header_id.as_ref()
            == Some(&(credential_id.clone(), header_id.clone()))
        {
            self.hide_ai_provider_secret(cx);
            self.request_settings_panel_refresh(cx);
            return;
        }
        self.hide_ai_provider_secret(cx);
        if let Some(draft) = self
            .ai
            .settings_credential_header_secret_drafts()
            .get(&(credential_id.clone(), header_id.clone()))
            && !draft.is_empty()
        {
            if let Some(input) = self.existing_text_input(format!(
                "ai.credential.{credential_id}.header.{header_id}.value"
            )) {
                input.update(cx, |input, cx| input.set_masked(false, cx));
            }
            self.ai.provider_view_mut().revealed_header_id = Some((credential_id, header_id));
            self.request_settings_panel_refresh(cx);
            return;
        }
        let generation = self.ai.provider_view().generation;
        let reveal_generation = self.ai.provider_view().reveal_generation;
        let settings = self.ai.pending_settings();
        let store = self.store_blocking_client();
        let jobs = self.blocking_jobs.clone();
        let reveal_credential_id = credential_id.clone();
        let reveal_header_id = header_id.clone();
        let selected_id = self.ai.provider_view().selected_id.clone();
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(jobs.submit_task("ai-secret-reveal", move |_| {
                let current = store
                    .request_fn(nyaterm_store::StoreDomain::Ai, |store| {
                        store.load_ai_settings()
                    })
                    .map_err(|error| error.to_string())?;
                Ok::<_, String>(
                    nyaterm_core::merge_masked_ai_settings(&current, settings)
                        .provider_credentials
                        .into_iter()
                        .find(|credential| credential.id == reveal_credential_id)
                        .and_then(|credential| {
                            credential
                                .extra_headers
                                .into_iter()
                                .find(|header| header.id == reveal_header_id)
                                .map(|header| header.value)
                        })
                        .unwrap_or_default(),
                )
            }))
            .await
            .and_then(|result| result);
            let _ = this.update(cx, |app, cx| {
                if app.ai.provider_view().generation != generation
                    || app.ai.provider_view().reveal_generation != reveal_generation
                    || app.ai.provider_view().selected_id != selected_id
                {
                    return;
                }
                if let Ok(secret) = result {
                    if let Some(input) = app.existing_text_input(format!(
                        "ai.credential.{credential_id}.header.{header_id}.value"
                    )) {
                        input.update(cx, |input, cx| {
                            input.set_content_silent(secret.expose_secret(), cx);
                            input.set_masked(false, cx);
                        });
                    }
                    app.ai.provider_view_mut().revealed_header_id =
                        Some((credential_id, header_id));
                    app.request_settings_panel_refresh(cx);
                }
            });
        })
        .detach();
    }
    pub(in crate::features) fn load_ai_provider_icons(&mut self, cx: &mut Context<Self>) {
        let records: Vec<_> = self
            .ai
            .settings_config()
            .provider_credentials
            .iter()
            .filter_map(|credential| {
                credential
                    .icon_data_url
                    .clone()
                    .map(|url| (credential.id.clone(), url))
            })
            .collect();
        let generation = self.ai.provider_view().generation;
        cx.spawn(async move |this, cx| {
            let icons = cx
                .background_spawn(async move {
                    records
                        .into_iter()
                        .filter_map(|(id, url)| {
                            decode_provider_icon(&url).ok().map(|image| (id, image))
                        })
                        .collect()
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                if app.ai.provider_view().generation == generation {
                    app.ai.provider_view_mut().icons = std::sync::Arc::new(icons);
                    app.request_settings_panel_refresh(cx);
                    app.defer_ai_panel_snapshot_flush(cx);
                }
            });
        })
        .detach();
    }
    pub(in crate::features) fn prompt_ai_provider_icon(
        &mut self,
        id: String,
        cx: &mut Context<Self>,
    ) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        let generation = self.ai.provider_view().generation;
        let jobs = self.blocking_jobs.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picker.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let result = await_blocking_job(jobs.submit_task("ai-provider-icon", move |_| {
                use base64::Engine as _;
                use std::io::Read as _;
                let file = std::fs::File::open(path).map_err(|_| "Cannot open icon")?;
                let mut bytes = Vec::new();
                file.take(2 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "Cannot read icon")?;
                if bytes.len() > 2 * 1024 * 1024 {
                    return Err("Icon exceeds 2 MiB".to_string());
                }
                let format = image::guess_format(&bytes).map_err(|_| "Invalid icon format")?;
                let mime = match format {
                    image::ImageFormat::Png => "png",
                    image::ImageFormat::Jpeg => "jpeg",
                    image::ImageFormat::Gif => "gif",
                    image::ImageFormat::Bmp => "bmp",
                    image::ImageFormat::WebP => "webp",
                    _ => return Err("Unsupported icon format".into()),
                };
                let url = format!(
                    "data:image/{mime};base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                );
                let image = decode_provider_icon(&url)?;
                Ok::<_, String>((url, image))
            }))
            .await
            .and_then(|result| result);
            let _ = this.update(cx, |app, cx| {
                if app.ai.provider_view().generation != generation {
                    return;
                }
                match result {
                    Ok((url, image)) => {
                        if let Some(credential) = app
                            .ai
                            .settings_config_mut()
                            .provider_credentials
                            .iter_mut()
                            .find(|credential| credential.id == id)
                        {
                            credential.icon_data_url = Some(url);
                        }
                        std::sync::Arc::make_mut(&mut app.ai.provider_view_mut().icons)
                            .insert(id, image);
                        app.persist_ai_settings_now(cx);
                    }
                    Err(error) => {
                        app.ai.provider_view_mut().message = Some(error);
                        app.request_settings_panel_refresh(cx);
                    }
                }
            });
        })
        .detach();
    }
    pub(in crate::features) fn ai_provider_validation_error(&self) -> Option<String> {
        let settings = self.ai.pending_settings();
        for credential in &settings.provider_credentials {
            if let Err(error) = nyaterm_core::custom_header_pairs(credential) {
                return Some(error.to_string());
            }
        }
        for credential in settings
            .provider_credentials
            .iter()
            .filter(|credential| credential.enabled)
        {
            if self.shell.settings_draft_snapshot().is_some_and(|base| {
                base.ai_settings
                    .provider_credentials
                    .iter()
                    .any(|original| original == credential)
            }) {
                continue;
            }
            if credential.name.trim().is_empty() {
                return Some(rust_i18n::t!("ai.providerNameRequired").to_string());
            }
            if provider_name_taken(
                &credential.name,
                &settings.provider_credentials,
                &credential.id,
            ) {
                return Some(rust_i18n::t!("ai.providerNameDuplicate").to_string());
            }
            if credential
                .base_url
                .as_deref()
                .is_none_or(|url| url.trim().is_empty())
            {
                return Some(rust_i18n::t!("ai.providerEndpointRequired").to_string());
            }
            if credential.base_url.as_deref().is_some_and(|base| {
                url::Url::parse(base.trim()).map_or(true, |url| {
                    !matches!(url.scheme(), "http" | "https") || url.host_str().is_none()
                })
            }) {
                return Some(rust_i18n::t!("ai.providerEndpointInvalid").to_string());
            }
            if provider_requires_api_key(credential)
                && credential
                    .api_key
                    .as_deref()
                    .is_none_or(|key| key.is_empty())
            {
                return Some(rust_i18n::t!("ai.providerApiKeyRequired").to_string());
            }
        }
        None
    }
}

fn decode_provider_icon(url: &str) -> Result<std::sync::Arc<gpui::RenderImage>, String> {
    use base64::Engine as _;
    let (_, encoded) = url.split_once(',').ok_or("Invalid icon")?;
    if encoded.len() > 2 * 1024 * 1024 * 4 / 3 + 4 {
        return Err("Icon exceeds 2 MiB".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "Invalid icon encoding")?;
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "Invalid icon")?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let mut pixels = reader
        .decode()
        .map_err(|_| "Invalid or oversized icon")?
        .thumbnail(128, 128)
        .to_rgba8();
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(std::sync::Arc::new(gpui::RenderImage::new(vec![
        image::Frame::new(pixels),
    ])))
}
