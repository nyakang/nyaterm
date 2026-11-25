use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use nyaterm_core::plugins::invocation::{ActionInput, ActionResult, validate_input};
use nyaterm_core::plugins::manifest::{ActionManifest, PluginManifest};
use nyaterm_core::plugins::preferences::PluginPreferences;
use nyaterm_core::plugins::template::expand;
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult, PluginStatus, validate_id};
use nyaterm_core::runtime::AppRuntime;
use nyaterm_store::plugin_preferences::PluginPreferenceStore;

use crate::package::{Package, StagingDirectory, io_error, load, reject_link, reject_path_links};
use crate::runtime::{CallTicket, RuntimeEngine, RuntimeInstance, RuntimeLimits, RuntimeToken};

mod preparation;
pub(crate) use preparation::PreparedChange;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginSource {
    Managed,
    Development(PathBuf),
}

#[derive(Clone, PartialEq, Eq)]
pub struct PluginSnapshot {
    pub session_events_authorized: bool,
    pub id: String,
    pub manifest: Option<PluginManifest>,
    pub enabled: bool,
    pub source: PluginSource,
    pub status: PluginStatus,
    pub revision: u64,
    pub error: Option<PluginError>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Contribution {
    pub id: String,
    pub plugin_id: String,
    pub revision: u64,
    pub action: ActionManifest,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct CatalogSnapshot {
    pub permission_requests: BTreeMap<String, Vec<nyaterm_core::plugins::extension::Grant>>,
    pub plugins: Vec<PluginSnapshot>,
    pub contributions: Vec<Contribution>,
    pub startup_error: Option<PluginError>,
    pub stopped: bool,
}

struct Entry {
    manifest: Option<PluginManifest>,
    templates: BTreeMap<String, String>,
    instance: Option<Arc<RuntimeInstance>>,
    enabled: bool,
    source: PluginSource,
    revision: u64,
    valid: Arc<AtomicBool>,
    error: Option<PluginError>,
}

impl Entry {
    fn retire(&mut self) {
        self.valid.store(false, Ordering::Release);
        if let Some(instance) = self.instance.take() {
            instance.stop();
        }
    }
    fn usable(&self) -> bool {
        self.enabled
            && self.error.is_none()
            && self.instance.as_ref().is_none_or(|i| i.token().active())
    }
}
impl Drop for Entry {
    fn drop(&mut self) {
        self.retire();
    }
}

pub struct Invocation {
    pub plugin_id: String,
    pub contribution_id: String,
    pub revision: u64,
    lease: InvocationLease,
    pending: InvocationResult,
}
/// Read-only generation proof retained by previews. Only the registry/runtime
/// can revoke it; windows cannot revive a retired result.
#[derive(Clone)]
pub struct InvocationLease {
    valid: Arc<AtomicBool>,
    runtime: Option<Arc<RuntimeToken>>,
}
impl InvocationLease {
    pub fn is_current(&self) -> bool {
        self.valid.load(Ordering::Acquire)
            && self.runtime.as_ref().is_none_or(|token| token.active())
    }
}
enum InvocationResult {
    Ready(PluginResult<ActionResult>),
    Wasm(CallTicket),
}

impl Invocation {
    pub fn lease(&self) -> InvocationLease {
        self.lease.clone()
    }
    pub fn cancel(&self) {
        if let InvocationResult::Wasm(ticket) = &self.pending {
            ticket.cancel();
        }
    }
    pub async fn result(self) -> PluginResult<ActionResult> {
        let result = match self.pending {
            InvocationResult::Ready(result) => result,
            InvocationResult::Wasm(ticket) => ticket.result().await,
        };
        if !self.lease.valid.load(Ordering::Acquire) {
            return Err(PluginError::new(
                ErrorCode::Cancelled,
                "Plugin result belongs to a retired revision",
            ));
        }
        result
    }
}

/// All catalog mutations are serialized by the service's one management worker.
/// Windows only receive immutable snapshots, never a second mutable registry.
pub struct PluginManager {
    environment: Arc<crate::environment::ExtensionEnvironment>,
    root: PathBuf,
    host_version: String,
    engine: Arc<RuntimeEngine>,
    limits: RuntimeLimits,
    store: PluginPreferenceStore,
    preferences: PluginPreferences,
    entries: BTreeMap<String, Entry>,
    revision: u64,
    stopped: bool,
}

impl PluginManager {
    pub fn set_host_adapter(&self, adapter: Arc<dyn crate::tasks::HostAdapter>) {
        self.environment.set_desktop(adapter);
    }
    pub fn for_runtime(runtime: &AppRuntime) -> PluginResult<Self> {
        Self::open(
            runtime.data_dir().join("plugins/v2"),
            env!("CARGO_PKG_VERSION"),
            RuntimeLimits::default(),
        )
    }

    pub fn open(root: PathBuf, host_version: &str, limits: RuntimeLimits) -> PluginResult<Self> {
        fs::create_dir_all(&root).map_err(|_| io_error())?;
        reject_link(&root)?;
        for directory in [
            &root.join("installed"),
            &root.join("staging"),
            &root.join("work/data"),
            &root.join("dev"),
        ] {
            fs::create_dir_all(directory).map_err(|_| io_error())?;
            reject_path_links(&root, directory)?;
        }
        let preferences_path = root.join("dev/preferences.redb");
        if preferences_path.exists() {
            reject_link(&preferences_path)?;
        }
        let store = PluginPreferenceStore::open(&preferences_path)?;
        let preferences = store.load()?;
        let engine = RuntimeEngine::new()?;
        let environment = crate::environment::ExtensionEnvironment::open(root.join("host"))?;
        engine.set_host_factory(environment.clone());
        let mut manager = Self {
            environment,
            root,
            host_version: host_version.into(),
            engine,
            limits,
            store,
            preferences,
            entries: BTreeMap::new(),
            revision: 0,
            stopped: false,
        };
        manager.discover()?;
        Ok(manager)
    }

    fn next_revision(&mut self) -> u64 {
        self.revision += 1;
        self.revision
    }

    fn prepare(
        &mut self,
        directory: &Path,
        enabled: bool,
        source: PluginSource,
    ) -> PluginResult<Entry> {
        let Package {
            manifest,
            templates,
            component,
        } = load(directory, &self.host_version)?;
        // Disabled installations also compile and validate the actual interface; guest code activates on first use.
        let instance = component
            .as_ref()
            .map(|bytes| {
                RuntimeInstance::prepare(self.engine.clone(), bytes, &manifest, self.limits.clone())
            })
            .transpose()?;
        if !enabled && let Some(instance) = &instance {
            instance.stop();
        }
        Ok(Entry {
            manifest: Some(manifest),
            templates,
            instance: if enabled { instance } else { None },
            enabled,
            source,
            revision: self.next_revision(),
            valid: Arc::new(AtomicBool::new(enabled)),
            error: None,
        })
    }

    fn discover(&mut self) -> PluginResult<()> {
        let paths = fs::read_dir(self.root.join("installed"))
            .map_err(|_| io_error())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| io_error())?;
        if paths.len() > 1024 {
            return Err(PluginError::new(
                ErrorCode::PackageLimit,
                "Too many installed plugins",
            ));
        }
        for path in paths {
            let id = path.file_name().to_str().ok_or_else(io_error)?.to_string();
            validate_id(&id)?;
            let preference = self
                .preferences
                .plugins
                .get(&id)
                .cloned()
                .unwrap_or_default();
            let source = preference
                .development_source
                .as_ref()
                .map_or(PluginSource::Managed, |p| {
                    PluginSource::Development(PathBuf::from(p))
                });
            let entry = match self.prepare(&path.path(), preference.enabled, source.clone()) {
                Ok(entry) if entry.manifest.as_ref().is_some_and(|m| m.id == id) => entry,
                result => Entry {
                    manifest: None,
                    templates: BTreeMap::new(),
                    instance: None,
                    enabled: preference.enabled,
                    source,
                    revision: self.next_revision(),
                    valid: Arc::new(AtomicBool::new(false)),
                    error: Some(result.err().unwrap_or_else(|| {
                        PluginError::new(
                            ErrorCode::Conflict,
                            "Installed directory does not match plugin ID",
                        )
                    })),
                },
            };
            self.entries.insert(id, entry);
        }
        Ok(())
    }

    pub fn snapshot(&self) -> CatalogSnapshot {
        let mut snapshot = CatalogSnapshot {
            permission_requests: self.environment.requests(),
            stopped: self.stopped,
            ..CatalogSnapshot::default()
        };
        for (id, entry) in &self.entries {
            let error = entry
                .error
                .clone()
                .or_else(|| entry.instance.as_ref().and_then(|i| i.token().fault()));
            let status = if let Some(error) = &error {
                if error.code == ErrorCode::Incompatible {
                    PluginStatus::Incompatible
                } else if entry.instance.is_some() {
                    PluginStatus::Faulted
                } else {
                    PluginStatus::LoadFailed
                }
            } else if entry.enabled {
                if entry.instance.is_some() {
                    PluginStatus::Running
                } else {
                    PluginStatus::Enabled
                }
            } else {
                PluginStatus::Installed
            };
            snapshot.plugins.push(PluginSnapshot {
                session_events_authorized: entry
                    .instance
                    .as_ref()
                    .is_some_and(|instance| instance.token().allows_session_events()),
                id: id.clone(),
                manifest: entry.manifest.clone(),
                enabled: entry.enabled,
                source: entry.source.clone(),
                status,
                revision: entry.revision,
                error,
            });
            if entry.usable()
                && let Some(manifest) = &entry.manifest
            {
                snapshot
                    .contributions
                    .extend(manifest.actions.iter().cloned().map(|action| Contribution {
                        id: manifest.contribution_id(&action),
                        plugin_id: id.clone(),
                        revision: entry.revision,
                        action,
                    }));
            }
        }
        snapshot
    }

    fn ensure_active(&self) -> PluginResult<()> {
        if self.stopped {
            Err(PluginError::new(
                ErrorCode::Shutdown,
                "Plugin service is shutting down",
            ))
        } else {
            Ok(())
        }
    }

    pub fn install(&mut self, source: &Path, development: bool) -> PluginResult<String> {
        self.replace(source, development, None)
    }

    pub fn update(&mut self, id: &str, source: &Path) -> PluginResult<String> {
        let prepared = self.plan_update(id, source)?.run()?;
        self.commit(prepared)
    }

    fn replace(
        &mut self,
        source: &Path,
        development: bool,
        expected: Option<&str>,
    ) -> PluginResult<String> {
        let prepared = self.plan_replace(source, development, expected)?.run()?;
        self.commit(prepared)
    }

    pub fn reload(&mut self, id: &str) -> PluginResult<()> {
        let prepared = self.plan_load(id, false)?.run()?;
        self.commit(prepared).map(|_| ())
    }

    pub fn authorize(
        &mut self,
        id: &str,
        expected_revision: u64,
        capability: &str,
        scope: &str,
    ) -> PluginResult<()> {
        use nyaterm_core::plugins::extension::{Capability, Grant, Scope};
        self.ensure_active()?;
        if self
            .entries
            .get(id)
            .is_none_or(|entry| entry.revision != expected_revision)
        {
            return Err(PluginError::new(
                ErrorCode::Cancelled,
                "Extension changed; review its permissions again",
            ));
        }
        validate_id(id)?;
        let package = load(&self.root.join("installed").join(id), &self.host_version)?;
        let capability = Capability::parse(capability).map_err(|_| {
            PluginError::new(ErrorCode::InvalidManifest, "Unknown extension capability")
        })?;
        let scope = match capability {
            Capability::Network => {
                let url = reqwest::Url::parse(scope).map_err(|_| io_error())?;
                if !matches!(url.scheme(), "https" | "http")
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                {
                    return Err(io_error());
                }
                Scope::Origin {
                    origin: url.origin().ascii_serialization(),
                }
            }
            Capability::LocalFilesRead | Capability::LocalFilesWrite => {
                let path = std::path::Path::new(scope);
                reject_link(path)?;
                if !path.is_absolute() || !path.is_dir() {
                    return Err(io_error());
                }
                Scope::Directory {
                    path: scope.to_owned(),
                }
            }
            Capability::ProcessExecute => {
                let path = std::path::Path::new(scope);
                reject_link(path)?;
                if !path.is_absolute() || !path.is_file() {
                    return Err(io_error());
                }
                Scope::Executable {
                    path: scope.to_owned(),
                }
            }
            Capability::RemoteFilesRead
            | Capability::RemoteFilesWrite
            | Capability::RemoteExecute
            | Capability::TerminalRead
            | Capability::TerminalWrite => {
                if scope.is_empty() || scope.len() > 256 {
                    return Err(io_error());
                }
                Scope::Connection {
                    id: scope.to_owned(),
                }
            }
            _ => Scope::Plugin,
        };
        self.environment.grant(
            &package.manifest,
            package.component.as_deref().unwrap_or_default(),
            Grant { capability, scope },
        )
    }

    pub fn revoke_authorizations(&mut self, id: &str) -> PluginResult<()> {
        self.ensure_active()?;
        validate_id(id)?;
        let package = load(&self.root.join("installed").join(id), &self.host_version)?;
        self.environment.revoke(
            &package.manifest,
            package.component.as_deref().unwrap_or_default(),
        )?;
        self.set_enabled(id, false)
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> PluginResult<()> {
        if enabled {
            let prepared = self.plan_load(id, true)?.run()?;
            self.commit(prepared)?;
        } else {
            self.ensure_active()?;
            validate_id(id)?;
            if !self.entries.contains_key(id) {
                return Err(PluginError::new(
                    ErrorCode::NotFound,
                    "Plugin is not installed",
                ));
            }
            let mut preferences = self.preferences.clone();
            preferences.plugins.entry(id.into()).or_default().enabled = false;
            self.store.save(&preferences)?;
            self.preferences = preferences;
            let revision = self.next_revision();
            let entry = self.entries.get_mut(id).unwrap();
            entry.retire();
            entry.enabled = false;
            entry.revision = revision;
        }
        Ok(())
    }

    /// Uninstall revokes all contributions and retains work/data/<id> by policy.
    pub fn uninstall(&mut self, id: &str) -> PluginResult<()> {
        self.ensure_active()?;
        validate_id(id)?;
        if !self.entries.contains_key(id) {
            return Err(PluginError::new(
                ErrorCode::NotFound,
                "Plugin is not installed",
            ));
        }
        let mut trash = StagingDirectory::new(&self.root.join("staging"))?;
        let target = self.root.join("installed").join(id);
        reject_path_links(&self.root, &target)?;
        fs::rename(&target, trash.path.join("old")).map_err(|_| io_error())?;
        let mut preferences = self.preferences.clone();
        preferences.plugins.remove(id);
        if let Err(error) = self.store.save(&preferences) {
            if fs::rename(trash.path.join("old"), &target).is_err() {
                trash.retain();
                return Err(PluginError::new(
                    ErrorCode::Io,
                    "Uninstall rollback failed; the old package is retained in staging for recovery",
                ));
            }
            return Err(error);
        }
        self.preferences = preferences;
        if let Some(mut old) = self.entries.remove(id) {
            old.retire();
        }
        Ok(())
    }

    pub fn invoke(&self, contribution_id: &str, input: ActionInput) -> PluginResult<Invocation> {
        self.invoke_checked(contribution_id, None, input)
    }

    /// Check the caller's selected contribution before any input reaches a guest.
    pub fn invoke_at_revision(
        &self,
        contribution_id: &str,
        expected_revision: u64,
        input: ActionInput,
    ) -> PluginResult<Invocation> {
        self.invoke_checked(contribution_id, Some(expected_revision), input)
    }

    fn invoke_checked(
        &self,
        contribution_id: &str,
        expected_revision: Option<u64>,
        input: ActionInput,
    ) -> PluginResult<Invocation> {
        self.ensure_active()?;
        let (id, action_id) = contribution_id
            .split_once(':')
            .ok_or_else(|| PluginError::new(ErrorCode::NotFound, "Choose a plugin action"))?;
        let entry = self
            .entries
            .get(id)
            .ok_or_else(|| PluginError::new(ErrorCode::NotFound, "Plugin is not installed"))?;
        if expected_revision.is_some_and(|revision| revision != entry.revision) {
            return Err(PluginError::new(
                ErrorCode::Cancelled,
                "Plugin action changed; select it again before invoking",
            ));
        }
        if !entry.usable() {
            return Err(PluginError::new(
                ErrorCode::Disabled,
                "Plugin action is disabled or faulted; enable or reload it",
            ));
        }
        let action = entry
            .manifest
            .as_ref()
            .and_then(|m| m.actions.iter().find(|a| a.id == action_id))
            .ok_or_else(|| {
                PluginError::new(ErrorCode::NotFound, "Plugin action is not declared")
            })?;
        let input = validate_input(action, &input)?;
        let pending = if let Some(template) = entry.templates.get(action_id) {
            InvocationResult::Ready(expand(action, template, &input))
        } else {
            InvocationResult::Wasm(
                entry
                    .instance
                    .as_ref()
                    .ok_or_else(|| {
                        PluginError::new(
                            ErrorCode::Initialization,
                            "Plugin component is unavailable",
                        )
                    })?
                    .invoke(action_id, action.result, input)?,
            )
        };
        Ok(Invocation {
            plugin_id: id.into(),
            contribution_id: contribution_id.into(),
            revision: entry.revision,
            lease: InvocationLease {
                valid: entry.valid.clone(),
                runtime: entry.instance.as_ref().map(|instance| instance.token()),
            },
            pending,
        })
    }

    pub fn shutdown(&mut self) {
        self.stopped = true;
        for entry in self.entries.values_mut() {
            entry.retire();
        }
        self.entries.clear();
    }
}

impl Drop for PluginManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use crate::manager::PluginManager;
    use crate::runtime::RuntimeLimits;
    use nyaterm_core::plugins::ErrorCode;
    use nyaterm_core::test_support::TestTempDir;
    use std::fs;

    #[test]
    fn prepared_update_cannot_overwrite_a_newer_registry_revision_or_preferences() {
        let root = TestTempDir::new("nyaterm-preparation-revision");
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("plugin.toml"), "schema_version = 2\nid = \"fixture\"\nname = \"Fixture\"\nversion = \"1.0.0\"\nauthors = []\ndescription = \"Fixture\"\nhost_version = \">=2.0.0-preview.4\"\napi_version = \"2.0.0\"\ncomponent = \"plugin.wasm\"\n[[actions]]\nid = \"count\"\nname = \"Count\"\ndescription = \"Count\"\nresult = \"text\"\n").unwrap();
        fs::write(
            source.join("plugin.wasm"),
            include_bytes!("../tests/fixtures/lifecycle.wasm"),
        )
        .unwrap();
        let host_root = root.join("host");
        let mut manager = PluginManager::open(
            host_root.clone(),
            "2.0.0-preview.4",
            RuntimeLimits::default(),
        )
        .unwrap();
        manager.install(&source, false).unwrap();
        let candidate = manager
            .plan_update("fixture", &source)
            .unwrap()
            .run()
            .unwrap();
        manager.set_enabled("fixture", false).unwrap();
        let current = manager.snapshot();
        assert_eq!(
            manager.commit(candidate).unwrap_err().code,
            ErrorCode::Cancelled
        );
        assert!(manager.snapshot() == current);
        drop(manager);
        let manager =
            PluginManager::open(host_root, "2.0.0-preview.4", RuntimeLimits::default()).unwrap();
        assert!(!manager.snapshot().plugins[0].enabled);
    }

    #[test]
    fn idle_zero_capacity_mailbox_shutdown_joins_without_a_polling_timeout() {
        let root = TestTempDir::new("nyaterm-zero-capacity-shutdown");
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        let manifest = fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/plugins/text-tools/plugin.toml"
        ))
        .unwrap();
        fs::write(source.join("plugin.toml"), manifest).unwrap();
        fs::write(
            source.join("plugin.wasm"),
            include_bytes!("../tests/fixtures/text-tools.wasm"),
        )
        .unwrap();
        let mut manager = PluginManager::open(
            root.join("host"),
            "2.0.0-preview.4",
            RuntimeLimits {
                queue_capacity: 0,
                ..RuntimeLimits::default()
            },
        )
        .unwrap();
        manager.install(&source, false).unwrap();
        manager.shutdown();
        assert!(manager.snapshot().stopped);
    }
}
