//! Server-issued, memory-only identities. Shared discovery credentials do not
//! grant access to another client's jobs or request cancellation.
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant};

struct Owner {
    generation: String,
    hash: [u8; 32],
    connections: usize,
    detached: Option<Instant>,
}
#[derive(Default)]
pub(super) struct ClientOwners {
    owners: HashMap<String, Owner>,
}
impl ClientOwners {
    pub fn attach(
        &mut self,
        generation: &str,
        token: Option<&str>,
    ) -> Result<(String, String), &'static str> {
        if let Some(token) = token {
            if token.len() != 64 {
                return Err("Invalid recovery credential.");
            }
            let hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
            let Some((id, owner)) = self.owners.iter_mut().find(|(_, o)| {
                o.generation == generation && super::constant_time_eq(&o.hash, &hash)
            }) else {
                return Err("Recovery credential expired or is unavailable.");
            };
            if owner
                .detached
                .is_some_and(|at| at.elapsed() >= Duration::from_secs(600))
            {
                return Err("Recovery credential expired.");
            }
            owner.connections += 1;
            owner.detached = None;
            return Ok((id.clone(), token.into()));
        }
        if self.owners.len() >= 256 {
            return Err("Too many retained MCP identities.");
        }
        let id = uuid::Uuid::new_v4().to_string();
        let token = hex::encode(rand::random::<[u8; 32]>());
        let hash = Sha256::digest(token.as_bytes()).into();
        self.owners.insert(
            id.clone(),
            Owner {
                generation: generation.into(),
                hash,
                connections: 1,
                detached: None,
            },
        );
        Ok((id, token))
    }
    pub fn detach(&mut self, id: &str) {
        if let Some(owner) = self.owners.get_mut(id) {
            owner.connections = owner.connections.saturating_sub(1);
            if owner.connections == 0 {
                owner.detached = Some(Instant::now());
            }
        }
    }
    pub fn expired(&mut self) -> Vec<String> {
        let ids = self
            .owners
            .iter()
            .filter(|(_, o)| {
                o.connections == 0
                    && o.detached
                        .is_some_and(|at| at.elapsed() >= Duration::from_secs(600))
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in &ids {
            self.owners.remove(id);
        }
        ids
    }
}
#[cfg(test)]
mod tests {
    use super::ClientOwners;
    #[test]
    fn resumes_only_the_issued_identity_and_old_detach_cannot_expire_new_socket() {
        let mut owners = ClientOwners::default();
        let (id, token) = owners.attach("generation", None).unwrap();
        let (resumed, _) = owners.attach("generation", Some(&token)).unwrap();
        assert_eq!(resumed, id);
        owners.detach(&id);
        assert_eq!(owners.owners[&id].connections, 1);
        assert!(owners.attach("different", Some(&token)).is_err());
        assert!(owners.attach("generation", Some(&"a".repeat(64))).is_err());
        owners.detach(&id);
        owners.owners.get_mut(&id).unwrap().detached =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(601));
        assert_eq!(owners.expired(), vec![id]);
        assert!(owners.attach("generation", Some(&token)).is_err());
    }
}
