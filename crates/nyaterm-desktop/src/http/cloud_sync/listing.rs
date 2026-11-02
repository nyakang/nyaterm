//! A failed/incomplete listing is never evidence that a sync space is empty.
use std::collections::BTreeSet;

use nyaterm_core::CloudSyncError;
use serde_json::Value;

pub(super) fn invalid() -> CloudSyncError {
    CloudSyncError::Remote("Cloud file listing is incomplete or invalid".into())
}

pub(super) fn name(item: &Value) -> Result<String, CloudSyncError> {
    item.get("name")
        .and_then(Value::as_str)
        .filter(|name| {
            !name.is_empty() && !matches!(*name, "." | "..") && !name.contains(['/', '\\', '\0'])
        })
        .map(ToOwned::to_owned)
        .ok_or_else(invalid)
}

pub(super) fn next(value: &Value, field: &str) -> Result<Option<String>, CloudSyncError> {
    match value.get(field) {
        None => Ok(None),
        Some(Value::String(token)) if !token.trim().is_empty() => Ok(Some(token.clone())),
        _ => Err(invalid()),
    }
}

#[derive(Default)]
pub(super) struct ListingProgress {
    pages: usize,
    cursors: BTreeSet<String>,
    names: BTreeSet<String>,
}

impl ListingProgress {
    pub(super) fn observe(
        &mut self,
        names: &[String],
        next: Option<&str>,
    ) -> Result<(), CloudSyncError> {
        self.pages += 1;
        if self.pages > 10_000
            || self.names.len() + names.len() > 100_000
            || names.iter().any(|name| !self.names.insert(name.clone()))
            || next.is_some_and(|token| token.len() > 16_384 || !self.cursors.insert(token.into()))
        {
            return Err(invalid());
        }
        Ok(())
    }
}

pub(super) fn validate_next_link(first: &str, next: &str) -> Result<(), CloudSyncError> {
    let first = url::Url::parse(first).map_err(|_| invalid())?;
    let next = url::Url::parse(next).map_err(|_| invalid())?;
    if next.scheme() != "https"
        || next.origin() != first.origin()
        || next.path() != first.path()
        || !next.username().is_empty()
        || next.password().is_some()
        || next.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ListingProgress, validate_next_link};

    #[test]
    fn pagination_rejects_cycles_duplicate_objects_and_redirected_bearer_requests() {
        let mut listing = ListingProgress::default();
        listing.observe(&["first".into()], Some("next")).unwrap();
        assert!(listing.observe(&["second".into()], Some("next")).is_err());
        assert!(listing.observe(&["first".into()], None).is_err());
        let first = "https://graph.microsoft.com/v1.0/me/drive/root/children";
        assert!(validate_next_link(first, &format!("{first}?skip=next")).is_ok());
        for link in [
            "https://other.example/v1.0/me/drive/root/children",
            "http://graph.microsoft.com/v1.0/me/drive/root/children",
            "https://graph.microsoft.com/v1.0/other/children",
        ] {
            assert!(validate_next_link(first, link).is_err());
        }
    }
}
