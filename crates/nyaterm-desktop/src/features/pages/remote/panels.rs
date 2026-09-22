//! The five remote panels that poll a host, as GPUI entities.
//!
//! Stats, GPU, NPU, Processes and Docker each show data fetched from the active SSH
//! session on an interval. Until Phase 4 they were built inline by `panel_body`, and
//! the schedule that fetched their data lived in one shell-wide clock
//! (`features/shell/remote_refresh.rs`).
//!
//! **What these entities own is the schedule, not the data.**
//! `NyaTermApp.remote_ops` stays the single authoritative owner of every pane's data,
//! in-flight job, failure streak and `last_refresh_at`, and the decision of whether a
//! refresh is due stays with it -- a panel asks "refresh if due" and the app answers.
//! The panel owns only the timer that does the asking, which is the thing that was in
//! the wrong place.
//!
//! One type with a `kind` discriminant rather than five near-identical types: five
//! instances are five entities with five entity ids, so they are as isolated as five
//! types would be, and the two matches below are smaller than the duplication.
//!
//! These are held by `NyaTermApp` for its whole life, so a hidden panel is **not**
//! dropped -- "not rendered" is not observable as a drop. That is deliberate rather
//! than incidental: the header status bar can want stats, GPU or NPU while the
//! matching panel is closed, so the entity has to outlive its own visibility to be
//! able to own that demand.
//!
//! Nothing here observes the app. A view embedded with `into_any_element` takes
//! GPUI's uncached path, whose `request_layout` calls `render` every frame, so a
//! child view of the app is repainted by the app's own paint and a
//! `cx.observe(&app, ..)` would buy nothing but coupling.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Context, Entity, IntoElement, ListAlignment, ListOffset, ListState, Render, Rgba,
    ScrollStrategy, Task, UniformListScrollHandle, WeakEntity, Window, div, prelude::*, px,
};
use nyaterm_transport::{RemoteGpuProcess, RemoteNpuProcess, RemoteProcess};
use nyaterm_ui::{NyaInputState, NyaNumberInputOptions, NyaNumberInputState};
use rust_i18n::t;

use super::docker_view::docker_panel;
use super::process::{process_display_mode, process_row_height_px};
use super::process_view::processes_panel;
use super::stats_view::{gpu_panel, npu_panel, stats_panel};
use crate::features::NyaTermApp;
use crate::features::remote::{
    DockerDerivedItems, DockerPresentationState, GpuPresentationState, NpuPresentationState,
    ProcessPresentationState, StatsPresentationState,
};
use crate::features::text_inputs::TextInputSetup;
use crate::models::{DockerTab, NavItem};
use crate::theme::ThemePalette;

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
pub(in crate::features) enum RemoteMonitorKind {
    Stats,
    Gpu,
    Npu,
    Processes,
    Docker,
}

impl RemoteMonitorKind {
    pub(in crate::features) const ALL: [RemoteMonitorKind; 5] = [
        RemoteMonitorKind::Stats,
        RemoteMonitorKind::Gpu,
        RemoteMonitorKind::Npu,
        RemoteMonitorKind::Processes,
        RemoteMonitorKind::Docker,
    ];

    /// The nav item whose panel shows this metric.
    pub(in crate::features) fn nav_item(self) -> NavItem {
        match self {
            RemoteMonitorKind::Stats => NavItem::Stats,
            RemoteMonitorKind::Gpu => NavItem::GpuMonitor,
            RemoteMonitorKind::Npu => NavItem::AscendNpuMonitor,
            RemoteMonitorKind::Processes => NavItem::Processes,
            RemoteMonitorKind::Docker => NavItem::Docker,
        }
    }
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
pub(in crate::features) struct PanelChrome {
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
pub(in crate::features) struct RemoteMonitorSnapshot {
    key: RemoteMonitorKey,
    data: RemoteMonitorData,
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
#[derive(Clone, Copy, Debug, PartialEq)]
struct RemoteMonitorKey {
    revision: u64,
    chrome: PanelChrome,
    has_session: bool,
    /// Right-panel width, which decides the process table's columns and row heights.
    /// Zero for the panes that do not depend on it.
    panel_width: f32,
}

pub(in crate::features) enum RemoteMonitorData {
    Stats(StatsPresentationState),
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
    },
    /// Boxed because it is far the largest variant -- `DockerPresentationState` alone
    /// carries fourteen fields including an inline `Option<DockerContainerDetails>` -- and
    /// leaving it unboxed makes every variant pay for it. One allocation per Docker
    /// snapshot build is the cheaper side of that trade.
    Docker(Box<DockerSnapshot>),
}

pub(in crate::features) struct DockerSnapshot {
    state: DockerPresentationState,
    /// The filtered list for the effective tab; every variant is already `Arc`-backed.
    derived: DockerDerivedItems,
    effective_tab: DockerTab,
    search: Entity<NyaInputState>,
}

/// One remote polling panel.
pub(in crate::features) struct RemoteMonitorPanel {
    kind: RemoteMonitorKind,
    /// Weak on purpose. `NyaTermApp` owns this entity, so a strong handle back would
    /// be a reference cycle that keeps both alive forever. The app always outlives
    /// the panels it owns, so the upgrade below only fails during teardown.
    app: WeakEntity<NyaTermApp>,
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
    #[cfg(test)]
    paint_count: usize,
}

impl RemoteMonitorPanel {
    /// Every pane kind renders from a snapshot rather than reading app state in `render`.
    pub(in crate::features::pages::remote) const SNAPSHOT_KINDS: [RemoteMonitorKind; 5] = [
        RemoteMonitorKind::Stats,
        RemoteMonitorKind::Gpu,
        RemoteMonitorKind::Npu,
        RemoteMonitorKind::Processes,
        RemoteMonitorKind::Docker,
    ];

    /// Run `f` against the app and queue the snapshot flush after the panel lease ends.
    ///
    /// Every panel-initiated mutation goes through here, which makes the panel its own
    /// flush boundary: no callback has to remember a separate step. Event handlers lease
    /// this panel while they run, so the flush is deferred until that lease is released.
    /// This method is called from event handlers, never from render.
    pub(in crate::features::pages::remote) fn with_app<R: Default>(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut NyaTermApp, &mut Context<NyaTermApp>) -> R,
    ) -> R {
        let Some(app) = self.app.upgrade() else {
            return R::default();
        };
        app.update(cx, |app, cx| {
            let result = f(app, cx);
            app.defer_remote_panel_snapshot_flush(cx);
            result
        })
    }

    /// The pane revision this panel last received, if any.
    #[cfg(test)]
    pub(in crate::features::pages::remote) fn snapshot_revision(&self) -> Option<u64> {
        self.snapshot.as_ref().map(|snapshot| snapshot.key.revision)
    }

    fn snapshot_key(&self) -> Option<RemoteMonitorKey> {
        self.snapshot.as_ref().map(|snapshot| snapshot.key)
    }

    fn set_snapshot(&mut self, snapshot: RemoteMonitorSnapshot, cx: &mut Context<Self>) {
        self.reconcile_scroll_state(&snapshot);
        self.snapshot = Some(snapshot);
        // Notifies this panel only. The app is untouched, so nothing else repaints.
        cx.notify();
    }

    fn new(kind: RemoteMonitorKind, app: WeakEntity<NyaTermApp>) -> Self {
        Self {
            kind,
            app,
            clock: None,
            snapshot: None,
            process_list: ListState::new(0, ListAlignment::Top, px(256.)),
            docker_container_scroll: UniformListScrollHandle::new(),
            docker_resource_scroll: UniformListScrollHandle::new(),
            #[cfg(test)]
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
                                || previous_mode != mode,
                        )
                    });
                if needs_reset {
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
    fn set_demand(&mut self, demand: bool, cx: &mut Context<Self>) {
        if demand == self.clock.is_some() {
            return;
        }
        if !demand {
            // Dropping the task cancels it.
            self.clock = None;
            return;
        }
        let kind = self.kind;
        let app = self.app.clone();
        self.clock = Some(cx.spawn(async move |_panel, cx| {
            loop {
                cx.background_executor()
                    .timer(REMOTE_PANEL_POLL_INTERVAL)
                    .await;
                let Some(app) = app.upgrade() else {
                    break;
                };
                // The app owns the decision: whether the interval is due, whether a
                // job is already in flight, and whether the app is calm enough to
                // submit one. This clock only decides *when to ask*.
                //
                // No error to handle: the strong handle from `upgrade` keeps the app
                // alive for the call, and a released app ends the loop above.
                app.update(cx, |app, cx| {
                    app.refresh_remote_monitor_if_due(kind, cx);
                    // Flush boundary: submitting a refresh changes the pane status.
                    app.flush_remote_panel_snapshots(cx);
                });
            }
        }));
    }

    /// Widened past this module so the session-activation boundary can assert that a
    /// switch reconciles demand without a paint.
    #[cfg(test)]
    pub(in crate::features) fn is_polling(&self) -> bool {
        self.clock.is_some()
    }

    #[cfg(test)]
    fn paint_count(&self) -> usize {
        self.paint_count
    }
}

impl Render for RemoteMonitorPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
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
            let has_session = snapshot.key.has_session;
            let panel_width = snapshot.key.panel_width;
            return match &snapshot.data {
                RemoteMonitorData::Stats(state) => {
                    let state = state.clone();
                    stats_panel(chrome, has_session, state, cx)
                }
                RemoteMonitorData::Gpu {
                    state,
                    processes,
                    search,
                } => {
                    let (state, processes, search) =
                        (state.clone(), processes.clone(), search.clone());
                    gpu_panel(chrome, has_session, state, processes, search, cx)
                }
                RemoteMonitorData::Npu {
                    state,
                    processes,
                    search,
                } => {
                    let (state, processes, search) =
                        (state.clone(), processes.clone(), search.clone());
                    npu_panel(chrome, has_session, state, processes, search, cx)
                }
                RemoteMonitorData::Processes {
                    state,
                    processes,
                    search,
                    nice,
                } => {
                    let (state, processes, search, nice) = (
                        state.clone(),
                        processes.clone(),
                        search.clone(),
                        nice.clone(),
                    );
                    processes_panel(
                        chrome,
                        has_session,
                        state,
                        processes,
                        panel_width,
                        search,
                        nice,
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

/// The five panel entities, as one field on `NyaTermApp`.
pub(in crate::features) struct RemotePanels {
    stats: Entity<RemoteMonitorPanel>,
    gpu: Entity<RemoteMonitorPanel>,
    npu: Entity<RemoteMonitorPanel>,
    processes: Entity<RemoteMonitorPanel>,
    docker: Entity<RemoteMonitorPanel>,
}

impl RemotePanels {
    pub(in crate::features) fn new(
        app: WeakEntity<NyaTermApp>,
        cx: &mut Context<NyaTermApp>,
    ) -> Self {
        let panel = |kind: RemoteMonitorKind, cx: &mut Context<NyaTermApp>| {
            let app = app.clone();
            cx.new(|_| RemoteMonitorPanel::new(kind, app))
        };
        Self {
            stats: panel(RemoteMonitorKind::Stats, cx),
            gpu: panel(RemoteMonitorKind::Gpu, cx),
            npu: panel(RemoteMonitorKind::Npu, cx),
            processes: panel(RemoteMonitorKind::Processes, cx),
            docker: panel(RemoteMonitorKind::Docker, cx),
        }
    }

    pub(in crate::features) fn entity(
        &self,
        kind: RemoteMonitorKind,
    ) -> &Entity<RemoteMonitorPanel> {
        match kind {
            RemoteMonitorKind::Stats => &self.stats,
            RemoteMonitorKind::Gpu => &self.gpu,
            RemoteMonitorKind::Npu => &self.npu,
            RemoteMonitorKind::Processes => &self.processes,
            RemoteMonitorKind::Docker => &self.docker,
        }
    }
}

impl NyaTermApp {
    /// Queue a remote-panel snapshot rebuild after the current GPUI entity leases end.
    pub(in crate::features) fn defer_remote_panel_snapshot_flush(&self, cx: &mut Context<Self>) {
        self.defer_app_update(cx, |app, cx| {
            app.flush_remote_panel_snapshots(cx);
        });
    }

    /// Push a fresh snapshot to every panel whose pane has moved on.
    ///
    /// **Never called from `render`.** The boundaries are the remote event drains, remote
    /// input subscriptions, a settings apply, `sync_remote_panels_after_activation` for
    /// a session switch, the panel's own refresh clock, and `RemoteMonitorPanel::with_app`
    /// for anything the panel initiates. GPUI runs `flush_effects` when the outermost
    /// `update` finishes, so a mutation and the panel update it causes land in the same
    /// cycle, before any paint.
    ///
    /// Cheap when nothing changed: one `u64` compare per kind.
    pub(in crate::features) fn flush_remote_panel_snapshots(&mut self, cx: &mut Context<Self>) {
        for kind in RemoteMonitorPanel::SNAPSHOT_KINDS {
            let key = self.remote_monitor_key(kind);
            let panel = self.remote_panels.entity(kind).clone();
            if panel.read(cx).snapshot_key() == Some(key) {
                continue;
            }
            let snapshot = self.build_remote_monitor_snapshot(kind, key, cx);
            panel.update(cx, |panel, cx| panel.set_snapshot(snapshot, cx));
        }

        // Freshness, asserted here rather than in render -- a render-time app read would
        // be recorded as a dependency and defeat the isolation this exists to provide.
        // This catches a flush that is itself wrong: a kind missing from the loop, or a
        // snapshot built for the wrong pane. It cannot catch a *missed boundary*, since a
        // boundary never reached never runs it; that is what the freshness tests cover.
        #[cfg(debug_assertions)]
        for kind in RemoteMonitorPanel::SNAPSHOT_KINDS {
            debug_assert_eq!(
                self.remote_panels.entity(kind).read(cx).snapshot_key(),
                Some(self.remote_monitor_key(kind)),
                "flush left a panel behind its pane"
            );
        }
    }

    fn remote_monitor_key(&self, kind: RemoteMonitorKind) -> RemoteMonitorKey {
        let revision = match kind {
            RemoteMonitorKind::Stats => self.remote_ops.stats_revision(),
            RemoteMonitorKind::Gpu => self.remote_ops.gpu_revision(),
            RemoteMonitorKind::Npu => self.remote_ops.npu_revision(),
            RemoteMonitorKind::Processes => self.remote_ops.process_revision(),
            RemoteMonitorKind::Docker => self.remote_ops.docker_revision(),
        };
        RemoteMonitorKey {
            revision,
            chrome: self.panel_chrome(),
            has_session: self.session.active_ssh_config().is_some(),
            panel_width: match kind {
                // Both read the width: the process table for its columns and row heights,
                // Docker for its compose layout.
                RemoteMonitorKind::Processes | RemoteMonitorKind::Docker => {
                    self.shell.right_panel_width()
                }
                _ => 0.,
            },
        }
    }

    fn panel_chrome(&self) -> PanelChrome {
        let palette = self.theme_palette();
        PanelChrome {
            transparent_surface: self.shell_transparent_color(palette.surface),
            surface: self.shell_surface_color(palette.surface),
            transparent_section_header: self.shell_transparent_color(palette.section_header),
            palette,
        }
    }

    fn build_remote_monitor_snapshot(
        &mut self,
        kind: RemoteMonitorKind,
        key: RemoteMonitorKey,
        cx: &mut Context<Self>,
    ) -> RemoteMonitorSnapshot {
        let data = match kind {
            RemoteMonitorKind::Stats => {
                RemoteMonitorData::Stats(self.remote_ops.stats_presentation())
            }
            RemoteMonitorKind::Gpu => {
                let state = self.remote_ops.gpu_presentation();
                let processes = self.remote_ops.derived_gpu_processes();
                // The registry hands back the existing entity after the first call, so
                // the seed only applies when the field is created.
                let search = self.text_input(
                    "remote.gpu.filter",
                    &state.search_draft.clone(),
                    TextInputSetup::placeholder(t!("gpuMonitor.search")),
                    cx,
                );
                RemoteMonitorData::Gpu {
                    state,
                    processes,
                    search,
                }
            }
            RemoteMonitorKind::Npu => {
                let state = self.remote_ops.npu_presentation();
                let processes = self.remote_ops.derived_npu_processes();
                let search = self.text_input(
                    "remote.npu.filter",
                    &state.search_draft.clone(),
                    TextInputSetup::placeholder(t!("ascendNpuMonitor.search")),
                    cx,
                );
                RemoteMonitorData::Npu {
                    state,
                    processes,
                    search,
                }
            }
            RemoteMonitorKind::Processes => {
                let state = self.remote_ops.process_presentation();
                let processes = self.remote_ops.derived_processes();
                let search = self.text_input(
                    "remote.process.filter",
                    &state.search_draft.clone(),
                    TextInputSetup::placeholder(t!("processManager.search")),
                    cx,
                );
                // Keyed per pid and only while something is selected, matching what the
                // view used to build inline.
                let nice = state.selected_pid.map(|pid| {
                    self.number_input(
                        format!("remote.process.{pid}.nice"),
                        &state.nice_draft.clone(),
                        NyaNumberInputOptions::default().range(-20.0, 19.0),
                        cx,
                    )
                });
                RemoteMonitorData::Processes {
                    state,
                    processes,
                    search,
                    nice,
                }
            }
            RemoteMonitorKind::Docker => {
                let state = self.remote_ops.docker_presentation();
                let derived = self.remote_ops.derived_docker_items();
                let effective_tab = self.remote_ops.docker_effective_tab();
                let search = self.text_input(
                    "remote.docker.filter",
                    &state.search_draft.clone(),
                    TextInputSetup::placeholder(t!("dockerManager.search")),
                    cx,
                );
                RemoteMonitorData::Docker(Box::new(DockerSnapshot {
                    state,
                    derived,
                    effective_tab,
                    search,
                }))
            }
        };
        RemoteMonitorSnapshot { key, data }
    }

    /// Refresh one pane if its interval is due and the app is calm enough.
    ///
    /// The two gates are the ones the shell-wide clock applied to all five at once, so
    /// a panel-owned clock still honours them: no session means nothing to poll, and a
    /// deferral means come back next beat rather than submit now.
    pub(in crate::features) fn refresh_remote_monitor_if_due(
        &mut self,
        kind: RemoteMonitorKind,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.session.active_ssh_config().is_none() {
            return false;
        }
        if self.remote_refresh_is_deferred() {
            return false;
        }
        match kind {
            RemoteMonitorKind::Stats => self.refresh_stats_if_due(cx),
            RemoteMonitorKind::Gpu => self.refresh_gpu_if_due(cx),
            RemoteMonitorKind::Npu => self.refresh_npu_if_due(cx),
            RemoteMonitorKind::Processes => self.refresh_processes_if_due(cx),
            RemoteMonitorKind::Docker => self.refresh_docker_if_due(cx),
        }
    }

    /// Whether anything currently wants this metric kept fresh.
    ///
    /// Two sources of demand, not one. A panel being on screen is the obvious one; the
    /// header status bar is the other, and it can be showing stats, GPU or NPU with
    /// that panel closed. Treating only the panel as demand would stop the header
    /// updating, which is the case the "armed on mount, dropped on unmount" sketch of
    /// this design missed -- and the reason the panel entities are owned for the app's
    /// whole life rather than created when shown.
    ///
    /// The settings flag is checked here as well as by `panel_body`, because a panel
    /// switched off in settings renders a placeholder rather than nothing, and a
    /// placeholder must not poll.
    pub(in crate::features) fn remote_monitor_demand(&self, kind: RemoteMonitorKind) -> bool {
        let summary = self.settings.summary();
        let shown = self.panel_is_rendered(kind.nav_item());
        match kind {
            RemoteMonitorKind::Stats => summary.ui_show_remote_stats,
            RemoteMonitorKind::Gpu => {
                (shown || self.header_status_needs_gpu()) && summary.ui_show_gpu_monitor
            }
            RemoteMonitorKind::Npu => {
                (shown || self.header_status_needs_npu()) && summary.ui_show_ascend_npu_monitor
            }
            RemoteMonitorKind::Processes => shown && summary.ui_show_process_manager,
            RemoteMonitorKind::Docker => shown && summary.ui_show_docker_manager,
        }
    }

    /// Start the clocks for panels that are wanted and stop the rest.
    ///
    /// Called from `render`, for the same reason `ensure_idle_lock_clock` is: every
    /// input this reads -- which panels are on screen, what the header is showing,
    /// which panels settings enable, whether a session with an SSH config is active --
    /// changes alongside a repaint, and there is no single event that covers all four.
    /// Enumerating the mutation sites instead would mean a new one silently leaving a
    /// panel unrefreshed -- which is the bug `3904c69b` fixed, where the one call site
    /// ran before any session existed and so never armed anything.
    ///
    /// Cheap enough to run per paint: one settings read, one rendered-panel test per
    /// kind, and five `Option::is_some` compares that do nothing unless demand
    /// actually flipped.
    pub(in crate::features) fn sync_remote_panel_demand(&mut self, cx: &mut Context<Self>) {
        let session_active = self.session.active_ssh_config().is_some();
        for kind in RemoteMonitorKind::ALL {
            let demand = session_active && self.remote_monitor_demand(kind);
            let panel = self.remote_panels.entity(kind).clone();
            panel.update(cx, |panel, cx| panel.set_demand(demand, cx));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gpui::{
        AppContext as _, Entity, IntoElement, ParentElement as _, Render, Styled as _,
        TestAppContext, VisualTestContext, div,
    };
    use nyaterm_core::{AppRuntime, RuntimeMode};

    use super::{RemoteMonitorKind, RemoteMonitorPanel};
    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::test_support::TestConfigDir;

    const ALL_KINDS: [RemoteMonitorKind; 5] = [
        RemoteMonitorKind::Stats,
        RemoteMonitorKind::Gpu,
        RemoteMonitorKind::Npu,
        RemoteMonitorKind::Processes,
        RemoteMonitorKind::Docker,
    ];

    fn app(cx: &mut TestAppContext, root: &Path) -> Entity<NyaTermApp> {
        // A uuid rather than a clock reading: these tests run in parallel and
        // Windows' ~15ms clock granularity lets a nanosecond timestamp repeat,
        // which would share one config dir and so one settings database.
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.to_path_buf(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    struct PanelHost {
        panels: Vec<Entity<RemoteMonitorPanel>>,
    }

    impl Render for PanelHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().size_full().children(
                self.panels
                    .iter()
                    .cloned()
                    .map(IntoElement::into_any_element),
            )
        }
    }

    /// Five instances means five entity ids, which is what makes them isolated views
    /// rather than one view with a mode flag.
    #[test]
    fn every_kind_gets_its_own_entity() {
        let test_dir = TestConfigDir::new("nyaterm-remote-panels");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, _| {
            let mut ids = ALL_KINDS
                .iter()
                .map(|kind| app.remote_panels.entity(*kind).entity_id())
                .collect::<Vec<_>>();
            ids.sort_unstable();
            let distinct = ids.len();
            ids.dedup();
            assert_eq!(ids.len(), distinct, "each panel must be its own entity");
        });
    }

    /// Each panel must paint through its entity.
    ///
    /// The views moved from being built inline during the app's own render to being
    /// child views, which puts them behind GPUI's `with_rendered_view` boundary and
    /// changes the element-id path they live on. This draws all five to confirm that
    /// relocation did not break the ones that register text inputs or read theme
    /// caches the root chrome used to prime first.
    #[test]
    fn all_five_panels_paint_as_child_views() {
        let test_dir = TestConfigDir::new("nyaterm-remote-panels");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| app.sync_component_theme(cx));
        let panels = cx.update_entity(&app, |app, _| {
            ALL_KINDS
                .iter()
                .map(|kind| app.remote_panels.entity(*kind).clone())
                .collect::<Vec<_>>()
        });

        let (_, cx) = cx.add_window_view(move |_, _| PanelHost { panels });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
    }

    struct AppHost {
        app: Entity<NyaTermApp>,
    }

    impl Render for AppHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().size_full().child(self.app.clone())
        }
    }

    /// The shell's panel dispatch must reach the entity.
    ///
    /// The test above renders the panels directly, which would keep passing if
    /// `panel_body` had been left pointing at the old inline views -- so it proves the
    /// entity works, not that anything uses it. This drives the real route:
    /// `render` -> `side_panel_stack` -> `right_panel_body` -> `panel_body`.
    ///
    /// The assertion is the entity's own paint count, not a side effect of the view it
    /// renders: the inline views this replaced register the same search-input ids, so
    /// an id-existence check would pass whether or not `panel_body` was ever switched
    /// over. Counting paints of the entity is the only thing that tells the two apart.
    /// The second half of each pair is what stops it passing vacuously -- it shows the
    /// right side really is rendering, and rendering the *other* panel.
    #[test]
    fn the_shell_panel_dispatch_renders_the_entity() {
        assert_eq!(
            painted_remote_panels(crate::models::NavItem::GpuMonitor),
            (true, false),
            "opening the GPU panel must paint the GPU entity and not the process one"
        );
        assert_eq!(
            painted_remote_panels(crate::models::NavItem::Processes),
            (false, true),
            "and opening the process panel must paint the other one"
        );
    }

    /// Open `panel`, paint the whole app, and report which of the GPU and process
    /// panel entities painted.
    fn painted_remote_panels(panel: crate::models::NavItem) -> (bool, bool) {
        let test_dir = TestConfigDir::new("nyaterm-remote-panels");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            let mut summary = app.settings.summary().clone();
            summary.ui_show_gpu_monitor = true;
            summary.ui_show_process_manager = true;
            app.settings.replace_summary(summary);
            // The real activation path, so this cannot pass by poking a combination of
            // panel fields the UI never produces.
            app.open_or_toggle_panel(panel, cx);
        });

        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |_, _| AppHost { app: host_app });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        // Twice: the first paint is what teaches `shell.viewport` the window size, and
        // the right side is only laid out inline once it is wide enough.
        for _ in 0..2 {
            cx.update(|window, cx| {
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            cx.run_until_parked();
        }

        cx.update(|_, cx| {
            let painted = |kind: RemoteMonitorKind| {
                app.read(cx)
                    .remote_panels
                    .entity(kind)
                    .read(cx)
                    .paint_count()
                    > 0
            };
            (
                painted(RemoteMonitorKind::Gpu),
                painted(RemoteMonitorKind::Processes),
            )
        })
    }

    /// Give the app an active session carrying an SSH config.
    ///
    /// `active_ssh_config` reads the active session's metadata, so this is the whole
    /// requirement. The host is empty because none of these tests advance the clock,
    /// so no refresh is ever submitted and nothing tries to connect.
    fn activate_ssh_session(app: &Entity<NyaTermApp>, cx: &mut TestAppContext) {
        cx.update_entity(app, |app, _| {
            app.session.register_session_metadata(
                "remote-panel-test",
                crate::models::SessionRuntimeMetadata {
                    ssh_config: Some(nyaterm_transport::SshSessionConfig::default()),
                    ssh_multiplex_key: None,
                    source_connection_id: None,
                    ai_execution_profile: nyaterm_core::AiExecutionProfile::Posix,
                    launch_config: crate::models::SessionLaunchConfig::Local(
                        nyaterm_transport::LocalSessionConfig::default(),
                    ),
                    disconnected: false,
                },
            );
            app.session
                .select_active_session("remote-panel-test".to_string());
        });
    }

    fn polling(app: &Entity<NyaTermApp>, cx: &mut TestAppContext) -> Vec<RemoteMonitorKind> {
        cx.update_entity(app, |app, cx| {
            RemoteMonitorKind::ALL
                .into_iter()
                .filter(|kind| app.remote_panels.entity(*kind).read(cx).is_polling())
                .collect()
        })
    }

    /// The state the app spends nearly all of its life in: nothing polls.
    #[test]
    fn nothing_polls_without_a_panel_or_a_session() {
        let test_dir = TestConfigDir::new("nyaterm-remote-panels");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| app.sync_remote_panel_demand(cx));
        assert_eq!(polling(&app, &mut cx), Vec::new());
    }

    /// An open panel polls, and only that one.
    ///
    /// The session is required: demand is gated on an active SSH config, so a panel
    /// open with no session still costs nothing.
    #[test]
    fn an_open_panel_polls_and_its_neighbours_do_not() {
        let test_dir = TestConfigDir::new("nyaterm-remote-panels");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            let mut summary = app.settings.summary().clone();
            summary.ui_show_gpu_monitor = true;
            app.settings.replace_summary(summary);
            app.open_or_toggle_panel(crate::models::NavItem::GpuMonitor, cx);
            app.sync_remote_panel_demand(cx);
        });
        assert_eq!(
            polling(&app, &mut cx),
            Vec::new(),
            "no session means nothing to poll, however many panels are open"
        );

        activate_ssh_session(&app, &mut cx);
        cx.update_entity(&app, |app, cx| app.sync_remote_panel_demand(cx));
        assert_eq!(
            polling(&app, &mut cx),
            vec![RemoteMonitorKind::Stats, RemoteMonitorKind::Gpu]
        );
    }

    /// Closing the panel must drop the clock, not merely mark it unwanted.
    ///
    /// This is the success criterion for the whole extraction: an inactive panel does
    /// not poll. It holds because the `Task` handle *is* the demand -- dropping it
    /// cancels the loop -- so there is no flag that can disagree with reality.
    #[test]
    fn closing_a_panel_drops_its_clock() {
        let test_dir = TestConfigDir::new("nyaterm-remote-panels");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, test_dir.path());
        activate_ssh_session(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            let mut summary = app.settings.summary().clone();
            summary.ui_show_gpu_monitor = true;
            app.settings.replace_summary(summary);
            app.open_or_toggle_panel(crate::models::NavItem::GpuMonitor, cx);
            app.sync_remote_panel_demand(cx);
        });
        assert_eq!(
            polling(&app, &mut cx),
            vec![RemoteMonitorKind::Stats, RemoteMonitorKind::Gpu]
        );

        cx.update_entity(&app, |app, cx| {
            // Toggling the same panel again closes it, which is what the activity bar
            // does.
            app.open_or_toggle_panel(crate::models::NavItem::GpuMonitor, cx);
            app.sync_remote_panel_demand(cx);
        });
        assert_eq!(polling(&app, &mut cx), vec![RemoteMonitorKind::Stats]);
    }

    /// The header status bar keeps stats polling with the Stats panel closed.
    ///
    /// This is the case that decided the design. "Armed on mount, dropped on unmount"
    /// would stop the header updating as soon as its panel closed, because the header
    /// is a second, independent consumer of the same metric. It works because the
    /// entity is owned for the app's whole life and its demand is not "am I visible"
    /// but "does anything want this".
    #[test]
    fn the_header_keeps_a_metric_polling_with_its_panel_closed() {
        let test_dir = TestConfigDir::new("nyaterm-remote-panels");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, test_dir.path());
        activate_ssh_session(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            let mut summary = app.settings.summary().clone();
            summary.ui_header_status_visible = true;
            summary.ui_header_status_mode = crate::models::HeaderStatusMode::Resources
                .persistence_id()
                .to_string();
            app.settings.replace_summary(summary);
            app.sync_remote_panel_demand(cx);
        });
        assert!(
            !cx.update_entity(&app, |app, _| {
                app.panel_is_rendered(crate::models::NavItem::Stats)
            }),
            "the Stats panel must be closed for this test to mean anything"
        );
        assert_eq!(polling(&app, &mut cx), vec![RemoteMonitorKind::Stats]);

        // Switching the header away does not stop Tauri-compatible background stats polling.
        cx.update_entity(&app, |app, cx| {
            let mut summary = app.settings.summary().clone();
            summary.ui_header_status_mode = crate::models::HeaderStatusMode::Session
                .persistence_id()
                .to_string();
            app.settings.replace_summary(summary);
            app.sync_remote_panel_demand(cx);
        });
        assert_eq!(polling(&app, &mut cx), vec![RemoteMonitorKind::Stats]);
    }

    /// A panel switched off in settings renders a placeholder, and a placeholder must
    /// not poll.
    #[test]
    fn a_panel_disabled_in_settings_does_not_poll() {
        let test_dir = TestConfigDir::new("nyaterm-remote-panels");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, test_dir.path());
        activate_ssh_session(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            let mut summary = app.settings.summary().clone();
            summary.ui_show_gpu_monitor = false;
            app.settings.replace_summary(summary);
            app.open_or_toggle_panel(crate::models::NavItem::GpuMonitor, cx);
            app.sync_remote_panel_demand(cx);
        });
        assert_eq!(polling(&app, &mut cx), vec![RemoteMonitorKind::Stats]);
    }

    /// A paint must arm the clock, with nothing calling the reconcile by hand.
    ///
    /// Every other test here calls `sync_remote_panel_demand` directly, and all of them
    /// keep passing with the call removed from `render` -- checked, not assumed. That is
    /// the same gap that let `3904c69b`'s dead clock through: they prove the mechanism and
    /// say nothing about the wiring. This one drives a real paint instead.
    ///
    /// It never advances the clock, so no refresh is submitted and nothing connects;
    /// arming is all that is being asserted.
    #[test]
    fn a_paint_arms_the_clock_for_an_open_panel() {
        let test_dir = TestConfigDir::new("nyaterm-remote-panels");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, test_dir.path());
        activate_ssh_session(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            let mut summary = app.settings.summary().clone();
            summary.ui_show_gpu_monitor = true;
            app.settings.replace_summary(summary);
            app.open_or_toggle_panel(crate::models::NavItem::GpuMonitor, cx);
        });
        assert_eq!(
            polling(&app, &mut cx),
            Vec::new(),
            "nothing has painted yet, so no clock can be armed"
        );

        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |_, _| AppHost { app: host_app });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();

        let armed = cx.update(|_, cx| {
            RemoteMonitorKind::ALL
                .into_iter()
                .filter(|kind| {
                    app.read(cx)
                        .remote_panels
                        .entity(*kind)
                        .read(cx)
                        .is_polling()
                })
                .collect::<Vec<_>>()
        });
        assert_eq!(
            armed,
            vec![RemoteMonitorKind::Stats, RemoteMonitorKind::Gpu],
            "a paint arms persistent stats plus the open GPU panel"
        );
    }
}

#[cfg(test)]
mod isolation_tests {
    use std::path::Path;

    use gpui::{
        AppContext as _, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
        VisualTestContext, div,
    };
    use nyaterm_core::{AiExecutionProfile, AppRuntime, RuntimeMode};
    use nyaterm_transport::{LocalSessionConfig, RemoteGpuOverview, RemoteStats, SshSessionConfig};
    use nyaterm_ui::NyaInputEvent;

    use super::RemoteMonitorKind;
    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::features::runtime_jobs::DockerResource;
    use crate::models::{DockerTab, NavItem, SessionLaunchConfig, SessionRuntimeMetadata};
    use crate::test_support::TestConfigDir;

    fn app(cx: &mut TestAppContext, root: &Path) -> gpui::Entity<NyaTermApp> {
        // A uuid rather than a clock reading: these tests run in parallel and Windows'
        // ~15ms clock granularity lets a nanosecond timestamp repeat, which would share
        // one config dir and so one settings database.
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.to_path_buf(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    struct AppHost {
        app: gpui::Entity<NyaTermApp>,
    }

    impl Render for AppHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().size_full().child(self.app.clone())
        }
    }

    fn paints(app: &gpui::Entity<NyaTermApp>, cx: &gpui::App) -> [usize; 3] {
        let count = |kind| {
            app.read(cx)
                .remote_panels
                .entity(kind)
                .read(cx)
                .paint_count()
        };
        [
            count(RemoteMonitorKind::Stats),
            count(RemoteMonitorKind::Gpu),
            count(RemoteMonitorKind::Npu),
        ]
    }

    /// Open the three panels on an SSH session and paint until things settle.
    fn hosted<'a>(
        cx: &'a mut TestAppContext,
        root: &Path,
    ) -> (gpui::Entity<NyaTermApp>, &'a mut VisualTestContext) {
        let app = app(cx, root);
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            let mut summary = app.settings.summary().clone();
            summary.ui_show_remote_stats = true;
            summary.ui_show_gpu_monitor = true;
            summary.ui_show_ascend_npu_monitor = true;
            app.settings.replace_summary(summary);
            app.session.register_session_metadata(
                "ssh-a",
                SessionRuntimeMetadata {
                    ssh_config: Some(SshSessionConfig::default()),
                    ssh_multiplex_key: None,
                    source_connection_id: None,
                    ai_execution_profile: AiExecutionProfile::Posix,
                    launch_config: SessionLaunchConfig::Local(LocalSessionConfig::default()),
                    disconnected: false,
                },
            );
            app.activate_session_id("ssh-a", cx);
            app.open_or_toggle_panel(NavItem::Stats, cx);
            app.flush_remote_panel_snapshots(cx);
        });
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| AppHost { app: host_app });
        let vcx: &mut VisualTestContext = vcx;
        vcx.run_until_parked();
        for _ in 0..3 {
            vcx.update(|window, cx| {
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
        }
        (app, vcx)
    }

    /// An `app.notify()` that changed nothing in a panel must not re-run its render.
    ///
    /// This is the whole point of the batch, and it is only true because the panel render
    /// makes **zero** `NyaTermApp` accesses: GPUI records every entity read during a
    /// view's render as a dependency of that view, so one app read -- diagnostics included
    /// -- would put the panel in `window.dirty_views` on every notify and the cached
    /// subtree would never be reused.
    ///
    /// It is also the `.cached()` measurement. If caching does not engage under the
    /// current parent sizing, content mask and text style, this fails.
    #[test]
    fn an_unrelated_app_notify_does_not_re_render_the_snapshot_panels() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());

        let before = vcx.update(|_, cx| paints(&app, cx));
        assert!(
            before[0] > 0,
            "the Stats panel must have painted at least once, or this test proves nothing"
        );
        for _ in 0..5 {
            vcx.update(|window, cx| {
                // Nothing about the remote panes changed; this is the shape of a terminal
                // output frame or a status text update.
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
        }
        let after = vcx.update(|_, cx| paints(&app, cx));

        assert_eq!(
            before, after,
            "five unrelated app notifies must not re-render any snapshot panel"
        );
    }

    /// A stats mutation reaches its panel in the same update cycle, with no extra paint.
    ///
    /// The snapshot arrives synchronously: the flush runs inside the same outer `update`
    /// as the mutation, so the revision matches before anything draws. The repaint that
    /// follows is a consequence of the panel's own notify, not a precondition for
    /// freshness.
    #[test]
    fn a_stats_mutation_reaches_its_panel_without_an_extra_root_paint() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());

        let revision = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops.apply_stats(RemoteStats::default());
                app.remote_ops.set_stats_status("loaded stats");
                // The boundary an event drain would use.
                app.flush_remote_panel_snapshots(cx);
                app.remote_ops.stats_revision()
            })
        });

        let delivered = vcx.update(|_, cx| {
            app.read(cx)
                .remote_panels
                .entity(RemoteMonitorKind::Stats)
                .read(cx)
                .snapshot_revision()
        });
        assert_eq!(
            delivered,
            Some(revision),
            "the snapshot must be current before any paint"
        );
    }

    /// A panel interaction must not flush while its own entity is leased by GPUI.
    #[test]
    fn a_panel_interaction_flushes_after_its_lease_is_released() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        let panel = vcx.update(|_, cx| {
            app.read(cx)
                .remote_panels
                .entity(RemoteMonitorKind::Stats)
                .clone()
        });
        let before = vcx.update(|_, cx| panel.read(cx).snapshot_revision());

        vcx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                panel.with_app(cx, |app, _| {
                    app.remote_ops.apply_stats(RemoteStats::default());
                    app.remote_ops.set_stats_status("loaded stats");
                });
                assert_eq!(
                    panel.snapshot_revision(),
                    before,
                    "the snapshot must remain unchanged until the panel lease is released"
                );
            });
        });
        vcx.run_until_parked();

        let after = vcx.update(|_, cx| panel.read(cx).snapshot_revision());
        assert!(
            after > before,
            "the deferred interaction flush must publish the changed stats state"
        );
    }

    /// A GPU mutation must not re-render Stats or NPU.
    #[test]
    fn a_gpu_mutation_does_not_re_render_stats_or_npu() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());

        let before = vcx.update(|_, cx| paints(&app, cx));
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops.apply_gpu(
                    "ssh-a",
                    RemoteGpuOverview {
                        available: true,
                        ..Default::default()
                    },
                );
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        let after = vcx.update(|_, cx| paints(&app, cx));

        assert_eq!(
            (after[0], after[2]),
            (before[0], before[2]),
            "a GPU refresh must leave the Stats and NPU panels alone"
        );
    }

    /// Typing in the GPU search field invalidates the GPU panel and nothing else.
    ///
    /// The panel holds the field's `Entity<NyaInputState>` from its snapshot, so reading
    /// it during render makes the panel a dependent of that entity -- which is the wanted
    /// direction of coupling. No app render is involved.
    #[test]
    fn typing_in_the_gpu_search_invalidates_only_that_panel() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        // The GPU panel has to be the one on screen for its search field to be built.
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_or_toggle_panel(NavItem::GpuMonitor, cx);
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();

        let before = vcx.update(|_, cx| paints(&app, cx));
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                // What the field's subscription does on a keystroke.
                app.remote_ops.apply_gpu_search("py".to_string());
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        let after = vcx.update(|_, cx| paints(&app, cx));

        assert!(
            after[1] > before[1],
            "the GPU panel must re-render for its own search change"
        );
        assert_eq!(
            (after[0], after[2]),
            (before[0], before[2]),
            "and the other two must not"
        );
    }

    /// A remote search subscription must publish its panel snapshot after the input event.
    #[test]
    fn remote_search_input_reaches_its_snapshot_after_the_subscription_runs() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_or_toggle_panel(NavItem::GpuMonitor, cx);
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();

        let field = vcx.update(|_, cx| {
            app.read(cx)
                .existing_text_input("remote.gpu.filter")
                .expect("the GPU panel should own its search field")
        });
        let before = vcx.update(|_, cx| {
            app.read(cx)
                .remote_panels
                .entity(RemoteMonitorKind::Gpu)
                .read(cx)
                .snapshot_revision()
        });

        vcx.update(|_, cx| {
            field.update(cx, |_, cx| {
                cx.emit(NyaInputEvent::Changed("py".to_string()));
            });
            assert_eq!(
                app.read(cx)
                    .remote_panels
                    .entity(RemoteMonitorKind::Gpu)
                    .read(cx)
                    .snapshot_revision(),
                before,
                "the snapshot must remain unchanged until the deferred flush runs"
            );
        });
        vcx.run_until_parked();

        let after = vcx.update(|_, cx| {
            app.read(cx)
                .remote_panels
                .entity(RemoteMonitorKind::Gpu)
                .read(cx)
                .snapshot_revision()
        });
        assert!(
            after > before,
            "the remote search subscription must publish its changed state"
        );
    }

    /// Open the Processes panel on an SSH session and settle.
    fn hosted_processes<'a>(
        cx: &'a mut TestAppContext,
        root: &Path,
    ) -> (gpui::Entity<NyaTermApp>, &'a mut VisualTestContext) {
        let app = app(cx, root);
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            let mut summary = app.settings.summary().clone();
            summary.ui_show_process_manager = true;
            app.settings.replace_summary(summary);
            app.session.register_session_metadata(
                "ssh-a",
                SessionRuntimeMetadata {
                    ssh_config: Some(SshSessionConfig::default()),
                    ssh_multiplex_key: None,
                    source_connection_id: None,
                    ai_execution_profile: AiExecutionProfile::Posix,
                    launch_config: SessionLaunchConfig::Local(LocalSessionConfig::default()),
                    disconnected: false,
                },
            );
            app.activate_session_id("ssh-a", cx);
            app.open_or_toggle_panel(NavItem::Processes, cx);
            app.flush_remote_panel_snapshots(cx);
        });
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| AppHost { app: host_app });
        let vcx: &mut VisualTestContext = vcx;
        vcx.run_until_parked();
        for _ in 0..3 {
            vcx.update(|window, cx| {
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
        }
        (app, vcx)
    }

    fn process_paints(app: &gpui::Entity<NyaTermApp>, cx: &gpui::App) -> usize {
        app.read(cx)
            .remote_panels
            .entity(RemoteMonitorKind::Processes)
            .read(cx)
            .paint_count()
    }

    fn process(pid: u32) -> nyaterm_transport::RemoteProcess {
        nyaterm_transport::RemoteProcess {
            pid,
            ppid: 1,
            user: "user".to_string(),
            state: "S".to_string(),
            cpu_percent: 1.0,
            memory_percent: 2.0,
            rss_kb: 3,
            vsz_kb: 4,
            elapsed: "00:01".to_string(),
            command: "sleep".to_string(),
            command_line: "sleep 10".to_string(),
        }
    }

    /// An `app.notify()` that changed nothing in the process pane must not re-render it.
    #[test]
    fn an_unrelated_app_notify_does_not_re_render_processes() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted_processes(&mut cx, test_dir.path());

        let before = vcx.update(|_, cx| process_paints(&app, cx));
        assert!(
            before > 0,
            "the Processes panel must have painted at least once, or this proves nothing"
        );
        for _ in 0..5 {
            vcx.update(|window, cx| {
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
        }
        assert_eq!(
            vcx.update(|_, cx| process_paints(&app, cx)),
            before,
            "five unrelated app notifies must not re-render the Processes panel"
        );
    }

    /// A process mutation reaches its panel before anything draws.
    #[test]
    fn a_process_mutation_reaches_its_panel_without_an_extra_root_paint() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted_processes(&mut cx, test_dir.path());

        let revision = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops
                    .apply_processes((0..4).map(process).collect());
                app.flush_remote_panel_snapshots(cx);
                app.remote_ops.process_revision()
            })
        });
        assert_eq!(
            vcx.update(|_, cx| {
                app.read(cx)
                    .remote_panels
                    .entity(RemoteMonitorKind::Processes)
                    .read(cx)
                    .snapshot_revision()
            }),
            Some(revision),
            "the snapshot must be current before any paint"
        );
    }

    /// A sibling pane's data must not re-render Processes.
    #[test]
    fn a_stats_mutation_does_not_re_render_processes() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted_processes(&mut cx, test_dir.path());

        let before = vcx.update(|_, cx| process_paints(&app, cx));
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops.apply_stats(RemoteStats::default());
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        assert_eq!(
            vcx.update(|_, cx| process_paints(&app, cx)),
            before,
            "a stats refresh must leave the Processes panel alone"
        );
    }

    /// Search, sort and the nice field each re-render only the Processes panel.
    #[test]
    fn process_interactions_update_only_the_process_panel() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted_processes(&mut cx, test_dir.path());
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops
                    .apply_processes((0..8).map(process).collect());
                app.flush_remote_panel_snapshots(cx);
            });
        });

        for (label, mutate) in [
            (
                "search",
                &(|app: &mut NyaTermApp| app.remote_ops.apply_process_search("sleep".to_string()))
                    as &dyn Fn(&mut NyaTermApp),
            ),
            ("sort", &|app: &mut NyaTermApp| {
                app.remote_ops
                    .toggle_process_sort(crate::models::RemoteProcessSortKey::Pid)
            }),
            ("selection", &|app: &mut NyaTermApp| {
                app.remote_ops.toggle_process_selection(3)
            }),
            ("nice input", &|app: &mut NyaTermApp| {
                app.remote_ops.apply_process_nice_input("-5".to_string())
            }),
        ] {
            let before = vcx.update(|_, cx| (process_paints(&app, cx), paints(&app, cx)));
            vcx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    mutate(app);
                    app.flush_remote_panel_snapshots(cx);
                });
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
            let after = vcx.update(|_, cx| (process_paints(&app, cx), paints(&app, cx)));
            assert!(
                after.0 > before.0,
                "{label} must re-render the Processes panel"
            );
            assert_eq!(
                after.1, before.1,
                "{label} must not re-render Stats, GPU or NPU"
            );
        }
    }

    /// Sorting by memory then narrowing the panel falls back to CPU immediately, and the
    /// snapshot follows -- the width is part of the key, so no pane revision has to move.
    #[test]
    fn narrowing_the_panel_falls_back_to_cpu_sort_in_the_snapshot() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted_processes(&mut cx, test_dir.path());
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops
                    .apply_processes((0..4).map(process).collect());
                app.shell.set_right_panel_width_for_test(700.);
                app.reconcile_remote_process_sort_columns();
                app.remote_ops
                    .toggle_process_sort(crate::models::RemoteProcessSortKey::Memory);
                app.flush_remote_panel_snapshots(cx);
                assert_eq!(
                    app.remote_ops.process_presentation().sort_key,
                    crate::models::RemoteProcessSortKey::Memory
                );
            });
        });

        let before = vcx.update(|_, cx| process_paints(&app, cx));
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                // What a resize drag does, one move at a time.
                app.shell.set_right_panel_width_for_test(360.);
                app.reconcile_remote_process_sort_columns();
                app.flush_remote_panel_snapshots(cx);
                assert_eq!(
                    app.remote_ops.process_presentation().sort_key,
                    crate::models::RemoteProcessSortKey::Cpu,
                    "a narrow panel cannot sort by memory"
                );
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        assert!(
            vcx.update(|_, cx| process_paints(&app, cx)) > before,
            "the narrower width must reach the panel, even though it is not pane state"
        );
    }

    /// The process panel owns a variable-height GPUI list and keeps its item count in
    /// step with snapshots without routing scroll state through `RemoteOpsFeatureState`.
    #[test]
    fn process_list_state_tracks_snapshots_and_clamps_a_shorter_list() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted_processes(&mut cx, test_dir.path());
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops
                    .apply_processes((0..100).map(process).collect());
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let panel = app
                .read(cx)
                .remote_panels
                .entity(RemoteMonitorKind::Processes)
                .read(cx);
            assert_eq!(panel.process_list.item_count(), 100);
            assert!(panel.process_list.max_offset_for_scrollbar().y > gpui::px(0.));
            panel.process_list.scroll_to(gpui::ListOffset {
                item_ix: 60,
                offset_in_item: gpui::px(0.),
            });
        });

        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops
                    .apply_processes((0..3).map(process).collect());
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let panel = app
                .read(cx)
                .remote_panels
                .entity(RemoteMonitorKind::Processes)
                .read(cx);
            assert_eq!(panel.process_list.item_count(), 3);
            assert!(panel.process_list.logical_scroll_top().item_ix < 3);
            assert_eq!(
                panel.process_list.max_offset_for_scrollbar().y,
                gpui::px(0.)
            );
        });
    }

    /// Open the Docker panel on an SSH session, with an overview, and settle.
    fn hosted_docker<'a>(
        cx: &'a mut TestAppContext,
        root: &Path,
    ) -> (gpui::Entity<NyaTermApp>, &'a mut VisualTestContext) {
        let app = app(cx, root);
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            let mut summary = app.settings.summary().clone();
            summary.ui_show_docker_manager = true;
            app.settings.replace_summary(summary);
            app.session.register_session_metadata(
                "ssh-a",
                SessionRuntimeMetadata {
                    ssh_config: Some(SshSessionConfig::default()),
                    ssh_multiplex_key: None,
                    source_connection_id: None,
                    ai_execution_profile: AiExecutionProfile::Posix,
                    launch_config: SessionLaunchConfig::Local(LocalSessionConfig::default()),
                    disconnected: false,
                },
            );
            app.activate_session_id("ssh-a", cx);
            app.open_or_toggle_panel(NavItem::Docker, cx);
            app.remote_ops.apply_docker_overview(docker_overview(3));
            app.flush_remote_panel_snapshots(cx);
        });
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| AppHost { app: host_app });
        let vcx: &mut VisualTestContext = vcx;
        vcx.run_until_parked();
        for _ in 0..3 {
            vcx.update(|window, cx| {
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
        }
        (app, vcx)
    }

    fn docker_overview(containers: usize) -> nyaterm_transport::RemoteDockerOverview {
        nyaterm_transport::RemoteDockerOverview {
            available: true,
            compose_available: true,
            containers: (0..containers)
                .map(|index| nyaterm_transport::DockerContainer {
                    id: format!("c{index}"),
                    name: format!("name{index}"),
                    image: "image".to_string(),
                    status: "Up".to_string(),
                    state: "running".to_string(),
                    ports: String::new(),
                    created_at: String::new(),
                    size: String::new(),
                    stats: None,
                })
                .collect(),
            images: (0..30)
                .map(|index| nyaterm_transport::DockerImage {
                    id: format!("i{index}"),
                    repository: "repo".to_string(),
                    tag: "latest".to_string(),
                    size: String::new(),
                    created_since: String::new(),
                })
                .collect(),
            ..Default::default()
        }
    }

    fn docker_paints(app: &gpui::Entity<NyaTermApp>, cx: &gpui::App) -> usize {
        app.read(cx)
            .remote_panels
            .entity(RemoteMonitorKind::Docker)
            .read(cx)
            .paint_count()
    }

    /// An `app.notify()` that changed nothing in the Docker pane must not re-render it.
    #[test]
    fn an_unrelated_app_notify_does_not_re_render_docker() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted_docker(&mut cx, test_dir.path());

        let before = vcx.update(|_, cx| docker_paints(&app, cx));
        assert!(
            before > 0,
            "the Docker panel must have painted at least once, or this proves nothing"
        );
        for _ in 0..5 {
            vcx.update(|window, cx| {
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
        }
        assert_eq!(
            vcx.update(|_, cx| docker_paints(&app, cx)),
            before,
            "five unrelated app notifies must not re-render the Docker panel"
        );
    }

    /// A Docker mutation reaches its panel before anything draws.
    #[test]
    fn a_docker_mutation_reaches_its_panel_without_an_extra_root_paint() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted_docker(&mut cx, test_dir.path());

        let revision = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops.apply_docker_overview(docker_overview(7));
                app.flush_remote_panel_snapshots(cx);
                app.remote_ops.docker_revision()
            })
        });
        assert_eq!(
            vcx.update(|_, cx| {
                app.read(cx)
                    .remote_panels
                    .entity(RemoteMonitorKind::Docker)
                    .read(cx)
                    .snapshot_revision()
            }),
            Some(revision),
            "the snapshot must be current before any paint"
        );
    }

    /// A sibling pane's data must not re-render Docker.
    #[test]
    fn a_stats_mutation_does_not_re_render_docker() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted_docker(&mut cx, test_dir.path());

        let before = vcx.update(|_, cx| docker_paints(&app, cx));
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops.apply_stats(RemoteStats::default());
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        assert_eq!(
            vcx.update(|_, cx| docker_paints(&app, cx)),
            before,
            "a stats refresh must leave the Docker panel alone"
        );
    }

    /// Search, tab, menus and compose expansion each re-render only Docker.
    #[test]
    fn docker_interactions_update_only_the_docker_panel() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted_docker(&mut cx, test_dir.path());

        for (label, mutate) in [
            (
                "search",
                &(|app: &mut NyaTermApp| app.remote_ops.apply_docker_search("name1".to_string()))
                    as &dyn Fn(&mut NyaTermApp),
            ),
            ("tab", &|app: &mut NyaTermApp| {
                app.remote_ops.set_docker_tab(DockerTab::Images)
            }),
            ("container menu", &|app: &mut NyaTermApp| {
                app.remote_ops
                    .toggle_docker_container_menu("c0".to_string())
            }),
            ("header menu", &|app: &mut NyaTermApp| {
                app.remote_ops.toggle_docker_header_menu()
            }),
            ("compose project", &|app: &mut NyaTermApp| {
                app.remote_ops
                    .toggle_compose_project("k".to_string(), "proj");
            }),
        ] {
            let before = vcx.update(|_, cx| (docker_paints(&app, cx), paints(&app, cx)));
            vcx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    mutate(app);
                    app.flush_remote_panel_snapshots(cx);
                });
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
            let after = vcx.update(|_, cx| (docker_paints(&app, cx), paints(&app, cx)));
            assert!(
                after.0 > before.0,
                "{label} must re-render the Docker panel"
            );
            assert_eq!(
                after.1, before.1,
                "{label} must not re-render Stats, GPU or NPU"
            );
        }
    }

    #[test]
    fn duplicate_docker_image_ids_keep_accessibility_nodes_unique() {
        let test_dir = TestConfigDir::new("nyaterm-docker-image-a11y");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted_docker(&mut cx, test_dir.path());

        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let mut overview = docker_overview(2);
                for (index, image) in overview.images.iter_mut().take(3).enumerate() {
                    image.id = "sha256:shared-image-id".to_string();
                    image.repository = format!("shared-repository-{index}");
                }
                app.remote_ops.apply_docker_overview(overview);
                app.remote_ops.set_docker_tab(DockerTab::Images);
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
    }

    /// The compose fallback survives the move into the snapshot: a host without compose
    /// support shows Containers even with Compose stored as the tab.
    #[test]
    fn the_compose_tab_falls_back_in_the_snapshot() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted_docker(&mut cx, test_dir.path());

        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops.set_docker_tab(DockerTab::Compose);
                app.flush_remote_panel_snapshots(cx);
                assert_eq!(
                    app.remote_ops.docker_effective_tab(),
                    DockerTab::Compose,
                    "the fixture host supports compose"
                );

                // A refresh from a host without compose support.
                let mut overview = docker_overview(2);
                overview.compose_available = false;
                app.remote_ops.apply_docker_overview(overview);
                app.flush_remote_panel_snapshots(cx);
                assert_eq!(
                    app.remote_ops.docker_effective_tab(),
                    DockerTab::Containers,
                    "compose is unavailable, so the effective tab falls back"
                );
            });
        });
    }

    /// Docker's fixed-height virtual lists expose real GPUI scroll handles, and the
    /// handle becomes non-scrollable again when refreshed data fits the viewport.
    #[test]
    fn docker_resource_list_uses_a_native_scroll_handle() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted_docker(&mut cx, test_dir.path());

        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops.apply_docker_overview(docker_overview(2));
                app.remote_ops.apply_docker_resource(DockerResource::Images(
                    (0..100)
                        .map(|index| nyaterm_transport::DockerImage {
                            id: format!("image-{index}"),
                            repository: format!("repo-{index}"),
                            tag: "latest".to_string(),
                            created_since: "now".to_string(),
                            size: "1 MB".to_string(),
                        })
                        .collect(),
                ));
                app.remote_ops.set_docker_tab(DockerTab::Images);
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let panel = app
                .read(cx)
                .remote_panels
                .entity(RemoteMonitorKind::Docker)
                .clone();
            panel.update(cx, |panel, cx| {
                assert!(panel.docker_resource_scroll.is_scrollable());
                panel
                    .docker_resource_scroll
                    .scroll_to_item_strict(60, gpui::ScrollStrategy::Top);
                cx.notify();
            });
        });
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let panel = app
                .read(cx)
                .remote_panels
                .entity(RemoteMonitorKind::Docker)
                .read(cx);
            assert!(
                panel
                    .docker_resource_scroll
                    .0
                    .borrow()
                    .base_handle
                    .offset()
                    .y
                    < gpui::px(0.)
            );
        });

        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops
                    .apply_docker_resource(DockerResource::Images(vec![
                        nyaterm_transport::DockerImage {
                            id: "image-0".to_string(),
                            repository: "repo-0".to_string(),
                            tag: "latest".to_string(),
                            created_since: "now".to_string(),
                            size: "1 MB".to_string(),
                        },
                    ]));
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let panel = app
                .read(cx)
                .remote_panels
                .entity(RemoteMonitorKind::Docker)
                .read(cx);
            assert!(!panel.docker_resource_scroll.is_scrollable());
            assert_eq!(
                panel
                    .docker_resource_scroll
                    .0
                    .borrow()
                    .base_handle
                    .offset()
                    .y,
                gpui::px(0.)
            );
        });
    }
}
