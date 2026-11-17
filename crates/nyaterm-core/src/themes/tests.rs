use super::{ThemeCatalog, ThemeConfig, builtins, parse_color, resolve_theme};
use serde_json::json;

#[test]
fn windterm_themes_are_read_only_builtins_with_editable_copies() {
    for id in ["windterm-dark", "windterm-light"] {
        assert!(crate::themes::is_builtin(id));
        let mut catalog = ThemeCatalog::default();
        let theme = resolve_theme(id, &catalog);
        assert_eq!(theme.id, id);
        assert!(theme.validate().is_empty());
        assert!(catalog.upsert(theme.clone()).is_err());
        let copy = theme.copy_as_custom();
        catalog.upsert(copy.clone()).unwrap();
        assert_eq!(resolve_theme(&copy.id, &catalog).colors, theme.colors);
    }
}

#[test]
fn windterm_application_preserves_other_scopes_and_existing_custom_themes() {
    use crate::models::settings::AppSettingsSummary;
    use crate::themes::management::{ThemeApplyTarget, ThemeMutation, ThemeSettings};

    let mut original = AppSettingsSummary::default();
    let mut legacy = builtins()[0].copy_as_custom();
    legacy.id = "windterm-dige-black".into();
    original.custom_themes.upsert(legacy.clone()).unwrap();
    original.terminal_theme = Some(legacy.id.clone());
    let base = ThemeSettings::from(&original);
    for id in ["windterm-dark", "windterm-light"] {
        for target in [
            ThemeApplyTarget::Ui,
            ThemeApplyTarget::Terminal,
            ThemeApplyTarget::BothFollowUi,
        ] {
            let commit = ThemeMutation::Apply {
                id: id.into(),
                target,
            }
            .prepare(&base)
            .unwrap();
            let mut settings = original.clone();
            commit.merge_into(&mut settings);
            assert_eq!(
                settings.theme,
                if target == ThemeApplyTarget::Terminal {
                    original.theme.as_str()
                } else {
                    id
                }
            );
            assert_eq!(
                settings.terminal_theme.as_deref(),
                match target {
                    ThemeApplyTarget::Ui => Some(legacy.id.as_str()),
                    ThemeApplyTarget::Terminal => Some(id),
                    ThemeApplyTarget::BothFollowUi => None,
                }
            );
            assert_eq!(settings.custom_themes, original.custom_themes);
        }
    }
}

#[test]
fn theme_application_changes_only_the_requested_scope() {
    use crate::themes::management::{ThemeApplyTarget, ThemeMutation, ThemeSettings};
    let original = crate::models::settings::AppSettingsSummary::default();
    let base = ThemeSettings::from(&original);
    for target in [
        ThemeApplyTarget::Ui,
        ThemeApplyTarget::Terminal,
        ThemeApplyTarget::BothFollowUi,
    ] {
        let commit = ThemeMutation::Apply {
            id: "dracula".into(),
            target,
        }
        .prepare(&base)
        .unwrap();
        let mut draft = original.clone();
        draft.terminal_font_size = 31;
        draft.background_image_path = Some("draft.png".into());
        commit.merge_into(&mut draft);
        assert_eq!(draft.terminal_font_size, 31);
        assert_eq!(draft.background_image_path.as_deref(), Some("draft.png"));
        assert_eq!(
            draft.theme,
            if target == ThemeApplyTarget::Terminal {
                &original.theme
            } else {
                "dracula"
            }
        );
        assert_eq!(
            draft.terminal_theme.as_deref(),
            if target == ThemeApplyTarget::Terminal {
                Some("dracula")
            } else {
                None
            }
        );
    }
}

#[test]
fn saving_and_deleting_an_active_theme_repairs_references_without_losing_unknown_records() {
    use crate::themes::management::{ThemeApplyTarget, ThemeMutation, ThemeSettings};
    let base = ThemeSettings::from(&crate::models::settings::AppSettingsSummary::default());
    let theme = builtins()[0].copy_as_custom();
    let id = theme.id.clone();
    let mut saved = ThemeMutation::Save {
        theme: Box::new(theme),
        target: Some(ThemeApplyTarget::BothFollowUi),
    }
    .prepare(&base)
    .unwrap();
    saved.settings.terminal = Some(id.clone());
    saved
        .settings
        .catalog
        .0
        .as_array_mut()
        .unwrap()
        .push(json!({"future": true}));
    let deleted = ThemeMutation::Delete { id }
        .prepare(&saved.settings)
        .unwrap();
    assert_eq!(deleted.settings.ui, crate::themes::DEFAULT_THEME_ID);
    assert_eq!(deleted.settings.terminal, None);
    assert_eq!(deleted.settings.catalog.0, json!([{"future": true}]));
    assert!(
        ThemeMutation::Delete {
            id: "github-dark".into()
        }
        .prepare(&base)
        .is_err()
    );
}

#[test]
fn theme_manager_repairs_automatic_references_and_explicit_ui_application_is_manual() {
    use crate::models::settings::AppSettingsSummary;
    use crate::themes::appearance::ThemeMode;
    use crate::themes::management::{ThemeApplyTarget, ThemeMutation, ThemeSettings};
    let theme = builtins()[0].copy_as_custom();
    let id = theme.id.clone();
    let mut summary = AppSettingsSummary {
        theme_mode: ThemeMode::System,
        light_theme: id.clone(),
        dark_theme: id.clone(),
        ..Default::default()
    };
    summary.custom_themes.upsert(theme).unwrap();
    let base = ThemeSettings::from(&summary);
    let deleted = ThemeMutation::Delete { id }.prepare(&base).unwrap();
    deleted.merge_into(&mut summary);
    assert_eq!(summary.theme_mode, ThemeMode::System);
    assert_eq!(summary.light_theme, "github-light");
    assert_eq!(summary.dark_theme, "github-dark");
    let applied = ThemeMutation::Apply {
        id: "nord".into(),
        target: ThemeApplyTarget::Ui,
    }
    .prepare(&base)
    .unwrap();
    applied.merge_into(&mut summary);
    assert_eq!(summary.theme_mode, ThemeMode::Manual);
    assert_eq!(summary.theme, "nord");
}

#[test]
fn tauri_builtins_and_optional_intense_foreground_round_trip() {
    for original in builtins() {
        assert!(
            original.validate().is_empty(),
            "{}: {:?}",
            original.id,
            original.validate()
        );
        let value = serde_json::to_value(original).unwrap();
        let loaded: ThemeConfig = serde_json::from_value(value).unwrap();
        assert_eq!(&loaded, original);
    }
    let mut theme = builtins()[0].clone();
    theme.colors.terminal.foreground_intense = None;
    assert_eq!(
        theme.color(super::ThemeColorField::TerminalForegroundIntense),
        theme.colors.terminal.foreground
    );
}
#[test]
fn catalog_preserves_unknown_fields_and_unsupported_records() {
    let mut raw = serde_json::to_value(builtins()[0].copy_as_custom()).unwrap();
    raw["future"] = json!({"version": 9});
    raw["colors"]["futureColor"] = json!("#112233");
    raw["colors"]["terminal"]["futureEffect"] = json!(true);
    let broken = json!({"id":"broken", "future": [1,2,3]});
    let mut catalog = ThemeCatalog(json!([raw, broken]));
    let id = catalog.records()[0]["id"].as_str().unwrap().to_string();
    let mut theme = catalog.resolve(&id).unwrap();
    theme.name = "Edited".into();
    catalog.upsert(theme).unwrap();
    assert_eq!(catalog.records()[0]["future"], json!({"version":9}));
    assert_eq!(catalog.records()[0]["colors"]["futureColor"], "#112233");
    assert_eq!(
        catalog.records()[0]["colors"]["terminal"]["futureEffect"],
        true
    );
    assert_eq!(catalog.records()[1], broken);
    assert!(catalog.resolve("broken").is_none());
    assert_eq!(resolve_theme("broken", &catalog).id, "github-dark");
}
#[test]
fn import_collisions_get_new_ids_and_invalid_colors_do_not_mutate_catalog() {
    let mut catalog = ThemeCatalog::default();
    let mut theme = builtins()[0].clone();
    theme.normalize_import(&catalog);
    assert!(theme.id.starts_with("custom-"));
    catalog.upsert(theme.clone()).unwrap();
    let first_id = theme.id.clone();
    theme.normalize_import(&catalog);
    assert_ne!(theme.id, first_id);
    let before = catalog.clone();
    theme.colors.primary = "invalid".into();
    assert!(catalog.upsert(theme).is_err());
    assert_eq!(catalog, before);
}

#[test]
fn editing_and_deleting_a_valid_theme_preserve_damaged_records_with_the_same_id() {
    let mut theme = builtins()[0].copy_as_custom();
    let damaged = json!({ "id": theme.id, "future": true });
    let mut catalog = ThemeCatalog(json!([damaged, theme]));
    theme.name = "Updated".into();
    catalog.upsert(theme.clone()).unwrap();
    assert_eq!(catalog.records()[0], damaged);
    assert_eq!(catalog.resolve(&theme.id), Some(theme.clone()));
    catalog.remove(&theme.id);
    assert_eq!(catalog.records(), &[damaged]);
    let before = catalog.clone();
    assert!(catalog.upsert(theme).is_err());
    assert_eq!(catalog, before);
}
#[test]
fn css_colors_accept_tauri_alpha_syntax() {
    assert_eq!(parse_color("rgb(1 4 9 / 0.45)"), Some([1, 4, 9, 115]));
    assert_eq!(
        parse_color("rgba(210, 153, 34, 0.35)"),
        Some([210, 153, 34, 89])
    );
    assert!(parse_color("not-a-color").is_none());
}

#[test]
fn unsupported_catalog_records_have_diagnostics_without_affecting_valid_themes() {
    let valid = builtins()[0].copy_as_custom();
    let mut invalid = valid.clone();
    invalid.id = "broken-colors".into();
    invalid.colors.terminal.red = "invalid".into();
    let catalog = ThemeCatalog(json!([
        valid,
        invalid,
        { "id": "future-theme", "version": 9 },
        valid
    ]));
    let diagnostics = catalog.diagnostics();
    assert_eq!(diagnostics.len(), 3);
    assert!(diagnostics[0].contains("terminal.red"));
    assert!(diagnostics[1].contains("future-theme"));
    assert!(diagnostics[2].contains("id: duplicate"));
    assert_eq!(catalog.available(), vec![valid]);
    assert_eq!(ThemeCatalog(json!({"version": 9})).diagnostics().len(), 1);
}

#[test]
fn imports_with_missing_metadata_get_tauri_compatible_defaults() {
    let mut value = serde_json::to_value(&builtins()[0]).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("id");
    object.remove("name");
    object.remove("label");
    let mut theme: ThemeConfig = serde_json::from_value(value).unwrap();
    theme.normalize_import(&ThemeCatalog::default());
    assert!(theme.id.starts_with("custom-"));
    assert_eq!(theme.name, "Imported Theme");
    assert!(theme.validate().is_empty());
}
