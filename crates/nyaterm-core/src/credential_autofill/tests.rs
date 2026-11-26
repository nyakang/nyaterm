use super::{
    CredentialPromptKind, detect_credential_prompt_kind, extract_credential_prompt_text,
    find_matching_credentials, validate_prompt_regex,
};
use crate::SavedCredential;

fn cred(
    id: &str,
    username: &str,
    user_re: Option<&str>,
    pass_re: Option<&str>,
    has_password: bool,
    enabled: bool,
) -> SavedCredential {
    SavedCredential {
        id: id.to_string(),
        sort_order: 0,
        name: id.to_string(),
        username: username.to_string(),
        password: None,
        username_prompt_regex: user_re.map(str::to_string),
        password_prompt_regex: pass_re.map(str::to_string),
        enabled,
        has_password,
    }
}

#[test]
fn strips_ansi_and_extracts_last_prompt_line() {
    let output = "\u{001b}[32mhello\u{001b}[0m\nPassword: ";
    assert_eq!(extract_credential_prompt_text(output), "Password:");
    assert_eq!(
        detect_credential_prompt_kind(output),
        Some(CredentialPromptKind::Password)
    );
}

#[test]
fn detects_username_and_chinese_password_prompts() {
    assert_eq!(
        detect_credential_prompt_kind("login as: "),
        Some(CredentialPromptKind::Username)
    );
    assert_eq!(
        detect_credential_prompt_kind("密码："),
        Some(CredentialPromptKind::Password)
    );
    assert!(detect_credential_prompt_kind("Password: \n").is_none());
}

#[test]
fn detects_localized_sudo_and_unicode_password_prompts() {
    let french_nbsp = "[sudo] Mot de passe de xxxx\u{00a0}:";
    assert_eq!(
        detect_credential_prompt_kind(french_nbsp),
        Some(CredentialPromptKind::Password)
    );
    assert_eq!(
        detect_credential_prompt_kind("[sudo] Mot\u{00a0}de\u{00a0}passe de user\u{202f}: "),
        Some(CredentialPromptKind::Password)
    );
    assert_eq!(
        detect_credential_prompt_kind("[sudo] CONTRASEÑA para alice:"),
        Some(CredentialPromptKind::Password)
    );
    assert_eq!(
        detect_credential_prompt_kind("[sudo] ПАРОЛЬ для alice:"),
        Some(CredentialPromptKind::Password)
    );
    assert_eq!(
        detect_credential_prompt_kind("Nom d'utilisateur\u{00a0}: "),
        Some(CredentialPromptKind::Username)
    );
    assert!(super::is_default_password_prompt(french_nbsp));
}

#[test]
fn matches_custom_regex_only() {
    let credentials = vec![
        cred("a", "alice", Some("login as"), Some("Password"), true, true),
        cred("b", "bob", None, None, true, true),
        cred("c", "carol", Some("user"), Some("secret"), false, true),
    ];
    let password_matches =
        find_matching_credentials(&credentials, CredentialPromptKind::Password, "Password:");
    assert_eq!(password_matches.len(), 1);
    assert_eq!(password_matches[0].id, "a");

    let username_matches =
        find_matching_credentials(&credentials, CredentialPromptKind::Username, "login as:");
    assert_eq!(username_matches.len(), 1);
    assert_eq!(username_matches[0].id, "a");
}

#[test]
fn validate_prompt_regex_rejects_empty_and_invalid() {
    assert!(validate_prompt_regex("Password"));
    assert!(!validate_prompt_regex(""));
    assert!(!validate_prompt_regex("("));
}

#[test]
fn builtin_entry_points_share_unicode_word_and_phrase_rules() {
    use super::is_default_password_prompt;
    use super::matching::detect_live_prompt_kind;
    for (prompts, expected) in [
        (
            &[
                "[sudo] Mot\u{a0}de\u{202f}passe de alice :",
                "MOT DE PASSE :",
                "CONTRASEÑA:",
                "ПАРОЛЬ:",
                "Hasło:",
                "Şifre:",
                "Sifre:",
                "Parolă:",
                "Frase\u{a0}passe:",
                "Parola d’ordine:",
                "Enter password for user:",
                "Passphrase:",
                "OTP:",
                "请输入密码：",
                "验证码：",
                "パスワード：",
                "비밀번호:",
            ][..],
            Some(CredentialPromptKind::Password),
        ),
        (
            &[
                "Nom d’utilisateur:",
                "Benutzername:",
                "Nombre de usuario:",
                "Nome de usuário:",
                "Nome utente:",
                "Имя пользователя:",
                "Kullanıcı adı:",
                "Login as:",
                "User name:",
                "Account:",
                "用户名：",
            ][..],
            Some(CredentialPromptKind::Username),
        ),
        (
            &[
                "Parola da cercare:",
                "Parolamento:",
                "Utilisateurs:",
                "Utilisateur connecté:",
                "Benutzer online:",
                "Usuario actual:",
                "Utente corrente:",
                "Пользователь подключён:",
                "Kullanıcı:",
                "Sifreleme:",
                "парольный:",
                "épassword:",
                "passwordé:",
                "Spinning:",
                "hotpot:",
                "enduser:",
                "myaccount:",
                "[sudo] Mot de passe de alice",
                "$ ",
            ][..],
            None,
        ),
    ] {
        for prompt in prompts {
            assert_eq!(detect_live_prompt_kind(prompt), expected, "live: {prompt}");
            assert_eq!(
                detect_credential_prompt_kind(prompt),
                expected,
                "output: {prompt}"
            );
            assert_eq!(
                is_default_password_prompt(prompt),
                expected == Some(CredentialPromptKind::Password),
                "default: {prompt}"
            );
        }
    }
}

#[test]
fn builtin_entry_points_preserve_their_text_extraction_policies() {
    use super::matching::detect_live_prompt_kind;
    assert_eq!(
        detect_live_prompt_kind("Password:\n"),
        Some(CredentialPromptKind::Password)
    );
    assert_eq!(detect_credential_prompt_kind("Password:\n"), None);
    assert_eq!(detect_credential_prompt_kind("Password:\r\n$ "), None);
    assert_eq!(
        detect_credential_prompt_kind("\x1b[32mMot de passe:\x1b[0m"),
        Some(CredentialPromptKind::Password)
    );
}
