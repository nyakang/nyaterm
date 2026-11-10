//! Windows services straight from the Service Control Manager.
//!
//! Replaces spawning PowerShell: no interpreter start-up, no execution-policy or
//! stdin-script quirks, and a listing in milliseconds. Everything here is a thin
//! wrapper over `advapi32`; handles are closed by RAII guards.
//!
//! Errors carry the Win32 code. "Access denied" (5) is reported with a fixed
//! English marker so permission handling works on any Windows display language.

use super::{ServiceAction, ServiceEntry, ServiceStartup, ServiceState, sort_by_name};
use anyhow::bail;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_MORE_DATA, GetLastError};
use windows_sys::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, ControlService, ENUM_SERVICE_STATUS_PROCESSW,
    EnumServicesStatusExW, OpenSCManagerW, OpenServiceW, QUERY_SERVICE_CONFIGW,
    QueryServiceConfig2W, QueryServiceConfigW, QueryServiceStatusEx, SC_ENUM_PROCESS_INFO,
    SC_HANDLE, SC_MANAGER_CONNECT, SC_MANAGER_ENUMERATE_SERVICE, SC_STATUS_PROCESS_INFO,
    SERVICE_AUTO_START, SERVICE_CHANGE_CONFIG, SERVICE_CONFIG_DESCRIPTION, SERVICE_CONTROL_STOP,
    SERVICE_DEMAND_START, SERVICE_DESCRIPTIONW, SERVICE_DISABLED, SERVICE_NO_CHANGE,
    SERVICE_QUERY_CONFIG, SERVICE_QUERY_STATUS, SERVICE_START, SERVICE_START_TYPE,
    SERVICE_STATE_ALL, SERVICE_STATUS, SERVICE_STATUS_PROCESS, SERVICE_STOP, SERVICE_STOPPED,
    SERVICE_WIN32, StartServiceW,
};

/// `ERROR_SERVICE_NOT_ACTIVE`: stopping something that is already stopped.
const ERROR_SERVICE_NOT_ACTIVE: u32 = 1062;
const STOP_WAIT: Duration = Duration::from_secs(30);

struct Handle(SC_HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle came from OpenSCManagerW/OpenServiceW and is
            // closed exactly once, here.
            unsafe { CloseServiceHandle(self.0) };
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// # Safety
/// `ptr` is null or points to a NUL-terminated UTF-16 string that stays valid
/// for the duration of the call.
unsafe fn from_wide(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0;
    // SAFETY: guaranteed by the caller; reads stop at the terminator.
    while unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(ptr, len) })
}

fn last_error(context: &str) -> anyhow::Error {
    // SAFETY: GetLastError has no preconditions.
    let code = unsafe { GetLastError() };
    if code == ERROR_ACCESS_DENIED {
        return anyhow::anyhow!("Access is denied: run NyaTerm as administrator to {context}");
    }
    anyhow::anyhow!(
        "{context}: {}",
        std::io::Error::from_raw_os_error(code as i32)
    )
}

fn open_manager(access: u32) -> anyhow::Result<Handle> {
    // SAFETY: null machine and database names select the local active database.
    let handle = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), access) };
    if handle.is_null() {
        return Err(last_error("open the Service Control Manager"));
    }
    Ok(Handle(handle))
}

fn open_service(
    manager: &Handle,
    name: &str,
    access: u32,
    context: &str,
) -> anyhow::Result<Handle> {
    let name = wide(name);
    // SAFETY: `name` is NUL-terminated and outlives the call.
    let handle = unsafe { OpenServiceW(manager.0, name.as_ptr(), access) };
    if handle.is_null() {
        return Err(last_error(context));
    }
    Ok(Handle(handle))
}

fn state_of(current: u32) -> (ServiceState, &'static str) {
    match current {
        4 => (ServiceState::Running, "running"),
        1 => (ServiceState::Stopped, "stopped"),
        7 => (ServiceState::Stopped, "paused"),
        2 => (ServiceState::Transitioning, "starting"),
        3 => (ServiceState::Transitioning, "stopping"),
        5 => (ServiceState::Transitioning, "continuing"),
        6 => (ServiceState::Transitioning, "pausing"),
        _ => (ServiceState::Unknown, ""),
    }
}

fn startup_of(start_type: SERVICE_START_TYPE) -> ServiceStartup {
    match start_type {
        0..=2 => ServiceStartup::Enabled,
        SERVICE_DEMAND_START => ServiceStartup::Static,
        SERVICE_DISABLED => ServiceStartup::Disabled,
        _ => ServiceStartup::Unknown,
    }
}

/// A `u64`-backed buffer: the Win32 structs written into it contain pointers and
/// need 8-byte alignment, which a `Vec<u8>` does not promise.
fn aligned_buffer(bytes: u32) -> Vec<u64> {
    vec![0; (bytes as usize).div_ceil(8) + 8]
}

/// Start type and description of one service, or defaults when the service
/// denies `QUERY_CONFIG` (a few protected ones do).
fn query_config(manager: &Handle, name: &str) -> (ServiceStartup, String) {
    let Ok(service) = open_service(manager, name, SERVICE_QUERY_CONFIG, "query a service") else {
        return (ServiceStartup::Unknown, String::new());
    };
    let mut startup = ServiceStartup::Unknown;
    let mut needed = 0u32;
    // SAFETY: a zero-length probe only reports the required size.
    unsafe { QueryServiceConfigW(service.0, std::ptr::null_mut(), 0, &mut needed) };
    if needed > 0 {
        let mut buffer = aligned_buffer(needed);
        // SAFETY: the buffer is at least `needed` bytes and 8-byte aligned.
        let ok = unsafe {
            QueryServiceConfigW(
                service.0,
                buffer.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>(),
                (buffer.len() * 8) as u32,
                &mut needed,
            )
        };
        if ok != 0 {
            // SAFETY: success means the buffer holds a QUERY_SERVICE_CONFIGW.
            let config = unsafe { &*buffer.as_ptr().cast::<QUERY_SERVICE_CONFIGW>() };
            startup = startup_of(config.dwStartType);
        }
    }

    let mut description = String::new();
    needed = 0;
    // SAFETY: zero-length probe.
    unsafe {
        QueryServiceConfig2W(
            service.0,
            SERVICE_CONFIG_DESCRIPTION,
            std::ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    if needed > 0 {
        let mut buffer = aligned_buffer(needed);
        // SAFETY: buffer is large enough and aligned for SERVICE_DESCRIPTIONW.
        let ok = unsafe {
            QueryServiceConfig2W(
                service.0,
                SERVICE_CONFIG_DESCRIPTION,
                buffer.as_mut_ptr().cast::<u8>(),
                (buffer.len() * 8) as u32,
                &mut needed,
            )
        };
        if ok != 0 {
            // SAFETY: success means a SERVICE_DESCRIPTIONW whose string lives in
            // the same buffer.
            description = unsafe {
                from_wide((*buffer.as_ptr().cast::<SERVICE_DESCRIPTIONW>()).lpDescription)
            };
        }
    }
    (startup, description.replace(['\r', '\n', '\t'], " "))
}

pub fn list() -> anyhow::Result<Vec<ServiceEntry>> {
    let manager = open_manager(SC_MANAGER_ENUMERATE_SERVICE)?;
    let mut entries = Vec::new();
    let mut resume = 0u32;
    loop {
        let (mut needed, mut returned) = (0u32, 0u32);
        let mut probe_resume = resume;
        // SAFETY: zero-length probe; reports ERROR_MORE_DATA and the size.
        let probe = unsafe {
            EnumServicesStatusExW(
                manager.0,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                std::ptr::null_mut(),
                0,
                &mut needed,
                &mut returned,
                &mut probe_resume,
                std::ptr::null(),
            )
        };
        if probe != 0 {
            break;
        }
        // SAFETY: no preconditions.
        if unsafe { GetLastError() } != ERROR_MORE_DATA {
            return Err(last_error("list services"));
        }
        let mut buffer = aligned_buffer(needed);
        // SAFETY: the buffer is at least `needed` bytes and aligned for the
        // ENUM_SERVICE_STATUS_PROCESSW records written into it.
        let ok = unsafe {
            EnumServicesStatusExW(
                manager.0,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                buffer.as_mut_ptr().cast::<u8>(),
                (buffer.len() * 8) as u32,
                &mut needed,
                &mut returned,
                &mut resume,
                std::ptr::null(),
            )
        };
        // SAFETY: no preconditions.
        if ok == 0 && unsafe { GetLastError() } != ERROR_MORE_DATA {
            return Err(last_error("list services"));
        }
        // SAFETY: `returned` records were written at the start of the buffer and
        // their strings point inside it, which outlives this loop body.
        let records = unsafe {
            std::slice::from_raw_parts(
                buffer.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>(),
                returned as usize,
            )
        };
        for record in records {
            // SAFETY: the pointers reference NUL-terminated strings in `buffer`.
            let (name, display) = unsafe {
                (
                    from_wide(record.lpServiceName),
                    from_wide(record.lpDisplayName),
                )
            };
            let status: SERVICE_STATUS_PROCESS = record.ServiceStatusProcess;
            let (state, detail) = state_of(status.dwCurrentState);
            let (startup, description) = query_config(&manager, &name);
            entries.push(ServiceEntry {
                description: match (description.is_empty(), display == name) {
                    (true, _) => display,
                    (false, true) => description,
                    (false, false) => format!("{display} — {description}"),
                },
                name,
                state,
                detail: detail.to_string(),
                startup,
                pid: Some(status.dwProcessId).filter(|pid| *pid > 0),
                scope: String::new(),
            });
        }
        if ok != 0 {
            break;
        }
    }
    sort_by_name(&mut entries);
    Ok(entries)
}

fn query_state(service: &Handle) -> anyhow::Result<u32> {
    let mut status = SERVICE_STATUS_PROCESS::default();
    let mut needed = 0u32;
    // SAFETY: `status` is a correctly sized SERVICE_STATUS_PROCESS.
    let ok = unsafe {
        QueryServiceStatusEx(
            service.0,
            SC_STATUS_PROCESS_INFO,
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast::<u8>(),
            std::mem::size_of::<SERVICE_STATUS_PROCESS>() as u32,
            &mut needed,
        )
    };
    if ok == 0 {
        return Err(last_error("query the service state"));
    }
    Ok(status.dwCurrentState)
}

fn start(service: &Handle) -> anyhow::Result<()> {
    // SAFETY: no arguments are passed to the service.
    if unsafe { StartServiceW(service.0, 0, std::ptr::null()) } == 0 {
        return Err(last_error("start the service"));
    }
    Ok(())
}

/// Ask the service to stop. `tolerate_inactive` lets a restart proceed when the
/// service was not running in the first place.
fn stop(service: &Handle, tolerate_inactive: bool) -> anyhow::Result<()> {
    let mut status = SERVICE_STATUS::default();
    // SAFETY: `status` is a valid out-parameter.
    if unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) } == 0 {
        // SAFETY: no preconditions.
        if tolerate_inactive && unsafe { GetLastError() } == ERROR_SERVICE_NOT_ACTIVE {
            return Ok(());
        }
        return Err(last_error("stop the service"));
    }
    Ok(())
}

fn wait_until_stopped(service: &Handle) -> anyhow::Result<()> {
    let deadline = Instant::now() + STOP_WAIT;
    loop {
        if query_state(service)? == SERVICE_STOPPED {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "the service did not stop within {} seconds",
                STOP_WAIT.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn set_start_type(service: &Handle, start_type: SERVICE_START_TYPE) -> anyhow::Result<()> {
    // SAFETY: every unchanged field is SERVICE_NO_CHANGE or null, as documented.
    let ok = unsafe {
        ChangeServiceConfigW(
            service.0,
            SERVICE_NO_CHANGE,
            start_type,
            SERVICE_NO_CHANGE,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok == 0 {
        return Err(last_error("change the startup type"));
    }
    Ok(())
}

pub fn run_action(name: &str, action: ServiceAction) -> anyhow::Result<()> {
    let manager = open_manager(SC_MANAGER_CONNECT)?;
    let access = match action {
        ServiceAction::Start => SERVICE_START,
        ServiceAction::Stop => SERVICE_STOP,
        ServiceAction::Restart => SERVICE_START | SERVICE_STOP | SERVICE_QUERY_STATUS,
        ServiceAction::Enable | ServiceAction::Disable => SERVICE_CHANGE_CONFIG,
    };
    let service = open_service(
        &manager,
        name,
        access,
        &format!("{} the service", action.label()),
    )?;
    match action {
        ServiceAction::Start => start(&service),
        ServiceAction::Stop => stop(&service, false),
        ServiceAction::Restart => {
            stop(&service, true)?;
            wait_until_stopped(&service)?;
            start(&service)
        }
        ServiceAction::Enable => set_start_type(&service, SERVICE_AUTO_START),
        ServiceAction::Disable => set_start_type(&service, SERVICE_DISABLED),
    }
}
