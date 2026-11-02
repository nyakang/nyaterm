//! Theme drafts are local; confirmed theme operations persist independently of settings.
use gpui::{Context, PathPromptOptions};
use nyaterm_core::themes::management::{ThemeApplyTarget, ThemeMutation, ThemeSettings};
use nyaterm_core::themes::{COLOR_FIELDS, ThemeConfig, contrast_ratio, is_builtin, resolve_theme};
use nyaterm_store::{StoreDomain, ThemeCommitOutcome, store_mutation};
use nyaterm_ui::ThemePalette;
use rust_i18n::t;

use crate::features::{NyaTermApp, runtime_jobs::await_blocking_job, text_inputs::TextInputSetup};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::features) enum ThemeIntent {
    Toggle,
    Select(String),
    Copy(String),
    Import,
    Delete,
    Apply(bool),
    CloseSettings,
    Quit(usize),
    Reload,
}

/// Text contrast below this ratio is reported next to the preview.
pub(in crate::features) const MIN_TEXT_CONTRAST: f32 = 4.5;

#[derive(Clone, Default, PartialEq)]
pub(in crate::features) struct ThemeDesignerState {
    pub open: bool,
    pub submitting: bool,
    pub delete_confirmation: bool,
    pub close_confirmation: bool,
    pub restore_focus: bool,
    expected: Option<ThemeSettings>,
    pub draft: Option<ThemeConfig>,
    baseline: Option<ThemeConfig>,
    pub terminal_tab: bool,
    pub errors: Vec<String>,
    pub preview: Option<ThemePalette>,
    /// Set only when the ratio is below [`MIN_TEXT_CONTRAST`].
    pub ui_contrast: Option<f32>,
    pub terminal_contrast: Option<f32>,
    pub pending: Option<ThemeIntent>,
    pub message: String,
    pub generation: u64,
}

impl ThemeDesignerState {
    pub fn dirty(&self) -> bool {
        self.draft != self.baseline
    }
    /// Built-in themes are read-only; every custom draft is directly editable.
    pub fn editable(&self) -> bool {
        self.draft
            .as_ref()
            .is_some_and(|theme| !is_builtin(&theme.id))
    }
    /// A copy or import that has never been saved to the catalog.
    pub fn unsaved_new(&self) -> bool {
        self.draft.is_some() && self.baseline.is_none()
    }
    pub fn blocked(&self) -> bool {
        self.submitting
            || self.pending.is_some()
            || self.close_confirmation
            || self.delete_confirmation
    }
    pub fn load(&mut self, theme: ThemeConfig) {
        self.generation += 1;
        self.open = true;
        self.restore_focus = false;
        self.delete_confirmation = false;
        self.baseline = Some(theme.clone());
        self.draft = Some(theme);
        self.pending = None;
        self.message.clear();
        self.recompute();
    }
    pub fn reset(&mut self) {
        let generation = self.generation + 1;
        *self = Self {
            generation,
            ..Default::default()
        };
    }
    pub fn edit(&mut self, field: &str, value: String) {
        if self.submitting || !self.editable() {
            return;
        }
        let Some(theme) = self.draft.as_mut() else {
            return;
        };
        match field {
            "name" => theme.name = value,
            "label" => theme.label = value.chars().take(14).collect(),
            "swatch" => theme.swatch = value,
            path => {
                if let Some(field) = COLOR_FIELDS.iter().find(|f| f.path() == path) {
                    theme.set_color(*field, value);
                }
            }
        }
        self.generation += 1;
        self.recompute();
    }
    fn recompute(&mut self) {
        let Some(theme) = self.draft.as_ref() else {
            self.preview = None;
            self.ui_contrast = None;
            self.terminal_contrast = None;
            self.errors.clear();
            return;
        };
        self.errors = theme.validate();
        let low = |ratio: Option<f32>| ratio.filter(|ratio| *ratio < MIN_TEXT_CONTRAST);
        self.ui_contrast = low(contrast_ratio(&theme.colors.text, &theme.colors.bg));
        self.terminal_contrast = low(contrast_ratio(
            &theme.colors.terminal.foreground,
            &theme.colors.terminal.background,
        ));
        if self.errors.is_empty() {
            self.preview = Some(nyaterm_ui::palette_from_theme(theme));
        }
    }
    pub fn saved(&mut self) {
        self.baseline = self.draft.clone();
    }
}

impl NyaTermApp {
    /// Whether quitting would silently drop a theme draft in this window.
    ///
    /// Settings is a per-window tab now, so more than one workspace can hold a
    /// theme draft; the process quit gate must visit each of them.
    pub(crate) fn has_unconfirmed_theme_changes(&self) -> bool {
        let state = self.settings.theme_designer();
        state.dirty() || state.submitting
    }

    pub(in crate::features) fn theme_designer_action(
        &mut self,
        intent: ThemeIntent,
        cx: &mut Context<Self>,
    ) {
        if self.settings.theme_designer().submitting {
            return;
        }
        if self.settings.theme_designer().pending.is_some() {
            return;
        }
        if self.settings.theme_designer().expected.is_none() {
            let expected = ThemeSettings::from(
                self.shell
                    .settings_draft_snapshot()
                    .map(|snapshot| &snapshot.settings)
                    .unwrap_or(self.settings.summary()),
            );
            self.settings.theme_designer_mut().expected = Some(expected);
        }
        if self.settings.theme_designer().dirty() {
            self.settings.theme_designer_mut().pending = Some(intent);
            self.focus_settings_tab(crate::models::SettingsTab::Appearance, cx);
            self.request_settings_panel_refresh(cx);
            return;
        }
        self.perform_theme_intent(intent, cx);
    }

    pub(in crate::features) fn resolve_theme_designer_pending(
        &mut self,
        save: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        let pending = self.settings.theme_designer_mut().pending.take();
        if save.is_none() && matches!(pending, Some(ThemeIntent::Quit(_))) {
            cx.emit(crate::features::AppLifecycleEvent::ShutdownCancelled);
        }
        if let Some(save) = save {
            if save {
                self.settings.theme_designer_mut().pending = pending;
                self.save_theme_designer(None, cx);
            } else if let Some(intent) = pending {
                let baseline = self.settings.theme_designer().baseline.clone();
                let editor = self.settings.theme_designer_mut();
                editor.draft = baseline;
                editor.generation += 1;
                editor.recompute();
                self.perform_theme_intent(intent, cx);
            }
        }
        self.request_settings_panel_refresh(cx);
    }

    fn perform_theme_intent(&mut self, intent: ThemeIntent, cx: &mut Context<Self>) {
        match intent {
            ThemeIntent::Toggle => {
                if self.settings.theme_designer().open {
                    self.settings.theme_designer_mut().reset();
                    self.settings.theme_designer_mut().restore_focus = true;
                } else {
                    let summary = self.settings.summary();
                    let theme = resolve_theme(&summary.theme, &summary.custom_themes);
                    self.settings.theme_designer_mut().load(theme);
                }
            }
            ThemeIntent::Select(id) => {
                let theme = resolve_theme(&id, &self.settings.summary().custom_themes);
                self.settings.theme_designer_mut().load(theme);
            }
            ThemeIntent::Copy(id) => {
                let theme =
                    resolve_theme(&id, &self.settings.summary().custom_themes).copy_as_custom();
                self.settings.theme_designer_mut().load(theme);
                self.settings.theme_designer_mut().baseline = None;
            }
            ThemeIntent::Import => {
                self.prompt_theme_import(cx);
            }
            ThemeIntent::Delete => {
                self.settings.theme_designer_mut().delete_confirmation = true;
            }
            ThemeIntent::Apply(close) => self.apply_settings_draft(close, cx),
            ThemeIntent::Reload => {
                self.reload_theme_manager(cx);
            }
            ThemeIntent::CloseSettings => {
                if self.settings_draft_dirty() {
                    self.settings.theme_designer_mut().open = true;
                    self.settings.theme_designer_mut().close_confirmation = true;
                } else {
                    self.cancel_settings(cx);
                }
            }
            ThemeIntent::Quit(count) => {
                if let Some(handle) = self.shell.main_window() {
                    let app = cx.entity();
                    cx.defer(move |cx| {
                        // The next gate may open an ordinary-settings dialog,
                        // which updates NyaRoot itself. Do not lease the root.
                        let _ = gpui::AnyWindowHandle::from(handle).update(cx, |_, window, cx| {
                            app.update(cx, |app, cx| {
                                app.handle_window_close_request_with_count(count, window, cx)
                            });
                        });
                    });
                }
            }
        }
        self.ensure_theme_designer_inputs(cx);
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }

    pub(in crate::features) fn edit_theme_designer(
        &mut self,
        field: &str,
        value: String,
        cx: &mut Context<Self>,
    ) {
        self.settings.theme_designer_mut().edit(field, value);
        self.request_settings_panel_refresh(cx);
    }

    /// Reverts to the saved theme, or abandons a never-saved copy for the active UI theme.
    pub(in crate::features) fn discard_theme_designer(&mut self, cx: &mut Context<Self>) {
        let state = self.settings.theme_designer();
        if state.blocked() || !state.dirty() {
            return;
        }
        let theme = state.baseline.clone().unwrap_or_else(|| {
            let summary = self.settings.summary();
            resolve_theme(&summary.theme, &summary.custom_themes)
        });
        self.settings.theme_designer_mut().load(theme);
        self.ensure_theme_designer_inputs(cx);
        self.request_settings_panel_refresh(cx);
    }

    pub(in crate::features) fn ensure_theme_designer_inputs(&mut self, cx: &mut Context<Self>) {
        if !self.settings.theme_designer().open || !self.settings.theme_designer().editable() {
            return;
        }
        let Some(theme) = self.settings.theme_designer().draft.clone() else {
            return;
        };
        let mut fields = vec![
            ("name", theme.name.as_str()),
            ("label", theme.label.as_str()),
            ("swatch", theme.swatch.as_str()),
        ];
        fields.extend(COLOR_FIELDS.iter().map(|f| (f.path(), theme.color(*f))));
        for (field, value) in fields {
            let id = format!("theme.designer.{field}");
            self.ensure_text_input(id.clone(), value, TextInputSetup::default(), cx);
            if let Some(input) = self.existing_text_input(id)
                && input.read(cx).value(cx) != value
            {
                input.update(cx, |input, cx| input.set_content_silent(value, cx));
            }
        }
    }

    pub(in crate::features) fn save_theme_designer(
        &mut self,
        target: Option<ThemeApplyTarget>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(theme) = self.settings.theme_designer().draft.clone() else {
            return false;
        };
        let mutation = if self.settings.theme_designer().dirty() {
            ThemeMutation::Save {
                theme: Box::new(theme),
                target,
            }
        } else if let Some(target) = target {
            ThemeMutation::Apply {
                id: theme.id,
                target,
            }
        } else {
            return false;
        };
        self.submit_theme_mutation(mutation, cx)
    }

    pub(in crate::features) fn confirm_theme_delete(&mut self, cx: &mut Context<Self>) {
        if !self.settings.theme_designer().delete_confirmation {
            return;
        }
        let Some(theme) = self.settings.theme_designer().draft.as_ref() else {
            return;
        };
        self.submit_theme_mutation(
            ThemeMutation::Delete {
                id: theme.id.clone(),
            },
            cx,
        );
    }

    fn submit_theme_mutation(&mut self, mutation: ThemeMutation, cx: &mut Context<Self>) -> bool {
        if self.settings.theme_designer().submitting {
            return false;
        }
        let expected = self
            .settings
            .theme_designer()
            .expected
            .clone()
            .unwrap_or_else(|| {
                ThemeSettings::from(
                    self.shell
                        .settings_draft_snapshot()
                        .map(|snapshot| &snapshot.settings)
                        .unwrap_or(self.settings.summary()),
                )
            });
        if let Err(error) = mutation.prepare(&expected) {
            self.settings.theme_designer_mut().message = error;
            self.request_settings_panel_refresh(cx);
            return false;
        }
        let revision = self
            .process_state
            .read(cx)
            .settings_draft_revisions()
            .settings;
        let deleting = match &mutation {
            ThemeMutation::Delete { id } => Some(id.clone()),
            _ => None,
        };
        let task = self.store_ui.try_submit(
            0,
            store_mutation(StoreDomain::Settings, move |store| {
                let result = store.commit_theme_mutation(&expected, &mutation)?;
                let saved = store.load_app_settings_summary()?;
                Ok((result, saved))
            }),
        );
        let task = match task {
            Ok(task) => task,
            Err(error) => {
                self.settings.theme_designer_mut().message = error.to_string();
                self.request_settings_panel_refresh(cx);
                return false;
            }
        };
        self.settings.theme_designer_mut().submitting = true;
        self.settings.theme_designer_mut().generation += 1;
        self.request_settings_panel_refresh(cx);
        cx.spawn(async move |this, cx| {
            let event = task.await;
            let _ = this.update(cx, |this, cx| {
                match event.outcome {
                    Ok((ThemeCommitOutcome::Committed(commit), saved)) => {
                        this.settings.merge_theme_commit(&commit);
                        if let Some(id) = deleting.as_deref() {
                            if this.settings.summary().theme == id {
                                this.settings.set_appearance_theme(
                                    nyaterm_core::themes::DEFAULT_THEME_ID.into(),
                                );
                            }
                            if this.settings.summary().terminal_theme.as_deref() == Some(id) {
                                this.settings.set_terminal_theme(None);
                            }
                        }
                        if let Some(mut snapshot) = this.shell.take_settings_draft_snapshot() {
                            commit.merge_into(&mut snapshot.settings);
                            this.shell.set_settings_draft_snapshot(snapshot);
                        }
                        this.settings.theme_designer_mut().saved();
                        this.settings.theme_designer_mut().expected = Some(commit.settings);
                        this.settings.theme_designer_mut().delete_confirmation = false;
                        this.settings.theme_designer_mut().message =
                            t!("themeDesigner.saved").to_string();
                        if deleting.is_some() {
                            let summary = this.settings.summary();
                            let theme = resolve_theme(&summary.theme, &summary.custom_themes);
                            this.settings.theme_designer_mut().load(theme);
                            this.ensure_theme_designer_inputs(cx);
                        }
                        this.custom_themes_changed(cx);
                        // Publishing is deferred to avoid updating a shell already on the stack.
                        // Only this publication may advance our settings draft revision.
                        if let Some(controller) = this.desktop_controller.clone() {
                            let source = this.workspace_id;
                            let app = cx.entity();
                            cx.defer(move |cx| {
                                let before = app
                                    .read(cx)
                                    .process_state
                                    .read(cx)
                                    .settings_draft_revisions()
                                    .settings;
                                let _ = controller.update(cx, |controller, cx| {
                                    controller.publish_theme_settings(source, saved, cx)
                                });
                                app.update(cx, |app, cx| {
                                    let after = app
                                        .process_state
                                        .read(cx)
                                        .settings_draft_revisions()
                                        .settings;
                                    if before == revision
                                        && let Some(mut snapshot) =
                                            app.shell.take_settings_draft_snapshot()
                                    {
                                        if snapshot.revisions.settings == revision {
                                            snapshot.revisions.settings = after;
                                        }
                                        app.shell.set_settings_draft_snapshot(snapshot);
                                    }
                                    app.finish_theme_pending(cx);
                                });
                            });
                        } else {
                            this.finish_theme_pending(cx);
                        }
                    }
                    Ok((ThemeCommitOutcome::Conflict, _)) => {
                        this.settings.theme_designer_mut().submitting = false;
                        this.settings.theme_designer_mut().message =
                            t!("themeDesigner.conflict").to_string();
                    }
                    Err(error) => {
                        this.settings.theme_designer_mut().submitting = false;
                        this.settings.theme_designer_mut().message = error.to_string();
                    }
                }
                this.request_settings_panel_refresh(cx);
            });
        })
        .detach();
        true
    }

    fn finish_theme_pending(&mut self, cx: &mut Context<Self>) {
        self.settings.theme_designer_mut().submitting = false;
        if let Some(intent) = self.settings.theme_designer_mut().pending.take() {
            self.perform_theme_intent(intent, cx);
        }
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }

    fn reload_theme_manager(&mut self, cx: &mut Context<Self>) {
        let task = self.store_ui.try_submit(
            0,
            nyaterm_store::store_request(StoreDomain::Settings, |store| {
                store.load_app_settings_summary()
            }),
        );
        let task = match task {
            Ok(task) => task,
            Err(error) => {
                self.settings.theme_designer_mut().message = error.to_string();
                self.request_settings_panel_refresh(cx);
                return;
            }
        };
        self.settings.theme_designer_mut().submitting = true;
        cx.spawn(async move |this, cx| {
            let event = task.await;
            let _ = this.update(cx, |this, cx| {
                this.settings.theme_designer_mut().submitting = false;
                match event.outcome {
                    Ok(saved) => {
                        let commit = nyaterm_core::themes::management::ThemeCommit {
                            settings: ThemeSettings::from(&saved),
                            fields: nyaterm_core::themes::management::ThemeChangedFields {
                                catalog: true,
                                ui: true,
                                terminal: true,
                            },
                        };
                        this.settings.merge_theme_commit(&commit);
                        if let Some(mut snapshot) = this.shell.take_settings_draft_snapshot() {
                            commit.merge_into(&mut snapshot.settings);
                            this.shell.set_settings_draft_snapshot(snapshot);
                        }
                        let theme = resolve_theme(&saved.theme, &saved.custom_themes);
                        this.settings.theme_designer_mut().expected = Some(commit.settings);
                        this.settings.theme_designer_mut().load(theme);
                        this.ensure_theme_designer_inputs(cx);
                        this.custom_themes_changed(cx);
                    }
                    Err(error) => this.settings.theme_designer_mut().message = error.to_string(),
                }
                this.request_settings_panel_refresh(cx);
            });
        })
        .detach();
    }

    fn custom_themes_changed(&mut self, cx: &mut Context<Self>) {
        self.invalidate_paint_theme_caches();
        self.sync_component_theme(cx);
        self.refresh_visible_terminal_surfaces(cx);
        self.defer_connection_panel_snapshot_flush(cx);
        self.defer_remote_panel_snapshot_flush(cx);
        self.defer_transfer_panel_snapshot_flush(cx);
        self.defer_ai_panel_snapshot_flush(cx);
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }

    fn prompt_theme_import(&mut self, cx: &mut Context<Self>) {
        self.settings.theme_designer_mut().generation += 1;
        let generation = self.settings.theme_designer().generation;
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("themeDesigner.import").into()),
        });
        let scheduler = self.blocking_jobs.clone();
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(Ok(Some(paths))) => match paths.into_iter().next() {
                    Some(path) => await_blocking_job(
                        scheduler.submit_task("theme-import", move |_| read_theme(&path)),
                    )
                    .await
                    .and_then(|r| r)
                    .map(Some),
                    None => Ok(None),
                },
                Ok(Ok(None)) => Ok(None),
                Ok(Err(error)) => Err(error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            let _ = this.update(cx, |this, cx| {
                this.finish_theme_import(generation, result, cx);
            });
        })
        .detach();
    }

    pub(in crate::features) fn finish_theme_import(
        &mut self,
        generation: u64,
        result: Result<Option<ThemeConfig>, String>,
        cx: &mut Context<Self>,
    ) {
        if self.settings.theme_designer().generation != generation
            || self.settings.theme_designer().submitting
        {
            return;
        }
        match result {
            Ok(Some(mut theme)) => {
                theme.normalize_import(&self.settings.summary().custom_themes);
                if theme.validate().is_empty() {
                    self.settings.theme_designer_mut().load(theme);
                    self.settings.theme_designer_mut().baseline = None;
                    self.ensure_theme_designer_inputs(cx);
                } else {
                    self.settings.theme_designer_mut().message =
                        t!("themeDesigner.invalidColor").to_string();
                }
            }
            Ok(None) => {}
            Err(error) => self.settings.theme_designer_mut().message = error,
        }
        self.request_settings_panel_refresh(cx);
    }

    pub(in crate::features) fn export_theme_designer(&mut self, cx: &mut Context<Self>) {
        let Some(theme) = self
            .settings
            .theme_designer()
            .draft
            .clone()
            .filter(|t| t.validate().is_empty())
        else {
            return;
        };
        self.settings.theme_designer_mut().generation += 1;
        let generation = self.settings.theme_designer().generation;
        let receiver =
            cx.prompt_for_new_path(self.runtime.config_dir(), Some("nyaterm-theme.json"));
        let scheduler = self.blocking_jobs.clone();
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(Ok(Some(path))) => await_blocking_job(
                    scheduler.submit_task("theme-export", move |_| write_theme(&path, &theme)),
                )
                .await
                .and_then(|r| r),
                Ok(Ok(None)) => return,
                Ok(Err(error)) => Err(error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            let _ = this.update(cx, |this, cx| {
                if this.settings.theme_designer().generation != generation {
                    return;
                }
                this.settings.theme_designer_mut().message = match result {
                    Ok(()) => t!("themeDesigner.exported").to_string(),
                    Err(error) => error,
                };
                this.request_settings_panel_refresh(cx);
            });
        })
        .detach();
    }
}

fn read_theme(path: &std::path::Path) -> Result<ThemeConfig, String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 1024 * 1024 {
        return Err("Theme JSON exceeds 1 MiB".into());
    }
    let theme: ThemeConfig = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    Ok(theme)
}

fn write_theme(path: &std::path::Path, theme: &ThemeConfig) -> Result<(), String> {
    use std::io::Write;
    let errors = theme.validate();
    if !errors.is_empty() {
        return Err(errors.join(", "));
    }
    let parent = path.parent().ok_or("Invalid theme path")?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut file, theme).map_err(|e| e.to_string())?;
    file.write_all(b"\n").map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ThemeDesignerState, read_theme, write_theme};
    use nyaterm_core::themes::builtins;
    #[test]
    fn invalid_edits_keep_the_last_valid_preview_and_never_mark_saved() {
        let mut state = ThemeDesignerState::default();
        state.load(builtins()[0].copy_as_custom());
        let preview = state.preview;
        let generation = state.generation;
        state.edit("primary", "broken".into());
        assert!(state.dirty());
        assert!(state.errors.iter().any(|field| field == "primary"));
        assert_eq!(state.preview, preview);
        assert!(state.generation > generation);
        state.edit("primary", "#123456".into());
        assert!(state.errors.is_empty());
        assert_eq!(state.preview.unwrap().primary, 0x123456);
        state.saved();
        assert!(!state.dirty());
        let generation = state.generation;
        state.reset();
        assert!(state.generation > generation);
        assert!(!state.open);
    }
    #[test]
    fn theme_files_round_trip_without_saving_settings_and_reject_invalid_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.json");
        let theme = builtins()[0].copy_as_custom();
        write_theme(&path, &theme).unwrap();
        assert_eq!(read_theme(&path).unwrap(), theme);
        let before = std::fs::read(&path).unwrap();
        let mut invalid = theme.clone();
        invalid.colors.primary = "invalid".into();
        assert!(write_theme(&path, &invalid).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::write(&path, b"not JSON").unwrap();
        assert!(read_theme(&path).is_err());
        std::fs::write(&path, vec![b' '; 1024 * 1024 + 1]).unwrap();
        assert!(read_theme(&path).unwrap_err().contains("1 MiB"));
    }
}
