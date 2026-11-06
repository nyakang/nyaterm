//! Path policies shared by browser navigation, state transitions and presentation.

use nyaterm_files::sftp::RemoteFilePath;
use std::path::Path;

pub(in crate::features) fn normalized_transfer_browser_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        ".".to_string()
    } else if trimmed == "/"
        || (looks_like_native_path(trimmed) && Path::new(trimmed).parent().is_none())
    {
        trimmed.to_string()
    } else if looks_like_native_path(trimmed) {
        trimmed.trim_end_matches(['/', '\\']).to_string()
    } else {
        trimmed.trim_end_matches('/').to_string()
    }
}

fn looks_like_native_path(path: &str) -> bool {
    path.contains('\\')
        || path.as_bytes().get(1) == Some(&b':')
        || (path.starts_with("//") && !path.starts_with("///"))
}

pub(in crate::features) fn transfer_browser_path_is_root(path: &str) -> bool {
    let path = normalized_transfer_browser_path(path);
    path == "."
        || path == "/"
        || (looks_like_native_path(&path) && Path::new(&path).parent().is_none())
}

/// Return the direct child to reveal when navigating to a remote ancestor.
/// Walking both display and raw parents preserves identities for undecodable names.
pub(in crate::features) fn remote_ancestor_selection(
    source: RemoteFilePath,
    destination: &mut RemoteFilePath,
) -> Option<String> {
    let target = normalized_transfer_browser_path(&destination.display_path);
    let mut child = source;
    while !transfer_browser_path_is_root(&child.display_path) {
        let parent = child.parent().ok()?;
        if parent == child {
            return None;
        }
        if normalized_transfer_browser_path(&parent.display_path) == target {
            // A supplied token must agree; display strings can alias distinct raw paths.
            if destination.raw_path_token.is_some()
                && destination.raw_path_token != parent.raw_path_token
            {
                return None;
            }
            if destination.raw_path_token.is_none() {
                destination.raw_path_token = parent.raw_path_token;
            }
            return Some(child.identity_key());
        }
        child = parent;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::remote_ancestor_selection;
    use nyaterm_files::sftp::RemoteFilePath;

    #[test]
    fn ancestor_navigation_preserves_raw_child_and_destination_paths() {
        let source = RemoteFilePath::from_raw("/a/unknown/c", b"/a/\xff/c");
        for (destination, expected) in [
            (
                "/a/unknown",
                RemoteFilePath::from_raw("/a/unknown/c", b"/a/\xff/c"),
            ),
            ("/a/", RemoteFilePath::from_raw("/a/unknown", b"/a/\xff")),
            ("/", RemoteFilePath::from_raw("/a", b"/a")),
        ] {
            let mut destination = RemoteFilePath::new(destination);
            assert_eq!(
                remote_ancestor_selection(source.clone(), &mut destination),
                Some(expected.identity_key())
            );
            assert_eq!(
                destination.raw_path_token,
                expected.parent().unwrap().raw_path_token
            );
        }
    }

    #[test]
    fn refresh_descendant_unrelated_and_prefix_paths_have_no_selection_intent() {
        for (source, target) in [
            ("/a/b", "/a/b"),
            ("/a/b", "/a/b/c"),
            ("/a/b", "/other"),
            ("/a/b2/c", "/a/b"),
            ("/", "/"),
            (".", "."),
        ] {
            let mut destination = RemoteFilePath::new(target);
            assert_eq!(
                remote_ancestor_selection(source.into(), &mut destination),
                None
            );
            assert!(destination.raw_path_token.is_none());
        }
    }

    #[test]
    fn explicit_destination_token_must_match_the_raw_ancestor() {
        let source = RemoteFilePath::from_raw("/same/child", b"/\xff/child");
        let mut other = RemoteFilePath::from_raw("/same", b"/\xfe");
        let original = other.clone();
        assert_eq!(remote_ancestor_selection(source.clone(), &mut other), None);
        assert_eq!(other, original);
        let mut matching = source.parent().unwrap();
        assert_eq!(
            remote_ancestor_selection(source.clone(), &mut matching),
            Some(source.identity_key())
        );
    }

    #[test]
    fn plain_paths_remain_supported_and_invalid_tokens_do_not_fall_back_to_display() {
        let mut target = RemoteFilePath::new("/a");
        assert_eq!(
            remote_ancestor_selection("/a/b/c".into(), &mut target),
            Some("/a/b".into())
        );
        let source = RemoteFilePath {
            display_path: "/a/b".into(),
            raw_path_token: Some("!invalid".into()),
        };
        assert_eq!(remote_ancestor_selection(source, &mut target), None);
    }
}
