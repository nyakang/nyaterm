use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};

use nyaterm_core::plugins::preferences::PluginPreference;
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult, validate_id};

use super::{Entry, PluginManager, PluginSource};
use crate::package::{Package, StagingDirectory, io_error, load, promote, reject_link, snapshot};
use crate::runtime::{RuntimeEngine, RuntimeInstance, RuntimeLimits};

pub(crate) struct Preparation {
    root: PathBuf,
    host_version: String,
    engine: Arc<RuntimeEngine>,
    limits: RuntimeLimits,
    source: PathBuf,
    source_kind: PluginSource,
    expected: Option<(String, u64)>,
    enabled: bool,
    promote: bool,
    save_enabled: bool,
}

pub(crate) struct PreparedChange {
    stage: StagingDirectory,
    entry: Entry,
    expected: Option<(String, u64)>,
    promote: bool,
    save_enabled: bool,
}

impl Preparation {
    pub(crate) fn run(self) -> PluginResult<PreparedChange> {
        if matches!(self.source_kind, PluginSource::Development(_))
            && !reject_link(&self.source)?.is_dir()
        {
            return Err(PluginError::new(
                ErrorCode::InvalidManifest,
                "Register a development directory, not an archive",
            ));
        }
        let stage = StagingDirectory::new(&self.root.join("staging"))?;
        snapshot(&self.source, &stage.path)?;
        let Package {
            manifest,
            templates,
            component,
        } = load(&stage.path, &self.host_version)?;
        if self
            .expected
            .as_ref()
            .is_some_and(|(id, _)| *id != manifest.id)
        {
            return Err(PluginError::new(
                ErrorCode::Conflict,
                "Update package has a different plugin ID",
            ));
        }
        let source_kind = match self.source_kind {
            PluginSource::Development(_) if self.promote => {
                PluginSource::Development(fs::canonicalize(&self.source).map_err(|_| io_error())?)
            }
            source => source,
        };
        let instance = component
            .as_ref()
            .map(|bytes| {
                RuntimeInstance::prepare(self.engine.clone(), bytes, &manifest, self.limits.clone())
            })
            .transpose()?;
        if !self.enabled
            && let Some(instance) = &instance
        {
            instance.stop();
        }
        Ok(PreparedChange {
            stage,
            entry: Entry {
                manifest: Some(manifest),
                templates,
                instance: if self.enabled { instance } else { None },
                enabled: self.enabled,
                source: source_kind,
                revision: 0,
                valid: Arc::new(AtomicBool::new(self.enabled)),
                error: None,
            },
            expected: self.expected,
            promote: self.promote,
            save_enabled: self.save_enabled,
        })
    }
}

impl PluginManager {
    pub(crate) fn set_change_handler(&self, handler: Arc<dyn Fn() + Send + Sync>) {
        self.environment.set_change_handler(handler.clone());
        self.engine.set_change_handler(handler);
    }

    pub(crate) fn plan_replace(
        &self,
        source: &Path,
        development: bool,
        expected: Option<&str>,
    ) -> PluginResult<Preparation> {
        self.ensure_active()?;
        let expected = expected
            .map(|id| {
                validate_id(id)?;
                self.entries
                    .get(id)
                    .map(|entry| (id.to_owned(), entry.revision))
                    .ok_or_else(|| PluginError::new(ErrorCode::NotFound, "Plugin is not installed"))
            })
            .transpose()?;
        let enabled = expected
            .as_ref()
            .is_none_or(|(id, _)| self.entries[id].enabled);
        Ok(Preparation {
            root: self.root.clone(),
            host_version: self.host_version.clone(),
            engine: self.engine.clone(),
            limits: self.limits.clone(),
            source: source.to_owned(),
            source_kind: if development {
                PluginSource::Development(source.to_owned())
            } else {
                PluginSource::Managed
            },
            expected,
            enabled,
            promote: true,
            save_enabled: false,
        })
    }

    pub(crate) fn plan_update(&self, id: &str, source: &Path) -> PluginResult<Preparation> {
        let development = matches!(
            self.entries.get(id).map(|entry| &entry.source),
            Some(PluginSource::Development(_))
        );
        self.plan_replace(source, development, Some(id))
    }

    pub(crate) fn plan_load(&self, id: &str, enable: bool) -> PluginResult<Preparation> {
        self.ensure_active()?;
        validate_id(id)?;
        let entry = self
            .entries
            .get(id)
            .ok_or_else(|| PluginError::new(ErrorCode::NotFound, "Plugin is not installed"))?;
        if !enable && let PluginSource::Development(source) = &entry.source {
            return self.plan_replace(source, true, Some(id));
        }
        Ok(Preparation {
            root: self.root.clone(),
            host_version: self.host_version.clone(),
            engine: self.engine.clone(),
            limits: self.limits.clone(),
            source: self.root.join("installed").join(id),
            source_kind: entry.source.clone(),
            expected: Some((id.to_owned(), entry.revision)),
            enabled: enable || entry.enabled,
            promote: false,
            save_enabled: enable,
        })
    }

    pub(crate) fn commit(&mut self, mut prepared: PreparedChange) -> PluginResult<String> {
        self.ensure_active()?;
        let id = prepared.entry.manifest.as_ref().unwrap().id.clone();
        match &prepared.expected {
            Some((expected, revision))
                if self
                    .entries
                    .get(expected)
                    .is_none_or(|entry| entry.revision != *revision) =>
            {
                return Err(PluginError::new(
                    ErrorCode::Cancelled,
                    "Plugin changed while its replacement was preparing",
                ));
            }
            None if self.entries.contains_key(&id) => {
                return Err(PluginError::new(
                    ErrorCode::Conflict,
                    "Plugin is already installed; use Update",
                ));
            }
            _ => {}
        }
        let mut preferences = self.preferences.clone();
        if prepared.promote {
            preferences.plugins.insert(
                id.clone(),
                PluginPreference {
                    enabled: prepared.entry.enabled,
                    development_source: match &prepared.entry.source {
                        PluginSource::Managed => None,
                        PluginSource::Development(path) => {
                            Some(path.to_string_lossy().into_owned())
                        }
                    },
                },
            );
            promote(
                &mut prepared.stage,
                &self.root.join("installed").join(&id),
                &self.root.join("staging"),
                || self.store.save(&preferences),
            )?;
        } else if prepared.save_enabled {
            preferences.plugins.entry(id.clone()).or_default().enabled = true;
            self.store.save(&preferences)?;
        }
        self.preferences = preferences;
        prepared.entry.revision = self.next_revision();
        if let Some(mut old) = self.entries.insert(id.clone(), prepared.entry) {
            old.retire();
        }
        Ok(id)
    }
}
