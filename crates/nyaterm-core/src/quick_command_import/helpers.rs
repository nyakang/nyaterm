use std::time::{SystemTime, UNIX_EPOCH};

use crate::uuid;

pub(super) fn require_text(value: &str, field: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} cannot be empty"));
    }
    Ok(trimmed.to_string())
}

pub(super) fn require_preserved_text(value: &str, field: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        return Err(format!("{field} cannot be empty"));
    }
    Ok(value.to_string())
}

pub(super) fn normalize_id(value: &str, field: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} cannot be empty"));
    }
    Ok(trimmed.to_string())
}

pub(super) fn trim_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(super) fn validate_one_of(value: &str, allowed: &[&str], field: &str) -> Result<(), String> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(format!("{field} must be one of: {}", allowed.join(", ")))
    }
}

pub(super) fn slugify(value: &str) -> String {
    let mut output = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
        } else if ch == '-' || ch == '_' {
            output.push(ch);
        } else if ch.is_whitespace() && !output.ends_with('-') {
            output.push('-');
        }
    }
    let output = output.trim_matches('-').to_string();
    if output.is_empty() {
        format!("category-{}", uuid())
    } else {
        output
    }
}

pub(super) fn map_windterm_icon(value: &str) -> Option<String> {
    let normalized = value.to_ascii_lowercase();
    let mappings = [
        ("kubernetes", "k8s"),
        ("k8s", "k8s"),
        ("docker", "docker"),
        ("linux", "linux"),
        ("ubuntu", "ubuntu"),
        ("debian", "debian"),
        ("centos", "centos"),
        ("fedora", "fedora"),
        ("apple", "apple"),
        ("github", "github"),
        ("gitlab", "gitlab"),
        ("nginx", "nginx"),
        ("redis", "redis"),
        ("postgres", "postgres"),
        ("mysql", "mysql"),
        ("mongo", "mongodb"),
        ("python", "python"),
        ("javascript", "js"),
        ("typescript", "ts"),
        ("rust", "rust"),
        ("node", "node"),
        ("php", "php"),
        ("aws", "aws"),
        ("gcp", "gcp"),
    ];

    mappings
        .iter()
        .find_map(|(needle, icon)| normalized.contains(needle).then(|| (*icon).to_string()))
}

pub(super) fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
