use std::sync::mpsc::{self, Receiver};

use rust_i18n::t;
use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::UI::Controls::{
    TASKDIALOGCONFIG, TDCBF_CLOSE_BUTTON, TDE_CONTENT, TDE_MAIN_INSTRUCTION, TDF_CALLBACK_TIMER,
    TDF_SHOW_MARQUEE_PROGRESS_BAR, TDM_CLICK_BUTTON, TDM_ENABLE_BUTTON, TDM_SET_ELEMENT_TEXT,
    TDM_SET_PROGRESS_BAR_MARQUEE, TDN_BUTTON_CLICKED, TDN_CREATED, TDN_TIMER, TaskDialogIndirect,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, SendMessageW};

use nyaterm_updater::install::UpdateStage;

fn stage_label(stage: UpdateStage) -> String {
    match stage {
        UpdateStage::Waiting => t!("updater.waitingForExit").to_string(),
        UpdateStage::BackingUp => t!("updater.backingUpInstall").to_string(),
        UpdateStage::Installing => t!("updater.applyingInstall").to_string(),
        UpdateStage::Replacing => t!("updater.replacingFiles").to_string(),
        UpdateStage::Starting => t!("updater.startingUpdatedApp").to_string(),
    }
}

enum Event {
    Stage(UpdateStage),
    Finished(Result<(), String>),
}

struct DialogState {
    rx: Receiver<Event>,
    finished: bool,
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn set_text(hwnd: HWND, element: i32, value: &str) {
    let value = wide(value);
    unsafe {
        SendMessageW(
            hwnd,
            TDM_SET_ELEMENT_TEXT as u32,
            element as WPARAM,
            value.as_ptr() as LPARAM,
        );
    }
}

unsafe extern "system" fn dialog_callback(
    hwnd: HWND,
    notification: u32,
    _wparam: WPARAM,
    _lparam: LPARAM,
    data: isize,
) -> i32 {
    let state = unsafe { &mut *(data as *mut DialogState) };
    match notification as i32 {
        TDN_CREATED => unsafe {
            SendMessageW(hwnd, TDM_ENABLE_BUTTON as u32, 8, 0);
            SendMessageW(hwnd, TDM_SET_PROGRESS_BAR_MARQUEE as u32, 1, 0);
        },
        TDN_TIMER => {
            while let Ok(event) = state.rx.try_recv() {
                match event {
                    Event::Stage(stage) => unsafe {
                        set_text(hwnd, TDE_CONTENT, &stage_label(stage))
                    },
                    Event::Finished(Ok(())) => {
                        state.finished = true;
                        unsafe { PostMessageW(hwnd, TDM_CLICK_BUTTON as u32, 8, 0) };
                    }
                    Event::Finished(Err(error)) => {
                        state.finished = true;
                        unsafe {
                            SendMessageW(hwnd, TDM_SET_PROGRESS_BAR_MARQUEE as u32, 0, 0);
                            set_text(hwnd, TDE_MAIN_INSTRUCTION, &t!("updater.updateFailed"));
                            set_text(hwnd, TDE_CONTENT, &error);
                            SendMessageW(hwnd, TDM_ENABLE_BUTTON as u32, 8, 1);
                        }
                    }
                }
            }
        }
        TDN_BUTTON_CLICKED if !state.finished => return 1,
        _ => {}
    }
    0
}

pub(super) fn run(
    language: &str,
    operation: impl FnOnce(&mut dyn FnMut(UpdateStage)) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let i18n_ready = crate::preload_i18n().is_ok();
    if i18n_ready {
        crate::i18n::apply_locale(language);
    }
    let (tx, rx) = mpsc::channel();
    let worker = crate::thread_owner::spawn_joinable("nyaterm-update", move || {
        let mut report = |stage| {
            let _ = tx.send(Event::Stage(stage));
        };
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(&mut report)))
                .unwrap_or_else(|_| Err("update helper panicked".to_string()));
        let _ = tx.send(Event::Finished(result.clone()));
        result
    })
    .map_err(|error| format!("failed to start update helper: {error}"))?;

    if i18n_ready {
        let title = wide("NyaTerm");
        let instruction = wide(&t!("updater.installing"));
        let content = wide(&stage_label(UpdateStage::Waiting));
        let mut state = DialogState {
            rx,
            finished: false,
        };
        let config = TASKDIALOGCONFIG {
            cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
            dwFlags: TDF_CALLBACK_TIMER | TDF_SHOW_MARQUEE_PROGRESS_BAR,
            dwCommonButtons: TDCBF_CLOSE_BUTTON,
            pszWindowTitle: title.as_ptr(),
            pszMainInstruction: instruction.as_ptr(),
            pszContent: content.as_ptr(),
            pfCallback: Some(dialog_callback),
            lpCallbackData: (&mut state as *mut DialogState) as isize,
            ..Default::default()
        };
        unsafe {
            TaskDialogIndirect(
                &config,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        }
    }

    worker
        .join()
        .map_err(|_| "update helper panicked".to_string())?
}
