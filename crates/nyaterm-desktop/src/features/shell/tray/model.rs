use nyaterm_core::packaging;
use nyaterm_transport::SessionKind;
use rust_i18n::t;

const SESSION_LIMIT: usize = 8;
const ACTION_PREFIX: &str = "nyaterm::tray::";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrayAction {
    Show,
    Hide,
    NewWindow,
    NewConnection,
    ActiveSessions,
    SyncPush,
    SyncPull,
    SyncHistory,
    Settings,
    MinimizeMode,
    Lock,
    Update,
    Quit,
    FocusSession(String),
}

impl TrayAction {
    pub(crate) fn menu_id(&self) -> String {
        let suffix = match self {
            Self::Show => "show",
            Self::Hide => "hide",
            Self::NewWindow => "new-window",
            Self::NewConnection => "new",
            Self::ActiveSessions => "active-sessions",
            Self::SyncPush => "push",
            Self::SyncPull => "pull",
            Self::SyncHistory => "sync-history",
            Self::Settings => "settings",
            Self::MinimizeMode => "minimize-mode",
            Self::Lock => "lock",
            Self::Update => "update",
            Self::Quit => "quit",
            Self::FocusSession(id) => return format!("{ACTION_PREFIX}session:{id}"),
        };
        format!("{ACTION_PREFIX}{suffix}")
    }

    pub(crate) fn from_menu_id(id: &str) -> Option<Self> {
        Some(match id.strip_prefix(ACTION_PREFIX)? {
            "show" => Self::Show,
            "hide" => Self::Hide,
            "new-window" => Self::NewWindow,
            "new" => Self::NewConnection,
            "active-sessions" => Self::ActiveSessions,
            "push" => Self::SyncPush,
            "pull" => Self::SyncPull,
            "sync-history" => Self::SyncHistory,
            "settings" => Self::Settings,
            "minimize-mode" => Self::MinimizeMode,
            "lock" => Self::Lock,
            "update" => Self::Update,
            "quit" => Self::Quit,
            suffix => Self::FocusSession(suffix.strip_prefix("session:")?.to_owned()),
        })
    }

    pub(crate) fn requires_unlock(&self) -> bool {
        !matches!(self, Self::Show | Self::Hide | Self::Lock | Self::Quit)
    }

    pub(crate) fn shows_window(&self) -> bool {
        !matches!(self, Self::Hide | Self::MinimizeMode | Self::Lock)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TraySyncState {
    Disabled,
    Idle,
    Running,
    Success,
    Failed,
    Conflict,
}

impl TraySyncState {
    pub(crate) fn aggregate(
        enabled: bool,
        auto_running: bool,
        states: impl IntoIterator<Item = Self>,
        target: Self,
    ) -> Self {
        if !enabled {
            return Self::Disabled;
        }
        let states: Vec<_> = states.into_iter().collect();
        if auto_running || states.contains(&Self::Running) {
            Self::Running
        } else if states.contains(&Self::Conflict) {
            Self::Conflict
        } else {
            target
        }
    }

    fn label(self) -> String {
        match self {
            Self::Disabled => t!("settings.syncState.disabled"),
            Self::Idle => t!("settings.syncState.idle"),
            Self::Running => t!("settings.syncState.running"),
            Self::Success => t!("settings.syncState.success"),
            Self::Failed => t!("settings.syncState.failed"),
            Self::Conflict => t!("settings.syncState.conflict"),
        }
        .to_string()
    }
}

#[derive(Clone)]
pub(crate) struct TraySession {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) kind: SessionKind,
}

pub(crate) struct TrayWindowSnapshot {
    pub(crate) sessions: Vec<TraySession>,
    pub(crate) sync_state: TraySyncState,
    pub(crate) sync_enabled: bool,
    pub(crate) minimize_to_tray: bool,
    pub(crate) lock_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TrayEntry {
    Action {
        action: TrayAction,
        label: String,
        enabled: bool,
    },
    Check {
        action: TrayAction,
        label: String,
        enabled: bool,
        checked: bool,
    },
    Info {
        id: &'static str,
        label: String,
    },
    Separator,
    Submenu {
        id: &'static str,
        label: String,
        entries: Vec<TrayEntry>,
    },
}

impl TrayEntry {
    fn action(action: TrayAction, label: String, locked: bool) -> Self {
        let enabled = !locked || !action.requires_unlock();
        Self::Action {
            action,
            label,
            enabled,
        }
    }

    fn action_enabled(&self, query: &TrayAction) -> bool {
        match self {
            Self::Action {
                action, enabled, ..
            }
            | Self::Check {
                action, enabled, ..
            } => action == query && *enabled,
            Self::Submenu { entries, .. } => {
                entries.iter().any(|entry| entry.action_enabled(query))
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TraySnapshot {
    pub(super) entries: Vec<TrayEntry>,
}

impl TraySnapshot {
    pub(crate) fn empty() -> Self {
        Self {
            entries: vec![
                TrayEntry::action(TrayAction::Show, t!("tray.show").to_string(), false),
                TrayEntry::action(
                    TrayAction::NewWindow,
                    t!("tray.newWindow").to_string(),
                    false,
                ),
                TrayEntry::Separator,
                TrayEntry::action(TrayAction::Quit, t!("tray.quit").to_string(), false),
            ],
        }
    }

    pub(crate) fn new(
        sessions: Vec<TraySession>,
        sync: TraySyncState,
        minimize_to_tray: bool,
        lock_available: bool,
        locked: bool,
    ) -> Self {
        Self::new_with_update_policy(
            sessions,
            sync,
            minimize_to_tray,
            lock_available,
            locked,
            packaging::updates_managed_externally(),
        )
    }

    /// Builds the snapshot for an explicit update policy so tests can assert both
    /// the ordinary and the externally managed menu.
    pub(crate) fn new_with_update_policy(
        mut sessions: Vec<TraySession>,
        sync: TraySyncState,
        minimize_to_tray: bool,
        lock_available: bool,
        locked: bool,
        updates_managed_externally: bool,
    ) -> Self {
        sessions.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| protocol_key(left.kind).cmp(protocol_key(right.kind)))
                .then_with(|| left.id.cmp(&right.id))
        });
        let count = sessions.len();
        let mut session_entries: Vec<_> = sessions
            .into_iter()
            .take(SESSION_LIMIT)
            .map(|session| {
                let protocol = match session.kind {
                    SessionKind::LocalPty => t!("tray.local").to_string(),
                    kind => protocol_key(kind).to_uppercase(),
                };
                TrayEntry::action(
                    TrayAction::FocusSession(session.id),
                    format!("{} · {protocol}", session.name),
                    locked,
                )
            })
            .collect();
        if count == 0 {
            session_entries.push(TrayEntry::Info {
                id: "empty-sessions",
                label: t!("tray.noActiveSessions").to_string(),
            });
        } else if count > SESSION_LIMIT {
            session_entries.push(TrayEntry::Info {
                id: "limited-sessions",
                label: t!("tray.sessionLimit").to_string(),
            });
        }
        session_entries.push(TrayEntry::Separator);
        session_entries.push(TrayEntry::action(
            TrayAction::ActiveSessions,
            t!("tray.openActiveSessions").to_string(),
            locked,
        ));

        let status = sync.label();
        let mut sync_entries = vec![TrayEntry::Info {
            id: "sync-status",
            label: t!("tray.currentStatus", status = status).to_string(),
        }];
        if matches!(
            sync,
            TraySyncState::Idle
                | TraySyncState::Success
                | TraySyncState::Failed
                | TraySyncState::Running
        ) {
            sync_entries.push(TrayEntry::Separator);
            for (action, label) in [
                (TrayAction::SyncPush, t!("tray.push")),
                (TrayAction::SyncPull, t!("tray.pull")),
            ] {
                sync_entries.push(TrayEntry::Action {
                    action,
                    label: label.to_string(),
                    enabled: !locked && sync != TraySyncState::Running,
                });
            }
        }
        sync_entries.push(TrayEntry::Separator);
        sync_entries.push(TrayEntry::action(
            TrayAction::SyncHistory,
            t!("tray.openSyncHistory").to_string(),
            locked,
        ));

        let mut entries = vec![
            TrayEntry::action(TrayAction::Show, t!("tray.show").to_string(), locked),
            TrayEntry::action(TrayAction::Hide, t!("tray.hide").to_string(), locked),
            TrayEntry::Separator,
            TrayEntry::action(
                TrayAction::NewWindow,
                t!("tray.newWindow").to_string(),
                locked,
            ),
            TrayEntry::action(
                TrayAction::NewConnection,
                t!("tray.newConnection").to_string(),
                locked,
            ),
            TrayEntry::Submenu {
                id: "sessions",
                label: t!("tray.activeSessions", count = count).to_string(),
                entries: session_entries,
            },
            TrayEntry::Submenu {
                id: "sync",
                label: t!("tray.cloudSync", status = status).to_string(),
                entries: sync_entries,
            },
            TrayEntry::Separator,
            TrayEntry::action(
                TrayAction::Settings,
                t!("tray.settings").to_string(),
                locked,
            ),
            TrayEntry::Check {
                action: TrayAction::MinimizeMode,
                label: t!("tray.minimizeToTray").to_string(),
                enabled: !locked,
                checked: minimize_to_tray,
            },
        ];
        if lock_available {
            entries.push(TrayEntry::action(
                TrayAction::Lock,
                t!("tray.lock").to_string(),
                locked,
            ));
        }
        // An externally managed build has no in-app update action to offer.
        if updates_managed_externally {
            entries.extend([
                TrayEntry::Separator,
                TrayEntry::action(TrayAction::Quit, t!("tray.quit").to_string(), locked),
            ]);
        } else {
            entries.extend([
                TrayEntry::action(TrayAction::Update, t!("tray.update").to_string(), locked),
                TrayEntry::Separator,
                TrayEntry::action(TrayAction::Quit, t!("tray.quit").to_string(), locked),
            ]);
        }
        Self { entries }
    }

    pub(crate) fn action_enabled(&self, action: &TrayAction) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.action_enabled(action))
    }
}

fn protocol_key(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::LocalPty => "local",
        SessionKind::Ssh => "ssh",
        SessionKind::Telnet => "telnet",
        SessionKind::RawTcp => "raw tcp",
        SessionKind::Serial => "serial",
        SessionKind::Rdp => "rdp",
        SessionKind::Vnc => "vnc",
    }
}

pub(super) fn escape_menu_text(text: &str) -> String {
    text.replace('&', "&&")
}

#[cfg(test)]
mod tests {
    use nyaterm_transport::SessionKind;

    use super::{
        TrayAction, TrayEntry, TraySession, TraySnapshot, TraySyncState, escape_menu_text,
    };

    fn sessions(count: usize) -> Vec<TraySession> {
        (0..count)
            .rev()
            .map(|index| TraySession {
                id: index.to_string(),
                name: format!("Server {index}"),
                kind: SessionKind::Ssh,
            })
            .collect()
    }

    fn submenu(snapshot: &TraySnapshot, id: &str) -> Vec<TrayEntry> {
        snapshot
            .entries
            .iter()
            .find_map(|entry| match entry {
                TrayEntry::Submenu {
                    id: candidate,
                    entries,
                    ..
                } if *candidate == id => Some(entries.clone()),
                _ => None,
            })
            .expect("submenu")
    }

    #[test]
    fn session_menu_reports_full_count_and_limits_only_shortcuts() {
        for count in [0, 8, 9] {
            let snapshot =
                TraySnapshot::new(sessions(count), TraySyncState::Idle, false, false, false);
            let entries = submenu(&snapshot, "sessions");
            let shortcuts = entries
                .iter()
                .filter(|entry| {
                    matches!(
                        entry,
                        TrayEntry::Action {
                            action: TrayAction::FocusSession(_),
                            ..
                        }
                    )
                })
                .count();
            assert_eq!(shortcuts, count.min(8));
            assert_eq!(
                entries.iter().any(|entry| matches!(
                    entry,
                    TrayEntry::Info {
                        id: "empty-sessions",
                        ..
                    }
                )),
                count == 0
            );
            assert_eq!(
                entries.iter().any(|entry| matches!(
                    entry,
                    TrayEntry::Info {
                        id: "limited-sessions",
                        ..
                    }
                )),
                count > 8
            );
            assert!(snapshot.action_enabled(&TrayAction::ActiveSessions));
            assert!(snapshot.entries.iter().any(|entry| matches!(entry, TrayEntry::Submenu { id: "sessions", label, .. } if label.contains(&format!("({count})")))));
        }
    }

    #[test]
    fn session_menu_sorts_names_then_protocol_then_id_and_preserves_special_text() {
        let input = vec![
            TraySession {
                id: "b".into(),
                name: "ALPHA & Ops".into(),
                kind: SessionKind::Ssh,
            },
            TraySession {
                id: "a".into(),
                name: "alpha & ops".into(),
                kind: SessionKind::Ssh,
            },
            TraySession {
                id: "c".into(),
                name: "Alpha & ops".into(),
                kind: SessionKind::Serial,
            },
            TraySession {
                id: "d".into(),
                name: "Beta".into(),
                kind: SessionKind::Vnc,
            },
        ];
        let snapshot = TraySnapshot::new(input, TraySyncState::Idle, false, false, false);
        let entries = submenu(&snapshot, "sessions");
        let ids: Vec<_> = entries
            .iter()
            .filter_map(|entry| match entry {
                TrayEntry::Action {
                    action: TrayAction::FocusSession(id),
                    ..
                } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, ["c", "a", "b", "d"]);
        assert!(
            matches!(&entries[0], TrayEntry::Action { label, .. } if label == "Alpha & ops · SERIAL")
        );
        assert_eq!(escape_menu_text("Alpha & ops"), "Alpha && ops");
    }

    #[test]
    fn sync_menu_gates_manual_operations_for_all_six_states() {
        for state in [
            TraySyncState::Disabled,
            TraySyncState::Idle,
            TraySyncState::Running,
            TraySyncState::Success,
            TraySyncState::Failed,
            TraySyncState::Conflict,
        ] {
            let snapshot = TraySnapshot::new(vec![], state, false, false, false);
            let entries = submenu(&snapshot, "sync");
            let visible = !matches!(state, TraySyncState::Disabled | TraySyncState::Conflict);
            let enabled = matches!(
                state,
                TraySyncState::Idle | TraySyncState::Success | TraySyncState::Failed
            );
            for action in [TrayAction::SyncPush, TrayAction::SyncPull] {
                assert_eq!(entries.iter().any(|entry| matches!(entry, TrayEntry::Action { action: candidate, .. } if *candidate == action)), visible);
                assert_eq!(snapshot.action_enabled(&action), enabled);
            }
            assert!(matches!(
                &entries[0],
                TrayEntry::Info {
                    id: "sync-status",
                    ..
                }
            ));
            assert!(snapshot.action_enabled(&TrayAction::SyncHistory));
        }
    }

    #[test]
    fn sync_aggregation_prioritizes_saved_disable_then_any_running_then_conflict() {
        let states = [TraySyncState::Conflict, TraySyncState::Running];
        assert_eq!(
            TraySyncState::aggregate(false, true, states, TraySyncState::Success),
            TraySyncState::Disabled
        );
        assert_eq!(
            TraySyncState::aggregate(true, false, states, TraySyncState::Success),
            TraySyncState::Running
        );
        assert_eq!(
            TraySyncState::aggregate(true, true, [], TraySyncState::Idle),
            TraySyncState::Running
        );
        assert_eq!(
            TraySyncState::aggregate(
                true,
                false,
                [TraySyncState::Conflict],
                TraySyncState::Success
            ),
            TraySyncState::Conflict
        );
        assert_eq!(
            TraySyncState::aggregate(true, false, [TraySyncState::Failed], TraySyncState::Success),
            TraySyncState::Success
        );
    }

    #[test]
    fn root_menu_groups_actions_and_uses_a_conditional_lock_and_typed_check() {
        for updates_managed_externally in [false, true] {
            let snapshot = TraySnapshot::new_with_update_policy(
                vec![],
                TraySyncState::Idle,
                true,
                true,
                false,
                updates_managed_externally,
            );
            let structure: Vec<_> = snapshot
                .entries
                .iter()
                .map(|entry| match entry {
                    TrayEntry::Action { action, .. } | TrayEntry::Check { action, .. } => {
                        action.menu_id()
                    }
                    TrayEntry::Separator => "separator".into(),
                    TrayEntry::Submenu { id, .. } => (*id).into(),
                    TrayEntry::Info { .. } => "info".into(),
                })
                .collect();
            let mut expected = [
                "nyaterm::tray::show",
                "nyaterm::tray::hide",
                "separator",
                "nyaterm::tray::new-window",
                "nyaterm::tray::new",
                "sessions",
                "sync",
                "separator",
                "nyaterm::tray::settings",
                "nyaterm::tray::minimize-mode",
                "nyaterm::tray::lock",
                "nyaterm::tray::update",
                "separator",
                "nyaterm::tray::quit",
            ]
            .map(str::to_string)
            .to_vec();
            if updates_managed_externally {
                expected.retain(|id| id != "nyaterm::tray::update");
            }
            assert_eq!(structure, expected);
            assert!(matches!(
                &snapshot.entries[9],
                TrayEntry::Check { checked: true, .. }
            ));
        }
        let snapshot = TraySnapshot::new(vec![], TraySyncState::Idle, false, false, false);
        assert!(!snapshot.action_enabled(&TrayAction::Lock));
        assert!(matches!(
            &snapshot.entries[9],
            TrayEntry::Check { checked: false, .. }
        ));
    }

    #[test]
    fn locked_menu_disables_feature_actions_but_keeps_window_and_quit_controls() {
        let snapshot = TraySnapshot::new(sessions(1), TraySyncState::Idle, false, true, true);
        for action in [
            TrayAction::NewWindow,
            TrayAction::NewConnection,
            TrayAction::Settings,
            TrayAction::MinimizeMode,
            TrayAction::SyncPush,
            TrayAction::SyncPull,
            TrayAction::ActiveSessions,
            TrayAction::SyncHistory,
            TrayAction::FocusSession("0".into()),
            TrayAction::Update,
        ] {
            assert!(!snapshot.action_enabled(&action));
            assert!(action.requires_unlock());
        }
        for action in [
            TrayAction::Show,
            TrayAction::Hide,
            TrayAction::Quit,
            TrayAction::Lock,
        ] {
            assert!(snapshot.action_enabled(&action));
            assert!(!action.requires_unlock());
        }
        for action in [TrayAction::Hide, TrayAction::MinimizeMode, TrayAction::Lock] {
            assert!(!action.shows_window());
        }
    }

    #[test]
    fn menu_ids_round_trip_without_interpreting_session_text_as_actions() {
        let action = TrayAction::FocusSession("settings:&:会话".into());
        assert_eq!(TrayAction::from_menu_id(&action.menu_id()), Some(action));
        assert_eq!(TrayAction::from_menu_id("settings"), None);
        assert_eq!(
            TrayAction::from_menu_id("nyaterm::tray::info::sync-status"),
            None
        );
    }
}
