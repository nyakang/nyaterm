use crate::authority::Authority;
use crate::host_io::LocalHost;
use crate::runtime::HostFactory;
use crate::tasks::{HostAdapter, TaskBroker, TaskContext};
use nyaterm_core::plugins::extension::{Authorization, Capability, Grant};
use nyaterm_core::plugins::extension::{HostRequest, TaskEvent};
use nyaterm_core::plugins::manifest::PluginManifest;
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult};
use nyaterm_store::extension_authorizations::AuthorizationStore;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

type ChangeHandler = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;

pub struct ExtensionEnvironment {
    desktop: Arc<RwLock<Option<Arc<dyn HostAdapter>>>>,
    root: PathBuf,
    store: AuthorizationStore,
    active: Mutex<BTreeMap<String, Vec<Authority>>>,
    requests: Arc<Mutex<BTreeMap<String, Vec<Grant>>>>,
    changed: ChangeHandler,
}

impl ExtensionEnvironment {
    pub fn requests(&self) -> BTreeMap<String, Vec<Grant>> {
        self.requests.lock().unwrap().clone()
    }
    pub fn set_change_handler(&self, handler: Arc<dyn Fn() + Send + Sync>) {
        *self.changed.lock().unwrap() = Some(handler);
    }
    pub fn set_desktop(&self, adapter: Arc<dyn HostAdapter>) {
        *self.desktop.write().unwrap() = Some(adapter);
    }
    pub fn grant(
        &self,
        manifest: &PluginManifest,
        component: &[u8],
        grant: Grant,
    ) -> PluginResult<()> {
        let identity = self.authorization_identity(manifest, component)?;
        let mut grants = self
            .store
            .load(&identity)?
            .map(|authorization| authorization.grants)
            .unwrap_or_default();
        let capabilities = declared(manifest)?;
        grants.retain(|grant| capabilities.contains(&grant.capability));
        if !grants.contains(&grant) {
            grants.push(grant);
        }
        self.authorize(manifest, component, grants)
    }
    pub fn open(root: PathBuf) -> PluginResult<Arc<Self>> {
        std::fs::create_dir_all(&root).map_err(|_| crate::package::io_error())?;
        crate::package::reject_link(&root)?;
        let path = root.join("authorizations.redb");
        if path.exists() {
            crate::package::reject_link(&path)?;
        }
        Ok(Arc::new(Self {
            desktop: Arc::new(RwLock::new(None)),
            store: AuthorizationStore::open(&path)?,
            root,
            active: Mutex::new(BTreeMap::new()),
            requests: Arc::new(Mutex::new(BTreeMap::new())),
            changed: Arc::new(Mutex::new(None)),
        }))
    }

    fn authorization_identity(
        &self,
        manifest: &PluginManifest,
        component: &[u8],
    ) -> PluginResult<String> {
        Ok(
            crate::catalog::trusted_source(&self.root.join("trusted"), manifest, component)?
                .unwrap_or(Self::identity(manifest, component)?),
        )
    }

    pub fn identity(manifest: &PluginManifest, component: &[u8]) -> PluginResult<String> {
        let mut hash = Sha256::new();
        hash.update(serde_json::to_vec(manifest).map_err(|_| crate::package::io_error())?);
        hash.update(component);
        Ok(hex::encode(hash.finalize()))
    }

    /// Called only by the host's authorization UI, never exported to guests.
    pub fn authorize(
        &self,
        manifest: &PluginManifest,
        component: &[u8],
        grants: Vec<Grant>,
    ) -> PluginResult<()> {
        let declared = declared(manifest)?;
        if grants
            .iter()
            .any(|grant| !declared.contains(&grant.capability))
        {
            return Err(PluginError::new(
                ErrorCode::PermissionDenied,
                "Cannot grant undeclared capabilities",
            ));
        }
        let identity = self.authorization_identity(manifest, component)?;
        let authorization = Authorization {
            plugin_id: manifest.id.clone(),
            source: identity.clone(),
            grants,
        };
        self.store.save(&identity, &authorization)?;
        // A denied request has not started. Explicit approval enables its retry
        // without destroying panel input or guest state. Each queued request
        // still retains its original operation and resource target.
        if let Some(authorities) = self.active.lock().unwrap().get(&manifest.id) {
            for authority in authorities {
                authority.update_authorization(&authorization);
            }
        }
        self.requests.lock().unwrap().remove(&manifest.id);
        Ok(())
    }

    pub fn revoke(&self, manifest: &PluginManifest, _component: &[u8]) -> PluginResult<()> {
        self.revoke_active(&manifest.id);
        self.requests.lock().unwrap().remove(&manifest.id);
        self.store.revoke_plugin(&manifest.id)
    }

    fn revoke_active(&self, id: &str) {
        if let Some(authorities) = self.active.lock().unwrap().remove(id) {
            for authority in authorities {
                authority.revoke();
            }
        }
    }
}

fn declared(manifest: &PluginManifest) -> PluginResult<Vec<Capability>> {
    manifest
        .capabilities
        .iter()
        .map(|value| {
            Capability::parse(value)
                .map_err(|_| PluginError::new(ErrorCode::InvalidManifest, "Unknown capability"))
        })
        .collect()
}

impl HostFactory for ExtensionEnvironment {
    fn create(&self, manifest: &PluginManifest, component: &[u8]) -> PluginResult<TaskBroker> {
        let identity = self.authorization_identity(manifest, component)?;
        let authorization = self.store.load(&identity)?.unwrap_or(Authorization {
            plugin_id: manifest.id.clone(),
            source: identity,
            grants: vec![],
        });
        let requests = self.requests.clone();
        let changed = self.changed.clone();
        let id = manifest.id.clone();
        let authority = Authority::new(declared(manifest)?, authorization).with_requests(Arc::new(
            move |grant| {
                let mut requests = requests.lock().unwrap();
                let pending = requests.entry(id.clone()).or_default();
                if pending.len() >= 16 || pending.contains(&grant) {
                    return;
                }
                pending.push(grant);
                drop(requests);
                let handler = changed.lock().unwrap().clone();
                if let Some(handler) = handler {
                    handler();
                }
            },
        ));
        {
            let mut active = self.active.lock().unwrap();
            let authorities = active.entry(manifest.id.clone()).or_default();
            authorities.retain(Authority::is_shared);
            authorities.push(authority.clone());
        }
        let root = self.root.join("data").join(&manifest.id);
        let adapter = Arc::new(RoutedHost {
            local: LocalHost::new(root)?,
            desktop: self.desktop.clone(),
        });
        Ok(TaskBroker::new(authority, adapter))
    }
}

struct RoutedHost {
    local: LocalHost,
    desktop: Arc<RwLock<Option<Arc<dyn HostAdapter>>>>,
}

impl HostAdapter for RoutedHost {
    fn execute(&self, request: HostRequest, context: &TaskContext) -> PluginResult<TaskEvent> {
        if matches!(request, HostRequest::ProcessExecute { .. }) {
            return crate::process::execute(&request, context);
        }
        if matches!(
            request,
            HostRequest::Http { .. }
                | HostRequest::LocalRead { .. }
                | HostRequest::LocalWrite { .. }
                | HostRequest::StorageRead { .. }
                | HostRequest::StorageWrite { .. }
        ) {
            self.local.execute(request, context)
        } else {
            let adapter = self.desktop.read().unwrap().clone().ok_or_else(|| {
                PluginError::new(ErrorCode::NotFound, "No desktop host is attached")
            })?;
            adapter.execute(request, context)
        }
    }
}
