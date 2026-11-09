use std::collections::HashSet;

use crate::session_import::mobaxterm_crypto::{SecretContext, decrypt};
use crate::session_import::{SessionImportError, SessionImportOptions};

use super::{
    MobaProtocol, MobaSection, MobaSession, MobaStoredPassword, MobaXtermCredentialDiagnosticKind,
    MobaXtermImportReport,
};

pub(super) enum CredentialSource {
    Named { name: String, username: String },
    Endpoint(PasswordEndpoint),
    Unsupported,
}

pub(super) struct PasswordEndpoint {
    protocol: MobaProtocol,
    host: String,
    username: String,
    port: Option<u16>,
}

impl PasswordEndpoint {
    fn parse(key: &str) -> Option<Self> {
        let (identity, host) = key.rsplit_once('@')?;
        if host.is_empty() || identity.is_empty() {
            return None;
        }
        let (protocol, port, username) = if let Some((prefix, username)) = identity.split_once(':')
        {
            let digit = prefix
                .find(|ch: char| ch.is_ascii_digit())
                .unwrap_or(prefix.len());
            let protocol = match &prefix[..digit].to_ascii_lowercase()[..] {
                "ssh" => MobaProtocol::Ssh,
                "telnet" => MobaProtocol::Telnet,
                "rdp" => MobaProtocol::Rdp,
                "vnc" => MobaProtocol::Vnc,
                _ => return None,
            };
            let port = if digit == prefix.len() {
                None
            } else {
                Some(
                    prefix[digit..]
                        .parse::<u16>()
                        .ok()
                        .filter(|port| *port != 0)?,
                )
            };
            (protocol, port, username)
        } else {
            (MobaProtocol::Ssh, None, identity)
        };
        if username.is_empty() {
            return None;
        }
        Some(Self {
            protocol,
            port,
            username: username.into(),
            host: host.into(),
        })
    }

    fn matches(&self, session: &MobaSession) -> bool {
        self.protocol == session.protocol
            && self
                .host
                .trim_matches(['[', ']'])
                .eq_ignore_ascii_case(session.host.trim_matches(['[', ']']))
            && session.login_reference.as_deref() == Some(self.username.as_str())
            && session.credential_reference.is_none()
            && self.port.is_none_or(|port| port == session.port)
    }
}

pub(super) struct CredentialEntry {
    pub stored: MobaStoredPassword,
    source: CredentialSource,
}

pub(super) struct EncryptedCredentials {
    pub entries: Vec<CredentialEntry>,
    unavailable: Vec<CredentialSource>,
    registry_only: bool,
}

impl EncryptedCredentials {
    pub fn read(
        sections: &[MobaSection],
        options: &SessionImportOptions<'_>,
        report: &mut MobaXtermImportReport,
    ) -> Result<Self, SessionImportError> {
        let misc = |key: &str| {
            sections
                .iter()
                .filter(|section| section.name.eq_ignore_ascii_case("Misc"))
                .flat_map(|section| &section.entries)
                .find(|(name, _)| name.eq_ignore_ascii_case(key))
                .map(|(_, value)| value.as_str())
        };
        let has_master = sections
            .iter()
            .any(|section| section.name.eq_ignore_ascii_case("Sesspass"));
        let mut ciphertexts = Vec::new();
        for section in sections {
            if section.name.eq_ignore_ascii_case("Passwords") {
                for (key, ciphertext) in &section.entries {
                    let source = PasswordEndpoint::parse(key)
                        .map(CredentialSource::Endpoint)
                        .unwrap_or(CredentialSource::Unsupported);
                    ciphertexts.push((source, key.as_str(), "", ciphertext.as_str()));
                }
            } else if section.name.eq_ignore_ascii_case("Credentials") {
                for (name, value) in &section.entries {
                    let (username, ciphertext) = value.split_once(':').unwrap_or(("", ""));
                    ciphertexts.push((
                        CredentialSource::Named {
                            name: name.clone(),
                            username: username.into(),
                        },
                        name.as_str(),
                        username,
                        ciphertext,
                    ));
                }
            }
        }
        let registry_only = misc("PasswordsInRegistry") == Some("1") && ciphertexts.is_empty();
        let has_ciphertext = ciphertexts.iter().any(|(_, _, _, value)| !value.is_empty());
        let mut result = Self {
            entries: Vec::new(),
            unavailable: Vec::new(),
            registry_only,
        };
        if registry_only {
            report.record(MobaXtermCredentialDiagnosticKind::RegistryOnly, 1);
        }
        if ciphertexts.is_empty() {
            return Ok(result);
        }
        if !options.mobaxterm_skip_encrypted_credentials
            && has_ciphertext
            && options.mobaxterm_master_password.is_none_or(str::is_empty)
            && (has_master
                || ciphertexts
                    .iter()
                    .any(|(_, _, _, value)| value.starts_with("_@")))
        {
            return Err(SessionImportError::MobaXtermMasterPasswordRequired);
        }
        for (source, login, username, ciphertext) in ciphertexts {
            if options.mobaxterm_skip_encrypted_credentials {
                result.unavailable.push(source);
                continue;
            }
            let endpoint = match &source {
                CredentialSource::Endpoint(endpoint) => Some(endpoint),
                _ => None,
            };
            let context = SecretContext {
                master_password: options.mobaxterm_master_password,
                session_p: misc("SessionP"),
                system_username: misc("MPSetAccount").filter(|value| !value.is_empty()),
                system_hostname: misc("MPSetComputer").filter(|value| !value.is_empty()),
                username: endpoint.map(|value| value.username.as_str()),
                hostname: endpoint.map(|value| value.host.as_str()),
            };
            let secret = if matches!(source, CredentialSource::Named { .. }) && username.is_empty()
            {
                None
            } else {
                decrypt(ciphertext, &context)
            };
            if let Some(password) = secret {
                let username = endpoint
                    .map(|value| value.username.clone())
                    .unwrap_or_else(|| username.into());
                result.entries.push(CredentialEntry {
                    stored: MobaStoredPassword {
                        login: login.into(),
                        username,
                        password,
                    },
                    source,
                });
            } else {
                result.unavailable.push(source);
            }
        }
        if options.mobaxterm_skip_encrypted_credentials {
            report.record(
                MobaXtermCredentialDiagnosticKind::SkippedEncrypted,
                result.unavailable.len(),
            );
        } else if !result.unavailable.is_empty() {
            if options
                .mobaxterm_master_password
                .is_some_and(|value| !value.is_empty())
            {
                // A failed entry invalidates this attempt, including any apparently
                // printable results from the same unauthenticated key.
                return Err(SessionImportError::MobaXtermCredentialDecryptionFailed);
            }
            if result.entries.is_empty() && has_ciphertext {
                return Err(SessionImportError::MobaXtermMasterPasswordRequired);
            }
            report.record(
                MobaXtermCredentialDiagnosticKind::Undecryptable,
                result.unavailable.len(),
            );
        }
        Ok(result)
    }

    pub fn unavailable_for(&self, session: &MobaSession) -> bool {
        self.registry_only
            || self.unavailable.iter().any(|source| match source {
                CredentialSource::Named { name, .. } => {
                    session.credential_reference.as_deref() == Some(name)
                }
                CredentialSource::Endpoint(endpoint) => endpoint.matches(session),
                CredentialSource::Unsupported => false,
            })
    }

    fn sources(&self) -> impl Iterator<Item = (Option<usize>, &CredentialSource)> {
        self.entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (Some(index), &entry.source))
            .chain(self.unavailable.iter().map(|source| (None, source)))
    }

    pub fn referenced_username<'a>(&'a self, session: &MobaSession) -> Option<&'a str> {
        if session.protocol == MobaProtocol::Serial {
            return None;
        }
        let reference = session.credential_reference.as_deref()?;
        let mut usernames = self.sources().filter_map(|(_, source)| match source {
            CredentialSource::Named { name, username } if name == reference => {
                Some(username.as_str())
            }
            _ => None,
        });
        let username = usernames.next()?;
        // The cleartext identity remains useful when the password is skipped,
        // but duplicate definitions must not silently choose an identity.
        (usernames.next().is_none() && !username.trim().is_empty()).then_some(username)
    }
}

pub(super) struct CredentialMatch {
    pub index: Option<usize>,
    pub diagnostic: Option<MobaXtermCredentialDiagnosticKind>,
}

pub(super) fn match_encrypted(
    session: &MobaSession,
    sessions: &[MobaSession],
    credentials: &EncryptedCredentials,
) -> CredentialMatch {
    if session.protocol == MobaProtocol::Serial {
        return CredentialMatch {
            index: None,
            diagnostic: None,
        };
    }
    let mut candidates = Vec::new();
    let mut ambiguous_endpoint = false;
    // Unavailable passwords still define references and can make a match
    // ambiguous. Successful decryption must not hide duplicate definitions.
    for (index, source) in credentials.sources() {
        match source {
            CredentialSource::Named { name, .. }
                if session.credential_reference.as_deref() == Some(name) =>
            {
                candidates.push((3, index));
            }
            CredentialSource::Endpoint(endpoint) if endpoint.matches(session) => {
                // A portless record may serve duplicate bookmarks for the same
                // endpoint, but never two different ports on the same host.
                let ports: HashSet<u16> = sessions
                    .iter()
                    .filter(|candidate| endpoint.matches(candidate))
                    .map(|candidate| candidate.port)
                    .collect();
                if endpoint.port.is_none() && ports.len() > 1 {
                    ambiguous_endpoint = true;
                } else {
                    candidates.push((if endpoint.port.is_some() { 2 } else { 1 }, index));
                }
            }
            _ => {}
        }
    }
    let best = candidates.iter().map(|(score, _)| *score).max();
    let mut candidates = candidates
        .into_iter()
        .filter(|(score, _)| Some(*score) == best);
    let candidate = candidates.next();
    if candidates.next().is_some() || (candidate.is_none() && ambiguous_endpoint) {
        return CredentialMatch {
            index: None,
            diagnostic: Some(MobaXtermCredentialDiagnosticKind::AmbiguousMatch),
        };
    }
    CredentialMatch {
        index: candidate.and_then(|(_, index)| index),
        diagnostic: (candidate.is_none() && session.credential_reference.is_some())
            .then_some(MobaXtermCredentialDiagnosticKind::MissingReference),
    }
}
