#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TransferContextMenuNode {
    Action(TransferContextMenuAction),
    Separator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TransferContextMenuAction {
    Open,
    Preview,
    OpenInternal,
    OpenExternal,
    Refresh,
    Upload,
    Download,
    SendTo,
    Move,
    AddToFavorites,
    CopyInfo,
    CopyDirectoryPath,
    Terminal,
    Ai,
    Properties,
    GoUp,
    NewFile,
    NewFolder,
    NewSymlink,
}

pub(super) fn transfer_context_action_visible_for_backend(
    action: TransferContextMenuAction,
    backend: nyaterm_transport::FileBrowserBackendKind,
) -> bool {
    backend == nyaterm_transport::FileBrowserBackendKind::Remote
        || !matches!(
            action,
            TransferContextMenuAction::Upload
                | TransferContextMenuAction::Download
                | TransferContextMenuAction::NewSymlink
        )
}

pub(super) fn transfer_visible_context_menu_nodes(
    nodes: impl IntoIterator<Item = TransferContextMenuNode>,
    backend: nyaterm_transport::FileBrowserBackendKind,
) -> Vec<TransferContextMenuNode> {
    let mut visible = Vec::new();
    for node in nodes {
        if let TransferContextMenuNode::Action(action) = node
            && !transfer_context_action_visible_for_backend(action, backend)
        {
            continue;
        }
        if node == TransferContextMenuNode::Separator
            && (visible.is_empty() || visible.last() == Some(&node))
        {
            continue;
        }
        visible.push(node);
    }
    if visible.last() == Some(&TransferContextMenuNode::Separator) {
        visible.pop();
    }
    visible
}

pub(super) fn transfer_action_bar_enabled(selection_count: usize, can_paste: bool) -> [bool; 5] {
    [
        selection_count > 0,
        selection_count > 0,
        can_paste,
        selection_count == 1,
        selection_count > 0,
    ]
}

pub(super) fn transfer_directory_terminal_actions_visible(
    is_directory: bool,
    is_symlink: bool,
) -> bool {
    is_directory && !is_symlink
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct TransferEntryMenuCapabilities {
    pub is_directory: bool,
    pub show_open_internal: bool,
    pub show_open_external: bool,
    pub show_preview: bool,
    pub has_ai_actions: bool,
    /// Whether at least one other browsable local or SSH session exists to send the
    /// selection to. Drives the "Send to" submenu after the download actions, matching
    /// the Tauri `openMoveDialog(getContextMenuEntries)` placement.
    pub has_send_targets: bool,
    pub is_tree_view: bool,
    pub terminal_available: bool,
}

pub(super) fn transfer_entry_context_menu_policy(
    capabilities: TransferEntryMenuCapabilities,
) -> Vec<TransferContextMenuNode> {
    use TransferContextMenuAction as Action;
    use TransferContextMenuNode::{Action as Item, Separator};

    let mut items = vec![Item(Action::Open)];
    if capabilities.show_preview {
        items.push(Item(Action::Preview));
    }
    if capabilities.show_open_internal {
        items.push(Item(Action::OpenInternal));
    }
    if capabilities.show_open_external {
        items.push(Item(Action::OpenExternal));
    }
    items.extend([Separator, Item(Action::Refresh)]);
    if capabilities.is_tree_view {
        items.extend([Item(Action::NewFile), Item(Action::NewFolder)]);
    }
    items.extend([Item(Action::Upload), Item(Action::Download), Separator]);
    if capabilities.is_tree_view {
        items.extend([Item(Action::NewSymlink), Separator]);
    }
    if capabilities.has_send_targets {
        items.extend([Item(Action::SendTo), Separator]);
    }
    items.extend([Item(Action::Move), Separator]);
    if capabilities.is_directory {
        items.extend([Item(Action::AddToFavorites), Separator]);
    }
    items.push(Item(Action::CopyInfo));
    if capabilities.terminal_available {
        items.extend([Separator, Item(Action::Terminal)]);
    }
    if capabilities.has_ai_actions {
        items.extend([Separator, Item(Action::Ai)]);
    }
    items.extend([Separator, Item(Action::Properties)]);
    items
}

pub(super) fn transfer_current_directory_context_menu_policy(
    terminal_available: bool,
) -> Vec<TransferContextMenuNode> {
    use TransferContextMenuAction as Action;
    use TransferContextMenuNode::{Action as Item, Separator};

    let mut items = vec![
        Item(Action::Refresh),
        Item(Action::Upload),
        Separator,
        Item(Action::NewFile),
        Item(Action::NewFolder),
        Item(Action::NewSymlink),
        Separator,
        Item(Action::CopyDirectoryPath),
    ];
    if terminal_available {
        items.push(Item(Action::Terminal));
    }
    items.extend([Separator, Item(Action::Properties)]);
    items
}

pub(super) fn transfer_parent_directory_context_menu_policy() -> Vec<TransferContextMenuNode> {
    use TransferContextMenuAction as Action;
    use TransferContextMenuNode::{Action as Item, Separator};

    vec![Item(Action::GoUp), Separator, Item(Action::Refresh)]
}

/// Inputs a candidate session contributes when deciding whether it can appear in
/// the "Send to" submenu.
///
/// Kept UI-independent so the eligibility rule can be tested without a live app:
/// a target must be connected (not disconnected), expose a browser backend, and be a
/// session other than the source the selection lives in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SendToCandidate {
    pub session_id: String,
    pub has_browser_backend: bool,
    pub is_disconnected: bool,
}

pub(super) fn send_to_candidate_is_eligible(
    source_session_id: &str,
    candidate: &SendToCandidate,
) -> bool {
    candidate.session_id != source_session_id
        && candidate.has_browser_backend
        && !candidate.is_disconnected
}

/// Resolve the destination directory for a "Send to" transfer.
///
/// Prefers the target session's own cached browser directory; otherwise falls
/// back to that session's home (or `cwd`). The source session's path must never
/// leak in here, so both fallbacks are explicit target-session inputs.
pub(super) fn send_to_target_directory(
    cached_current_path: Option<&str>,
    target_home_or_cwd: Option<&str>,
) -> String {
    let normalize = |value: &str| -> Option<String> {
        let trimmed = value.trim();
        (!trimmed.is_empty() && trimmed != ".").then(|| trimmed.trim_end_matches('/').to_string())
    };
    cached_current_path
        .and_then(normalize)
        .or_else(|| target_home_or_cwd.and_then(normalize))
        .unwrap_or_else(|| ".".to_string())
}

/// Join a target directory and a source entry name into a destination path.
///
/// The entry name comes from the source listing; the directory is the resolved
/// target directory above. Root is preserved so `/` + `file` becomes `/file`.
#[cfg(test)]
pub(super) fn send_to_destination_path(target_dir: &str, entry_name: &str) -> String {
    let dir = target_dir.trim_end_matches('/');
    match dir {
        "" if target_dir.starts_with('/') => format!("/{entry_name}"),
        "" | "." => entry_name.to_string(),
        dir => format!("{dir}/{entry_name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SendToCandidate, TransferContextMenuAction as Action, TransferContextMenuNode as Node,
        TransferEntryMenuCapabilities, send_to_candidate_is_eligible, send_to_destination_path,
        send_to_target_directory, transfer_action_bar_enabled,
        transfer_context_action_visible_for_backend,
        transfer_current_directory_context_menu_policy,
        transfer_directory_terminal_actions_visible, transfer_entry_context_menu_policy,
        transfer_parent_directory_context_menu_policy, transfer_visible_context_menu_nodes,
    };

    #[test]
    fn action_bar_enables_selection_and_clipboard_commands_independently() {
        assert_eq!(transfer_action_bar_enabled(0, false), [false; 5]);
        assert_eq!(
            transfer_action_bar_enabled(0, true),
            [false, false, true, false, false]
        );
        assert_eq!(
            transfer_action_bar_enabled(1, false),
            [true, true, false, true, true]
        );
        assert_eq!(
            transfer_action_bar_enabled(3, true),
            [true, true, true, false, true]
        );
    }

    #[test]
    fn directory_terminal_actions_exclude_files_and_symlinks() {
        assert!(transfer_directory_terminal_actions_visible(true, false));
        assert!(!transfer_directory_terminal_actions_visible(true, true));
        assert!(!transfer_directory_terminal_actions_visible(false, false));
    }

    #[test]
    fn local_backend_hides_remote_only_actions() {
        use nyaterm_transport::FileBrowserBackendKind::{Local, Remote};

        for action in [Action::Upload, Action::Download, Action::NewSymlink] {
            assert!(!transfer_context_action_visible_for_backend(action, Local));
            assert!(transfer_context_action_visible_for_backend(action, Remote));
        }
        assert!(transfer_context_action_visible_for_backend(
            Action::Properties,
            Local
        ));
        assert!(transfer_context_action_visible_for_backend(
            Action::SendTo,
            Local
        ));
    }

    #[test]
    fn file_menu_groups_copy_and_terminal_actions() {
        assert_eq!(
            transfer_entry_context_menu_policy(TransferEntryMenuCapabilities {
                show_open_internal: true,
                has_ai_actions: true,
                terminal_available: true,
                ..Default::default()
            }),
            vec![
                Node::Action(Action::Open),
                Node::Action(Action::OpenInternal),
                Node::Separator,
                Node::Action(Action::Refresh),
                Node::Action(Action::Upload),
                Node::Action(Action::Download),
                Node::Separator,
                Node::Action(Action::Move),
                Node::Separator,
                Node::Action(Action::CopyInfo),
                Node::Separator,
                Node::Action(Action::Terminal),
                Node::Separator,
                Node::Action(Action::Ai),
                Node::Separator,
                Node::Action(Action::Properties),
            ]
        );
    }

    #[test]
    fn preview_action_follows_open_and_only_for_files() {
        let file = transfer_entry_context_menu_policy(TransferEntryMenuCapabilities {
            show_preview: true,
            ..Default::default()
        });
        assert_eq!(file[0], Node::Action(Action::Open));
        assert_eq!(file[1], Node::Action(Action::Preview));

        // A directory never offers preview, so the caller passes show_preview:false.
        let directory = transfer_entry_context_menu_policy(TransferEntryMenuCapabilities {
            is_directory: true,
            ..Default::default()
        });
        assert!(
            !directory.contains(&Node::Action(Action::Preview)),
            "directories must not show a preview action"
        );
    }

    #[test]
    fn directory_menu_inserts_favorite_group() {
        let items = transfer_entry_context_menu_policy(TransferEntryMenuCapabilities {
            is_directory: true,
            ..Default::default()
        });
        assert!(items.windows(3).any(|group| {
            group
                == [
                    Node::Action(Action::AddToFavorites),
                    Node::Separator,
                    Node::Action(Action::CopyInfo),
                ]
        }));
        assert_eq!(items.last(), Some(&Node::Action(Action::Properties)));
    }

    #[test]
    fn preview_action_shows_for_every_file_including_unrenderable_types() {
        // Parity: the preview action is offered for any non-directory file. A
        // type the preview cannot render (e.g. a `.zip`) still gets the action;
        // the window opens and shows the unsupported message. The capability is
        // driven by `show_transfer_preview_menu_entry`, which is `!is_directory`.
        let unrenderable = transfer_entry_context_menu_policy(TransferEntryMenuCapabilities {
            show_open_external: true,
            show_preview: true,
            ..Default::default()
        });
        assert_eq!(unrenderable[0], Node::Action(Action::Open));
        assert_eq!(unrenderable[1], Node::Action(Action::Preview));
    }

    #[test]
    fn current_and_parent_directory_menus_match_tauri_groups() {
        assert_eq!(
            transfer_current_directory_context_menu_policy(true),
            vec![
                Node::Action(Action::Refresh),
                Node::Action(Action::Upload),
                Node::Separator,
                Node::Action(Action::NewFile),
                Node::Action(Action::NewFolder),
                Node::Action(Action::NewSymlink),
                Node::Separator,
                Node::Action(Action::CopyDirectoryPath),
                Node::Action(Action::Terminal),
                Node::Separator,
                Node::Action(Action::Properties),
            ]
        );
        assert_eq!(
            transfer_parent_directory_context_menu_policy(),
            vec![
                Node::Action(Action::GoUp),
                Node::Separator,
                Node::Action(Action::Refresh),
            ]
        );
    }

    #[test]
    fn send_to_submenu_follows_download_with_its_own_separator() {
        let items = transfer_entry_context_menu_policy(TransferEntryMenuCapabilities {
            has_send_targets: true,
            ..Default::default()
        });
        let download = items
            .iter()
            .position(|node| node == &Node::Action(Action::Download))
            .expect("download action present");
        assert_eq!(items[download + 1], Node::Separator);
        assert_eq!(items[download + 2], Node::Action(Action::SendTo));
        assert_eq!(items[download + 3], Node::Separator);
        assert_eq!(items[download + 4], Node::Action(Action::Move));
    }

    #[test]
    fn send_to_submenu_absent_without_eligible_targets() {
        let items = transfer_entry_context_menu_policy(TransferEntryMenuCapabilities {
            ..Default::default()
        });
        assert!(!items.contains(&Node::Action(Action::SendTo)));
        let download = items
            .iter()
            .position(|node| node == &Node::Action(Action::Download))
            .expect("download action present");
        assert_eq!(items[download + 1], Node::Separator);
        assert_eq!(items[download + 2], Node::Action(Action::Move));
    }

    #[test]
    fn tree_entries_offer_creation_and_terminal_requires_availability() {
        let items = transfer_entry_context_menu_policy(TransferEntryMenuCapabilities {
            is_tree_view: true,
            ..Default::default()
        });
        assert!(items.contains(&Node::Action(Action::NewFile)));
        assert!(items.contains(&Node::Action(Action::NewFolder)));
        assert!(items.contains(&Node::Action(Action::NewSymlink)));
        assert!(!items.contains(&Node::Action(Action::Terminal)));
        assert!(
            !transfer_current_directory_context_menu_policy(false)
                .contains(&Node::Action(Action::Terminal))
        );
    }

    #[test]
    fn local_menu_removes_remote_actions_without_empty_groups() {
        use nyaterm_transport::FileBrowserBackendKind::Local;

        let nodes = transfer_visible_context_menu_nodes(
            transfer_entry_context_menu_policy(TransferEntryMenuCapabilities {
                is_tree_view: true,
                ..Default::default()
            }),
            Local,
        );
        assert!(!nodes.contains(&Node::Action(Action::Upload)));
        assert!(!nodes.contains(&Node::Action(Action::Download)));
        assert!(!nodes.contains(&Node::Action(Action::NewSymlink)));
        assert!(
            !nodes
                .windows(2)
                .any(|pair| pair == [Node::Separator, Node::Separator])
        );
        assert_ne!(nodes.last(), Some(&Node::Separator));
    }

    #[test]
    fn send_to_candidate_excludes_source_and_unbrowsable_sessions() {
        let source = "session-source";
        assert!(!send_to_candidate_is_eligible(
            source,
            &SendToCandidate {
                session_id: source.to_string(),
                has_browser_backend: true,
                is_disconnected: false,
            }
        ));
        assert!(!send_to_candidate_is_eligible(
            source,
            &SendToCandidate {
                session_id: "session-b".to_string(),
                has_browser_backend: false,
                is_disconnected: false,
            }
        ));
        assert!(!send_to_candidate_is_eligible(
            source,
            &SendToCandidate {
                session_id: "session-c".to_string(),
                has_browser_backend: true,
                is_disconnected: true,
            }
        ));
        assert!(send_to_candidate_is_eligible(
            source,
            &SendToCandidate {
                session_id: "session-d".to_string(),
                has_browser_backend: true,
                is_disconnected: false,
            }
        ));
    }

    #[test]
    fn send_to_target_directory_prefers_cache_then_home_never_source() {
        // Cached browser path wins.
        assert_eq!(
            send_to_target_directory(Some("/srv/data/"), Some("/home/bob")),
            "/srv/data"
        );
        // No usable cache falls back to the target session's home/cwd.
        assert_eq!(
            send_to_target_directory(None, Some("/home/bob")),
            "/home/bob"
        );
        assert_eq!(
            send_to_target_directory(Some("."), Some("/home/bob")),
            "/home/bob"
        );
        // Nothing usable at all degrades to the safe relative default, and it is
        // never seeded from a source path (the caller passes only target inputs).
        assert_eq!(send_to_target_directory(Some("   "), None), ".");
        assert_eq!(send_to_target_directory(None, None), ".");
    }

    #[test]
    fn send_to_destination_path_joins_directory_and_entry_name() {
        assert_eq!(
            send_to_destination_path("/srv/data", "file.txt"),
            "/srv/data/file.txt"
        );
        assert_eq!(
            send_to_destination_path("/srv/data/", "file.txt"),
            "/srv/data/file.txt"
        );
        assert_eq!(send_to_destination_path("/", "file.txt"), "/file.txt");
        assert_eq!(send_to_destination_path(".", "file.txt"), "file.txt");
    }
}
