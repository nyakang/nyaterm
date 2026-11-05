//! UI-independent runtime SSH authentication messages.

use crate::{SshCredentialPrompt, SshKeyAuthConfig};
use nyaterm_core::SecretString;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshAuthenticationMethod {
    Password,
    Publickey,
    KeyboardInteractive,
}

#[derive(Debug, Clone)]
pub struct SshAuthenticationRequest {
    pub prompt: SshCredentialPrompt,
    pub available_methods: Vec<SshAuthenticationMethod>,
    pub stage: SshAuthenticationStage,
    /// Responses from completed factors that must survive subsequent prompts.
    pub accepted_response_ids: Vec<String>,
    /// Opaque provenance of a runtime-selected key, used when unlocking that key.
    pub key_response_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SshAuthenticationStage {
    #[default]
    Primary,
    AdditionalFactor,
}

#[derive(Debug, Clone)]
pub enum SshAuthenticationMaterial {
    Password(SecretString),
    Key(SshKeyAuthConfig),
    KeyPassphrase(SecretString),
}

#[derive(Debug, Clone)]
pub struct SshAuthenticationResponse {
    pub material: SshAuthenticationMaterial,
    pub response_id: Option<String>,
}
