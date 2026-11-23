use nyaterm_files::file_browser::{FileBrowserBackendKind, file_browser_name, file_browser_parent};
use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry};
use nyaterm_ui::file_navigation::Breadcrumb;
use rust_i18n::t;
pub fn breadcrumbs(
    kind: FileBrowserBackendKind,
    path: &RemoteFilePath,
) -> Vec<Breadcrumb<RemoteFilePath>> {
    let mut current = path.clone();
    let mut segments = Vec::new();
    for _ in 0..128 {
        if current.display_path.is_empty() {
            break;
        }
        let parent = file_browser_parent(kind, &current.display_path);
        let posix_root = matches!(
            kind,
            FileBrowserBackendKind::Remote | FileBrowserBackendKind::Adb
        ) && current.display_path.trim_end_matches('/').is_empty();
        let is_root = posix_root || parent == current.display_path;
        segments.push(Breadcrumb {
            id: current.identity_key(),
            label: if posix_root {
                "/".to_string()
            } else if is_root {
                current.display_path.clone()
            } else {
                file_browser_name(kind, &current.display_path)
            },
            children_tooltip: t!(
                "fileExplorer.showChildDirectories",
                path = current.display_path
            )
            .to_string(),
            key: current.clone(),
        });
        if is_root {
            break;
        }
        current = if kind == FileBrowserBackendKind::Remote {
            let Ok(parent) = current.parent() else { break };
            parent
        } else {
            RemoteFilePath::new(parent)
        };
    }
    segments.reverse();
    segments
}

pub fn child_paths(entries: &[SftpFileEntry], hidden: bool) -> Vec<(String, RemoteFilePath)> {
    let mut entries = entries
        .iter()
        .filter(|entry| {
            entry.is_directory()
                && entry.name != "."
                && entry.name != ".."
                && (hidden || !entry.name.starts_with('.'))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        nyaterm_files::file_browser::listing::natural_compare(&a.name, &b.name)
            .then_with(|| a.name.cmp(&b.name))
    });
    entries
        .into_iter()
        .map(|entry| (entry.name.clone(), entry.remote_path()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::breadcrumbs;
    use nyaterm_files::file_browser::FileBrowserBackendKind;
    use nyaterm_files::sftp::RemoteFilePath;

    #[test]
    fn absolute_breadcrumbs_stop_at_the_posix_root() {
        for kind in [FileBrowserBackendKind::Remote, FileBrowserBackendKind::Adb] {
            for (path, labels) in [
                ("/", vec!["/"]),
                ("/root", vec!["/", "root"]),
                ("/root/", vec!["/", "root"]),
                ("/home/nya/work", vec!["/", "home", "nya", "work"]),
            ] {
                let segments = breadcrumbs(kind, &RemoteFilePath::new(path));
                assert_eq!(
                    segments
                        .iter()
                        .map(|s| s.label.as_str())
                        .collect::<Vec<_>>(),
                    labels,
                    "{kind:?}: {path}"
                );
                assert_eq!(segments[0].key.display_path, "/");
                assert_eq!(segments.last().unwrap().key.display_path, path);
            }
        }
    }

    #[test]
    fn remote_breadcrumb_targets_preserve_raw_bytes_including_the_root() {
        let path = RemoteFilePath::from_raw("/data/�/work", b"/data/\xff/work");
        let segments = breadcrumbs(FileBrowserBackendKind::Remote, &path);
        let targets = segments
            .iter()
            .map(|s| s.key.raw_path().unwrap().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            targets,
            [
                b"/".to_vec(),
                b"/data".to_vec(),
                b"/data/\xff".to_vec(),
                b"/data/\xff/work".to_vec()
            ]
        );
        assert_eq!(segments.last().unwrap().key, path);
        assert_eq!(segments[0].label, "/");
        assert_eq!(segments[0].id, segments[0].key.identity_key());
    }

    #[test]
    fn relative_breadcrumbs_keep_the_current_directory_root() {
        let segments = breadcrumbs(
            FileBrowserBackendKind::Remote,
            &RemoteFilePath::new("work/src"),
        );
        assert_eq!(
            segments
                .iter()
                .map(|s| s.label.as_str())
                .collect::<Vec<_>>(),
            [".", "work", "src"]
        );
    }

    #[cfg(windows)]
    #[test]
    fn local_breadcrumbs_keep_drive_and_unc_roots() {
        for (path, root) in [
            (r"C:\Users\nya", r"C:\"),
            (r"\\server\share\folder", r"\\server\share\"),
        ] {
            let segments = breadcrumbs(FileBrowserBackendKind::Local, &RemoteFilePath::new(path));
            assert_eq!(segments[0].label, root);
            assert_eq!(segments[0].key.display_path, root);
            assert_eq!(segments.last().unwrap().key.display_path, path);
            assert!(segments.iter().all(|s| !s.label.is_empty()));
        }
    }
}
