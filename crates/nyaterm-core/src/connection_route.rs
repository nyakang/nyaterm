use serde::{Deserialize, Serialize};

/// Ephemeral authenticated loopback relay. Never persisted as a saved connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayEndpoint {
    pub address: std::net::SocketAddr,
    pub token: crate::SecretString,
}

impl RelayEndpoint {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.address.ip().is_loopback()
            || self.address.port() == 0
            || self.token.expose_secret().len() != 32
        {
            return Err("invalid authenticated loopback endpoint");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::RelayEndpoint;
    #[test]
    fn remote_endpoints_are_rejected_and_tokens_are_redacted() {
        let endpoint = RelayEndpoint {
            address: "192.0.2.1:23".parse().unwrap(),
            token: "12345678901234567890123456789012".into(),
        };
        assert!(endpoint.validate().is_err());
        assert!(!format!("{endpoint:?}").contains(endpoint.token.expose_secret()));
    }
}
