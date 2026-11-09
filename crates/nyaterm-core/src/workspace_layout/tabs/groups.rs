use super::{SmartSplitMode, TerminalWindowNode, WorkspacePaneNode, WorkspaceSplitDirection};
use crate::workspace_layout::pane::uuid_v4_like;

pub const GROUP_MIN_WIDTH: f32 = 240.;
pub const GROUP_MIN_HEIGHT: f32 = 160.;
pub const GROUP_DIVIDER_SIZE: f32 = 4.;

impl TerminalWindowNode {
    pub fn node_ids(&self) -> Vec<String> {
        match self {
            Self::Leaf { id, .. } => vec![id.clone()],
            Self::Split {
                id, first, second, ..
            } => {
                let mut ids = vec![id.clone()];
                ids.extend(first.node_ids());
                ids.extend(second.node_ids());
                ids
            }
        }
    }
    pub fn minimum_size(&self) -> (f32, f32) {
        match self {
            Self::Leaf { .. } => (GROUP_MIN_WIDTH, GROUP_MIN_HEIGHT),
            Self::Split {
                direction,
                first,
                second,
                ..
            } => {
                let (w1, h1) = first.minimum_size();
                let (w2, h2) = second.minimum_size();
                match direction {
                    WorkspaceSplitDirection::Horizontal => {
                        (w1.max(w2), h1 + h2 + GROUP_DIVIDER_SIZE)
                    }
                    WorkspaceSplitDirection::Vertical => (w1 + w2 + GROUP_DIVIDER_SIZE, h1.max(h2)),
                }
            }
        }
    }
    pub fn split_minimum_extents(&self, split: &str) -> Option<(f32, f32)> {
        match self {
            Self::Leaf { .. } => None,
            Self::Split {
                id,
                direction,
                first,
                second,
                ..
            } if id == split => {
                let a = first.minimum_size();
                let b = second.minimum_size();
                Some(match direction {
                    WorkspaceSplitDirection::Horizontal => (a.1, b.1),
                    WorkspaceSplitDirection::Vertical => (a.0, b.0),
                })
            }
            Self::Split { first, second, .. } => first
                .split_minimum_extents(split)
                .or_else(|| second.split_minimum_extents(split)),
        }
    }

    pub fn leaf_for_tab(&self, tab: &str) -> Option<&str> {
        match self {
            Self::Leaf { id, tab_ids, .. } => {
                tab_ids.iter().any(|id| id == tab).then_some(id.as_str())
            }
            Self::Split { first, second, .. } => {
                first.leaf_for_tab(tab).or_else(|| second.leaf_for_tab(tab))
            }
        }
    }

    pub fn leaf_tabs(&self, leaf: &str) -> Option<(&[String], Option<&str>)> {
        match self {
            Self::Leaf {
                id,
                tab_ids,
                active_tab_id,
            } => (id == leaf).then_some((tab_ids, active_tab_id.as_deref())),
            Self::Split { first, second, .. } => {
                first.leaf_tabs(leaf).or_else(|| second.leaf_tabs(leaf))
            }
        }
    }

    /// Expand one compatibility tab at its original position. Other tabs in the
    /// group stay in the first resulting group, in their existing order.
    pub fn expand_pane_tab(&mut self, tab: &str, panes: &WorkspacePaneNode) -> bool {
        match self {
            Self::Leaf {
                id,
                tab_ids,
                active_tab_id,
            } if tab_ids.iter().any(|id| id == tab) => {
                let mut expanded = Self::from_panes(panes);
                if let Some(Self::Leaf {
                    id: first_id,
                    tab_ids: first_tabs,
                    active_tab_id: first_active,
                }) = expanded.first_leaf_mut()
                {
                    *first_id = id.clone();
                    let first_session = first_tabs[0].clone();
                    *first_tabs = tab_ids
                        .iter()
                        .map(|id| {
                            if id == tab {
                                first_session.clone()
                            } else {
                                id.clone()
                            }
                        })
                        .collect();
                    *first_active = active_tab_id.as_ref().map(|id| {
                        if id == tab {
                            first_session.clone()
                        } else {
                            id.clone()
                        }
                    });
                }
                *self = expanded;
                true
            }
            Self::Leaf { .. } => false,
            Self::Split { first, second, .. } => {
                first.expand_pane_tab(tab, panes) || second.expand_pane_tab(tab, panes)
            }
        }
    }

    fn first_leaf_mut(&mut self) -> Option<&mut Self> {
        match self {
            Self::Leaf { .. } => Some(self),
            Self::Split { first, .. } => first.first_leaf_mut(),
        }
    }

    pub fn from_panes(panes: &WorkspacePaneNode) -> Self {
        match panes {
            WorkspacePaneNode::Leaf { session_id } => {
                Self::leaf(vec![session_id.clone()], Some(session_id.clone()))
            }
            WorkspacePaneNode::Split {
                id,
                direction,
                ratio_percent,
                first,
                second,
            } => Self::Split {
                id: format!("tw-legacy-{id}"),
                direction: *direction,
                ratio_percent: *ratio_percent,
                first: Box::new(Self::from_panes(first)),
                second: Box::new(Self::from_panes(second)),
            },
        }
    }

    pub fn tile_in_bounds(
        tabs: &[String],
        mode: SmartSplitMode,
        width: f32,
        height: f32,
    ) -> Option<Self> {
        let tabs = Self::unique_tabs(tabs.to_vec());
        if tabs.is_empty() {
            return None;
        }
        Some(Self::tile_groups(
            &tabs,
            mode,
            width.max(0.),
            height.max(0.),
        ))
    }

    fn tile_groups(tabs: &[String], mode: SmartSplitMode, width: f32, height: f32) -> Self {
        let horizontal = height >= GROUP_MIN_HEIGHT * 2. + GROUP_DIVIDER_SIZE;
        let vertical = width >= GROUP_MIN_WIDTH * 2. + GROUP_DIVIDER_SIZE;
        let direction = match mode {
            SmartSplitMode::Horizontal if horizontal => Some(WorkspaceSplitDirection::Horizontal),
            SmartSplitMode::Vertical if vertical => Some(WorkspaceSplitDirection::Vertical),
            SmartSplitMode::Auto if horizontal || vertical => Some(
                if vertical && (!horizontal || width / GROUP_MIN_WIDTH >= height / GROUP_MIN_HEIGHT)
                {
                    WorkspaceSplitDirection::Vertical
                } else {
                    WorkspaceSplitDirection::Horizontal
                },
            ),
            _ => None,
        };
        let Some(direction) = direction.filter(|_| tabs.len() > 1) else {
            return Self::leaf(tabs.to_vec(), None);
        };
        let mid = tabs.len().div_ceil(2);
        let ratio = mid as f32 / tabs.len() as f32;
        // Equal available space is needed when an odd number would otherwise
        // make the smaller side too small to hold even one group.
        let extent = match direction {
            WorkspaceSplitDirection::Horizontal => height,
            WorkspaceSplitDirection::Vertical => width,
        } - GROUP_DIVIDER_SIZE;
        let minimum = match direction {
            WorkspaceSplitDirection::Horizontal => GROUP_MIN_HEIGHT,
            WorkspaceSplitDirection::Vertical => GROUP_MIN_WIDTH,
        };
        let min_percent = (minimum / extent * 100.).ceil() as u8;
        let max_percent = ((1. - minimum / extent) * 100.).floor() as u8;
        let ratio_percent = ((ratio * 100.).round() as u8).clamp(min_percent, max_percent);
        let ratio = ratio_percent as f32 / 100.;
        let (w1, h1, w2, h2) = match direction {
            WorkspaceSplitDirection::Horizontal => {
                (width, extent * ratio, width, extent * (1. - ratio))
            }
            WorkspaceSplitDirection::Vertical => {
                (extent * ratio, height, extent * (1. - ratio), height)
            }
        };
        Self::Split {
            id: format!("tw-split-{}", uuid_v4_like()),
            direction,
            ratio_percent,
            first: Box::new(Self::tile_groups(&tabs[..mid], mode, w1, h1)),
            second: Box::new(Self::tile_groups(&tabs[mid..], mode, w2, h2)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GROUP_MIN_HEIGHT, GROUP_MIN_WIDTH};
    use crate::workspace_layout::pane::{
        SmartSplitMode, WorkspacePaneNode, WorkspaceSplitDirection,
    };
    use crate::workspace_layout::tabs::{TabDockEdge, TabDockZone, TerminalWindowNode};

    #[test]
    fn dock_zone_keeps_edge_priority_at_equal_distances() {
        assert_eq!(
            TabDockZone::detect(10., 10., 800., 600.),
            TabDockZone::Edge(TabDockEdge::Left)
        );
        assert_eq!(
            TabDockZone::detect(790., 10., 800., 600.),
            TabDockZone::Edge(TabDockEdge::Right)
        );
        assert_eq!(
            TabDockZone::detect(400., 10., 800., 600.),
            TabDockZone::Edge(TabDockEdge::Top)
        );
        assert_eq!(
            TabDockZone::detect(400., 590., 800., 600.),
            TabDockZone::Edge(TabDockEdge::Bottom)
        );
        assert_eq!(
            TabDockZone::detect(400., 300., 800., 600.),
            TabDockZone::Center
        );
    }

    #[test]
    fn dock_zone_orders_nan_distances_after_finite_edges() {
        assert_eq!(
            TabDockZone::detect(f32::NAN, 10., 800., 600.),
            TabDockZone::Edge(TabDockEdge::Top)
        );
        assert_eq!(
            TabDockZone::detect(10., f32::NAN, 800., 600.),
            TabDockZone::Edge(TabDockEdge::Left)
        );
    }

    fn assert_leaf_sizes(tree: &TerminalWindowNode, width: f32, height: f32) {
        match tree {
            TerminalWindowNode::Leaf { .. } => {
                assert!(
                    width + 0.01 >= GROUP_MIN_WIDTH && height + 0.01 >= GROUP_MIN_HEIGHT,
                    "group {width}x{height}"
                );
            }
            TerminalWindowNode::Split {
                direction,
                ratio_percent,
                first,
                second,
                ..
            } => {
                let ratio = *ratio_percent as f32 / 100.;
                match direction {
                    WorkspaceSplitDirection::Horizontal => {
                        assert_leaf_sizes(first, width, (height - 4.) * ratio);
                        assert_leaf_sizes(second, width, (height - 4.) * (1. - ratio));
                    }
                    WorkspaceSplitDirection::Vertical => {
                        assert_leaf_sizes(first, (width - 4.) * ratio, height);
                        assert_leaf_sizes(second, (width - 4.) * (1. - ratio), height);
                    }
                }
            }
        }
    }

    fn tabs(count: usize) -> Vec<String> {
        (0..count).map(|n| format!("session-{n}")).collect()
    }

    #[test]
    fn tiling_keeps_every_session_and_respects_available_space() {
        for count in [2, 3, 4, 8, 100] {
            for mode in [
                SmartSplitMode::Auto,
                SmartSplitMode::Horizontal,
                SmartSplitMode::Vertical,
            ] {
                for size in [(240., 160.), (484., 324.), (489., 328.), (1280., 800.)] {
                    let ids = tabs(count);
                    let tree =
                        TerminalWindowNode::tile_in_bounds(&ids, mode, size.0, size.1).unwrap();
                    assert_eq!(tree.collect_tab_ids(), ids);
                    assert_leaf_sizes(&tree, size.0, size.1);
                    let (w, h) = tree.minimum_size();
                    assert!(
                        w <= size.0 + 1. && h <= size.1 + 1.,
                        "{mode:?} {size:?}: {w}x{h}"
                    );
                    for group in tree.leaf_ids() {
                        let (tabs, active) = tree.leaf_tabs(&group).unwrap();
                        assert!(!tabs.is_empty());
                        assert!(active.is_some_and(|id| tabs.iter().any(|tab| tab == id)));
                    }
                }
            }
        }
    }

    #[test]
    fn auto_tiling_chooses_the_axis_that_fits_the_viewport() {
        let ids = tabs(4);
        for (size, expected) in [
            ((1000., GROUP_MIN_HEIGHT), WorkspaceSplitDirection::Vertical),
            ((GROUP_MIN_WIDTH, 700.), WorkspaceSplitDirection::Horizontal),
        ] {
            assert!(
                matches!(TerminalWindowNode::tile_in_bounds(&ids, SmartSplitMode::Auto, size.0, size.1), Some(TerminalWindowNode::Split { direction, .. }) if direction == expected)
            );
        }
    }

    #[test]
    fn legacy_panes_expand_in_place_without_hiding_other_tabs() {
        let old = WorkspacePaneNode::Split {
            id: "legacy".into(),
            direction: WorkspaceSplitDirection::Horizontal,
            ratio_percent: 37,
            first: Box::new(WorkspacePaneNode::leaf("a")),
            second: Box::new(WorkspacePaneNode::leaf("b")),
        };
        let mut tree =
            TerminalWindowNode::leaf(vec!["x".into(), "a".into(), "y".into()], Some("y".into()));
        let original = tree.first_leaf_id().unwrap();
        assert!(tree.expand_pane_tab("a", &old));
        assert_eq!(tree.collect_tab_ids(), ["x", "a", "y", "b"]);
        assert_eq!(tree.leaf_tabs(&original).unwrap().1, Some("y"));
        assert!(matches!(
            tree,
            TerminalWindowNode::Split {
                direction: WorkspaceSplitDirection::Horizontal,
                ratio_percent: 37,
                ..
            }
        ));
        let ordered = tree.collect_tab_ids();
        let saved = tree.serialize_layout(&ordered).unwrap();
        let restored = TerminalWindowNode::restore_layout(&saved, &ordered).unwrap();
        assert_eq!(restored.collect_tab_ids(), ordered);
        assert_eq!(restored.active_tabs(), ["y", "b"]);
    }

    #[test]
    fn invalid_dock_target_leaves_source_placement_unchanged() {
        let mut tree =
            TerminalWindowNode::tile_in_bounds(&tabs(4), SmartSplitMode::Auto, 1280., 800.)
                .unwrap();
        let before = tree.clone();
        assert!(!tree.dock_tab(
            "session-0",
            "missing",
            TabDockZone::Edge(TabDockEdge::Right)
        ));
        assert_eq!(tree, before);
        assert!(!tree.move_tab_to_leaf("session-0", "missing"));
        assert_eq!(tree, before);
    }
    #[test]
    fn unsupported_persisted_split_is_rejected_before_conversion() {
        let layout = crate::RestorableTerminalWindowNode::Split {
            direction: "future-layout".into(),
            ratio: 0.5,
            first: Box::new(crate::RestorableTerminalWindowNode::Leaf {
                tab_indexes: vec![0],
                active_tab_index: Some(0),
            }),
            second: Box::new(crate::RestorableTerminalWindowNode::Leaf {
                tab_indexes: vec![1],
                active_tab_index: Some(1),
            }),
        };
        assert!(TerminalWindowNode::restore_layout(&layout, &tabs(2)).is_none());
    }

    #[test]
    fn closing_an_active_tab_selects_its_neighbor_in_the_same_group() {
        let mut tree = TerminalWindowNode::leaf(tabs(4), Some("session-2".into()));
        let tree = tree.remove_tab("session-2").unwrap();
        assert_eq!(tree.active_tabs(), ["session-3"]);
    }

    #[test]
    fn dropping_at_the_end_of_a_single_group_reorders_the_tab() {
        let mut tree = TerminalWindowNode::leaf(tabs(3), Some("session-1".into()));
        let group = tree.first_leaf_id().unwrap();
        assert!(tree.move_tab_to_leaf("session-0", &group));
        assert_eq!(
            tree.collect_tab_ids(),
            ["session-1", "session-2", "session-0"]
        );
        assert_eq!(tree.active_tabs(), ["session-0"]);
    }

    #[test]
    fn center_drop_in_its_own_single_tab_group_keeps_the_layout() {
        let mut tree =
            TerminalWindowNode::tile_in_bounds(&tabs(2), SmartSplitMode::Vertical, 1000., 600.)
                .unwrap();
        let group = tree.leaf_for_tab("session-0").unwrap().to_string();
        let before = tree.clone();
        assert!(tree.move_tab_to_leaf("session-0", &group));
        assert_eq!(tree, before);
    }
}
