//! Bounded per-instance task admission. Adapters own actual I/O and cancellation.
use crate::authority::Authority;
use nyaterm_core::plugins::extension::{HostRequest, TaskEvent};
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

pub const MAX_TASK_BYTES: usize = 1024 * 1024;
const MAX_TASKS: usize = 16;
type WakeHandler = Arc<Mutex<Option<Arc<dyn Fn(u64) + Send + Sync>>>>;
#[derive(Default)]
struct OutputQueue {
    chunks: VecDeque<Vec<u8>>,
    bytes: usize,
}

pub trait HostAdapter: Send + Sync + 'static {
    /// Check cancellation and session generation before side effects and during I/O.
    /// Never include credentials, request bodies or terminal output in errors.
    fn execute(&self, request: HostRequest, context: &TaskContext) -> PluginResult<TaskEvent>;
}

#[derive(Clone)]
pub struct TaskContext {
    cancelled: Arc<AtomicBool>,
    authority: Authority,
    output: Arc<Mutex<OutputQueue>>,
    wake: WakeHandler,
    id: u64,
}

impl TaskContext {
    pub fn output(&self, request: &HostRequest, bytes: &[u8]) -> PluginResult<()> {
        self.check(request)?;
        let mut output = self.output.lock().unwrap();
        if output.chunks.len() >= 64 || output.bytes + bytes.len() > MAX_TASK_BYTES {
            return Err(PluginError::new(
                ErrorCode::QueueFull,
                "Extension output consumer is too slow",
            ));
        }
        output.bytes += bytes.len();
        output.chunks.push_back(bytes.to_vec());
        drop(output);
        let handler = self.wake.lock().unwrap().clone();
        if let Some(handler) = handler {
            handler(self.id);
        }
        Ok(())
    }

    pub fn plugin_id(&self) -> String {
        self.authority.plugin_id()
    }
    pub fn check(&self, request: &HostRequest) -> PluginResult<()> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(PluginError::new(
                ErrorCode::Cancelled,
                "Extension task cancelled",
            ));
        }
        self.authority.check(request)
    }
}

struct Task {
    cancelled: Arc<AtomicBool>,
    result: Arc<Mutex<Option<TaskEvent>>>,
    output: Arc<Mutex<OutputQueue>>,
}

struct Work {
    id: u64,
    request: HostRequest,
    task: Task,
}

pub struct TaskBroker {
    adapter: Arc<dyn HostAdapter>,
    wake: WakeHandler,
    sender: Option<mpsc::SyncSender<Work>>,
    tasks: BTreeMap<u64, Task>,
    next: u64,
    authority: Authority,
}

impl TaskBroker {
    pub(crate) fn authority(&self) -> Authority {
        self.authority.clone()
    }
    pub fn new(authority: Authority, adapter: Arc<dyn HostAdapter>) -> Self {
        Self {
            adapter,
            sender: None,
            tasks: BTreeMap::new(),
            next: 0,
            authority,
            wake: Arc::new(Mutex::new(None)),
        }
    }

    fn start_workers(&mut self) -> PluginResult<()> {
        if self.sender.is_some() {
            return Ok(());
        }
        let (sender, receiver) = mpsc::sync_channel::<Work>(MAX_TASKS);
        let receiver = Arc::new(Mutex::new(receiver));
        for _ in 0..2 {
            let receiver = receiver.clone();
            let authority = self.authority.clone();
            let adapter = self.adapter.clone();
            let wake = self.wake.clone();
            std::thread::Builder::new()
                .name("extension-io".into())
                .spawn(move || {
                    loop {
                        let work = match receiver.lock().unwrap().recv() {
                            Ok(work) => work,
                            Err(_) => break,
                        };
                        let context = TaskContext {
                            cancelled: work.task.cancelled.clone(),
                            authority: authority.clone(),
                            output: work.task.output.clone(),
                            wake: wake.clone(),
                            id: work.id,
                        };
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            context.check(&work.request)?;
                            adapter.execute(work.request, &context)
                        }));
                        let event = if work.task.cancelled.load(Ordering::Acquire) {
                            TaskEvent::Cancelled
                        } else {
                            match result {
                                Ok(Ok(event)) => match &event {
                                    TaskEvent::Output { bytes }
                                    | TaskEvent::Completed { bytes, .. }
                                        if bytes.len() > MAX_TASK_BYTES =>
                                    {
                                        TaskEvent::Failed {
                                            code: "output_limit".into(),
                                        }
                                    }
                                    _ => event,
                                },
                                Ok(Err(error)) => TaskEvent::Failed {
                                    code: format!("{:?}", error.code),
                                },
                                Err(_) => TaskEvent::Failed {
                                    code: "adapter_fault".into(),
                                },
                            }
                        };
                        *work.task.result.lock().unwrap() = Some(event);
                        let handler = wake.lock().unwrap().clone();
                        if let Some(handler) = handler {
                            handler(work.id);
                        }
                    }
                })
                .map_err(|_| {
                    PluginError::new(
                        ErrorCode::Initialization,
                        "Cannot start extension I/O worker",
                    )
                })?;
        }
        self.sender = Some(sender);
        Ok(())
    }

    pub fn set_wake(&self, handler: Arc<dyn Fn(u64) + Send + Sync>) {
        *self.wake.lock().unwrap() = Some(handler);
    }

    pub fn submit(&mut self, request: HostRequest) -> PluginResult<u64> {
        self.authority.check(&request)?;
        if self.tasks.len() >= MAX_TASKS {
            return Err(PluginError::new(
                ErrorCode::QueueFull,
                "Too many extension tasks",
            ));
        }
        self.start_workers()?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let result = Arc::new(Mutex::new(None));
        let output = Arc::new(Mutex::new(OutputQueue::default()));
        self.sender
            .as_ref()
            .ok_or_else(missing)?
            .try_send(Work {
                id: self.next + 1,
                request,
                task: Task {
                    cancelled: cancelled.clone(),
                    result: result.clone(),
                    output: output.clone(),
                },
            })
            .map_err(|_| PluginError::new(ErrorCode::QueueFull, "Extension task queue is full"))?;
        self.next += 1;
        self.tasks.insert(
            self.next,
            Task {
                cancelled,
                result,
                output,
            },
        );
        Ok(self.next)
    }

    pub fn poll(&mut self, id: u64) -> PluginResult<TaskEvent> {
        if !self.authority.is_active() {
            self.cancel(id)?;
            self.tasks.remove(&id);
            return Ok(TaskEvent::Cancelled);
        }
        let task = self.tasks.get(&id).ok_or_else(missing)?;
        if task.cancelled.load(Ordering::Acquire) {
            self.tasks.remove(&id);
            return Ok(TaskEvent::Cancelled);
        }
        {
            let mut output = task.output.lock().unwrap();
            if let Some(bytes) = output.chunks.pop_front() {
                output.bytes -= bytes.len();
                return Ok(TaskEvent::Output { bytes });
            }
        }
        let event = task.result.lock().unwrap().take();
        if let Some(event) = event {
            self.tasks.remove(&id);
            Ok(event)
        } else {
            Ok(TaskEvent::Pending)
        }
    }

    pub fn cancel(&self, id: u64) -> PluginResult<()> {
        self.tasks
            .get(&id)
            .ok_or_else(missing)?
            .cancelled
            .store(true, Ordering::Release);
        Ok(())
    }
}

impl Drop for TaskBroker {
    fn drop(&mut self) {
        for task in self.tasks.values() {
            task.cancelled.store(true, Ordering::Release);
        }
        self.sender.take();
    }
}

fn missing() -> PluginError {
    PluginError::new(ErrorCode::NotFound, "Extension task does not exist")
}
