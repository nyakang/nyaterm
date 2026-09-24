use super::{
    CredentialPromptKind, credential_password_prompt_target_user,
    credential_password_prompt_targets_user, detect_credential_prompt_kind,
    extract_credential_prompt_text, find_matching_credentials,
    find_password_only_fallback_credentials, validate_prompt_regex,
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
fn matches_custom_regex_and_password_fallback() {
    let credentials = vec![
        cred("a", "alice", Some("login as"), Some("Password"), true, true),
        cred("b", "bob", None, None, true, true),
        cred("c", "carol", Some("user"), Some("secret"), false, true),
    ];
    let password_matches =
        find_matching_credentials(&credentials, CredentialPromptKind::Password, "Password:");
    assert_eq!(password_matches.len(), 1);
    assert_eq!(password_matches[0].id, "a");

    let fallback = find_password_only_fallback_credentials(&credentials);
    assert_eq!(fallback.len(), 3);

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
fn extracts_sudo_password_prompt_target_user() {
    assert_eq!(
        credential_password_prompt_target_user("[sudo] password for root:"),
        Some("root".to_string())
    );
    assert_eq!(
        credential_password_prompt_target_user("Password for alice:"),
        Some("alice".to_string())
    );
    assert_eq!(
        credential_password_prompt_target_user("Enter password for root@host:"),
        Some("root@host".to_string())
    );
    assert_eq!(
        credential_password_prompt_target_user("Enter password for user 'root':"),
        Some("root".to_string())
    );
    assert_eq!(
        credential_password_prompt_target_user("密码（root 的密码）"),
        None
    );
}

#[test]
fn extracts_chinese_sudo_password_prompt_target_user() {
    assert_eq!(
        credential_password_prompt_target_user("用户 root 的密码："),
        Some("root".to_string())
    );
    assert_eq!(
        credential_password_prompt_target_user("账号 alice 的密码:"),
        Some("alice".to_string())
    );
}

#[test]
fn plain_password_prompt_has_no_target_user() {
    assert_eq!(credential_password_prompt_target_user("Password:"), None);
    assert_eq!(credential_password_prompt_target_user("密码："), None);
    assert!(!credential_password_prompt_targets_user(
        "Password:",
        "root"
    ));
}

#[test]
fn sudo_prompt_matches_only_the_named_user() {
    assert!(credential_password_prompt_targets_user(
        "[sudo] password for root:",
        "root"
    ));
    assert!(!credential_password_prompt_targets_user(
        "[sudo] password for root:",
        "alice"
    ));
    assert!(credential_password_prompt_targets_user(
        "用户 root 的密码：",
        "root"
    ));
    assert!(!credential_password_prompt_targets_user(
        "用户 root 的密码：",
        "alice"
    ));
    assert!(credential_password_prompt_targets_user(
        "Enter password for root@host:",
        "root"
    ));
    assert!(credential_password_prompt_targets_user(
        "[sudo] password for ROOT:",
        "root"
    ));
    assert!(!credential_password_prompt_targets_user(
        "[sudo] password for root:",
        ""
    ));
}
