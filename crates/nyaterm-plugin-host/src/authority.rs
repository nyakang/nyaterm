//! Authorization is checked at admission and by adapters immediately before I/O.
use nyaterm_core::plugins::extension::{Authorization, Capability, Grant, HostRequest, Scope};
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult};
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone)]
pub struct Authority {
    declared: Arc<Vec<Capability>>,
    authorization: Arc<RwLock<Authorization>>,
    active: Arc<AtomicBool>,
    denied_request: Option<Arc<dyn Fn(Grant) + Send + Sync>>,
}

impl Authority {
    pub fn new(declared: Vec<Capability>, authorization: Authorization) -> Self {
        Self {
            declared: Arc::new(declared),
            authorization: Arc::new(RwLock::new(authorization)),
            active: Arc::new(AtomicBool::new(true)),
            denied_request: None,
        }
    }

    pub fn with_requests(mut self, handler: Arc<dyn Fn(Grant) + Send + Sync>) -> Self {
        self.denied_request = Some(handler);
        self
    }

    pub fn revoke(&self) {
        self.active.store(false, Ordering::Release);
        self.authorization.write().unwrap().grants.clear();
    }

    pub(crate) fn update_authorization(&self, authorization: &Authorization) {
        let mut current = self.authorization.write().unwrap();
        if self.is_active()
            && current.plugin_id == authorization.plugin_id
            && current.source == authorization.source
        {
            *current = authorization.clone();
        }
    }

    pub fn allows_session_events(&self) -> bool {
        self.is_active()
            && self.declared.contains(&Capability::SessionsRead)
            && self
                .authorization
                .read()
                .unwrap()
                .grants
                .iter()
                .any(|grant| {
                    grant.capability == Capability::SessionsRead && grant.scope == Scope::Plugin
                })
    }

    pub fn check(&self, request: &HostRequest) -> PluginResult<()> {
        if !self.active.load(Ordering::Acquire) {
            return Err(denied());
        }
        let capability = request.capability();
        if !self.declared.contains(&capability) {
            return Err(denied());
        }
        if matches!(capability, Capability::Storage | Capability::Ui) {
            return Ok(());
        }
        let scope = request_scope(request)?;
        if self
            .authorization
            .read()
            .unwrap()
            .grants
            .iter()
            .any(|grant| grant.capability == capability && grant.scope == scope)
        {
            Ok(())
        } else {
            if let Some(handler) = &self.denied_request {
                handler(Grant { capability, scope });
            }
            Err(denied())
        }
    }

    pub(crate) fn is_shared(&self) -> bool {
        Arc::strong_count(&self.active) > 1
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub(crate) fn plugin_id(&self) -> String {
        self.authorization.read().unwrap().plugin_id.clone()
    }
}

pub fn request_scope(request: &HostRequest) -> PluginResult<Scope> {
    if let Some(session) = request.session() {
        return Ok(Scope::Connection {
            id: session
                .connection_id
                .clone()
                .unwrap_or_else(|| session.id.clone()),
        });
    }
    Ok(match request {
        HostRequest::Http { url, .. } => {
            let url = reqwest::Url::parse(url).map_err(|_| denied())?;
            if !matches!(url.scheme(), "https" | "http")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.host_str().is_none()
            {
                return Err(denied());
            }
            Scope::Origin {
                origin: url.origin().ascii_serialization(),
            }
        }
        HostRequest::LocalRead { root, .. } | HostRequest::LocalWrite { root, .. } => {
            Scope::Directory { path: root.clone() }
        }
        HostRequest::ProcessExecute { executable, .. } => Scope::Executable {
            path: executable.clone(),
        },
        _ => Scope::Plugin,
    })
}

fn denied() -> PluginError {
    PluginError::new(
        ErrorCode::PermissionDenied,
        "Extension access requires authorization",
    )
}

#[cfg(test)]
mod tests {
    use crate::authority::Authority;
    use nyaterm_core::plugins::extension::{Authorization, Capability, Grant, HostRequest, Scope};

    #[test]
    fn background_session_events_require_an_explicit_session_list_grant() {
        let authorization = Authorization {
            plugin_id: "test".into(),
            source: "fixture".into(),
            grants: vec![],
        };
        let denied = Authority::new(vec![Capability::SessionsRead], authorization.clone());
        assert!(!denied.allows_session_events());
        let granted = Authority::new(
            vec![Capability::SessionsRead],
            Authorization {
                grants: vec![Grant {
                    capability: Capability::SessionsRead,
                    scope: Scope::Plugin,
                }],
                ..authorization
            },
        );
        assert!(granted.allows_session_events());
        granted.revoke();
        assert!(!granted.allows_session_events());
    }

    #[test]
    fn declared_and_granted_origin_are_both_required_and_revocation_is_live() {
        let auth = Authority::new(
            vec![Capability::Network],
            Authorization {
                plugin_id: "test".into(),
                source: "test-key".into(),
                grants: vec![Grant {
                    capability: Capability::Network,
                    scope: Scope::Origin {
                        origin: "https://example.com".into(),
                    },
                }],
            },
        );
        let request = |url: &str| HostRequest::Http {
            url: url.into(),
            method: "GET".into(),
            body: vec![],
        };
        assert!(auth.check(&request("https://example.com/path")).is_ok());
        assert!(
            auth.check(&request("https://example.com.evil.test/path"))
                .is_err()
        );
        assert!(auth.check(&request("http://example.com/path")).is_err());
        auth.revoke();
        assert!(auth.check(&request("https://example.com/path")).is_err());
    }
}
