use super::docker_view::docker_panel;
use super::process::{process_display_mode, process_row_height_px};
use super::process_view::processes_panel;
use super::stats_view::{NetworkCardPresentation, gpu_panel, npu_panel, stats_panel};
use crate::actions::{RemoteMonitorAction, RemoteMonitorActionHandler};
use crate::models::DockerTab;
use crate::presentation::{
    DockerDerivedItems, DockerPresentationState, GpuPresentationState, NpuPresentationState,
    ProcessPresentationState, StatsPresentationState,
};
use gpui::{
    Context, Entity, IntoElement, ListAlignment, ListOffset, ListState, Render, Rgba, ScrollHandle,
    ScrollStrategy, Task, UniformListScrollHandle, Window, div, prelude::*, px,
};
use nyaterm_remote_ops::ascend_npu::RemoteNpuProcess;
use nyaterm_remote_ops::gpu::RemoteGpuProcess;
use nyaterm_remote_ops::process_control::ProcessControlCapabilities;
use nyaterm_remote_ops::remote_process::RemoteProcess;
use nyaterm_ui::ThemePalette;
use nyaterm_ui::{NyaInputState, NyaNumberInputState, NyaSelectState};
use rust_i18n::t;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often a polling panel asks whether its own interval has come due.
///
/// The per-panel intervals are user settings in whole seconds floored at one, so a
/// one-second clock is exactly as fine as the finest thing it can service. This is
/// the cadence the one shell-wide clock used, kept unchanged; what moved is who owns
/// it. Sleeping straight to the next due moment instead would mean a settings change
/// mid-sleep landing up to a whole interval late.
const REMOTE_PANEL_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Which of the five polling panels an entity is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteMonitorKind {
    Stats,
    Gpu,
    Npu,
    Processes,
    Docker,
}

impl RemoteMonitorKind {
    pub const ALL: [RemoteMonitorKind; 5] = [
        RemoteMonitorKind::Stats,
        RemoteMonitorKind::Gpu,
        RemoteMonitorKind::Npu,
        RemoteMonitorKind::Processes,
        RemoteMonitorKind::Docker,
    ];
}

/// Colours a polling panel needs, already resolved.
///
/// The views used to call `theme_palette()` and `shell_transparent_color(..)` on the app
/// while rendering. Resolving them into the snapshot is what lets a panel render without
/// touching `NyaTermApp` at all -- and that matters beyond tidiness, because GPUI records
/// every entity read during a view's render as a dependency of that view. One app read
/// here would re-dirty the panel on every unrelated `app.notify()` and undo the whole
/// point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelChrome {
    pub palette: ThemePalette,
    /// `shell_transparent_color(palette.surface)`.
    pub transparent_surface: Rgba,
    /// `shell_surface_color(palette.surface)`, which the process and Docker row menus
    /// sit on.
    pub surface: Rgba,
    /// `shell_transparent_color(palette.section_header)`, the Docker section headers.
    pub transparent_section_header: Rgba,
}

/// Everything a Stats/GPU/NPU panel renders from.
///
/// Carries the pane revision it was built at, so a flush can skip a panel that is already
/// current and a boundary-time assertion can confirm none was left behind.
pub struct RemoteMonitorSnapshot {
    pub key: RemoteMonitorKey,
    pub data: RemoteMonitorData,
}

/// Everything outside the pane data that a snapshot depends on.
///
/// The pane revision alone is not enough: the theme, whether a session is active, and the
/// right-panel width all change the rendered output without touching `remote_ops`, so a
/// flush keyed on the revision alone would skip a panel that needs rebuilding.
///
/// **This is a safety net for a flush that runs, not a replacement for the boundaries.**
/// A boundary that is never reached never compares anything, so a missed boundary is still
/// a stale panel -- which is why the freshness tests drive each one rather than trusting
/// the key. The explicit boundary list stays the discipline; the key only stops a flush
/// from concluding "nothing changed" when something outside the pane did.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteMonitorKey {
    pub revision: u64,
    pub host_identity: crate::actions::HostInteractionToken,
    pub process_control: ProcessControlCapabilities,
    pub local_unsupported: bool,
    pub chrome: PanelChrome,
    pub has_session: bool,
    pub ssh_compatibility_mode: bool,
    /// Right-panel width, which decides the process table's columns and row heights.
    /// Zero for the panes that do not depend on it.
    pub panel_width: f32,
    /// Whether the Stats pane lists logged-in users. Always false for the other panes.
    pub show_users: bool,
    pub stats_interval: u32,
}

pub enum RemoteMonitorData {
    Stats {
        state: StatsPresentationState,
        network_select: Entity<NyaSelectState>,
    },
    Gpu {
        state: GpuPresentationState,
        processes: Arc<[RemoteGpuProcess]>,
        /// The search field's state entity, owned by the app's input registry and handed
        /// over so the panel can build the element without asking for it. Reading this
        /// entity during render is *wanted*: typing notifies it, which invalidates this
        /// panel and nothing else.
        search: Entity<NyaInputState>,
    },
    Npu {
        state: NpuPresentationState,
        processes: Arc<[RemoteNpuProcess]>,
        search: Entity<NyaInputState>,
    },
    Processes {
        state: ProcessPresentationState,
        processes: Arc<[RemoteProcess]>,
        search: Entity<NyaInputState>,
        /// Only built while a process is selected, and keyed per pid.
        nice: Option<Entity<NyaNumberInputState>>,
        sort: Entity<NyaSelectState>,
    },
    /// Boxed because it is far the largest variant -- `DockerPresentationState` alone
    /// carries fourteen fields including an inline `Option<DockerContainerDetails>` -- and
    /// leaving it unboxed makes every variant pay for it. One allocation per Docker
    /// snapshot build is the cheaper side of that trade.
    Docker(Box<DockerSnapshot>),
}

pub struct DockerSnapshot {
    pub state: DockerPresentationState,
    /// The filtered list for the effective tab; every variant is already `Arc`-backed.
    pub derived: DockerDerivedItems,
    pub effective_tab: DockerTab,
    pub search: Entity<NyaInputState>,
}

/// One remote polling panel.
pub struct RemoteMonitorPanel {
    kind: RemoteMonitorKind,
    /// The application supplies a handler capturing a weak owner, preventing a
    /// cycle between its panel entities and their action callbacks.
    handler: RemoteMonitorActionHandler,
    /// The refresh schedule, and the only record of whether this panel is wanted.
    ///
    /// A `Task` cancels when dropped, so `None` is not merely a flag saying the panel
    /// should not poll -- it is the poll being gone. That makes "an inactive panel
    /// does not poll" a property of the type rather than of a predicate somebody has
    /// to remember to check, and it is why there is no separate `demand: bool`: the
    /// task's existence *is* the demand, so the two cannot disagree.
    clock: Option<Task<()>>,
    /// The snapshot this panel renders from.
    snapshot: Option<RemoteMonitorSnapshot>,
    /// Display time advanced by the existing polling task, without rebuilding data.
    network_clock: Instant,
    network_scroll: ScrollHandle,
    /// Variable-height process rows live here because expanding a process changes only
    /// that row's measured height. The panel entity is the authoritative UI owner.
    process_list: ListState,
    /// Docker keeps separate positions for containers and the resource tabs, matching
    /// the two independent offsets the old wheel-only implementation exposed.
    docker_container_scroll: UniformListScrollHandle,
    docker_resource_scroll: UniformListScrollHandle,
    /// Paints of *this entity*, so a test can tell the entity route apart from the
    /// inline views it replaced. Both register the same search-input ids, so no
    /// externally visible side effect distinguishes them.
    #[cfg(any(test, feature = "test-support"))]
    paint_count: usize,
}

impl RemoteMonitorPanel {
    /// Every pane kind renders from a snapshot rather than reading app state in `render`.
    pub const SNAPSHOT_KINDS: [RemoteMonitorKind; 5] = [
        RemoteMonitorKind::Stats,
        RemoteMonitorKind::Gpu,
        RemoteMonitorKind::Npu,
        RemoteMonitorKind::Processes,
        RemoteMonitorKind::Docker,
    ];

    /// Dispatch from event handlers. The caller must defer snapshot delivery until
    /// this panel's current entity lease ends; render never invokes the handler.
    pub(crate) fn dispatch(&self, action: RemoteMonitorAction, cx: &mut Context<Self>) -> bool {
        (self.handler)(action, None, cx)
    }
    pub(crate) fn dispatch_in(
        &self,
        action: RemoteMonitorAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        (self.handler)(action, Some(window), cx)
    }

    #[cfg(feature = "test-support")]
    pub fn test_snapshot(&self) -> Option<&RemoteMonitorSnapshot> {
        self.snapshot.as_ref()
    }
    #[cfg(feature = "test-support")]
    pub fn test_network_clock(&self) -> Instant {
        self.network_clock
    }
    #[cfg(feature = "test-support")]
    pub fn test_process_count(&self) -> usize {
        self.process_list.item_count()
    }
    #[cfg(feature = "test-support")]
    pub fn test_process_max_scroll(&self) -> gpui::Pixels {
        self.process_list.max_offset_for_scrollbar().y
    }
    #[cfg(feature = "test-support")]
    pub fn test_process_top_index(&self) -> usize {
        self.process_list.logical_scroll_top().item_ix
    }
    #[cfg(feature = "test-support")]
    pub fn test_process_row_bounds(&self, index: usize) -> Option<gpui::Bounds<gpui::Pixels>> {
        self.process_list.bounds_for_item(index)
    }
    #[cfg(feature = "test-support")]
    pub fn test_scroll_process_to(&self, offset: ListOffset) {
        self.process_list.scroll_to(offset);
    }
    #[cfg(feature = "test-support")]
    pub fn test_docker_resources_scrollable(&self) -> bool {
        self.docker_resource_scroll.is_scrollable()
    }
    #[cfg(feature = "test-support")]
    pub fn test_docker_resource_offset(&self) -> gpui::Pixels {
        self.docker_resource_scroll
            .0
            .borrow()
            .base_handle
            .offset()
            .y
    }
    #[cfg(feature = "test-support")]
    pub fn test_scroll_docker_resources_to(&self, index: usize) {
        self.docker_resource_scroll
            .scroll_to_item_strict(index, ScrollStrategy::Top);
    }
    #[cfg(feature = "test-support")]
    pub fn test_dispatch(&self, action: RemoteMonitorAction, cx: &mut Context<Self>) -> bool {
        self.dispatch(action, cx)
    }

    /// The pane revision this panel last received, if any.
    #[cfg(any(test, feature = "test-support"))]
    pub fn snapshot_revision(&self) -> Option<u64> {
        self.snapshot.as_ref().map(|snapshot| snapshot.key.revision)
    }

    pub fn snapshot_key(&self) -> Option<RemoteMonitorKey> {
        self.snapshot.as_ref().map(|snapshot| snapshot.key.clone())
    }

    pub fn set_snapshot(&mut self, snapshot: RemoteMonitorSnapshot, cx: &mut Context<Self>) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|old| old.key.host_identity != snapshot.key.host_identity)
        {
            self.network_scroll = ScrollHandle::new();
        }
        self.network_clock = Instant::now();
        self.reconcile_scroll_state(&snapshot);
        self.snapshot = Some(snapshot);
        // Notifies this panel only. The app is untouched, so nothing else repaints.
        cx.notify();
    }

    pub fn new(kind: RemoteMonitorKind, handler: RemoteMonitorActionHandler) -> Self {
        Self {
            kind,
            handler,
            clock: None,
            snapshot: None,
            network_clock: Instant::now(),
            network_scroll: ScrollHandle::new(),
            process_list: ListState::new(0, ListAlignment::Top, px(256.)),
            docker_container_scroll: UniformListScrollHandle::new(),
            docker_resource_scroll: UniformListScrollHandle::new(),
            #[cfg(any(test, feature = "test-support"))]
            paint_count: 0,
        }
    }

    fn reconcile_scroll_state(&mut self, snapshot: &RemoteMonitorSnapshot) {
        match &snapshot.data {
            RemoteMonitorData::Processes {
                state, processes, ..
            } => {
                let (reset_to_top, needs_reset) =
                    self.snapshot.as_ref().map_or((true, true), |previous| {
                        let RemoteMonitorData::Processes {
                            state: previous_state,
                            processes: previous_processes,
                            ..
                        } = &previous.data
                        else {
                            return (true, true);
                        };
                        let reset_to_top = previous.key.has_session != snapshot.key.has_session
                            || previous_state.search_draft != state.search_draft
                            || previous_state.sort_key != state.sort_key
                            || previous_state.sort_direction != state.sort_direction;
                        let previous_selected_index = previous_state.selected_pid.and_then(|pid| {
                            previous_processes
                                .iter()
                                .position(|process| process.pid == pid)
                        });
                        let selected_index = state.selected_pid.and_then(|pid| {
                            processes.iter().position(|process| process.pid == pid)
                        });
                        let previous_mode = process_display_mode(previous.key.panel_width);
                        let mode = process_display_mode(snapshot.key.panel_width);
                        (
                            reset_to_top,
                            reset_to_top
                                || previous_processes.len() != processes.len()
                                || previous_selected_index != selected_index
                                || previous_mode != mode
                                || previous.key.panel_width != snapshot.key.panel_width
                                || (selected_index.is_some()
                                    && previous.key.revision != snapshot.key.revision),
                        )
                    });
                if needs_reset && !reset_to_top && self.process_list.item_count() == processes.len()
                {
                    // Width and expanded content can change heights without changing row identities.
                    // Keep the position inside the current row instead of jumping to its top.
                    self.process_list.remeasure();
                } else if needs_reset {
                    let previous_top = self.process_list.logical_scroll_top();
                    let row_height =
                        process_row_height_px(process_display_mode(snapshot.key.panel_width));
                    self.process_list
                        .reset_with_uniform_height(processes.len(), px(row_height));
                    let restored_top = if reset_to_top {
                        ListOffset::default()
                    } else {
                        ListOffset {
                            item_ix: previous_top.item_ix.min(processes.len().saturating_sub(1)),
                            offset_in_item: px(0.),
                        }
                    };
                    self.process_list.scroll_to(restored_top);
                }
            }
            RemoteMonitorData::Docker(docker) => {
                let reset_to_top = self.snapshot.as_ref().is_none_or(|previous| {
                    let RemoteMonitorData::Docker(previous_docker) = &previous.data else {
                        return true;
                    };
                    previous.key.has_session != snapshot.key.has_session
                        || previous_docker.effective_tab != docker.effective_tab
                        || previous_docker.state.search_draft != docker.state.search_draft
                });
                if reset_to_top {
                    self.docker_container_scroll
                        .scroll_to_item_strict(0, ScrollStrategy::Top);
                    self.docker_resource_scroll
                        .scroll_to_item_strict(0, ScrollStrategy::Top);
                }
            }
            _ => {}
        }
    }

    /// Start or stop this panel's refresh schedule.
    ///
    /// Idempotent, and cheap in the steady state: one `Option::is_some` compare. The
    /// caller reconciles every paint, so this is called far more often than it acts.
    ///
    /// No `cx.notify()` on either edge -- nothing this panel renders depends on
    /// whether it is polling, and notifying from the app's own paint would ask for a
    /// frame that has nothing new in it.
    pub fn set_demand(&mut self, demand: bool, cx: &mut Context<Self>) {
        if demand == self.clock.is_some() {
            return;
        }
        if !demand {
            // Dropping the task cancels it.
            self.clock = None;
            return;
        }
        let kind = self.kind;
        let handler = self.handler.clone();
        self.clock = Some(cx.spawn(async move |_panel, cx| {
            loop {
                cx.background_executor()
                    .timer(REMOTE_PANEL_POLL_INTERVAL)
                    .await;
                if !cx.update(|cx| handler(RemoteMonitorAction::Tick(kind), None, cx)) {
                    break;
                }
            }
        }));
    }

    pub fn advance_network_clock(&mut self, now: Instant, cx: &mut Context<Self>) {
        let Some(RemoteMonitorSnapshot {
            data: RemoteMonitorData::Stats { state, .. },
            ..
        }) = self.snapshot.as_ref()
        else {
            return;
        };
        let Some(last) = state.last_success_at else {
            return;
        };
        if now.saturating_duration_since(last).as_secs()
            != self.network_clock.saturating_duration_since(last).as_secs()
        {
            self.network_clock = now;
            cx.notify();
        }
    }

    /// Widened past this module so the session-activation boundary can assert that a
    /// switch reconciles demand without a paint.
    #[cfg(any(test, feature = "test-support"))]
    pub fn is_polling(&self) -> bool {
        self.clock.is_some()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn paint_count(&self) -> usize {
        self.paint_count
    }
}

impl Render for RemoteMonitorPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-support"))]
        {
            self.paint_count += 1;
        }
        // Snapshot kinds render with **zero** `NyaTermApp` access, diagnostics included:
        // GPUI records every entity read during a view's render as a dependency of that
        // view, so one app read here would re-dirty this panel on every unrelated
        // `app.notify()`.
        if Self::SNAPSHOT_KINDS.contains(&self.kind) {
            let Some(snapshot) = self.snapshot.as_ref() else {
                // Nothing pushed yet; the next boundary fills it in.
                return div().into_any_element();
            };
            let chrome = snapshot.key.chrome;
            if snapshot.key.local_unsupported {
                return div()
                    .size_full()
                    .bg(chrome.transparent_surface)
                    .child(nyaterm_ui::empty_panel(
                        t!("hostTools.localWindowsUnsupported"),
                        chrome.palette,
                    ))
                    .into_any_element();
            }
            if snapshot.key.ssh_compatibility_mode {
                return div()
                    .size_full()
                    .bg(chrome.transparent_surface)
                    .child(nyaterm_ui::empty_panel(
                        t!("panel.sshCompatibilityModeUnavailable"),
                        chrome.palette,
                    ))
                    .into_any_element();
            }
            let has_session = snapshot.key.has_session;
            let panel_width = snapshot.key.panel_width;
            return match &snapshot.data {
                RemoteMonitorData::Stats {
                    state,
                    network_select,
                } => stats_panel(
                    chrome,
                    has_session,
                    snapshot.key.show_users,
                    state.clone(),
                    network_select.clone(),
                    NetworkCardPresentation {
                        now: self.network_clock,
                        interval: Duration::from_secs(u64::from(snapshot.key.stats_interval)),
                        scroll: self.network_scroll.clone(),
                    },
                    cx,
                ),
                RemoteMonitorData::Gpu {
                    state,
                    processes,
                    search,
                } => {
                    let (state, processes, search) =
                        (state.clone(), processes.clone(), search.clone());
                    gpu_panel(
                        chrome,
                        has_session,
                        state,
                        processes,
                        search,
                        panel_width,
                        cx,
                    )
                }
                RemoteMonitorData::Npu {
                    state,
                    processes,
                    search,
                } => {
                    let (state, processes, search) =
                        (state.clone(), processes.clone(), search.clone());
                    npu_panel(
                        chrome,
                        has_session,
                        state,
                        processes,
                        search,
                        panel_width,
                        cx,
                    )
                }
                RemoteMonitorData::Processes {
                    state,
                    processes,
                    search,
                    nice,
                    sort,
                } => {
                    let sort = sort.clone();
                    let (state, processes, search, nice) = (
                        state.clone(),
                        processes.clone(),
                        search.clone(),
                        nice.clone(),
                    );
                    processes_panel(
                        chrome,
                        snapshot.key.process_control,
                        snapshot.key.host_identity,
                        has_session,
                        state,
                        processes,
                        panel_width,
                        search,
                        nice,
                        sort,
                        self.process_list.clone(),
                        cx,
                    )
                }
                RemoteMonitorData::Docker(docker) => {
                    let (state, derived, effective_tab, search) = (
                        docker.state.clone(),
                        docker.derived.clone(),
                        docker.effective_tab,
                        docker.search.clone(),
                    );
                    docker_panel(
                        chrome,
                        has_session,
                        state,
                        derived,
                        effective_tab,
                        panel_width,
                        search,
                        self.docker_container_scroll.clone(),
                        self.docker_resource_scroll.clone(),
                        cx,
                    )
                }
            };
        }
        // Every kind is a snapshot kind now, so this is unreachable in practice; it
        // stays as the empty case rather than a panic because a panel with no snapshot
        // yet is a legitimate transient state.
        div().into_any_element()
    }
}
