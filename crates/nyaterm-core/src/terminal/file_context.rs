//! Ephemeral shell file-access evidence. Never persisted or used as a history scope.
use crate::completion::types::CompletionScope;
use base64::{Engine as _, engine::general_purpose::STANDARD};

pub mod filter;

#[derive(Clone, PartialEq, Eq)]
pub struct ShellFileContext {
    pub binding: String,
    pub instance: u64,
    pub user: u64,
    pub sequence: u64,
    pub home: String,
    pub cwd: String,
}
impl std::fmt::Debug for ShellFileContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ShellFileContext([REDACTED])")
    }
}
impl ShellFileContext {
    pub fn parse_osc(payload: &str) -> Option<Self> {
        if payload.len() > 4096 {
            return None;
        }
        let mut fields = payload
            .strip_prefix("7777;NyaTermFileContext;1;")?
            .split(';');
        let binding = fields.next()?;
        if binding.len() != 32 || !binding.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let instance = fields.next()?.parse().ok()?;
        let user = fields.next()?.parse().ok()?;
        let sequence = fields.next()?.parse().ok()?;
        let path = |field: &str| {
            let value = String::from_utf8(STANDARD.decode(field).ok()?).ok()?;
            (value.len() <= 1024 && value.starts_with('/') && !value.chars().any(char::is_control))
                .then_some(value)
        };
        let home = path(fields.next()?)?;
        let cwd = path(fields.next()?)?;
        if fields.next().is_some() || instance == 0 || sequence == 0 {
            return None;
        }
        Some(Self {
            binding: binding.into(),
            instance,
            user,
            sequence,
            home,
            cwd,
        })
    }
    pub fn same_shell(&self, other: &Self) -> bool {
        self.binding == other.binding
            && self.instance == other.instance
            && self.user == other.user
            && self.home == other.home
    }
    pub fn cache_key(&self) -> String {
        CompletionScope::new(&[
            &self.binding,
            &self.instance.to_string(),
            &self.user.to_string(),
            &self.home,
            &self.cwd,
        ])
        .0
    }
}

#[cfg(test)]
mod tests {
    use super::ShellFileContext;
    #[test]
    fn file_context_is_bounded_versioned_and_redacted() {
        let mark = "7777;NyaTermFileContext;1;0123456789abcdef0123456789abcdef;123;1000;1;L2hvbWUvdXNlcg==;L3RtcA==";
        let context = ShellFileContext::parse_osc(mark).unwrap();
        assert_eq!(context.home, "/home/user");
        assert_eq!(context.cwd, "/tmp");
        assert_eq!(format!("{context:?}"), "ShellFileContext([REDACTED])");
        for invalid in [
            mark.replace(";1;012", ";2;012"),
            format!("{mark};extra"),
            mark.replace("L3RtcA==", "YmFk"),
            mark.replace("L3RtcA==", "Lw0K"),
        ] {
            assert!(ShellFileContext::parse_osc(&invalid).is_none());
        }
    }
}
