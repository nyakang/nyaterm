use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, SyncSender, TrySendError},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use futures::channel::oneshot;
use nyaterm_core::plugins::invocation::{
    ActionInput, ActionResult, ParameterValue, validate_result,
};
use nyaterm_core::plugins::manifest::{PluginManifest, ResultKind};
use nyaterm_core::plugins::{API_VERSION, API_VERSION_PARTS, ErrorCode, PluginError, PluginResult};
use wasmtime::component::{Component, Linker};
use wasmtime::{
    Config, Engine, ResourceLimiter, Store, StoreLimits, StoreLimitsBuilder, UpdateDeadline,
};

mod bindings {
    wasmtime::component::bindgen!({ path: "../nyaterm-plugin-api/wit", world: "plugin" });
}
use bindings::nyaterm::plugin::types as wit;

#[cfg(test)]
type CallEpochHook = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
pub struct RuntimeLimits {
    pub memory_bytes: usize,
    pub fuel: u64,
    pub timeout: Duration,
    pub queue_capacity: usize,
    #[cfg(test)]
    pub(crate) on_call_epoch: Option<CallEpochHook>,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            memory_bytes: 32 * 1024 * 1024,
            fuel: 20_000_000,
            timeout: Duration::from_secs(2),
            queue_capacity: 8,
            #[cfg(test)]
            on_call_epoch: None,
        }
    }
}

pub struct RuntimeEngine {
    engine: Engine,
    clock: Arc<EpochClock>,
    on_change: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    ticker: Mutex<Option<JoinHandle<()>>>,
    host_factory: Mutex<Option<Arc<dyn HostFactory>>>,
}

pub trait HostFactory: Send + Sync {
    fn create(
        &self,
        manifest: &PluginManifest,
        component: &[u8],
    ) -> PluginResult<crate::tasks::TaskBroker>;
}

#[derive(Default)]
struct ClockState {
    calls: usize,
    stopping: bool,
    #[cfg(test)]
    ticks: u64,
}

#[derive(Default)]
struct EpochClock {
    state: Mutex<ClockState>,
    changed: Condvar,
}

struct ActiveCall(Arc<EpochClock>);

impl Drop for ActiveCall {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.calls -= 1;
        if state.calls == 0 {
            self.0.changed.notify_one();
        }
    }
}

impl RuntimeEngine {
    /// Verify the component interface without executing candidate code.
    pub fn validate_component(&self, bytes: &[u8]) -> PluginResult<()> {
        validate_api_marker(bytes)?;
        let failure = || {
            PluginError::new(
                ErrorCode::Incompatible,
                "Invalid extension component interface",
            )
        };
        let component = Component::new(&self.engine, bytes).map_err(|_| failure())?;
        let mut linker = Linker::<GuestState>::new(&self.engine);
        bindings::Plugin::add_to_linker::<_, wasmtime::component::HasSelf<GuestState>>(
            &mut linker,
            |state| state,
        )
        .map_err(|_| failure())?;
        bindings::PluginPre::new(linker.instantiate_pre(&component).map_err(|_| failure())?)
            .map_err(|_| failure())?;
        Ok(())
    }

    pub fn new() -> PluginResult<Arc<Self>> {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .consume_fuel(true)
            .epoch_interruption(true)
            .max_wasm_stack(512 * 1024);
        let engine = Engine::new(&config).map_err(|_| {
            PluginError::new(
                ErrorCode::Initialization,
                "Cannot initialize the Wasm engine",
            )
        })?;
        let clock = Arc::new(EpochClock::default());
        let engine_for_tick = engine.clone();
        let clock_for_tick = clock.clone();
        let ticker = thread::Builder::new()
            .name("plugin-epoch".into())
            .spawn(move || {
                let mut state = clock_for_tick.state.lock().unwrap();
                let mut next_tick = Instant::now() + Duration::from_millis(10);
                loop {
                    while state.calls == 0 && !state.stopping {
                        state = clock_for_tick.changed.wait(state).unwrap();
                        next_tick = Instant::now() + Duration::from_millis(10);
                    }
                    if state.stopping {
                        break;
                    }
                    let (next, elapsed) = clock_for_tick
                        .changed
                        .wait_timeout(state, next_tick.saturating_duration_since(Instant::now()))
                        .unwrap();
                    state = next;
                    if elapsed.timed_out() && state.calls > 0 {
                        engine_for_tick.increment_epoch();
                        #[cfg(test)]
                        {
                            state.ticks += 1;
                            clock_for_tick.changed.notify_all();
                        }
                        next_tick = Instant::now() + Duration::from_millis(10);
                    }
                }
            })
            .map_err(|_| {
                PluginError::new(
                    ErrorCode::Initialization,
                    "Cannot start the plugin deadline clock",
                )
            })?;
        Ok(Arc::new(Self {
            engine,
            clock,
            on_change: Mutex::new(None),
            ticker: Mutex::new(Some(ticker)),
            host_factory: Mutex::new(None),
        }))
    }

    fn active_call(&self) -> ActiveCall {
        let mut state = self.clock.state.lock().unwrap();
        state.calls += 1;
        if state.calls == 1 {
            self.clock.changed.notify_one();
        }
        ActiveCall(self.clock.clone())
    }

    pub(crate) fn set_change_handler(&self, handler: Arc<dyn Fn() + Send + Sync>) {
        *self.on_change.lock().unwrap() = Some(handler);
    }

    pub fn set_host_factory(&self, factory: Arc<dyn HostFactory>) {
        *self.host_factory.lock().unwrap() = Some(factory);
    }

    fn changed(&self) {
        if let Some(handler) = self.on_change.lock().unwrap().as_ref() {
            handler();
        }
    }
}

impl Drop for RuntimeEngine {
    fn drop(&mut self) {
        self.clock.state.lock().unwrap().stopping = true;
        self.clock.changed.notify_one();
        if let Some(ticker) = self.ticker.get_mut().unwrap().take() {
            let _ = ticker.join();
        }
    }
}

pub struct RuntimeToken {
    session_events: Option<crate::authority::Authority>,
    active: AtomicBool,
    fault: Mutex<Option<PluginError>>,
}

impl RuntimeToken {
    pub fn allows_session_events(&self) -> bool {
        self.active()
            && self
                .session_events
                .as_ref()
                .is_some_and(crate::authority::Authority::allows_session_events)
    }
    pub fn active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
    pub fn fault(&self) -> Option<PluginError> {
        self.fault.lock().unwrap().clone()
    }
}

struct Request {
    input: ActionInput,
    action: String,
    kind: ResultKind,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
    reply: oneshot::Sender<PluginResult<ActionResult>>,
}

enum RuntimeRequest {
    TaskReady(u64),
    Invoke(Request),
    Shutdown,
}

pub struct CallTicket {
    reply: Option<oneshot::Receiver<PluginResult<ActionResult>>>,
    cancelled: Arc<AtomicBool>,
    token: Arc<RuntimeToken>,
}

impl CallTicket {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub async fn result(mut self) -> PluginResult<ActionResult> {
        let result = self.reply.take().unwrap().await;
        if self.cancelled.load(Ordering::Acquire) {
            return Err(cancelled());
        }
        // A ticket is tied to the old instance token, never to a mutable catalog ID.
        if !self.token.active() && result.as_ref().is_ok_and(|value| value.is_ok()) {
            return Err(cancelled());
        }
        result.unwrap_or_else(|_| Err(self.token.fault().unwrap_or_else(cancelled)))
    }
}

impl Drop for CallTicket {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn cancelled() -> PluginError {
    PluginError::new(
        ErrorCode::Cancelled,
        "Plugin call cancelled or superseded by a lifecycle change",
    )
}

pub struct RuntimeInstance {
    sender: Mutex<Option<Arc<SyncSender<RuntimeRequest>>>>,
    token: Arc<RuntimeToken>,
    thread: Mutex<Option<JoinHandle<()>>>,
    limits: RuntimeLimits,
}

struct GuestState {
    tasks: Option<crate::tasks::TaskBroker>,
    limits: GuestLimits,
    stop: Arc<RuntimeToken>,
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
    #[cfg(test)]
    on_call_epoch: Option<(String, CallEpochHook)>,
}

impl bindings::nyaterm::plugin::host::Host for GuestState {
    fn submit(&mut self, request: Vec<u8>) -> Result<u64, String> {
        if request.len() > crate::tasks::MAX_TASK_BYTES * 8 || !self.stop.active() {
            return Err("Extension request is unavailable or exceeds its limit".into());
        }
        let request =
            serde_json::from_slice(&request).map_err(|_| "Invalid host request".to_string())?;
        self.tasks
            .as_mut()
            .ok_or("Host capabilities are not configured")?
            .submit(request)
            .map_err(|e| e.message)
    }
    fn poll(&mut self, task: u64) -> Result<Vec<u8>, String> {
        let event = self
            .tasks
            .as_mut()
            .ok_or("Host capabilities are not configured")?
            .poll(task)
            .map_err(|e| e.message)?;
        serde_json::to_vec(&event).map_err(|_| "Cannot encode task result".into())
    }
    fn cancel(&mut self, task: u64) -> Result<(), String> {
        self.tasks
            .as_ref()
            .ok_or("Host capabilities are not configured")?
            .cancel(task)
            .map_err(|e| e.message)
    }
}

impl bindings::nyaterm::plugin::types::Host for GuestState {}

struct GuestLimits {
    limits: StoreLimits,
    exceeded: bool,
}
impl ResourceLimiter for GuestLimits {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let result = self.limits.memory_growing(current, desired, maximum);
        if !result.as_ref().is_ok_and(|allowed| *allowed) {
            self.exceeded = true;
        }
        result
    }
    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let result = self.limits.table_growing(current, desired, maximum);
        if !result.as_ref().is_ok_and(|allowed| *allowed) {
            self.exceeded = true;
        }
        result
    }
    fn instances(&self) -> usize {
        self.limits.instances()
    }
    fn tables(&self) -> usize {
        self.limits.tables()
    }
    fn memories(&self) -> usize {
        self.limits.memories()
    }
}

fn reset_budget(
    store: &mut Store<GuestState>,
    limits: &RuntimeLimits,
    cancel: Arc<AtomicBool>,
    deadline: Instant,
) -> PluginResult<()> {
    store.data_mut().cancelled = cancel;
    store.data_mut().deadline = deadline;
    store.data_mut().limits.exceeded = false;
    store.set_fuel(limits.fuel).map_err(|_| {
        PluginError::new(
            ErrorCode::Initialization,
            "Cannot set the guest execution budget",
        )
    })?;
    store.set_epoch_deadline(1);
    Ok(())
}

fn execution_error(store: &Store<GuestState>, error: &wasmtime::Error) -> PluginError {
    if !store.data().stop.active() || store.data().cancelled.load(Ordering::Acquire) {
        return cancelled();
    }
    if Instant::now() >= store.data().deadline {
        return PluginError::new(
            ErrorCode::Timeout,
            "Plugin exceeded its execution deadline; reload to retry",
        );
    }
    if store.data().limits.exceeded {
        return PluginError::new(
            ErrorCode::MemoryLimit,
            "Plugin exceeded a memory or table limit; reload to retry",
        );
    }
    if error.downcast_ref::<wasmtime::Trap>() == Some(&wasmtime::Trap::OutOfFuel) {
        return PluginError::new(
            ErrorCode::Budget,
            "Plugin exhausted its CPU budget; reload to retry",
        );
    }
    PluginError::new(
        ErrorCode::Trap,
        "Plugin trapped or exceeded a resource limit; reload to retry",
    )
}

pub fn validate_api_marker(bytes: &[u8]) -> PluginResult<()> {
    let mut markers = 0;
    // parse_all descends into nested core modules, where the SDK's marker lives.
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        match payload
            .map_err(|_| PluginError::new(ErrorCode::Incompatible, "Invalid Wasm component"))?
        {
            wasmparser::Payload::CustomSection(section)
                if section.name() == "nyaterm:plugin-api" =>
            {
                markers += 1;
                if section.data() != API_VERSION.as_bytes() {
                    return Err(PluginError::new(
                        ErrorCode::Incompatible,
                        "Unsupported embedded Wasm API version",
                    ));
                }
            }
            _ => {}
        }
    }
    if markers != 1 {
        return Err(PluginError::new(
            ErrorCode::Incompatible,
            "Wasm must contain exactly one SDK API version marker",
        ));
    }
    Ok(())
}

impl RuntimeInstance {
    /// Compile and initialize completely before making an instance visible to the registry.
    pub fn prepare(
        engine: Arc<RuntimeEngine>,
        bytes: &[u8],
        manifest: &PluginManifest,
        limits: RuntimeLimits,
    ) -> PluginResult<Arc<Self>> {
        validate_api_marker(bytes)?;
        let tasks = engine
            .host_factory
            .lock()
            .unwrap()
            .as_ref()
            .map(|factory| factory.create(manifest, bytes))
            .transpose()?;
        let component = Component::new(&engine.engine, bytes).map_err(|_| {
            PluginError::new(
                ErrorCode::Incompatible,
                "Cannot compile plugin component; verify its component interface",
            )
        })?;
        let mut preflight = Linker::new(&engine.engine);
        bindings::Plugin::add_to_linker::<_, wasmtime::component::HasSelf<GuestState>>(
            &mut preflight,
            |state: &mut GuestState| state,
        )
        .map_err(|_| PluginError::new(ErrorCode::Incompatible, "Cannot link extension host"))?;
        bindings::PluginPre::new(preflight.instantiate_pre(&component).map_err(|_| {
            PluginError::new(ErrorCode::Incompatible, "Invalid extension interface")
        })?)
        .map_err(|_| PluginError::new(ErrorCode::Incompatible, "Invalid extension exports"))?;
        let token = Arc::new(RuntimeToken {
            session_events: tasks.as_ref().map(crate::tasks::TaskBroker::authority),
            active: AtomicBool::new(true),
            fault: Mutex::new(None),
        });
        let (sender, receiver) = mpsc::sync_channel::<RuntimeRequest>(limits.queue_capacity);
        let sender = Arc::new(sender);
        if let Some(tasks) = &tasks {
            // A host callback must not keep its own runtime mailbox alive after stop.
            let sender = Arc::downgrade(&sender);
            tasks.set_wake(Arc::new(move |id| {
                if let Some(sender) = sender.upgrade() {
                    let _ = sender.send(RuntimeRequest::TaskReady(id));
                }
            }));
        }
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let identity = wit::Identity {
            id: manifest.id.clone(),
            version: manifest.version.clone(),
        };
        let token_for_worker = token.clone();
        let limits_for_worker = limits.clone();
        let thread = thread::Builder::new()
            .name(format!("plugin-{}", manifest.id))
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    // Installation validates and compiles, but guest code starts only
                    // after an actual command/menu/session-event invocation.
                    let _ = ready_tx.send(Ok(()));
                    let first = loop {
                        if !token_for_worker.active() {
                            return Ok(());
                        }
                        match receiver.recv() {
                            Ok(RuntimeRequest::Invoke(request)) => {
                                if !token_for_worker.active()
                                    || request.cancelled.load(Ordering::Acquire)
                                    || Instant::now() >= request.deadline
                                {
                                    let _ = request.reply.send(Err(cancelled()));
                                    continue;
                                }
                                break request;
                            }
                            Ok(RuntimeRequest::TaskReady(_)) => continue,
                            Ok(RuntimeRequest::Shutdown) | Err(_) => return Ok(()),
                        }
                    };
                    let limits = limits_for_worker;
                    let state = GuestState {
                        tasks,
                        limits: GuestLimits {
                            exceeded: false,
                            limits: StoreLimitsBuilder::new()
                                .memory_size(limits.memory_bytes)
                                .instances(8)
                                .memories(4)
                                .tables(4)
                                .table_elements(16_384)
                                .trap_on_grow_failure(true)
                                .build(),
                        },
                        stop: token_for_worker.clone(),
                        cancelled: Arc::new(AtomicBool::new(false)),
                        deadline: Instant::now() + limits.timeout,
                        #[cfg(test)]
                        on_call_epoch: None,
                    };
                    let mut store = Store::new(&engine.engine, state);
                    store.limiter(|state| &mut state.limits);
                    store.epoch_deadline_callback(|context| {
                        #[cfg(test)]
                        let mut context = context;
                        #[cfg(test)]
                        if let Some((action, hook)) = context.data_mut().on_call_epoch.take() {
                            hook(&action);
                        }
                        let state = context.data();
                        if !state.stop.active() || state.cancelled.load(Ordering::Acquire) {
                            return Err(wasmtime::Error::msg("plugin cancelled"));
                        }
                        if Instant::now() >= state.deadline {
                            return Err(wasmtime::Error::msg("plugin deadline"));
                        }
                        Ok(UpdateDeadline::Continue(1))
                    });
                    let initialization = (|| {
                        let _active = engine.active_call();
                        reset_budget(&mut store, &limits, first.cancelled.clone(), first.deadline)?;
                        let mut linker = Linker::new(&engine.engine);
                        bindings::Plugin::add_to_linker::<
                            _,
                            wasmtime::component::HasSelf<GuestState>,
                        >(&mut linker, |state: &mut GuestState| state)
                        .map_err(|_| {
                            PluginError::new(
                                ErrorCode::Initialization,
                                "Cannot link extension host",
                            )
                        })?;
                        let plugin = bindings::Plugin::instantiate(&mut store, &component, &linker)
                            .map_err(|error| execution_error(&store, &error))?;
                        let version = plugin
                            .call_api_version(&mut store)
                            .map_err(|error| execution_error(&store, &error))?;
                        if (version.major, version.minor, version.patch) != API_VERSION_PARTS {
                            return Err(PluginError::new(
                                ErrorCode::Incompatible,
                                "Component exports a different API version",
                            ));
                        }
                        plugin
                            .call_initialize(&mut store, &identity)
                            .map_err(|error| execution_error(&store, &error))?
                            .map_err(|_| {
                                PluginError::new(
                                    ErrorCode::Initialization,
                                    "Plugin initialization failed",
                                )
                            })?;
                        Ok(plugin)
                    })();
                    let plugin = match initialization {
                        Ok(plugin) => plugin,
                        Err(error) => {
                            let _ = first.reply.send(Err(error.clone()));
                            return Err(error);
                        }
                    };
                    let mut first = Some(first);
                    while token_for_worker.active() {
                        let incoming = first
                            .take()
                            .map(RuntimeRequest::Invoke)
                            .map(Ok)
                            .unwrap_or_else(|| receiver.recv());
                        let request = match incoming {
                            Ok(RuntimeRequest::TaskReady(id)) => {
                                let _active = engine.active_call();
                                reset_budget(
                                    &mut store,
                                    &limits,
                                    Arc::new(AtomicBool::new(false)),
                                    Instant::now() + limits.timeout,
                                )?;
                                plugin
                                    .call_task_ready(&mut store, id)
                                    .map_err(|error| execution_error(&store, &error))?;
                                continue;
                            }
                            Ok(RuntimeRequest::Invoke(request)) => request,
                            Ok(RuntimeRequest::Shutdown) | Err(_) => break,
                        };
                        if !token_for_worker.active()
                            || request.cancelled.load(Ordering::Acquire)
                            || Instant::now() >= request.deadline
                        {
                            let _ = request.reply.send(Err(cancelled()));
                            continue;
                        }
                        let _active = engine.active_call();
                        reset_budget(&mut store, &limits, request.cancelled, request.deadline)?;
                        let input = wit::ActionInput {
                            action: request.action,
                            arguments: request
                                .input
                                .parameters
                                .into_iter()
                                .map(|(name, value)| wit::Argument {
                                    name,
                                    value: match value {
                                        ParameterValue::String(v) => wit::Value::Text(v),
                                        ParameterValue::Integer(v) => wit::Value::Integer(v),
                                        ParameterValue::Boolean(v) => wit::Value::Boolean(v),
                                    },
                                })
                                .collect(),
                            text: request.input.text,
                        };
                        #[cfg(test)]
                        {
                            store.data_mut().on_call_epoch = limits
                                .on_call_epoch
                                .clone()
                                .map(|hook| (input.action.clone(), hook));
                        }
                        let result = match plugin.call_invoke(&mut store, &input) {
                            Ok(Ok(wit::ActionResult::Command(v))) => Ok(ActionResult::Command {
                                title: v.title,
                                command: v.command,
                            }),
                            Ok(Ok(wit::ActionResult::Text(text))) => Ok(ActionResult::Text(text)),
                            Ok(Err(error)) => {
                                let message = match error.kind {
                                    wit::ErrorKind::InvalidInput => {
                                        "Plugin rejected these inputs; check action parameters"
                                    }
                                    wit::ErrorKind::UnsupportedAction => {
                                        "Plugin does not implement this declared action"
                                    }
                                    wit::ErrorKind::Failed => "Plugin reported an action failure",
                                };
                                let _ = request
                                    .reply
                                    .send(Err(PluginError::new(ErrorCode::InvalidResult, message)));
                                continue;
                            }
                            Err(error) => Err(execution_error(&store, &error)),
                        }
                        .and_then(|value| {
                            validate_result(request.kind, &value)?;
                            Ok(value)
                        });
                        let fatal = result.as_ref().err().cloned();
                        if let Some(error) = &fatal {
                            *token_for_worker.fault.lock().unwrap() = Some(error.clone());
                            token_for_worker.active.store(false, Ordering::Release);
                        }
                        let _ = request.reply.send(result);
                        if let Some(error) = fatal {
                            return Err(error);
                        }
                    }
                    // Cooperative lifecycle export gets a fresh bounded budget. It is not
                    // trusted to finish, and the ticker can terminate it like any action.
                    store.data_mut().stop = Arc::new(RuntimeToken {
                        session_events: None,
                        active: AtomicBool::new(true),
                        fault: Mutex::new(None),
                    });
                    let _active = engine.active_call();
                    reset_budget(
                        &mut store,
                        &limits,
                        Arc::new(AtomicBool::new(false)),
                        Instant::now() + limits.timeout,
                    )?;
                    let _ = plugin.call_shutdown(&mut store);
                    Ok(())
                }));
                if let Err(error) = outcome.unwrap_or_else(|_| {
                    Err(PluginError::new(
                        ErrorCode::Trap,
                        "Plugin runtime worker failed",
                    ))
                }) {
                    *token_for_worker.fault.lock().unwrap() = Some(error);
                }
                token_for_worker.active.store(false, Ordering::Release);
                engine.changed();
            })
            .map_err(|_| {
                PluginError::new(ErrorCode::Initialization, "Cannot start plugin worker")
            })?;
        match ready_rx.recv_timeout(limits.timeout + Duration::from_secs(1)) {
            Ok(Ok(())) => Ok(Arc::new(Self {
                sender: Mutex::new(Some(sender)),
                token,
                thread: Mutex::new(Some(thread)),
                limits,
            })),
            result => {
                token.active.store(false, Ordering::Release);
                let _ = sender.try_send(RuntimeRequest::Shutdown);
                drop(sender);
                let _ = thread.join();
                Err(result.ok().and_then(Result::err).unwrap_or_else(|| {
                    PluginError::new(
                        ErrorCode::Initialization,
                        "Plugin initialization did not complete",
                    )
                }))
            }
        }
    }

    pub fn token(&self) -> Arc<RuntimeToken> {
        self.token.clone()
    }

    pub fn invoke(
        &self,
        action: &str,
        kind: ResultKind,
        input: ActionInput,
    ) -> PluginResult<CallTicket> {
        if !self.token.active() {
            return Err(self.token.fault().unwrap_or_else(cancelled));
        }
        let (reply, response) = oneshot::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let request = Request {
            input,
            action: action.into(),
            kind,
            deadline: Instant::now() + self.limits.timeout,
            cancelled: cancelled.clone(),
            reply,
        };
        self.sender
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(cancelled_error)?
            .try_send(RuntimeRequest::Invoke(request))
            .map_err(|error| match error {
                TrySendError::Full(_) => PluginError::new(
                    ErrorCode::QueueFull,
                    "Plugin action queue is full; wait and retry",
                ),
                TrySendError::Disconnected(_) => self.token.fault().unwrap_or_else(cancelled_error),
            })?;
        Ok(CallTicket {
            reply: Some(response),
            cancelled,
            token: self.token.clone(),
        })
    }

    pub fn stop(&self) {
        self.token.active.store(false, Ordering::Release);
        if let Some(sender) = self.sender.lock().unwrap().take() {
            // Closing also wakes a zero-capacity mailbox when its receiver has
            // not entered recv yet; shutdown never depends on free queue space.
            let _ = sender.try_send(RuntimeRequest::Shutdown);
        }
        let mut worker = self.thread.lock().unwrap();
        if let Some(thread) = worker.take() {
            let _ = thread.join();
        }
    }
}

fn cancelled_error() -> PluginError {
    cancelled()
}

impl Drop for RuntimeInstance {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::RuntimeEngine;
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn epoch_clock_parks_without_calls_and_resumes_then_parks_again() {
        let engine = RuntimeEngine::new().unwrap();
        thread::sleep(Duration::from_millis(60));
        assert_eq!(engine.clock.state.lock().unwrap().ticks, 0);
        let active = engine.active_call();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut state = engine.clock.state.lock().unwrap();
        while state.ticks == 0 {
            let (next, _) = engine
                .clock
                .changed
                .wait_timeout(state, deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            state = next;
            assert!(
                Instant::now() < deadline,
                "active guest clock did not resume"
            );
        }
        drop(state);
        drop(active);
        let ticks = engine.clock.state.lock().unwrap().ticks;
        thread::sleep(Duration::from_millis(60));
        assert_eq!(engine.clock.state.lock().unwrap().ticks, ticks);
    }
}
