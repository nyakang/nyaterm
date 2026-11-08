//! The Ports and Services panels.
//!
//! Each panel is its own GPUI entity and the single owner of its data, filter,
//! sort, selection and refresh clock. It never reads `NyaTermApp` while
//! rendering: the app pushes the resolved host (this machine or an SSH
//! connection) and the theme at its own boundaries, and everything else a panel
//! needs lives here. Refreshes run on the blocking-job scheduler and come back
//! as typed results, so a slow `sudo` or a hung SSH host never stalls a frame.
//!
//! Filtering and sorting run over the last snapshot on the client, so typing and
//! chips are instant. Elevation is explicit and per panel: the sudo password is
//! held in memory only, passes through stdin, and is dropped on host change.

mod state;
mod view;

use super::panels::PanelChrome;
use crate::blocking_jobs::BlockingJobScheduler;
use crate::features::NyaTermApp;
use gpui::{
    ClipboardItem, Context, Entity, IntoElement, ListAlignment, ListOffset, ListState, Render,
    SharedString, Subscription, Task, WeakEntity, Window, div, prelude::*, px,
};
use nyaterm_core::SecretString;
use nyaterm_remote_ops::host::{
    Elevation, HostOs, HostTarget, looks_like_permission_error, signal_pid,
};
use nyaterm_remote_ops::ports::{
    PortEntry, PortsSnapshot, ProcessDetails, list_ports, process_details,
};
use nyaterm_remote_ops::services::{
    ServiceAction, ServiceBackend, ServiceEntry, ServicesSnapshot, fetch_logs, list_services,
    run_action,
};
use nyaterm_ui::{NyaInputEvent, NyaInputKind, NyaInputState};
use rust_i18n::t;
use state::{PortRow, PortsView, ServicesView, derive_ports, derive_services};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PORTS_INTERVAL: Duration = Duration::from_secs(4);
const SERVICES_INTERVAL: Duration = Duration::from_secs(8);
const CLOCK_TICK: Duration = Duration::from_secs(1);
/// The first refresh after a panel opens should not wait a whole tick.
const FIRST_TICK: Duration = Duration::from_millis(40);
const LOG_LINES: u32 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum HostToolKind {
    Ports,
    Services,
}

impl HostToolKind {
    pub(in crate::features) fn nav_item(self) -> crate::models::NavItem {
        match self {
            Self::Ports => crate::models::NavItem::Ports,
            Self::Services => crate::models::NavItem::Services,
        }
    }

    fn interval(self) -> Duration {
        match self {
            Self::Ports => PORTS_INTERVAL,
            Self::Services => SERVICES_INTERVAL,
        }
    }
}

/// Where the panel's tools run, resolved by the app.
#[derive(Clone)]
pub(in crate::features) struct HostContext {
    /// Identity of the machine: data from one host is never shown for another.
    pub key: String,
    pub label: String,
    pub target: HostTarget,
    pub os: HostOs,
}

// `Ready` is the common case and is cloned rarely (only when the host changes).
#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
pub(in crate::features) enum HostResolution {
    Ready(HostContext),
    Unavailable(String),
}

enum LoadState {
    Idle,
    Loading,
    Ready,
    Failed(SharedString),
}

/// What to retry once the user authorizes elevation.
#[derive(Clone)]
enum PendingAction {
    Service(ServiceEntry, ServiceAction),
    Kill { pid: u32, force: bool },
}

struct LogsDrawer {
    title: SharedString,
    lines: Vec<SharedString>,
    loading: bool,
    entry: ServiceEntry,
}

/// Extra facts about the selected port's owner, fetched on demand.
struct PortDetailsState {
    key: String,
    loading: bool,
    details: ProcessDetails,
}

enum RefreshOutput {
    Ports(PortsSnapshot),
    Services(ServicesSnapshot),
}

pub(in crate::features) struct HostToolsPanel {
    kind: HostToolKind,
    /// Weak: the app owns this panel. Used only to open confirmation dialogs.
    app: WeakEntity<NyaTermApp>,
    scheduler: BlockingJobScheduler,
    chrome: Option<PanelChrome>,
    host: Option<HostResolution>,
    host_signature: String,
    clock: Option<Task<()>>,
    load: LoadState,
    /// Bumped on every submitted job and on host change; stale replies are dropped.
    generation: u64,
    last_refresh: Option<Instant>,

    search: Entity<NyaInputState>,
    query: String,
    /// Rows have variable height: a selected row expands in place.
    list: ListState,
    selected: Option<String>,
    port_details: Option<PortDetailsState>,

    ports: Arc<[PortEntry]>,
    ports_owners_hidden: bool,
    ports_rows: Vec<PortRow>,
    ports_view: PortsView,

    services: Arc<[ServiceEntry]>,
    services_backend: Option<ServiceBackend>,
    services_partial: bool,
    services_rows: Vec<ServiceEntry>,
    services_view: ServicesView,

    /// One-line result of the last action, shown under the toolbar.
    status: Option<(SharedString, bool)>,
    busy: Option<SharedString>,
    logs: Option<LogsDrawer>,

    elevated: bool,
    sudo_password: Option<SecretString>,
    sudo_prompt: Option<SharedString>,
    sudo_input: Entity<NyaInputState>,
    pending_action: Option<PendingAction>,

    _subscriptions: Vec<Subscription>,
}

impl HostToolsPanel {
    pub(in crate::features) fn new(
        kind: HostToolKind,
        app: WeakEntity<NyaTermApp>,
        scheduler: BlockingJobScheduler,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = match kind {
            HostToolKind::Ports => t!("hostTools.searchPorts"),
            HostToolKind::Services => t!("hostTools.searchServices"),
        }
        .to_string();
        let search = cx.new(|cx| NyaInputState::new(cx, "").placeholder(placeholder));
        let sudo_input = cx.new(|cx| {
            NyaInputState::new(cx, "")
                .placeholder(t!("hostTools.sudoPassword").to_string())
                .kind(NyaInputKind::Secret)
                .masked(true)
                .submit_on_enter(true)
        });
        let search_subscription = cx.subscribe(&search, |this, _, event, cx| {
            if let NyaInputEvent::Changed(text) | NyaInputEvent::Submitted(text) = event {
                this.set_query(text.clone(), cx);
            }
        });
        let sudo_subscription = cx.subscribe(&sudo_input, |this, _, event, cx| {
            if let NyaInputEvent::Submitted(_) = event {
                this.submit_sudo(cx);
            }
        });
        Self {
            kind,
            app,
            scheduler,
            chrome: None,
            host: None,
            host_signature: String::new(),
            clock: None,
            load: LoadState::Idle,
            generation: 0,
            last_refresh: None,
            search,
            query: String::new(),
            list: ListState::new(0, ListAlignment::Top, px(256.)),
            selected: None,
            port_details: None,
            ports: Arc::from(Vec::new()),
            ports_owners_hidden: false,
            ports_rows: Vec::new(),
            ports_view: PortsView::default(),
            services: Arc::from(Vec::new()),
            services_backend: None,
            services_partial: false,
            services_rows: Vec::new(),
            services_view: ServicesView::default(),
            status: None,
            busy: None,
            logs: None,
            elevated: false,
            sudo_password: None,
            sudo_prompt: None,
            sudo_input,
            pending_action: None,
            _subscriptions: vec![search_subscription, sudo_subscription],
        }
    }

    // ------------------------------------------------------ app boundary --

    #[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
    pub(in crate::features) fn loaded_port_count(&self) -> usize {
        self.ports.len()
    }

    #[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
    pub(in crate::features) fn loaded_service_count(&self) -> usize {
        self.services.len()
    }

    pub(in crate::features) fn host_ready(&self) -> bool {
        matches!(self.host, Some(HostResolution::Ready(_)))
    }

    pub(in crate::features) fn signature(&self) -> &str {
        &self.host_signature
    }

    pub(in crate::features) fn set_chrome(&mut self, chrome: PanelChrome, cx: &mut Context<Self>) {
        if self.chrome != Some(chrome) {
            self.chrome = Some(chrome);
            cx.notify();
        }
    }

    /// Install the host the app resolved. Data and elevation belong to a host,
    /// so a different one starts from a clean slate.
    pub(in crate::features) fn set_host(
        &mut self,
        signature: String,
        host: HostResolution,
        cx: &mut Context<Self>,
    ) {
        let new_key = match &host {
            HostResolution::Ready(context) => Some(context.key.clone()),
            HostResolution::Unavailable(_) => None,
        };
        let old_key = match &self.host {
            Some(HostResolution::Ready(context)) => Some(context.key.clone()),
            _ => None,
        };
        self.host = Some(host);
        self.host_signature = signature;
        if new_key != old_key {
            self.reset_for_new_host();
        }
        cx.notify();
    }

    fn reset_for_new_host(&mut self) {
        self.generation += 1;
        self.load = LoadState::Idle;
        self.last_refresh = None;
        self.ports = Arc::from(Vec::new());
        self.ports_rows.clear();
        self.list.reset(0);
        self.ports_owners_hidden = false;
        self.services = Arc::from(Vec::new());
        self.services_rows.clear();
        self.services_backend = None;
        self.selected = None;
        self.port_details = None;
        self.status = None;
        self.busy = None;
        self.logs = None;
        self.elevated = false;
        self.sudo_password = None;
        self.sudo_prompt = None;
        self.pending_action = None;
    }

    /// Polling exists only while the panel is visible and has a host. Dropping
    /// the task cancels it, so an idle panel costs nothing.
    pub(in crate::features) fn set_demand(&mut self, demand: bool, cx: &mut Context<Self>) {
        if demand == self.clock.is_some() {
            return;
        }
        if !demand {
            self.clock = None;
            return;
        }
        // Refresh promptly when (re)opened instead of showing stale data.
        self.last_refresh = None;
        self.clock = Some(cx.spawn(async move |this, cx| {
            let mut delay = FIRST_TICK;
            loop {
                cx.background_executor().timer(delay).await;
                delay = CLOCK_TICK;
                if this.update(cx, |panel, cx| panel.tick(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    #[cfg(test)]
    pub(in crate::features) fn is_polling(&self) -> bool {
        self.clock.is_some()
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        if matches!(self.load, LoadState::Loading) || self.busy.is_some() {
            return;
        }
        let due = self
            .last_refresh
            .is_none_or(|at| at.elapsed() >= self.kind.interval());
        if due {
            self.refresh(cx);
        }
    }

    // --------------------------------------------------------- derivation --

    fn set_query(&mut self, text: String, cx: &mut Context<Self>) {
        if self.query == text {
            return;
        }
        self.query = text;
        self.rederive();
        self.list.scroll_to(ListOffset::default());
        cx.notify();
    }

    fn rederive(&mut self) {
        // Polling refreshes usually change nothing; skip the list reset then so
        // an unchanged panel never repaints or loses its scroll position.
        let (changed, count) = match self.kind {
            HostToolKind::Ports => {
                let rows = derive_ports(&self.ports, &self.query, self.ports_view);
                let changed = rows != self.ports_rows;
                self.ports_rows = rows;
                (changed, self.ports_rows.len())
            }
            HostToolKind::Services => {
                let rows = derive_services(&self.services, &self.query, self.services_view);
                let changed = rows != self.services_rows;
                self.services_rows = rows;
                (changed, self.services_rows.len())
            }
        };
        if changed || self.list.item_count() != count {
            let top = self.list.logical_scroll_top();
            self.list.reset(count);
            self.list.scroll_to(ListOffset {
                item_ix: top.item_ix.min(count.saturating_sub(1)),
                offset_in_item: px(0.),
            });
        }
        if let Some(selected) = &self.selected {
            let still_visible = match self.kind {
                HostToolKind::Ports => self.ports_rows.iter().any(|e| &e.key() == selected),
                HostToolKind::Services => self.services_rows.iter().any(|e| &e.name == selected),
            };
            if !still_visible {
                self.selected = None;
                self.port_details = None;
            }
        }
    }

    /// The selected row's height changed (expanded, details arrived, logs).
    fn remeasure_list(&self) {
        self.list.remeasure();
    }

    fn selected_index(&self) -> Option<usize> {
        let selected = self.selected.as_deref()?;
        match self.kind {
            HostToolKind::Ports => self.ports_rows.iter().position(|r| r.key() == selected),
            HostToolKind::Services => self.services_rows.iter().position(|r| r.name == selected),
        }
    }

    // ------------------------------------------------------------ refresh --

    fn elevation(&self) -> Elevation {
        match (&self.elevated, &self.sudo_password) {
            (false, _) => Elevation::None,
            (true, Some(password)) => Elevation::SudoPassword(password.clone()),
            (true, None) => Elevation::SudoPasswordless,
        }
    }

    fn context(&self) -> Option<HostContext> {
        match &self.host {
            Some(HostResolution::Ready(context)) => Some(context.clone()),
            _ => None,
        }
    }

    pub(in crate::features) fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(context) = self.context() else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        self.last_refresh = Some(Instant::now());
        if !matches!(self.load, LoadState::Ready) {
            self.load = LoadState::Loading;
        }
        let kind = self.kind;
        let elevation = self.elevation();
        let task = self.scheduler.submit_task("host-tools-refresh", move |_| {
            match kind {
                HostToolKind::Ports => {
                    list_ports(&context.target, context.os, elevation).map(RefreshOutput::Ports)
                }
                HostToolKind::Services => list_services(&context.target, context.os, elevation)
                    .map(RefreshOutput::Services),
            }
            .map_err(|error| error.to_string())
        });
        let task = match task {
            Ok(task) => task,
            Err(error) => {
                self.fail_refresh(error.to_string(), cx);
                return;
            }
        };
        cx.spawn(async move |this, cx| {
            let result = match task.await {
                Ok(result) => result,
                Err(error) => Err(error.to_string()),
            };
            let _ = this.update(cx, |panel, cx| panel.apply_refresh(generation, result, cx));
        })
        .detach();
        cx.notify();
    }

    fn apply_refresh(
        &mut self,
        generation: u64,
        result: Result<RefreshOutput, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        match result {
            Ok(RefreshOutput::Ports(snapshot)) => {
                self.ports = snapshot.entries.into();
                self.ports_owners_hidden = snapshot.owners_hidden;
            }
            Ok(RefreshOutput::Services(snapshot)) => {
                self.services_backend = Some(snapshot.backend);
                self.services_partial = snapshot.partial;
                self.services = snapshot.entries.into();
            }
            Err(message) => {
                self.fail_refresh(message, cx);
                return;
            }
        }
        self.load = LoadState::Ready;
        self.rederive();
        cx.notify();
    }

    fn fail_refresh(&mut self, message: String, cx: &mut Context<Self>) {
        if self.elevated && self.sudo_password.is_none() && needs_password(&message) {
            // `sudo -n` was refused: ask once instead of failing silently.
            self.sudo_prompt = Some(t!("hostTools.sudoNeeded").to_string().into());
        } else if self.elevated && self.sudo_password.is_some() && needs_password(&message) {
            // The stored password was wrong or expired.
            self.sudo_password = None;
            self.sudo_prompt = Some(t!("hostTools.sudoWrong").to_string().into());
        }
        // Keep showing the last good data; only an empty panel shows the error.
        if matches!(self.load, LoadState::Ready) {
            self.status = Some((message.into(), true));
        } else {
            self.load = LoadState::Failed(message.into());
        }
        cx.notify();
    }

    // ---------------------------------------------------------- elevation --

    pub(in crate::features) fn supports_elevation(&self) -> bool {
        self.context().is_some_and(|context| context.os.is_unix())
    }

    fn toggle_elevation(&mut self, cx: &mut Context<Self>) {
        if self.elevated {
            self.elevated = false;
            self.sudo_password = None;
            self.sudo_prompt = None;
            self.pending_action = None;
        } else {
            self.elevated = true;
            self.sudo_prompt = None;
        }
        self.status = None;
        self.refresh(cx);
    }

    fn submit_sudo(&mut self, cx: &mut Context<Self>) {
        let value = self.sudo_input.read(cx).value(cx);
        if value.is_empty() {
            return;
        }
        self.sudo_input.update(cx, |input, cx| input.clear(cx));
        self.sudo_password = Some(SecretString::new(value));
        self.sudo_prompt = None;
        self.elevated = true;
        if let Some(action) = self.pending_action.take() {
            self.run_pending(action, cx);
        } else {
            self.refresh(cx);
        }
    }

    fn cancel_sudo(&mut self, cx: &mut Context<Self>) {
        self.sudo_prompt = None;
        self.pending_action = None;
        if self.sudo_password.is_none() {
            self.elevated = false;
        }
        self.sudo_input.update(cx, |input, cx| input.clear(cx));
        cx.notify();
    }

    // ------------------------------------------------------------ actions --

    fn run_pending(&mut self, action: PendingAction, cx: &mut Context<Self>) {
        match action {
            PendingAction::Service(entry, action) => self.service_action(entry, action, cx),
            PendingAction::Kill { pid, force } => self.kill_pid(pid, force, cx),
        }
    }

    fn start_busy(&mut self, label: String, cx: &mut Context<Self>) {
        self.busy = Some(label.into());
        self.status = None;
        cx.notify();
    }

    fn finish_action(
        &mut self,
        generation: u64,
        success_label: String,
        retry: PendingAction,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        self.busy = None;
        match result {
            Ok(()) => {
                self.status = Some((success_label.into(), false));
                self.refresh(cx);
            }
            Err(message) => {
                let can_elevate = self.supports_elevation();
                if can_elevate && looks_like_permission_error(&message) {
                    // Offer elevation and retry the same action on success.
                    self.elevated = true;
                    self.sudo_password = None;
                    self.pending_action = Some(retry);
                    self.sudo_prompt = Some(
                        t!("hostTools.sudoNeededFor", reason = first_line(&message))
                            .to_string()
                            .into(),
                    );
                } else {
                    self.status = Some((message.into(), true));
                }
                cx.notify();
            }
        }
    }

    fn service_action(
        &mut self,
        entry: ServiceEntry,
        action: ServiceAction,
        cx: &mut Context<Self>,
    ) {
        let (Some(context), Some(backend)) = (self.context(), self.services_backend) else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        self.start_busy(
            format!("{} {}…", t!(action_label_key(action)), entry.name),
            cx,
        );
        let elevation = self.elevation();
        let job_entry = entry.clone();
        let task = self
            .scheduler
            .submit_task("host-tools-service-action", move |_| {
                run_action(
                    &context.target,
                    context.os,
                    backend,
                    &job_entry,
                    action,
                    elevation,
                )
                .map_err(|error| error.to_string())
            });
        let label = format!("{} {}", t!(action_done_key(action)), entry.name);
        let retry = PendingAction::Service(entry, action);
        self.await_action(task, generation, label, retry, cx);
    }

    fn kill_pid(&mut self, pid: u32, force: bool, cx: &mut Context<Self>) {
        let Some(context) = self.context() else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        self.start_busy(t!("hostTools.stopping", pid = pid).to_string(), cx);
        let elevation = self.elevation();
        let task = self.scheduler.submit_task("host-tools-kill", move |_| {
            signal_pid(&context.target, context.os, pid, force, elevation)
                .map_err(|error| error.to_string())
        });
        let label = t!("hostTools.stopped", pid = pid).to_string();
        self.await_action(
            task,
            generation,
            label,
            PendingAction::Kill { pid, force },
            cx,
        );
    }

    fn await_action(
        &mut self,
        task: Result<
            crate::blocking_jobs::JobTask<Result<(), String>>,
            crate::blocking_jobs::JobRejected,
        >,
        generation: u64,
        label: String,
        retry: PendingAction,
        cx: &mut Context<Self>,
    ) {
        let task = match task {
            Ok(task) => task,
            Err(error) => {
                self.busy = None;
                self.status = Some((error.to_string().into(), true));
                cx.notify();
                return;
            }
        };
        cx.spawn(async move |this, cx| {
            let result = match task.await {
                Ok(result) => result,
                Err(error) => Err(error.to_string()),
            };
            let _ = this.update(cx, |panel, cx| {
                panel.finish_action(generation, label, retry, result, cx)
            });
        })
        .detach();
    }

    fn open_logs(&mut self, entry: ServiceEntry, cx: &mut Context<Self>) {
        let (Some(context), Some(backend)) = (self.context(), self.services_backend) else {
            return;
        };
        self.logs = Some(LogsDrawer {
            title: entry.name.clone().into(),
            lines: Vec::new(),
            loading: true,
            entry: entry.clone(),
        });
        self.remeasure_list();
        let elevation = self.elevation();
        let name = entry.name.clone();
        let task = self.scheduler.submit_task("host-tools-logs", move |_| {
            fetch_logs(
                &context.target,
                context.os,
                backend,
                &entry,
                LOG_LINES,
                elevation,
            )
            .map_err(|error| error.to_string())
        });
        let Ok(task) = task else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = match task.await {
                Ok(result) => result,
                Err(error) => Err(error.to_string()),
            };
            let _ = this.update(cx, |panel, cx| {
                let Some(drawer) = panel.logs.as_mut().filter(|d| d.entry.name == name) else {
                    return;
                };
                drawer.loading = false;
                let text = result.unwrap_or_else(|error| error);
                drawer.lines = text
                    .lines()
                    .map(|line| SharedString::from(line.to_string()))
                    .collect();
                if drawer.lines.is_empty() {
                    drawer.lines = vec![t!("hostTools.noLogs").to_string().into()];
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn copy(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.status = Some((t!("hostTools.copied").to_string().into(), false));
        cx.notify();
    }

    fn select(&mut self, key: String, cx: &mut Context<Self>) {
        self.selected = if self.selected.as_deref() == Some(key.as_str()) {
            None
        } else {
            Some(key)
        };
        self.port_details = None;
        if self.kind == HostToolKind::Ports
            && let Some(selected) = self.selected.clone()
            && let Some(row) = self
                .ports_rows
                .iter()
                .find(|r| r.key() == selected)
                .cloned()
        {
            self.load_port_details(&row, cx);
        }
        self.remeasure_list();
        if let Some(ix) = self.selected_index() {
            self.list.scroll_to_reveal_item(ix);
        }
        cx.notify();
    }

    fn selected_port(&self) -> Option<&PortRow> {
        let selected = self.selected.as_deref()?;
        self.ports_rows.iter().find(|row| row.key() == selected)
    }

    fn selected_service(&self) -> Option<&ServiceEntry> {
        let selected = self.selected.as_deref()?;
        self.services_rows.iter().find(|row| row.name == selected)
    }

    /// The list stays cheap by not asking for the owner's user and command line;
    /// they are fetched here, for the one row the user opened.
    fn load_port_details(&mut self, row: &PortRow, cx: &mut Context<Self>) {
        let (Some(context), Some(pid)) = (self.context(), row.pid) else {
            return;
        };
        let key = row.key();
        self.port_details = Some(PortDetailsState {
            key: key.clone(),
            loading: true,
            details: ProcessDetails::default(),
        });
        let elevation = self.elevation();
        let task = self
            .scheduler
            .submit_task("host-tools-port-details", move |_| {
                process_details(&context.target, context.os, pid, elevation)
                    .map_err(|error| error.to_string())
            });
        let Ok(task) = task else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = match task.await {
                Ok(result) => result,
                Err(error) => Err(error.to_string()),
            };
            let _ = this.update(cx, |panel, cx| {
                let Some(state) = panel.port_details.as_mut().filter(|s| s.key == key) else {
                    return;
                };
                state.loading = false;
                state.details = result.unwrap_or_default();
                panel.remeasure_list();
                cx.notify();
            });
        })
        .detach();
    }
}

fn needs_password(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("password")
        || lower.contains("try again")
        || lower.contains("authentication")
        || looks_like_permission_error(message)
}

fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or(message).trim().to_string()
}

fn action_label_key(action: ServiceAction) -> &'static str {
    match action {
        ServiceAction::Start => "hostTools.starting",
        ServiceAction::Stop => "hostTools.stoppingService",
        ServiceAction::Restart => "hostTools.restarting",
        ServiceAction::Enable => "hostTools.enabling",
        ServiceAction::Disable => "hostTools.disabling",
    }
}

fn action_done_key(action: ServiceAction) -> &'static str {
    match action {
        ServiceAction::Start => "hostTools.started",
        ServiceAction::Stop => "hostTools.stoppedService",
        ServiceAction::Restart => "hostTools.restarted",
        ServiceAction::Enable => "hostTools.enabled",
        ServiceAction::Disable => "hostTools.disabled",
    }
}

impl Render for HostToolsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(chrome) = self.chrome else {
            // The app pushes the theme before the panel is ever shown.
            return div().into_any_element();
        };
        match self.kind {
            HostToolKind::Ports => view::ports_panel(self, chrome, window, cx),
            HostToolKind::Services => view::services_panel(self, chrome, window, cx),
        }
    }
}
