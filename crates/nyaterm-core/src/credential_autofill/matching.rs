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
/// A credential the autofill pipeline can act on: an entry from the security
/// center's credential vault, or an OTP entry whose prompt pattern matches.
#[derive(Clone, PartialEq, Eq)]
pub enum CredentialAutofillTarget {
    Vault(SavedCredential),
    Otp(OtpPromptTarget),
}

/// What the matcher and the suggestion panel need from an OTP entry. The code
/// itself is generated when the user picks the row, never held here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OtpPromptTarget {
    pub id: String,
    pub issuer: String,
    pub username: String,
    pub prompt_regex: String,
}

impl CredentialAutofillTarget {
    pub fn id(&self) -> &str {
        match self {
            Self::Vault(credential) => &credential.id,
            Self::Otp(otp) => &otp.id,
        }
    }

    pub fn is_otp(&self) -> bool {
        matches!(self, Self::Otp(_))
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
/// `Send` always stands on a prior user step: the only path to it is the
/// pending vault-credential flow, which the user enters by selecting a
/// username suggestion. Remote-controlled terminal output therefore stays an
/// offer rather than an authorization to disclose a secret.
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
        } => CredentialAutofillAction::Suggest {
            kind: *kind,
            matches: matches.clone(),
            clear_pending: *clear_pending,
        },
        CredentialAutofillMatchOutcome::AutoFill { credential, kind } => {
            CredentialAutofillAction::Send {
                target: credential.clone(),
                kind: *kind,
            }
        }
    }
}

impl std::fmt::Debug for CredentialAutofillTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialAutofillTarget([REDACTED])")
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
                }
            }
        }
    }
}

/// Regex-matched suggestions: only entries whose own prompt pattern matches the
/// prompt are offered, so an unmatched prompt offers none. Saved credentials and
/// OTP entries use the same pattern semantics; OTP entries answer a password
/// or verification-code prompt, never a username prompt.
fn find_matching_credential_targets_cached(
    credentials: &[CredentialAutofillTarget],
    kind: CredentialPromptKind,
    output: &str,
    regex_cache: &mut HashMap<String, regex::Regex>,
) -> Vec<CredentialAutofillTarget> {
    credentials
        .iter()
        .filter(|target| match target {
            CredentialAutofillTarget::Vault(credential) => {
                credential_matches_prompt_cached(credential, kind, output, regex_cache)
            }
            CredentialAutofillTarget::Otp(otp) => {
                otp_matches_prompt_cached(otp, kind, output, regex_cache)
            }
        })
        .cloned()
        .collect()
}

fn otp_matches_prompt_cached(
    otp: &OtpPromptTarget,
    kind: CredentialPromptKind,
    output: &str,
    regex_cache: &mut HashMap<String, regex::Regex>,
) -> bool {
    if kind != CredentialPromptKind::Password {
        return false;
    }
    let pattern = otp.prompt_regex.trim();
    if pattern.is_empty() {
        return false;
    }
    let cache_key = format!("otp:{}:{kind:?}:{pattern}", otp.id);
    if !regex_cache.contains_key(&cache_key) {
        if regex_cache.len() >= CREDENTIAL_AUTOFILL_MATCH_REGEX_CACHE_LIMIT {
            regex_cache.clear();
        }
        let Some(regex) = compile_prompt_regex(pattern) else {
            return false;
        };
        regex_cache.insert(cache_key.clone(), regex);
    }
    regex_cache
        .get(&cache_key)
        .is_some_and(|regex| regex.is_match(output))
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

const PASSWORD_PROMPT_KEYWORDS: &[&str] = &[
    "password",
    "passphrase",
    "passcode",
    "pin",
    "otp",
    "verification code",
    "authentication code",
    "auth code",
    "2fa",
    "mfa",
    "密码",
    "口令",
    "验证码",
    "动态码",
    "动态口令",
    "mot de passe",
    "phrase de passe",
    "phrase secrète",
    "phrase secrete",
    "code secret",
    "passwort",
    "kennwort",
    "contraseña",
    "contrasena",
    "frase de paso",
    "senha",
    "frase-passe",
    "frase passe",
    "frase secreta",
    "parola d'ordine",
    "parola d’ordine",
    "parola",
    "wachtwoord",
    "hasło",
    "haslo",
    "пароль",
    "кодовая фраза",
    "парольная фраза",
    "şifre",
    "heslo",
    "lösenord",
    "passord",
    "adgangskode",
    "salasana",
    "jelszó",
    "parolă",
    "パスワード",
    "パスフレーズ",
    "비밀번호",
    "암호",
];

const USERNAME_PROMPT_KEYWORDS: &[&str] = &[
    "username",
    "user name",
    "login as",
    "login",
    "account",
    "user",
    "用户名",
    "用户",
    "账号",
    "账户",
    "登录名",
    "nom d'utilisateur",
    "nom d’utilisateur",
    "utilisateur",
    "benutzername",
    "benutzer",
    "nombre de usuario",
    "usuario",
    "nome de usuário",
    "nome de usuario",
    "usuário",
    "nome utente",
    "utente",
    "gebruikersnaam",
    "nazwa użytkownika",
    "имя пользователя",
    "пользователь",
    "kullanıcı adı",
    "kullanıcı",
    "ユーザー名",
    "ユーザ名",
    "사용자 이름",
    "사용자명",
];

fn normalize_prompt_for_keyword_lookup(trimmed: &str) -> String {
    // Collapse Unicode whitespace (including NBSP/NNBSP) and fold Unicode case
    // so multi-word localized prompts match regardless of spacing or casing.
    let mut normalized = String::with_capacity(trimmed.len());
    for token in trimmed.split_whitespace() {
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.push_str(&token.to_lowercase());
    }
    normalized
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
    let normalized = normalize_prompt_for_keyword_lookup(trimmed);
    if PASSWORD_PROMPT_KEYWORDS
        .iter()
        .any(|keyword| normalized.contains(keyword))
    {
        return Some(CredentialPromptKind::Password);
    }
    if USERNAME_PROMPT_KEYWORDS
        .iter()
        .any(|keyword| normalized.contains(keyword))
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
            } => {
                assert_eq!(kind, CredentialPromptKind::Username);
                assert_eq!(matches.len(), 1);
                assert!(matches!(
                    &matches[0],
                    CredentialAutofillTarget::Vault(credential) if credential.id == "c1"
                ));
                assert!(!clear_pending);
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn credential_autofill_worker_offers_nothing_when_no_password_pattern_matches() {
        let mut regex_cache = HashMap::new();
        let output = credential_autofill_match_outcome(
            request(
                "Password:",
                CredentialPromptKind::Password,
                vault_credentials(vec![
                    credential("c1", "", None, None, true),
                    credential(
                        "c2",
                        "root",
                        Some("login as:"),
                        Some("Enter ssh key passphrase:"),
                        true,
                    ),
                ]),
                None,
            ),
            &mut regex_cache,
        );

        assert!(
            matches!(
                output,
                CredentialAutofillMatchOutcome::NoMatch {
                    clear_pending: true
                }
            ),
            "an unmatched password prompt must not list every credential"
        );
    }

    #[test]
    fn credential_autofill_worker_suggests_every_matching_password() {
        let mut regex_cache = HashMap::new();
        let output = credential_autofill_match_outcome(
            request(
                "Password:",
                CredentialPromptKind::Password,
                vault_credentials(vec![
                    credential("c1", "", None, Some("Password:"), true),
                    credential("c2", "", None, Some("^Password:$"), true),
                    credential("c3", "", None, Some("Enter passphrase:"), true),
                ]),
                None,
            ),
            &mut regex_cache,
        );

        match output {
            CredentialAutofillMatchOutcome::Suggest {
                matches,
                clear_pending,
                ..
            } => {
                let ids: Vec<&str> = matches.iter().map(CredentialAutofillTarget::id).collect();
                assert_eq!(ids, ["c1", "c2"]);
                assert!(clear_pending);
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    fn otp_target(id: &str, prompt_regex: &str) -> CredentialAutofillTarget {
        CredentialAutofillTarget::Otp(super::OtpPromptTarget {
            id: id.to_string(),
            issuer: "GitHub".to_string(),
            username: "octocat".to_string(),
            prompt_regex: prompt_regex.to_string(),
        })
    }

    #[test]
    fn otp_entry_matches_a_verification_code_prompt_by_pattern() {
        let mut regex_cache = HashMap::new();
        let output = credential_autofill_match_outcome(
            request(
                "Verification code:",
                CredentialPromptKind::Password,
                vec![otp_target("o1", "verification code:")],
                None,
            ),
            &mut regex_cache,
        );

        match output {
            CredentialAutofillMatchOutcome::Suggest { matches, .. } => {
                assert_eq!(matches.len(), 1);
                assert!(matches[0].is_otp());
                assert_eq!(matches[0].id(), "o1");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn otp_entry_without_a_pattern_never_matches() {
        let mut regex_cache = HashMap::new();
        let output = credential_autofill_match_outcome(
            request(
                "Verification code:",
                CredentialPromptKind::Password,
                vec![otp_target("o1", "   ")],
                None,
            ),
            &mut regex_cache,
        );

        assert!(matches!(
            output,
            CredentialAutofillMatchOutcome::NoMatch {
                clear_pending: true
            }
        ));
    }

    #[test]
    fn otp_entry_is_never_offered_for_a_username_prompt() {
        let mut regex_cache = HashMap::new();
        let output = credential_autofill_match_outcome(
            request(
                "login as:",
                CredentialPromptKind::Username,
                vec![otp_target("o1", "login as:")],
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

    #[test]
    fn credentials_and_otp_entries_share_one_suggestion_list() {
        let mut regex_cache = HashMap::new();
        let mut targets = vault_credentials(vec![credential(
            "c1",
            "",
            None,
            Some("Verification code:"),
            true,
        )]);
        targets.push(otp_target("o1", "Verification code:"));
        let output = credential_autofill_match_outcome(
            request(
                "Verification code:",
                CredentialPromptKind::Password,
                targets,
                None,
            ),
            &mut regex_cache,
        );

        match output {
            CredentialAutofillMatchOutcome::Suggest { matches, .. } => {
                let ids: Vec<&str> = matches.iter().map(CredentialAutofillTarget::id).collect();
                assert_eq!(ids, ["c1", "o1"], "credentials come first");
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

    #[test]
    fn live_prompt_detection_handles_french_sudo_with_nbsp_and_unicode() {
        let french_nbsp = "[sudo] Mot de passe de xxxx\u{00a0}:";
        assert_eq!(
            super::detect_live_prompt_kind(french_nbsp),
            Some(CredentialPromptKind::Password)
        );
        assert_eq!(
            super::detect_live_prompt_kind("[sudo] Mot\u{00a0}de\u{00a0}passe de user\u{202f}:"),
            Some(CredentialPromptKind::Password)
        );
        assert_eq!(
            super::detect_live_prompt_kind("[sudo] CONTRASEÑA para xxxx:"),
            Some(CredentialPromptKind::Password)
        );
        assert_eq!(
            super::detect_live_prompt_kind("[sudo] Hasło użytkownika xxxx:"),
            Some(CredentialPromptKind::Password)
        );
        assert_eq!(
            super::detect_live_prompt_kind("[sudo] Пароль для xxxx:"),
            Some(CredentialPromptKind::Password)
        );
        assert_eq!(
            super::detect_live_prompt_kind("Nom d'utilisateur\u{00a0}:"),
            Some(CredentialPromptKind::Username)
        );

        let mut regex_cache = HashMap::new();
        for pattern in [r"\[sudo\] (Mot de passe de|password for) xxxx\s*:", "Mot"] {
            let output = credential_autofill_match_outcome(
                request(
                    french_nbsp,
                    super::detect_live_prompt_kind(french_nbsp).expect("detected password prompt"),
                    vault_credentials(vec![credential("c1", "xxxx", None, Some(pattern), true)]),
                    None,
                ),
                &mut regex_cache,
            );
            assert!(
                matches!(
                    output,
                    CredentialAutofillMatchOutcome::Suggest {
                        kind: CredentialPromptKind::Password,
                        ref matches,
                        ..
                    } if matches.len() == 1 && matches[0].id() == "c1"
                ),
                "pattern {pattern} failed to match French sudo prompt"
            );
        }
    }
}
