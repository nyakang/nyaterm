use super::pane::{SmartSplitMode, WorkspacePaneNode, WorkspaceSplitDirection};
use super::tabs::{TabDockEdge, TabDockZone, TerminalWindowNode};

#[cfg(test)]
mod workspace_pane_tests {
    use super::{WorkspacePaneNode, WorkspaceSplitDirection};
    use crate::RestorableWorkspacePaneNode;

    #[test]
    fn workspace_pane_serialize_restore_roundtrip() {
        let ordered = vec!["a".into(), "b".into(), "c".into()];
        let mut root = WorkspacePaneNode::leaf("a");
        assert!(root.split_leaf(
            "a",
            "b".into(),
            WorkspaceSplitDirection::Vertical,
            "s1".into(),
        ));
        assert!(root.split_leaf(
            "b",
            "c".into(),
            WorkspaceSplitDirection::Horizontal,
            "s2".into(),
        ));
        let layout = root.serialize_layout(&ordered).expect("layout");
        match &layout {
            RestorableWorkspacePaneNode::Split { .. } => {}
            _ => panic!("expected split"),
        }
        let restored = WorkspacePaneNode::restore_layout(&layout, &ordered).expect("restore");
        assert!(restored.is_split());
        let mut ids = restored.session_ids();
        ids.sort();
        let mut expected = ordered.clone();
        expected.sort();
        assert_eq!(ids, expected);
    }

    #[test]
    fn replacing_session_preserves_workspace_pane_tree() {
        let mut root = WorkspacePaneNode::leaf("a");
        assert!(root.split_leaf(
            "a",
            "b".into(),
            WorkspaceSplitDirection::Vertical,
            "s1".into(),
        ));
        assert!(root.replace_session_id("a", "reconnected-a"));
        assert_eq!(root.session_ids(), vec!["reconnected-a", "b"]);
        assert!(root.is_split());
    }
}

#[cfg(test)]
mod terminal_window_tests {
    use super::{
        SmartSplitMode, TabDockEdge, TabDockZone, TerminalWindowNode, WorkspaceSplitDirection,
    };

    fn split_tab_to_edge(
        root: &mut TerminalWindowNode,
        tab_id: &str,
        direction: WorkspaceSplitDirection,
        before: bool,
    ) -> bool {
        let Some(target_leaf_id) = root.first_leaf_id() else {
            return false;
        };
        let edge = match (direction, before) {
            (WorkspaceSplitDirection::Vertical, true) => TabDockEdge::Left,
            (WorkspaceSplitDirection::Vertical, false) => TabDockEdge::Right,
            (WorkspaceSplitDirection::Horizontal, true) => TabDockEdge::Top,
            (WorkspaceSplitDirection::Horizontal, false) => TabDockEdge::Bottom,
        };
        root.dock_tab(tab_id, &target_leaf_id, TabDockZone::Edge(edge))
    }

    #[test]
    fn split_tab_creates_two_leaves() {
        let mut root =
            TerminalWindowNode::leaf(vec!["a".into(), "b".into(), "c".into()], Some("a".into()));
        assert!(split_tab_to_edge(
            &mut root,
            "b",
            WorkspaceSplitDirection::Vertical,
            false,
        ));
        assert!(matches!(root, TerminalWindowNode::Split { .. }));
        let tabs = root.collect_tab_ids();
        assert_eq!(tabs.len(), 3);
        assert!(root.contains_tab("b"));
        assert!(root.set_active_tab("b"));
    }

    #[test]
    fn remove_tab_collapses_empty_leaf() {
        let mut root = TerminalWindowNode::leaf(vec!["a".into(), "b".into()], Some("a".into()));
        assert!(split_tab_to_edge(
            &mut root,
            "b",
            WorkspaceSplitDirection::Horizontal,
            true,
        ));
        let next = root.remove_tab("b").expect("remaining leaf");
        assert!(matches!(next, TerminalWindowNode::Leaf { .. }));
        assert_eq!(next.collect_tab_ids(), vec!["a".to_string()]);
    }

    #[test]
    fn replacing_tab_preserves_terminal_window_leaf() {
        let mut root = TerminalWindowNode::leaf(vec!["a".into(), "b".into()], Some("a".into()));
        assert!(root.replace_tab_id("a", "reconnected-a"));
        assert_eq!(
            root.collect_tab_ids(),
            vec!["reconnected-a".to_string(), "b".to_string()]
        );
        assert_eq!(root.active_tabs(), vec!["reconnected-a".to_string()]);
    }

    #[test]
    fn move_tab_to_other_leaf() {
        let mut root = TerminalWindowNode::leaf(vec!["a".into(), "b".into()], Some("a".into()));
        assert!(split_tab_to_edge(
            &mut root,
            "b",
            WorkspaceSplitDirection::Vertical,
            false,
        ));
        let leaves = root.leaf_ids();
        assert_eq!(leaves.len(), 2);
        // move a into b's leaf
        let target = leaves
            .into_iter()
            .find(|id| {
                // leaf containing b
                match &root {
                    TerminalWindowNode::Split { first, second, .. } => {
                        matches!(first.as_ref(), TerminalWindowNode::Leaf { id: lid, tab_ids, .. } if lid == id && tab_ids.iter().any(|t| t == "b"))
                            || matches!(second.as_ref(), TerminalWindowNode::Leaf { id: lid, tab_ids, .. } if lid == id && tab_ids.iter().any(|t| t == "b"))
                    }
                    _ => false,
                }
            })
            .expect("target leaf");
        assert!(root.move_tab_to_leaf("a", &target));
        // after move, single leaf should remain if other empty collapsed via remove_tab path
        let tabs = root.collect_tab_ids();
        assert!(tabs.contains(&"a".to_string()) && tabs.contains(&"b".to_string()));
    }

    #[test]
    fn dock_tab_to_edge_splits_target() {
        let mut root = TerminalWindowNode::leaf(vec!["a".into(), "b".into()], Some("a".into()));
        // Create two leaves first via split
        assert!(split_tab_to_edge(
            &mut root,
            "b",
            WorkspaceSplitDirection::Vertical,
            false,
        ));
        let leaves = root.leaf_ids();
        assert_eq!(leaves.len(), 2);
        let a_leaf = leaves
            .iter()
            .find(|id| match &root {
                TerminalWindowNode::Split { first, second, .. } => {
                    matches!(first.as_ref(), TerminalWindowNode::Leaf { id: lid, tab_ids, .. } if lid == *id && tab_ids.iter().any(|t| t == "a"))
                        || matches!(second.as_ref(), TerminalWindowNode::Leaf { id: lid, tab_ids, .. } if lid == *id && tab_ids.iter().any(|t| t == "a"))
                }
                _ => false,
            })
            .cloned()
            .expect("a leaf");
        // Dock b onto a's left edge
        assert!(root.dock_tab("b", &a_leaf, TabDockZone::Edge(TabDockEdge::Left)));
        assert_eq!(root.leaf_ids().len(), 2);
        assert!(root.contains_tab("a") && root.contains_tab("b"));
    }

    #[test]
    fn smart_split_auto_tiles_four_tabs() {
        let tabs = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        let root = TerminalWindowNode::build_smart_split_layout(&tabs, SmartSplitMode::Auto)
            .expect("layout");
        assert!(matches!(root, TerminalWindowNode::Split { .. }));
        let mut ids = root.collect_tab_ids();
        ids.sort();
        let mut expected = tabs.clone();
        expected.sort();
        assert_eq!(ids, expected);
        // Each leaf should hold a single tab in smart split.
        fn max_leaf_tabs(node: &TerminalWindowNode) -> usize {
            match node {
                TerminalWindowNode::Leaf { tab_ids, .. } => tab_ids.len(),
                TerminalWindowNode::Split { first, second, .. } => {
                    max_leaf_tabs(first).max(max_leaf_tabs(second))
                }
            }
        }
        assert_eq!(max_leaf_tabs(&root), 1);
    }

    #[test]
    fn smart_split_horizontal_keeps_direction() {
        let tabs = vec!["a".into(), "b".into(), "c".into()];
        let root = TerminalWindowNode::build_smart_split_layout(&tabs, SmartSplitMode::Horizontal)
            .expect("layout");
        match root {
            TerminalWindowNode::Split { direction, .. } => {
                assert_eq!(direction, WorkspaceSplitDirection::Horizontal);
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn serialize_restore_layout_roundtrip() {
        use crate::RestorableTerminalWindowNode;
        let ordered = vec!["a".into(), "b".into(), "c".into()];
        let mut root = TerminalWindowNode::leaf(ordered.clone(), Some("a".into()));
        assert!(split_tab_to_edge(
            &mut root,
            "b",
            WorkspaceSplitDirection::Vertical,
            false,
        ));
        let layout = root.serialize_layout(&ordered).expect("layout");
        match &layout {
            RestorableTerminalWindowNode::Split { .. } => {}
            _ => panic!("expected split layout"),
        }
        let restored = TerminalWindowNode::restore_layout(&layout, &ordered).expect("restore");
        assert!(matches!(restored, TerminalWindowNode::Split { .. }));
        assert_eq!(
            {
                let mut ids = restored.collect_tab_ids();
                ids.sort();
                ids
            },
            {
                let mut ids = ordered.clone();
                ids.sort();
                ids
            }
        );
    }

    #[test]
    fn place_tab_before_reorders_and_moves() {
        let mut root =
            TerminalWindowNode::leaf(vec!["a".into(), "b".into(), "c".into()], Some("a".into()));
        assert!(root.place_tab_before("c", "a"));
        assert_eq!(
            root.collect_tab_ids(),
            vec!["c".to_string(), "a".to_string(), "b".to_string()]
        );
        assert!(split_tab_to_edge(
            &mut root,
            "b",
            WorkspaceSplitDirection::Vertical,
            false,
        ));
        // Move c next to b (other leaf)
        assert!(root.place_tab_before("c", "b"));
        assert!(root.contains_tab("c") && root.contains_tab("b"));
        let ids = root.leaf_tab_ids_for_tab("b").expect("b leaf");
        assert!(ids.iter().any(|id| id == "b") && ids.iter().any(|id| id == "c"));
        let a_pos = ids.iter().position(|id| id == "c");
        let b_pos = ids.iter().position(|id| id == "b");
        assert!(a_pos.is_some() && b_pos.is_some() && a_pos.unwrap() < b_pos.unwrap());
    }

    #[test]
    fn dock_zone_detects_edges() {
        use super::{TabDockEdge, TabDockZone};
        assert_eq!(
            TabDockZone::detect(10.0, 100.0, 400.0, 300.0),
            TabDockZone::Edge(TabDockEdge::Left)
        );
        assert_eq!(
            TabDockZone::detect(200.0, 150.0, 400.0, 300.0),
            TabDockZone::Center
        );
        assert_eq!(
            TabDockZone::detect(390.0, 100.0, 400.0, 300.0),
            TabDockZone::Edge(TabDockEdge::Right)
        );
        assert_eq!(
            TabDockZone::detect(200.0, 5.0, 400.0, 300.0),
            TabDockZone::Edge(TabDockEdge::Top)
        );
    }
}
