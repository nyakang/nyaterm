use rust_i18n::t;

use gpui::{Context, KeyDownEvent, PathPromptOptions, SharedString, Window};
use nyaterm_core::KeywordHighlightRule;
use nyaterm_store::{StoreDomain, store_request};

use crate::features::{NyaTermApp, runtime_jobs::await_blocking_job, text_inputs::TextInputSetup};
use crate::models::{KeywordHighlightEditorField, KeywordHighlightPathPromptResult};

const MAX_KEYWORD_HIGHLIGHT_IMPORT_BYTES: u64 = 4 * 1024 * 1024;

impl NyaTermApp {
    pub(in crate::features) fn toggle_keyword_highlights(&mut self, cx: &mut Context<Self>) {
        let enabled = self.settings.toggle_keyword_highlights();
        if !enabled {
            self.settings.clear_keyword_highlight_edit();
            self.forget_text_inputs("keyword.highlight.");
        }
        self.save_keyword_highlights(cx);
    }

    fn save_keyword_highlights(&mut self, cx: &mut Context<Self>) {
        self.invalidate_paint_theme_caches();
        if self.defer_settings_persistence(cx) {
            return;
        }
        let config = self.settings.keyword_config().clone();
        self.submit_store_request(
            0,
            store_request(StoreDomain::Settings, move |store| {
                store.save_keyword_highlights(&config)
            }),
            |this, event, cx| {
                match event.outcome {
                    Ok(config) => {
                        this.settings.replace_keyword_config(config);
                        this.settings
                            .update_store_status("keyword highlight settings saved", true);
                    }
                    Err(error) => this.settings.update_store_status(
                        format!("keyword highlight settings save failed: {error}"),
                        false,
                    ),
                }
                this.shell
                    .set_status(this.settings.store_status().message.to_string());
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn prompt_keyword_highlight_import(&mut self, cx: &mut Context<Self>) {
        if self.block_import_for_settings_draft(cx) {
            return;
        }
        if !self.settings.begin_keyword_highlight_path_prompt() {
            self.shell
                .set_status("keyword highlight import picker is already open".to_string());
            cx.notify();
            return;
        }
        let options = PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(SharedString::from("Import keyword highlight JSON")),
        };
        let store = self.store_blocking_client();
        let scheduler = self.blocking_jobs.clone();
        let receiver = cx.prompt_for_paths(options);
        self.shell
            .set_status("selecting keyword highlight import file".to_string());
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(Ok(Some(paths))) => match paths.into_iter().next() {
                    Some(path) => {
                        let task = scheduler.submit_task("keyword-highlight-import", move |_| {
                            match read_keyword_highlight_import_text(&path) {
                                Ok(raw) => match store
                                    .request_fn(StoreDomain::Settings, move |database| {
                                        database.import_keyword_highlights_json(&raw)
                                    }) {
                                    Ok((_, result)) => KeywordHighlightPathPromptResult::Imported {
                                        imported_rules: result.imported_rules,
                                        updated_rules: result.updated_rules,
                                        total_rules: result.total_rules,
                                    },
                                    Err(error) => {
                                        KeywordHighlightPathPromptResult::Failed(error.to_string())
                                    }
                                },
                                Err(error) => {
                                    KeywordHighlightPathPromptResult::Failed(error.to_string())
                                }
                            }
                        });
                        await_blocking_job(task)
                            .await
                            .unwrap_or_else(KeywordHighlightPathPromptResult::Failed)
                    }
                    None => KeywordHighlightPathPromptResult::Cancelled,
                },
                Ok(Ok(None)) => KeywordHighlightPathPromptResult::Cancelled,
                Ok(Err(error)) => KeywordHighlightPathPromptResult::Failed(error.to_string()),
                Err(_) => KeywordHighlightPathPromptResult::Closed,
            };
            let _ = this.update(cx, |this, cx| {
                this.apply_keyword_highlight_import_result(result, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn apply_keyword_highlight_import_result(
        &mut self,
        result: KeywordHighlightPathPromptResult,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.finish_keyword_highlight_path_prompt() {
            return;
        }
        match result {
            KeywordHighlightPathPromptResult::Imported {
                imported_rules,
                updated_rules,
                total_rules,
            } => {
                self.refresh_keyword_highlights(cx);
                self.rebase_open_settings_draft(cx);
                self.shell.set_status(format!(
                    "imported {imported_rules} keyword highlight rule(s), updated {updated_rules}, total {total_rules}"
                ));
                self.settings
                    .update_store_status(self.shell.status().to_string(), true);
            }
            KeywordHighlightPathPromptResult::Cancelled => {
                self.shell
                    .set_status("keyword highlight import cancelled".to_string());
            }
            KeywordHighlightPathPromptResult::Failed(error) => {
                self.shell
                    .set_status(format!("keyword highlight import failed: {error}"));
                self.settings
                    .update_store_status(self.shell.status().to_string(), false);
            }
            KeywordHighlightPathPromptResult::Closed => {
                self.shell.set_status(
                    "keyword highlight import picker closed before returning".to_string(),
                );
            }
        }
    }

    pub(in crate::features) fn refresh_keyword_highlights(&mut self, cx: &mut Context<Self>) {
        self.invalidate_paint_theme_caches();
        self.submit_store_request(
            0,
            store_request(StoreDomain::Settings, |store| {
                store.load_keyword_highlights()
            }),
            |this, event, cx| {
                match event.outcome {
                    Ok(config) => {
                        this.settings.replace_keyword_config(config);
                        this.forget_text_inputs("keyword.highlight.");
                    }
                    Err(error) => {
                        let message = format!("keyword highlight refresh failed: {error}");
                        this.shell.set_status(message.clone());
                        this.settings.update_store_status(message, false);
                    }
                }
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn toggle_keyword_highlight_builtin(
        &mut self,
        rule_id: String,
        cx: &mut Context<Self>,
    ) {
        self.settings.toggle_keyword_highlight_builtin(rule_id);
        self.save_keyword_highlights(cx);
    }

    pub(in crate::features) fn toggle_keyword_highlight_rule(
        &mut self,
        rule_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.settings.toggle_keyword_highlight_rule(&rule_id) {
            self.save_keyword_highlights(cx);
        }
    }

    /// Build the four inputs an opened keyword row draws.
    ///
    /// The row is the reveal boundary, so it is where they are created; the row's
    /// render only looks them up.
    pub(in crate::features) fn ensure_keyword_highlight_inputs(
        &mut self,
        rule_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(rule) = self
            .settings
            .keyword_config()
            .rules
            .iter()
            .find(|rule| rule.id == rule_id)
            .cloned()
        else {
            return;
        };
        for (field, value, setup) in [
            (
                KeywordHighlightEditorField::Name,
                rule.name.clone(),
                TextInputSetup::placeholder(t!("settings.keywordHighlightNewRule")),
            ),
            (
                KeywordHighlightEditorField::Patterns,
                rule.patterns.join("\n"),
                TextInputSetup::multi_line(""),
            ),
            (
                KeywordHighlightEditorField::ColorDark,
                rule.color_dark.clone(),
                TextInputSetup::placeholder("#rrggbb"),
            ),
            (
                KeywordHighlightEditorField::ColorLight,
                rule.color_light.clone(),
                TextInputSetup::placeholder("#rrggbb"),
            ),
        ] {
            let id = Self::keyword_highlight_text_input_id(rule_id, field);
            self.ensure_text_input(id, &value, setup, cx);
        }
    }

    pub(in crate::features) fn expand_keyword_highlight_rule(
        &mut self,
        rule_id: String,
        cx: &mut Context<Self>,
    ) {
        for forgotten_id in self
            .settings
            .toggle_keyword_highlight_expanded(rule_id.clone())
        {
            self.forget_text_inputs(&keyword_highlight_text_input_prefix(&forgotten_id));
        }
        // Opening a row is what reveals its four fields, so it is what builds them.
        // A row that just closed forgot its inputs above, and re-opening rebuilds.
        if self
            .settings
            .keyword_highlight_presentation()
            .expanded_id
            .as_deref()
            == Some(rule_id.as_str())
        {
            self.ensure_keyword_highlight_inputs(&rule_id, cx);
        }
        cx.notify();
    }

    pub(in crate::features) fn add_keyword_highlight_rule(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = format!(
            "kh-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        self.settings
            .add_keyword_highlight_rule(KeywordHighlightRule {
                id: id.clone(),
                name: "New rule".to_string(),
                patterns: Vec::new(),
                color_dark: "#79c0ff".to_string(),
                color_light: "#0969da".to_string(),
                enabled: true,
            });
        let input = self.text_input(
            keyword_highlight_text_input_id(&id, KeywordHighlightEditorField::Name),
            "New rule",
            TextInputSetup::placeholder(t!("settings.keywordHighlightNewRule")),
            cx,
        );
        window.focus(&input.read(cx).focus_handle(), cx);
        self.save_keyword_highlights(cx);
    }

    pub(in crate::features) fn remove_keyword_highlight_rule(
        &mut self,
        rule_id: String,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.remove_keyword_highlight_rule(&rule_id) {
            return;
        }
        self.forget_text_inputs(&keyword_highlight_text_input_prefix(&rule_id));
        self.save_keyword_highlights(cx);
    }

    pub(in crate::features) fn set_keyword_highlight_rule_color(
        &mut self,
        rule_id: String,
        dark: bool,
        color: &str,
        cx: &mut Context<Self>,
    ) {
        let color = if color.trim().is_empty() {
            if dark { "#79c0ff" } else { "#0969da" }.to_string()
        } else {
            normalize_keyword_highlight_color(color)
        };
        if self
            .settings
            .set_keyword_highlight_rule_color(&rule_id, dark, color.clone())
        {
            let field = if dark {
                KeywordHighlightEditorField::ColorDark
            } else {
                KeywordHighlightEditorField::ColorLight
            };
            self.reset_text_input(
                &keyword_highlight_text_input_id(&rule_id, field),
                &color,
                cx,
            );
            self.save_keyword_highlights(cx);
        }
    }

    pub(in crate::features) fn focus_keyword_highlight_field(
        &mut self,
        rule_id: String,
        field: KeywordHighlightEditorField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = self
            .settings
            .keyword_config()
            .rules
            .iter()
            .find(|rule| rule.id == rule_id)
            .map(|rule| match field {
                KeywordHighlightEditorField::Name => rule.name.clone(),
                KeywordHighlightEditorField::Patterns => rule.patterns.join("\n"),
                KeywordHighlightEditorField::ColorDark => rule.color_dark.clone(),
                KeywordHighlightEditorField::ColorLight => rule.color_light.clone(),
            })
            .unwrap_or_default();
        let setup = match field {
            KeywordHighlightEditorField::Name => {
                TextInputSetup::placeholder(t!("settings.keywordHighlightNewRule"))
            }
            KeywordHighlightEditorField::Patterns => TextInputSetup::multi_line(""),
            KeywordHighlightEditorField::ColorDark | KeywordHighlightEditorField::ColorLight => {
                TextInputSetup::placeholder("#rrggbb")
            }
        };
        let input = self.text_input(
            keyword_highlight_text_input_id(&rule_id, field),
            &value,
            setup,
            cx,
        );
        self.settings.begin_keyword_highlight_edit(rule_id, field);
        window.focus(&input.read(cx).focus_handle(), cx);
        cx.notify();
    }

    pub(in crate::features) fn handle_keyword_highlight_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let interaction = self.settings.keyword_highlight_presentation();
        let Some(rule_id) = interaction.edit_id else {
            return false;
        };
        let field = interaction.edit_field;
        match event.keystroke.key.as_str() {
            "escape" => {
                self.settings.clear_keyword_highlight_edit();
                window.focus(self.settings.keyword_highlight_focus(), cx);
                self.shell
                    .set_status("keyword rule edit cancelled".to_string());
                cx.notify();
            }
            "tab" => {
                self.focus_keyword_highlight_field(rule_id, field.next(), window, cx);
            }
            "enter" if field == KeywordHighlightEditorField::Name => {
                self.focus_keyword_highlight_field(
                    rule_id,
                    KeywordHighlightEditorField::Patterns,
                    window,
                    cx,
                );
            }
            "enter" => {
                self.settings.clear_keyword_highlight_edit();
                window.focus(self.settings.keyword_highlight_focus(), cx);
                self.save_keyword_highlights(cx);
            }
            _ => return false,
        }
        true
    }

    pub(in crate::features) fn apply_keyword_highlight_input(
        &mut self,
        field_id: &str,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let Some((rule_id, field)) = parse_keyword_highlight_text_input_id(field_id) else {
            return;
        };
        let normalized_color = matches!(
            field,
            KeywordHighlightEditorField::ColorDark | KeywordHighlightEditorField::ColorLight
        )
        .then(|| normalize_keyword_highlight_color(&text));
        let value = normalized_color.clone().unwrap_or(text);
        if !self
            .settings
            .apply_keyword_highlight_rule_input(rule_id, field, value)
        {
            return;
        }
        if let Some(color) = normalized_color {
            self.reset_text_input(&keyword_highlight_text_input_id(rule_id, field), &color, cx);
        }
        self.save_keyword_highlights(cx);
    }

    pub(in crate::features) fn keyword_highlight_text_input_id(
        rule_id: &str,
        field: KeywordHighlightEditorField,
    ) -> String {
        keyword_highlight_text_input_id(rule_id, field)
    }
}

fn keyword_highlight_text_input_id(rule_id: &str, field: KeywordHighlightEditorField) -> String {
    format!("keyword.highlight.{rule_id}.{}", field.input_key())
}

fn keyword_highlight_text_input_prefix(rule_id: &str) -> String {
    format!("keyword.highlight.{rule_id}.")
}

fn parse_keyword_highlight_text_input_id(
    field_id: &str,
) -> Option<(&str, KeywordHighlightEditorField)> {
    let (rule_id, field) = field_id.rsplit_once('.')?;
    if rule_id.is_empty() {
        return None;
    }
    Some((rule_id, KeywordHighlightEditorField::from_input_key(field)?))
}

fn normalize_keyword_highlight_color(value: &str) -> String {
    let value = value.trim();
    let digits = value.strip_prefix('#').unwrap_or(value);
    let mut normalized = String::from("#");
    normalized.extend(
        digits
            .chars()
            .filter(char::is_ascii_hexdigit)
            .take(6)
            .map(|ch| ch.to_ascii_lowercase()),
    );
    normalized
}

fn read_keyword_highlight_import_text(path: &std::path::Path) -> Result<String, String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_KEYWORD_HIGHLIGHT_IMPORT_BYTES {
        return Err(format!(
            "import file is too large to import ({} bytes > {} bytes)",
            metadata.len(),
            MAX_KEYWORD_HIGHLIGHT_IMPORT_BYTES
        ));
    }
    std::fs::read_to_string(path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{normalize_keyword_highlight_color, parse_keyword_highlight_text_input_id};
    use crate::models::KeywordHighlightEditorField;

    #[test]
    fn parses_keyword_highlight_rule_ids_containing_dots() {
        assert_eq!(
            parse_keyword_highlight_text_input_id("custom.rule.color-dark"),
            Some(("custom.rule", KeywordHighlightEditorField::ColorDark))
        );
    }

    #[test]
    fn rejects_invalid_keyword_highlight_text_input_ids() {
        for field_id in ["rule", ".name", "rule.unknown"] {
            assert_eq!(parse_keyword_highlight_text_input_id(field_id), None);
        }
    }

    #[test]
    fn normalizes_progressive_keyword_highlight_colors() {
        assert_eq!(normalize_keyword_highlight_color("A2-c4_FF9"), "#a2c4ff");
        assert_eq!(normalize_keyword_highlight_color(""), "#");
    }
}
