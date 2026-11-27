//! Check consumers without compiling GPUI or introducing workspace dependencies.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use nyaterm_i18n::{SharedCatalogBackend, available_locales, preload};
use regex::Regex;
use rust_i18n::Backend;

fn rust_sources(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).expect("consumer sources must be readable") {
            let path = entry.expect("source directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[test]
fn every_literal_translation_key_in_shared_catalog_consumers_exists_in_every_locale() {
    preload().unwrap();
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace crates directory");
    let catalogs: Vec<_> = available_locales()
        .into_iter()
        .map(|locale| {
            let keys = SharedCatalogBackend
                .messages_for_locale(&locale)
                .expect("registered catalog")
                .into_iter()
                .map(|(key, _)| key.into_owned())
                .collect::<HashSet<_>>();
            (locale, keys)
        })
        .collect();

    // This is a literal-call check, not Rust name resolution. Dynamic keys need
    // domain tests (for example desktop's shortcut registry test). Do not use
    // this scan to decide which translations are safe to delete.
    let pattern = Regex::new(r#"\bt!\s*[({\[]\s*"([^"]+)""#).unwrap();
    let mut consumers = BTreeSet::new();
    let mut missing = BTreeSet::new();
    for entry in fs::read_dir(crates).expect("workspace crates must be readable") {
        let path = entry.expect("crate directory entry").path();
        let manifest = path.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let manifest = fs::read_to_string(manifest).expect("crate manifest must be UTF-8");
        if !manifest
            .lines()
            .any(|line| line.trim_start().starts_with("nyaterm-i18n"))
        {
            continue;
        }
        consumers.insert(path.file_name().unwrap().to_string_lossy().into_owned());
        for file in rust_sources(&path.join("src")) {
            let source = fs::read_to_string(&file).expect("source must be UTF-8");
            for captures in pattern.captures_iter(&source) {
                let key = &captures[1];
                for (locale, keys) in &catalogs {
                    if !keys.contains(key) {
                        let relative = file.strip_prefix(crates).unwrap();
                        missing.insert(format!("{locale}/{key} in {}", relative.display()));
                    }
                }
            }
        }
    }
    assert!(consumers.contains("nyaterm-desktop"));
    assert!(consumers.contains("nyaterm-ai-gpui"));
    assert!(missing.is_empty(), "missing translations: {missing:#?}");
}
