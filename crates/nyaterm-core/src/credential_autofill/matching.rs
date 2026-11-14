//! Metadata-only matching. The caller owns expiry, authorization and secret resolution.
use super::{CredentialPromptKind, compile_prompt_regex, get_credential_prompt_pattern};
use crate::models::SavedCredential;
use std::collections::HashMap;

const CREDENTIAL_AUTOFILL_MATCH_REGEX_CACHE_LIMIT: usize = 512;

#[derive(Default)]
pub struct CredentialAutofillMatcher {
    regex_cache: HashMap<String, regex::Regex>,
}
impl CredentialAutofillMatcher {
    pub fn evaluate(
        &mut self,
        request: CredentialAutofillMatchRequest,
    ) -> CredentialAutofillMatchOutcome {
        credential_autofill_match_outcome(request, &mut self.regex_cache)
    }
}
/// A credential the autofill pipeline can act on. Vault entries come from the
/// security center; the connection-password candidate is derived from the
/// active session's source connection and is resolved through the connection
/// store at fill time, never the credential vault.
#[derive(Clone, PartialEq, Eq)]
pub enum CredentialAutofillTarget {
    Vault(SavedCredential),
    ConnectionPassword(ConnectionPasswordTarget),
}

#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionPasswordTarget {
    pub connection_id: String,
    pub connection_name: String,
    pub username: String,
}

impl CredentialAutofillTarget {
    pub fn enabled(&self) -> bool {
        match self {
            Self::Vault(credential) => credential.enabled,
            Self::ConnectionPassword(_) => true,
        }
    }

    pub fn username(&self) -> &str {
        match self {
            Self::Vault(credential) => &credential.username,
            Self::ConnectionPassword(candidate) => &candidate.username,
        }
    }

    pub fn is_connection_password(&self) -> bool {
        matches!(self, Self::ConnectionPassword(_))
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PendingCredentialAutofill {
    pub session_id: String,
    pub credential_id: String,
    pub expires_at_ms: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub struct CredentialAutofillMatchRequestKey {
    pub request_id: u64,
    pub session_id: String,
    pub prompt_text: String,
}

#[derive(Clone)]
pub struct CredentialAutofillMatchRequest {
    pub key: CredentialAutofillMatchRequestKey,
    pub current_line: String,
    pub prompt_kind: CredentialPromptKind,
    pub credentials: Vec<CredentialAutofillTarget>,
    pub pending: Option<PendingCredentialAutofill>,
}

#[derive(Clone)]
pub struct CredentialAutofillMatchEvent {
    pub key: CredentialAutofillMatchRequestKey,
    pub outcome: CredentialAutofillMatchOutcome,
}

#[derive(Clone)]
pub enum CredentialAutofillMatchOutcome {
    Suggest {
        kind: CredentialPromptKind,
        matches: Vec<CredentialAutofillTarget>,
        clear_pending: bool,
        /// True when the prompt pattern selected these credentials. False for
        /// the "every enabled credential" fallback, which is a list to pick
        /// from rather than a match worth defaulting to.
        precise: bool,
    },
    AutoFill {
        credential: CredentialAutofillTarget,
        kind: CredentialPromptKind,
    },
    NoMatch {
        clear_pending: bool,
    },
}

/// What the app does with a match reply.
///
/// `Send` always stands on a prior user step. The only path to it is the
/// pending vault-credential flow, which the user enters by selecting a
/// username suggestion; a connection password can never reach it, so
/// remote-controlled terminal output stays an offer rather than an
/// authorization to disclose a secret.
#[derive(Clone, PartialEq, Eq)]
pub enum CredentialAutofillAction {
    Send {
        target: CredentialAutofillTarget,
        kind: CredentialPromptKind,
    },
    Suggest {
        kind: CredentialPromptKind,
        matches: Vec<CredentialAutofillTarget>,
        clear_pending: bool,
        precise: bool,
    },
    None {
        clear_pending: bool,
    },
}

pub fn credential_autofill_action(
    outcome: &CredentialAutofillMatchOutcome,
) -> CredentialAutofillAction {
    match outcome {
        CredentialAutofillMatchOutcome::NoMatch { clear_pending } => {
            CredentialAutofillAction::None {
                clear_pending: *clear_pending,
            }
        }
        CredentialAutofillMatchOutcome::Suggest {
            kind,
            matches,
            clear_pending,
            precise,
        } => CredentialAutofillAction::Suggest {
            kind: *kind,
            matches: matches.clone(),
            clear_pending: *clear_pending,
            precise: *precise,
        },
        CredentialAutofillMatchOutcome::AutoFill { credential, kind }
            if !credential.is_connection_password() =>
        {
            CredentialAutofillAction::Send {
                target: credential.clone(),
                kind: *kind,
            }
        }
        // A connection password is decrypted and sent only from an explicit
        // selection in the suggestion panel. Terminal output is remote input, so
        // a prompt a remote process can print -- `[sudo] password for root:` --
        // must never authorize sending the saved secret on its own. Downgrade
        // any such reply to a suggestion the user has to pick.
        CredentialAutofillMatchOutcome::AutoFill { credential, kind } => {
            CredentialAutofillAction::Suggest {
                kind: *kind,
                matches: vec![credential.clone()],
                clear_pending: true,
                precise: true,
            }
        }
    }
}

impl std::fmt::Debug for CredentialAutofillTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialAutofillTarget([REDACTED])")
    }
}
impl std::fmt::Debug for ConnectionPasswordTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConnectionPasswordTarget([REDACTED])")
    }
}
impl std::fmt::Debug for PendingCredentialAutofill {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PendingCredentialAutofill([REDACTED])")
    }
}
impl std::fmt::Debug for CredentialAutofillMatchRequestKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialAutofillMatchRequestKey([REDACTED])")
    }
}
impl std::fmt::Debug for CredentialAutofillMatchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialAutofillMatchRequest([REDACTED])")
    }
}
impl std::fmt::Debug for CredentialAutofillMatchEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialAutofillMatchEvent([REDACTED])")
    }
}
impl std::fmt::Debug for CredentialAutofillMatchOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialAutofillMatchOutcome([REDACTED])")
    }
}
impl std::fmt::Debug for CredentialAutofillAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialAutofillAction([REDACTED])")
    }
}
fn credential_autofill_match_outcome(
    request: CredentialAutofillMatchRequest,
    regex_cache: &mut HashMap<String, regex::Regex>,
) -> CredentialAutofillMatchOutcome {
    if let Some(pending) = request.pending.as_ref()
        && pending.session_id == request.key.session_id
    {
        let pending_credential = request.credentials.iter().find_map(|target| match target {
            CredentialAutofillTarget::Vault(credential)
                if credential.id == pending.credential_id =>
            {
                Some(credential)
            }
            _ => None,
        });
        if let Some(credential) = pending_credential
            && (credential_matches_prompt_cached(
                credential,
                CredentialPromptKind::Password,
                &request.current_line,
                regex_cache,
            ) || credential_matches_prompt_cached(
                credential,
                CredentialPromptKind::Password,
                &request.key.prompt_text,
                regex_cache,
            ))
        {
            return CredentialAutofillMatchOutcome::AutoFill {
                credential: CredentialAutofillTarget::Vault(credential.clone()),
                kind: CredentialPromptKind::Password,
            };
        }
        if detect_live_prompt_kind(&request.current_line) != Some(CredentialPromptKind::Password) {
            return CredentialAutofillMatchOutcome::NoMatch {
                clear_pending: false,
            };
        }
    }

    match request.prompt_kind {
        CredentialPromptKind::Password => {
            let matches = find_matching_credential_targets_cached(
                &request.credentials,
                CredentialPromptKind::Password,
                &request.key.prompt_text,
                regex_cache,
            );
            if !matches.is_empty() {
                return CredentialAutofillMatchOutcome::Suggest {
                    kind: CredentialPromptKind::Password,
                    matches,
                    clear_pending: true,
                    precise: true,
                };
            }
            let fallback = find_password_only_fallback_targets(&request.credentials);
            if !fallback.is_empty() {
                return CredentialAutofillMatchOutcome::Suggest {
                    kind: CredentialPromptKind::Password,
                    matches: fallback,
                    clear_pending: true,
                    precise: false,
                };
            }
            CredentialAutofillMatchOutcome::NoMatch {
                clear_pending: true,
            }
        }
        CredentialPromptKind::Username => {
            let matches = find_matching_credential_targets_cached(
                &request.credentials,
                CredentialPromptKind::Username,
                &request.key.prompt_text,
                regex_cache,
            );
            if matches.is_empty() {
                CredentialAutofillMatchOutcome::NoMatch {
                    clear_pending: false,
                }
            } else {
                CredentialAutofillMatchOutcome::Suggest {
                    kind: CredentialPromptKind::Username,
                    matches,
                    clear_pending: false,
                    precise: true,
                }
            }
        }
    }
}

/// Regex-matched credential suggestions come from vault candidates only; the
/// connection-password candidate has no prompt pattern of its own.
fn find_matching_credential_targets_cached(
    credentials: &[CredentialAutofillTarget],
    kind: CredentialPromptKind,
    output: &str,
    regex_cache: &mut HashMap<String, regex::Regex>,
) -> Vec<CredentialAutofillTarget> {
    credentials
        .iter()
        .filter_map(|target| match target {
            CredentialAutofillTarget::Vault(credential)
                if credential_matches_prompt_cached(credential, kind, output, regex_cache) =>
            {
                Some(target.clone())
            }
            _ => None,
        })
        .collect()
}

/// Default password-prompt fallback (Tauri parity): every enabled target,
/// preserving order so the connection-password candidate stays first.
fn find_password_only_fallback_targets(
    credentials: &[CredentialAutofillTarget],
) -> Vec<CredentialAutofillTarget> {
    credentials
        .iter()
        .filter(|target| target.enabled())
        .cloned()
        .collect()
}

fn credential_matches_prompt_cached(
    credential: &SavedCredential,
    kind: CredentialPromptKind,
    output: &str,
    regex_cache: &mut HashMap<String, regex::Regex>,
) -> bool {
    if !credential.enabled {
        return false;
    }
    if kind == CredentialPromptKind::Username && credential.username.trim().is_empty() {
        return false;
    }
    if kind == CredentialPromptKind::Password && !credential.has_password {
        return false;
    }

    let pattern = get_credential_prompt_pattern(credential, kind);
    if pattern.is_empty() {
        return false;
    }
    let cache_key = format!("{}:{kind:?}:{pattern}", credential.id);
    if !regex_cache.contains_key(&cache_key) {
        if regex_cache.len() >= CREDENTIAL_AUTOFILL_MATCH_REGEX_CACHE_LIMIT {
            regex_cache.clear();
        }
        let Some(regex) = compile_prompt_regex(&pattern) else {
            return false;
        };
        regex_cache.insert(cache_key.clone(), regex);
    }
    regex_cache
        .get(&cache_key)
        .is_some_and(|regex| regex.is_match(output))
}

pub fn detect_live_prompt_kind(prompt: &str) -> Option<CredentialPromptKind> {
    let trimmed = prompt.trim();
    if trimmed.is_empty()
        || !trimmed
            .chars()
            .last()
            .is_some_and(|ch| ch == ':' || ch == '：')
    {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("password")
        || lower.contains("passphrase")
        || lower.contains("passcode")
        || lower.contains("pin")
        || lower.contains("otp")
        || lower.contains("verification code")
        || lower.contains("authentication code")
        || lower.contains("auth code")
        || lower.contains("2fa")
        || lower.contains("mfa")
        || trimmed.contains("密码")
        || trimmed.contains("口令")
        || trimmed.contains("验证码")
        || trimmed.contains("动态码")
        || trimmed.contains("动态口令")
    {
        return Some(CredentialPromptKind::Password);
    }
    if lower.contains("username")
        || lower.contains("user name")
        || lower.contains("login as")
        || lower.contains("login")
        || lower.contains("account")
        || lower.contains("user")
        || trimmed.contains("用户名")
        || trimmed.contains("用户")
        || trimmed.contains("账号")
        || trimmed.contains("账户")
        || trimmed.contains("登录名")
    {
        return Some(CredentialPromptKind::Username);
    }
    None
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        CredentialAutofillMatchOutcome, CredentialAutofillMatchRequest,
        CredentialAutofillMatchRequestKey, CredentialAutofillTarget, CredentialPromptKind,
        PendingCredentialAutofill, SavedCredential, credential_autofill_action,
        credential_autofill_match_outcome,
    };

    fn credential(
        id: &str,
        username: &str,
        username_prompt_regex: Option<&str>,
        password_prompt_regex: Option<&str>,
        has_password: bool,
    ) -> SavedCredential {
        SavedCredential {
            id: id.to_string(),
            sort_order: 0,
            name: id.to_string(),
            username: username.to_string(),
            password: None,
            username_prompt_regex: username_prompt_regex.map(str::to_string),
            password_prompt_regex: password_prompt_regex.map(str::to_string),
            enabled: true,
            has_password,
        }
    }

    fn vault_credentials(credentials: Vec<SavedCredential>) -> Vec<CredentialAutofillTarget> {
        credentials
            .into_iter()
            .map(CredentialAutofillTarget::Vault)
            .collect()
    }

    fn request(
        prompt_text: &str,
        prompt_kind: CredentialPromptKind,
        credentials: Vec<CredentialAutofillTarget>,
        pending: Option<PendingCredentialAutofill>,
    ) -> CredentialAutofillMatchRequest {
        CredentialAutofillMatchRequest {
            key: CredentialAutofillMatchRequestKey {
                request_id: 1,
                session_id: "s1".to_string(),
                prompt_text: prompt_text.to_string(),
            },
            current_line: prompt_text.to_string(),
            prompt_kind,
            credentials,
            pending,
        }
    }

    #[test]
    fn credential_autofill_worker_suggests_matching_username() {
        let mut regex_cache = HashMap::new();
        let output = credential_autofill_match_outcome(
            request(
                "login as:",
                CredentialPromptKind::Username,
                vault_credentials(vec![credential(
                    "c1",
                    "root",
                    Some("login as:"),
                    None,
                    true,
                )]),
                None,
            ),
            &mut regex_cache,
        );

        match output {
            CredentialAutofillMatchOutcome::Suggest {
                kind,
                matches,
                clear_pending,
                precise,
            } => {
                assert_eq!(kind, CredentialPromptKind::Username);
                assert_eq!(matches.len(), 1);
                assert!(matches!(
                    &matches[0],
                    CredentialAutofillTarget::Vault(credential) if credential.id == "c1"
                ));
                assert!(!clear_pending);
                assert!(precise, "a username prompt match must be a precise match");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn credential_autofill_worker_falls_back_to_password_only_credentials() {
        let mut regex_cache = HashMap::new();
        let output = credential_autofill_match_outcome(
            request(
                "Password:",
                CredentialPromptKind::Password,
                vault_credentials(vec![credential("c1", "", None, None, true)]),
                None,
            ),
            &mut regex_cache,
        );

        match output {
            CredentialAutofillMatchOutcome::Suggest {
                kind,
                matches,
                clear_pending,
                precise,
            } => {
                assert_eq!(kind, CredentialPromptKind::Password);
                assert_eq!(matches.len(), 1);
                assert!(matches!(
                    &matches[0],
                    CredentialAutofillTarget::Vault(credential) if credential.id == "c1"
                ));
                assert!(clear_pending);
                assert!(
                    !precise,
                    "the every-credential fallback is not a precise match"
                );
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn credential_autofill_worker_autofills_pending_password() {
        let mut regex_cache = HashMap::new();
        let output = credential_autofill_match_outcome(
            request(
                "Password:",
                CredentialPromptKind::Password,
                vault_credentials(vec![credential(
                    "c1",
                    "root",
                    Some("login as:"),
                    Some("Password:"),
                    true,
                )]),
                Some(PendingCredentialAutofill {
                    session_id: "s1".to_string(),
                    credential_id: "c1".to_string(),
                    expires_at_ms: u64::MAX,
                }),
            ),
            &mut regex_cache,
        );

        match output {
            CredentialAutofillMatchOutcome::AutoFill { credential, kind } => {
                assert!(matches!(
                    credential,
                    CredentialAutofillTarget::Vault(saved) if saved.id == "c1"
                ));
                assert_eq!(kind, CredentialPromptKind::Password);
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn credential_autofill_worker_connects_password_variant_precedes_vault_fallback() {
        let mut regex_cache = HashMap::new();
        let candidate =
            CredentialAutofillTarget::ConnectionPassword(super::ConnectionPasswordTarget {
                connection_id: "conn-1".to_string(),
                connection_name: "prod".to_string(),
                username: "dev".to_string(),
            });
        let mut credentials = vec![candidate.clone()];
        credentials.extend(vault_credentials(vec![credential(
            "c1", "", None, None, true,
        )]));
        let output = credential_autofill_match_outcome(
            request(
                "Password:",
                CredentialPromptKind::Password,
                credentials,
                None,
            ),
            &mut regex_cache,
        );

        match output {
            CredentialAutofillMatchOutcome::Suggest {
                kind,
                matches,
                clear_pending,
                precise,
            } => {
                assert_eq!(kind, CredentialPromptKind::Password);
                assert_eq!(matches.len(), 2);
                assert_eq!(matches[0], candidate, "connection password stays first");
                assert!(matches!(
                    &matches[1],
                    CredentialAutofillTarget::Vault(credential) if credential.id == "c1"
                ));
                assert!(clear_pending);
                assert!(
                    !precise,
                    "a bare password prompt reaches the fallback list, not a precise match"
                );
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn credential_autofill_worker_ignores_connection_password_as_username() {
        let mut regex_cache = HashMap::new();
        let candidate =
            CredentialAutofillTarget::ConnectionPassword(super::ConnectionPasswordTarget {
                connection_id: "conn-1".to_string(),
                connection_name: "prod".to_string(),
                username: "dev".to_string(),
            });
        let output = credential_autofill_match_outcome(
            request(
                "login as:",
                CredentialPromptKind::Username,
                vec![candidate],
                None,
            ),
            &mut regex_cache,
        );

        assert!(matches!(
            output,
            CredentialAutofillMatchOutcome::NoMatch {
                clear_pending: false
            }
        ));
    }

    fn connection_password_target(username: &str) -> CredentialAutofillTarget {
        CredentialAutofillTarget::ConnectionPassword(super::ConnectionPasswordTarget {
            connection_id: "conn-1".to_string(),
            connection_name: "prod".to_string(),
            username: username.to_string(),
        })
    }

    #[test]
    fn sudo_prompt_for_login_user_suggests_connection_password_without_sending_it() {
        // A remote process can print this; it must not be able to authorize
        // sending the saved connection password.
        let mut regex_cache = HashMap::new();
        let candidate = connection_password_target("root");
        let outcome = credential_autofill_match_outcome(
            request(
                "[sudo] password for root:",
                CredentialPromptKind::Password,
                vec![candidate.clone()],
                None,
            ),
            &mut regex_cache,
        );

        // The sole candidate does reach the user as a suggestion. It arrives
        // through the every-credential fallback (the connection password has no
        // vault prompt pattern), so it is not pre-selected...
        assert_eq!(
            credential_autofill_action(&outcome),
            super::CredentialAutofillAction::Suggest {
                kind: CredentialPromptKind::Password,
                matches: vec![candidate],
                clear_pending: true,
                precise: false,
            }
        );
        assert!(
            !matches!(
                credential_autofill_action(&outcome),
                super::CredentialAutofillAction::Send { .. }
            ),
            "a matching sudo prompt must never resolve to a send"
        );
    }

    #[test]
    fn connection_password_autofill_reply_never_sends() {
        // Defense in depth: even a reply that claims AutoFill is downgraded to
        // a suggestion when it carries the connection password.
        let candidate = connection_password_target("root");
        let action = credential_autofill_action(&CredentialAutofillMatchOutcome::AutoFill {
            credential: candidate.clone(),
            kind: CredentialPromptKind::Password,
        });

        assert_eq!(
            action,
            super::CredentialAutofillAction::Suggest {
                kind: CredentialPromptKind::Password,
                matches: vec![candidate],
                clear_pending: true,
                precise: true,
            }
        );
    }

    #[test]
    fn pending_vault_credential_still_autofills_after_user_selection() {
        // The pending flow is entered by selecting a username suggestion, so
        // its auto-fill keeps the vault path working.
        let outcome = CredentialAutofillMatchOutcome::AutoFill {
            credential: CredentialAutofillTarget::Vault(credential("c1", "root", None, None, true)),
            kind: CredentialPromptKind::Password,
        };

        assert!(matches!(
            credential_autofill_action(&outcome),
            super::CredentialAutofillAction::Send {
                kind: CredentialPromptKind::Password,
                ..
            }
        ));
    }

    #[test]
    fn no_match_reply_produces_no_action() {
        assert_eq!(
            credential_autofill_action(&CredentialAutofillMatchOutcome::NoMatch {
                clear_pending: true
            }),
            super::CredentialAutofillAction::None {
                clear_pending: true
            }
        );
    }
}
