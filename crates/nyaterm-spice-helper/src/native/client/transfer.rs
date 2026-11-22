use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::CString;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::rc::{Rc, Weak};

use nyaterm_core::secret::SecretString;
use nyaterm_remote_desktop::spice::protocol::{SpiceControl, SpiceErrorKind, SpiceTransferState};

use crate::native::abi::{Api, Error, Object};
use crate::native::client::NativeClient;

const MAX_TRANSFERS: usize = 4;

#[derive(Clone, Copy)]
struct Progress {
    completed: u64,
    total: u64,
    state: SpiceTransferState,
}
type ProgressMap = RefCell<BTreeMap<u64, Progress>>;

pub(super) struct Transfers {
    active: BTreeMap<u64, Object>,
    progress: Rc<ProgressMap>,
}

impl Transfers {
    pub fn new() -> Self {
        Self {
            active: BTreeMap::new(),
            progress: Rc::default(),
        }
    }

    pub fn next_event(&mut self, api: &Api) -> Option<SpiceControl> {
        let (transfer_id, progress) = self.progress.borrow_mut().pop_first()?;
        if progress.state != SpiceTransferState::Running
            && let Some(cancel) = self.active.remove(&transfer_id)
        {
            unsafe {
                (api.g_object_unref)(cancel);
            }
        }
        Some(SpiceControl::TransferProgress {
            transfer_id,
            completed: progress.completed,
            total: progress.total,
            state: progress.state,
        })
    }

    pub fn shutdown(&mut self, api: &Api) {
        for (_, cancel) in std::mem::take(&mut self.active) {
            unsafe {
                (api.g_cancellable_cancel)(cancel);
                (api.g_object_unref)(cancel);
            }
        }
        self.progress.borrow_mut().clear();
    }
}

// Completion owns this box; it has no borrowed Api/Client pointers. Cancelling
// and dropping a session cannot leave a callback pointing into freed userdata.
struct TransferCallback {
    id: u64,
    sink: Weak<ProgressMap>,
    last: Progress,
    finish: unsafe extern "C" fn(Object, Object, *mut *mut Error) -> i32,
    free_error: unsafe extern "C" fn(*mut Error),
    matches_error: unsafe extern "C" fn(*const Error, u32, i32) -> i32,
    io_error_domain: u32,
}

impl NativeClient<'_> {
    pub fn send_files(
        &mut self,
        transfer_id: u64,
        paths: &[SecretString],
    ) -> Result<(), SpiceErrorKind> {
        SpiceControl::SendFiles {
            transfer_id,
            paths: paths.to_vec(),
        }
        .validate()
        .map_err(|_| SpiceErrorKind::FileTransfer)?;
        let main = {
            let state = self.callbacks.state.borrow();
            if state.config.settings.view_only || !state.config.settings.file_transfer {
                return Err(SpiceErrorKind::FileTransfer);
            }
            if state.main.is_null() || !state.agent.capabilities.file_transfer {
                return Err(SpiceErrorKind::AgentUnavailable);
            }
            state.main
        };
        if self.transfers.active.len() >= MAX_TRANSFERS
            || self.transfers.active.contains_key(&transfer_id)
        {
            return Err(SpiceErrorKind::ResourceLimit);
        }
        let strings: Vec<_> = paths
            .iter()
            .map(|path| {
                if !std::path::Path::new(path.expose_secret()).is_absolute() {
                    return Err(SpiceErrorKind::FileTransfer);
                }
                CString::new(path.expose_secret()).map_err(|_| SpiceErrorKind::FileTransfer)
            })
            .collect::<Result<_, _>>()?;
        let api = self.callbacks.api();
        let mut files = Vec::new();
        for path in strings {
            let file = unsafe { (api.g_file_new_for_path)(path.as_ptr()) };
            if file.is_null() {
                for file in files {
                    unsafe {
                        (api.g_object_unref)(file);
                    }
                }
                return Err(SpiceErrorKind::FileTransfer);
            }
            files.push(file);
        }
        files.push(ptr::null_mut());
        let cancel = unsafe { (api.g_cancellable_new)() };
        if cancel.is_null() {
            for &file in files.iter().filter(|p| !p.is_null()) {
                unsafe {
                    (api.g_object_unref)(file);
                }
            }
            return Err(SpiceErrorKind::FileTransfer);
        }
        let context = Box::new(TransferCallback {
            id: transfer_id,
            sink: Rc::downgrade(&self.transfers.progress),
            last: Progress {
                completed: 0,
                total: 0,
                state: SpiceTransferState::Running,
            },
            finish: api.spice_main_channel_file_copy_finish,
            free_error: api.g_error_free,
            matches_error: api.g_error_matches,
            io_error_domain: unsafe { (api.g_io_error_quark)() },
        });
        let context = Box::into_raw(context).cast();
        self.transfers.active.insert(transfer_id, cancel);
        // GIO retains the source files and cancellable before returning.
        unsafe {
            (api.spice_main_channel_file_copy_async)(
                main,
                files.as_ptr(),
                0,
                cancel,
                Some(progress),
                context,
                Some(complete),
                context,
            );
            for &file in files.iter().filter(|p| !p.is_null()) {
                (api.g_object_unref)(file);
            }
        }
        Ok(())
    }

    pub fn cancel_transfer(&mut self, transfer_id: u64) {
        if let Some(&cancel) = self.transfers.active.get(&transfer_id) {
            unsafe {
                (self.callbacks.api().g_cancellable_cancel)(cancel);
            }
        }
    }
}

unsafe extern "C" fn progress(completed: i64, total: i64, data: Object) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let context = unsafe { &mut *data.cast::<TransferCallback>() };
        if completed < 0 || total < 0 || completed > total {
            return;
        }
        context.last = Progress {
            completed: completed as u64,
            total: total as u64,
            state: SpiceTransferState::Running,
        };
        if let Some(sink) = context.sink.upgrade() {
            sink.borrow_mut().insert(context.id, context.last);
        }
    }));
}

unsafe extern "C" fn complete(source: Object, result: Object, data: Object) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut context = unsafe { Box::from_raw(data.cast::<TransferCallback>()) };
        let mut error: *mut Error = ptr::null_mut();
        let success = unsafe { (context.finish)(source, result, &mut error) } != 0;
        // Error codes are only meaningful within their domain. Never expose
        // error messages, which may contain local paths or guest data.
        let cancelled = !error.is_null()
            && unsafe { (context.matches_error)(error, context.io_error_domain, 19) != 0 };
        if !error.is_null() {
            unsafe {
                (context.free_error)(error);
            }
        }
        context.last.state = if success {
            SpiceTransferState::Completed
        } else if cancelled {
            SpiceTransferState::Cancelled
        } else {
            SpiceTransferState::Failed
        };
        if success {
            context.last.completed = context.last.total;
        }
        if let Some(sink) = context.sink.upgrade() {
            sink.borrow_mut().insert(context.id, context.last);
        }
    }));
}
