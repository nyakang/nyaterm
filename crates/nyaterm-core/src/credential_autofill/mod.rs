//! Terminal-output credential autofill helpers (Tauri `credentialAutofill.ts` parity).

pub mod matching;

use crate::SavedCredential;
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialPromptKind {
    Username,
    Password,
}

fn csi_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // CSI sequences: ESC [ ... final-byte
    RE.get_or_init(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").expect("csi regex"))
}

fn osc_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)").expect("osc regex"))
}

fn other_esc_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Single-char ESC sequences and 8-bit CSI/OSC intro leftovers.
    RE.get_or_init(|| Regex::new(r"\x1b[@-Z\\-_]").expect("esc regex"))
}

fn username_prompt_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(user\s*name|username|login|login\s+as|account|user|nom\s+d['’]utilisateur|utilisateur|benutzername|benutzer|nombre\s+de\s+usuario|usuario|nome\s+de\s+usu[áa]rio|usu[áa]rio|nome\s+utente|utente|gebruikersnaam|nazwa\s+u[żz]ytkownika|имя\s+пользователя|пользователь|kullan[ıi]c[ıi]\s+ad[ıi])\b|(?:用户名|用户|账号|账户|登录名|ユーザー名|ユーザ名|사용자\s*이름|사용자명)",
        )
        .expect("username prompt regex")
    })
}

fn password_prompt_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(pass(word|phrase|code)?|pin|otp|verification\s*code|auth(entication)?\s*code|2fa|mfa|mot\s+de\s+passe|phrase\s+de\s+passe|phrase\s+secr[èe]te|code\s+secret|passwort|kennwort|contrase[nñ]a|frase\s+de\s+paso|senha|frase[- ]passe|frase\s+secreta|parola\s+d['’]ordine|parola|wachtwoord|has[łl]o|пароль|кодовая\s+фраза|парольная\s+фраза|[şs]ifre|heslo|l[öo]senord|passord|adgangskode|salasana|jelsz[óo]|parol[ăa])\b|(?:密码|口令|验证码|动态码|动态口令|パスワード|パスフレーズ|비밀번호|암호)",
        )
        .expect("password prompt regex")
    })
}

fn prompt_terminator_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[:\u{ff1a}]\s*$").expect("prompt terminator regex"))
}

pub fn strip_terminal_control_sequences(text: &str) -> String {
    let without_osc = osc_pattern().replace_all(text, "");
    let without_csi = csi_pattern().replace_all(&without_osc, "");
    other_esc_pattern()
        .replace_all(&without_csi, "")
        .into_owned()
}

pub fn extract_credential_prompt_text(output: &str) -> String {
    let stripped = strip_terminal_control_sequences(output);
    if stripped
        .chars()
        .last()
        .is_some_and(|ch| ch == '\r' || ch == '\n')
    {
        return String::new();
    }

    let normalized = stripped.replace('\r', "\n");
    let mut collapsed = String::with_capacity(normalized.len());
    let mut prev_nl = false;
    for ch in normalized.chars() {
        if ch == '\n' {
            if !prev_nl {
                collapsed.push('\n');
            }
            prev_nl = true;
        } else {
            prev_nl = false;
            collapsed.push(ch);
        }
    }

    let prompt = collapsed
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if prompt.chars().count() > 500 {
        prompt
            .chars()
            .rev()
            .take(500)
            .collect::<String>()
            .chars()
            .rev()
            .collect()
    } else {
        prompt
    }
}

pub fn detect_credential_prompt_kind(output: &str) -> Option<CredentialPromptKind> {
    let prompt = extract_credential_prompt_text(output);
    if prompt.is_empty() || !prompt_terminator_pattern().is_match(&prompt) {
        return None;
    }
    if password_prompt_pattern().is_match(&prompt) {
        return Some(CredentialPromptKind::Password);
    }
    if username_prompt_pattern().is_match(&prompt) {
        return Some(CredentialPromptKind::Username);
    }
    None
}

pub fn is_default_password_prompt(output: &str) -> bool {
    let prompt = extract_credential_prompt_text(output);
    !prompt.is_empty()
        && prompt_terminator_pattern().is_match(&prompt)
        && password_prompt_pattern().is_match(&prompt)
}

pub fn compile_prompt_regex(pattern: &str) -> Option<Regex> {
    Regex::new(&format!("(?im){pattern}")).ok()
}

pub fn get_credential_prompt_pattern(
    credential: &SavedCredential,
    kind: CredentialPromptKind,
) -> String {
    let custom = match kind {
        CredentialPromptKind::Username => credential.username_prompt_regex.as_deref(),
        CredentialPromptKind::Password => credential.password_prompt_regex.as_deref(),
    };
    custom
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("")
        .to_string()
}

pub fn credential_matches_prompt(
    credential: &SavedCredential,
    kind: CredentialPromptKind,
    output: &str,
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
    let Some(regex) = compile_prompt_regex(&pattern) else {
        return false;
    };
    regex.is_match(output)
}

pub fn find_matching_credentials(
    credentials: &[SavedCredential],
    kind: CredentialPromptKind,
    output: &str,
) -> Vec<SavedCredential> {
    credentials
        .iter()
        .filter(|credential| credential_matches_prompt(credential, kind, output))
        .cloned()
        .collect()
}

pub fn validate_prompt_regex(pattern: &str) -> bool {
    let trimmed = pattern.trim();
    !trimmed.is_empty() && compile_prompt_regex(trimmed).is_some()
}

#[cfg(test)]
mod tests;
