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

mod executor;
mod jobs;
mod state;
#[cfg(test)]
mod tests;
mod view;

use super::panels::PanelChrome;
use crate::blocking_jobs::{BlockingJobScheduler, JobCancellationHandle};
use crate::features::NyaTermApp;
use executor::{HostToolsExecutor, NativeExecutor, Output, Request};
use gpui::{
    ClipboardItem, Context, Entity, IntoElement, ListAlignment, ListOffset, ListState, Render,
    SharedString, Subscription, Task, WeakEntity, Window, div, prelude::*, px,
};
use jobs::{Jobs, Lane, Ticket};
use nyaterm_core::SecretString;
use nyaterm_remote_ops::host::{Elevation, HostOs, HostTarget, looks_like_permission_error};
use nyaterm_remote_ops::ports::{PortEntry, PortsSnapshot, ProcessDetails};
use nyaterm_remote_ops::services::{ServiceAction, ServiceBackend, ServiceEntry, ServicesSnapshot};
use nyaterm_ui::{NyaInputEvent, NyaInputKind, NyaInputState};
use rust_i18n::t;
use state::{PortRow, PortsView, ServicesView, derive_ports, derive_services};
use std::collections::HashMap;
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
    entry: ServiceEntry,
}

/// Extra facts about the selected port's owner, fetched on demand.
struct PortDetailsState {
    key: String,
    details: ProcessDetails,
    error: Option<SharedString>,
}

enum RefreshOutput {
    Ports(PortsSnapshot),
    Services(ServicesSnapshot),
}

struct ActionPresentation {
    label: SharedString,
    done: String,
    retry: PendingAction,
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
    jobs: Jobs,
    cancellations: HashMap<Ticket, JobCancellationHandle>,
    executor: Arc<dyn HostToolsExecutor>,
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
    action: Option<ActionPresentation>,
    logs: Option<LogsDrawer>,

    elevated: bool,
    sudo_password: Option<SecretString>,
    sudo_prompt: Option<SharedString>,
    sudo_input: Entity<NyaInputState>,
    pending_action: Option<(u64, PendingAction)>,

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
            jobs: Jobs::default(),
            cancellations: HashMap::new(),
            executor: Arc::new(NativeExecutor),
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
            action: None,
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

    pub(in crate::features) fn host_epoch(&self) -> u64 {
        self.jobs.epoch
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
        let changed = new_key != old_key || self.host_signature != signature;
        self.host = Some(host);
        self.host_signature = signature;
        if changed {
            self.reset_for_new_host(cx);
        }
        cx.notify();
    }

    pub(in crate::features) fn invalidate_connection(&mut self, cx: &mut Context<Self>) {
        self.reset_for_new_host(cx);
    }

    fn reset_for_new_host(&mut self, cx: &mut Context<Self>) {
        for handle in self.cancellations.drain().map(|(_, handle)| handle) {
            handle.cancel();
        }
        self.jobs.reset();
        self.clear_authorization(cx);
        self.load = LoadState::Idle;
        self.last_refresh = None;
        self.ports = Arc::from(Vec::new());
        self.ports_rows.clear();
        self.list.reset(0);
        self.ports_owners_hidden = false;
        self.services = Arc::from(Vec::new());
        self.services_rows.clear();
        self.services_backend = None;
        self.services_partial = false;
        self.selected = None;
        self.port_details = None;
        self.status = None;
        self.action = None;
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
            self.clear_authorization(cx);
            self.jobs.refresh_requested = false;
            for lane in [Lane::Refresh, Lane::Logs, Lane::Details] {
                self.cancel_lane(lane);
            }
            cx.notify();
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
        if !self.can_refresh() {
            return;
        }
        if self.jobs.pending(Lane::Refresh)
            || self.jobs.pending(Lane::Action)
            || self.sudo_prompt.is_some()
        {
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
                HostToolKind::Services => self.services_rows.iter().any(|e| &e.key() == selected),
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
            HostToolKind::Services => self.services_rows.iter().position(|r| r.key() == selected),
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

    pub(in crate::features) fn can_refresh(&self) -> bool {
        self.clock.is_some() && self.host_ready()
    }

    fn busy_label(&self) -> Option<&SharedString> {
        self.jobs
            .pending(Lane::Action)
            .then(|| self.action.as_ref().map(|action| &action.label))
            .flatten()
    }

    fn cancel_lane(&mut self, lane: Lane) {
        if let Some(ticket) = self.jobs.ticket(lane)
            && let Some(handle) = self.cancellations.get(&ticket)
        {
            handle.cancel();
        }
    }

    fn submit_job(&mut self, lane: Lane, key: String, request: Request, cx: &mut Context<Self>) {
        if lane != Lane::Action && !self.can_refresh() {
            return;
        }
        let Some(context) = self.context() else {
            return;
        };
        self.cancel_lane(lane);
        let ticket = self.jobs.start(lane, key.clone());
        let executor = self.executor.clone();
        let elevation = self.elevation();
        let task = self.scheduler.submit_task("host-tools", move |_| {
            executor.execute(&context, request, elevation)
        });
        let task = match task {
            Ok(task) => task,
            Err(error) => {
                self.apply_job(lane, ticket, &key, Err(error.to_string()), cx);
                return;
            }
        };
        self.cancellations
            .insert(ticket, task.cancellation_handle());
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|error| Err(error.to_string()));
            let _ = this.update(cx, |panel, cx| {
                panel.apply_job(lane, ticket, &key, result, cx)
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.can_refresh() {
            return;
        }
        if self.jobs.pending(Lane::Refresh) || self.jobs.pending(Lane::Action) {
            self.jobs.refresh_requested = true;
            return;
        }
        self.jobs.refresh_requested = false;
        self.last_refresh = Some(Instant::now());
        self.submit_job(
            Lane::Refresh,
            String::new(),
            Request::Refresh(self.kind),
            cx,
        );
    }

    fn drain_requested_refresh(&mut self, cx: &mut Context<Self>) {
        if self.clock.is_some()
            && self.jobs.refresh_requested
            && !self.jobs.pending(Lane::Refresh)
            && !self.jobs.pending(Lane::Action)
            && self.sudo_prompt.is_none()
        {
            self.refresh(cx);
        }
    }

    fn apply_job(
        &mut self,
        lane: Lane,
        ticket: Ticket,
        key: &str,
        result: Result<Output, String>,
        cx: &mut Context<Self>,
    ) {
        let cancelled = self
            .cancellations
            .remove(&ticket)
            .is_some_and(|handle| handle.is_cancelled());
        if !self.jobs.finish(lane, ticket, key) {
            return;
        }
        if cancelled && lane != Lane::Action {
            self.drain_requested_refresh(cx);
            cx.notify();
            return;
        }
        if matches!(lane, Lane::Logs | Lane::Details)
            && let Err(message) = &result
            && needs_password(message)
        {
            self.sudo_password = None;
            self.discard_sudo_input(cx);
            if self.clock.is_some() && self.supports_elevation() {
                self.sudo_prompt = Some(t!("hostTools.sudoNeeded").to_string().into());
            }
        }
        match lane {
            Lane::Refresh => match result {
                Ok(Output::Refresh(RefreshOutput::Ports(snapshot))) => {
                    self.ports = snapshot.entries.into();
                    self.ports_owners_hidden = snapshot.owners_hidden;
                    self.load = LoadState::Ready;
                    self.rederive();
                }
                Ok(Output::Refresh(RefreshOutput::Services(snapshot))) => {
                    self.services = snapshot.entries.into();
                    self.services_backend = Some(snapshot.backend);
                    self.services_partial = snapshot.partial;
                    self.load = LoadState::Ready;
                    self.rederive();
                }
                Err(message) => self.fail_refresh(message, cx),
                _ => self.fail_refresh("unexpected host-tools result".into(), cx),
            },
            Lane::Action => {
                let Some(action) = self.action.take() else {
                    return;
                };
                match result {
                    Ok(Output::Action) => {
                        self.status = Some((action.done.into(), false));
                        self.jobs.refresh_requested = true;
                    }
                    Err(message)
                        if self.clock.is_some()
                            && self.supports_elevation()
                            && needs_password(&message) =>
                    {
                        self.discard_sudo_input(cx);
                        self.elevated = true;
                        self.sudo_password = None;
                        self.pending_action = Some((ticket.epoch, action.retry));
                        self.sudo_prompt = Some(
                            t!("hostTools.sudoNeededFor", reason = first_line(&message))
                                .to_string()
                                .into(),
                        );
                    }
                    Err(message) => self.status = Some((message.into(), true)),
                    _ => self.status = Some(("unexpected host-tools result".into(), true)),
                }
            }
            Lane::Logs => {
                if let Some(drawer) = self
                    .logs
                    .as_mut()
                    .filter(|drawer| drawer.entry.key() == key)
                {
                    let text = match result {
                        Ok(Output::Logs(text)) => text,
                        Err(error) => error,
                        _ => "unexpected log result".into(),
                    };
                    drawer.lines = text
                        .lines()
                        .map(|line| SharedString::from(line.to_string()))
                        .collect();
                    if drawer.lines.is_empty() {
                        drawer.lines = vec![t!("hostTools.noLogs").to_string().into()];
                    }
                    self.remeasure_list();
                }
            }
            Lane::Details => {
                if let Some(state) = self.port_details.as_mut().filter(|state| state.key == key) {
                    match result {
                        Ok(Output::Details(details)) => {
                            state.details = details;
                            state.error = None;
                        }
                        Err(error) => state.error = Some(error.into()),
                        _ => state.error = Some("unexpected process details result".into()),
                    }
                    self.remeasure_list();
                }
            }
        }
        self.drain_requested_refresh(cx);
        cx.notify();
    }

    fn fail_refresh(&mut self, message: String, cx: &mut Context<Self>) {
        if self.clock.is_some() && needs_password(&message) && self.supports_elevation() {
            self.discard_sudo_input(cx);
            let wrong = self.sudo_password.is_some();
            self.sudo_password = None;
            self.sudo_prompt = Some(
                t!(if wrong {
                    "hostTools.sudoWrong"
                } else {
                    "hostTools.sudoNeeded"
                })
                .to_string()
                .into(),
            );
        }
        if matches!(self.load, LoadState::Ready) {
            self.status = Some((message.into(), true));
        } else {
            self.load = LoadState::Failed(message.into());
        }
        cx.notify();
    }

    pub(in crate::features) fn supports_elevation(&self) -> bool {
        self.context().is_some_and(|context| context.os.is_unix())
    }

    fn discard_sudo_input(&mut self, cx: &mut Context<Self>) {
        self.sudo_input
            .update(cx, |input, cx| input.discard_content(cx));
    }

    fn clear_authorization(&mut self, cx: &mut Context<Self>) {
        self.elevated = false;
        self.sudo_password = None;
        self.sudo_prompt = None;
        self.pending_action = None;
        self.discard_sudo_input(cx);
    }

    fn toggle_elevation(&mut self, cx: &mut Context<Self>) {
        if self.jobs.pending(Lane::Action) {
            return;
        }
        if self.elevated {
            self.clear_authorization(cx);
        } else {
            self.elevated = true;
            self.discard_sudo_input(cx);
        }
        self.cancel_lane(Lane::Refresh);
        self.status = None;
        self.refresh(cx);
    }

    fn submit_sudo(&mut self, cx: &mut Context<Self>) {
        if self.jobs.pending(Lane::Action) || self.sudo_prompt.is_none() || !self.host_ready() {
            return;
        }
        let value = self.sudo_input.read(cx).value(cx);
        if value.is_empty() {
            return;
        }
        self.sudo_password = Some(SecretString::new(value));
        self.discard_sudo_input(cx);
        self.sudo_prompt = None;
        self.elevated = true;
        if let Some((epoch, action)) = self.pending_action.take() {
            if epoch == self.jobs.epoch {
                self.run_pending(action, cx);
            }
        } else {
            self.cancel_lane(Lane::Refresh);
            self.refresh(cx);
        }
        cx.notify();
    }

    fn cancel_sudo(&mut self, cx: &mut Context<Self>) {
        if self.jobs.pending(Lane::Action) {
            return;
        }
        self.clear_authorization(cx);
        self.cancel_lane(Lane::Refresh);
        cx.notify();
    }

    fn run_pending(&mut self, action: PendingAction, cx: &mut Context<Self>) {
        match action {
            PendingAction::Service(entry, action) => self.service_action(entry, action, cx),
            PendingAction::Kill { pid, force } => self.kill_pid(pid, force, cx),
        }
    }

    fn start_action(
        &mut self,
        retry: PendingAction,
        label: String,
        done: String,
        cx: &mut Context<Self>,
    ) {
        if self.jobs.pending(Lane::Action) || !self.host_ready() {
            return;
        }
        self.cancel_lane(Lane::Refresh);
        self.pending_action = None;
        self.sudo_prompt = None;
        self.discard_sudo_input(cx);
        self.action = Some(ActionPresentation {
            label: label.into(),
            done,
            retry: retry.clone(),
        });
        self.status = None;
        self.submit_job(
            Lane::Action,
            String::new(),
            Request::Action(retry, self.services_backend),
            cx,
        );
    }

    fn service_action(
        &mut self,
        entry: ServiceEntry,
        action: ServiceAction,
        cx: &mut Context<Self>,
    ) {
        if self.services_backend.is_none() {
            return;
        }
        let label = format!("{} {}…", t!(action_label_key(action)), entry.name);
        let done = format!("{} {}", t!(action_done_key(action)), entry.name);
        self.start_action(PendingAction::Service(entry, action), label, done, cx);
    }

    fn kill_pid(&mut self, pid: u32, force: bool, cx: &mut Context<Self>) {
        self.start_action(
            PendingAction::Kill { pid, force },
            t!("hostTools.stopping", pid = pid).to_string(),
            t!("hostTools.stopped", pid = pid).to_string(),
            cx,
        );
    }

    fn kill_pid_for_epoch(&mut self, epoch: u64, pid: u32, force: bool, cx: &mut Context<Self>) {
        if epoch == self.jobs.epoch {
            self.kill_pid(pid, force, cx);
        }
    }

    fn service_action_for_epoch(
        &mut self,
        epoch: u64,
        entry: ServiceEntry,
        action: ServiceAction,
        cx: &mut Context<Self>,
    ) {
        if epoch == self.jobs.epoch {
            self.service_action(entry, action, cx);
        }
    }

    fn open_logs(&mut self, entry: ServiceEntry, cx: &mut Context<Self>) {
        if !self.can_refresh() {
            return;
        }
        let Some(backend) = self.services_backend else {
            return;
        };
        let key = entry.key();
        self.logs = Some(LogsDrawer {
            title: entry.name.clone().into(),
            lines: Vec::new(),
            entry: entry.clone(),
        });
        self.remeasure_list();
        self.submit_job(Lane::Logs, key, Request::Logs(entry, backend), cx);
    }

    fn close_logs(&mut self, cx: &mut Context<Self>) {
        self.cancel_lane(Lane::Logs);
        self.logs = None;
        self.remeasure_list();
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
        self.cancel_lane(Lane::Details);
        self.port_details = None;
        if self.kind == HostToolKind::Ports
            && let Some(selected) = self.selected.clone()
            && let Some(row) = self
                .ports_rows
                .iter()
                .find(|row| row.key() == selected)
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
        self.services_rows.iter().find(|row| row.key() == selected)
    }

    fn load_port_details(&mut self, row: &PortRow, cx: &mut Context<Self>) {
        if !self.can_refresh() {
            return;
        }
        let Some(pid) = row.pid else {
            return;
        };
        let key = row.key();
        self.port_details = Some(PortDetailsState {
            key: key.clone(),
            details: ProcessDetails::default(),
            error: None,
        });
        self.submit_job(Lane::Details, key, Request::Details(pid), cx);
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
