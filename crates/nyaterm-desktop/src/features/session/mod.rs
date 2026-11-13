//! Session lifecycle, prompts, recording and file-transfer session runtimes.
mod transfer_wire;

use crate::app_shell::session_hub::ssh_connections::SshConnectionLease;
use remote_files::RemoteFilesSession;
use std::collections::HashMap;

mod auth_runtime;
mod duplicate_prompts;
pub(in crate::features) mod notice_runtime;
pub(in crate::features) mod notices;
mod prompt_runtime;
mod recording_runtime;
pub(in crate::features) mod remote_files;
pub(in crate::features) mod runtime_auth;
mod session_dialog_runtime;
mod session_lifecycle;
mod session_order;
mod session_runtime;
pub(in crate::features) use session_runtime::{
    SessionStartFocusTarget, SshSessionConfigBuildContext,
};
mod session_state;
mod start_tab_actions;
mod startup_restore_runtime;
mod state;
mod temporary_ssh_link;
mod trzsz_runtime;
mod xymodem_runtime;
pub(in crate::features) use xymodem_runtime::SerialUploadProtocol;
mod zmodem_runtime;

#[derive(Default)]
struct SessionProtocolRuntimeState {
    xymodem: HashMap<String, xymodem_runtime::XymodemSessionState>,
    zmodem: HashMap<String, zmodem_runtime::ZmodemSessionState>,
    trzsz: HashMap<String, trzsz_runtime::TrzszSessionState>,
    remote_files: HashMap<String, RemoteFilesSession>,
    ssh_connections: HashMap<String, SshConnectionLease>,
}

impl SessionProtocolRuntimeState {
    fn shutdown_workers(&mut self) {
        for state in self.xymodem.values_mut() {
            state.stop_worker();
        }
        for state in self.zmodem.values_mut() {
            state.stop_worker();
        }
        for state in self.trzsz.values_mut() {
            state.stop_workers();
        }
        self.remote_files.clear();
        self.ssh_connections.clear();
    }
}

impl Drop for SessionProtocolRuntimeState {
    fn drop(&mut self) {
        self.shutdown_workers();
    }
}

pub(in crate::features) use auth_runtime::{
    AgentPromptBroker, AgentPromptRequest, AgentPromptState, CredentialPromptBroker,
    CredentialPromptRequest, CredentialPromptState, HostKeyPromptBroker, HostKeyPromptChoice,
    HostKeyPromptIssue, HostKeyPromptRequest, KeyboardInteractivePromptState,
    NativeHostKeyVerifier, NativeOtpProvider, unix_seconds_now,
};
pub(in crate::features) use duplicate_prompts::{
    DuplicatePromptAction, SftpDuplicatePromptState, UploadBatchDuplicateResolver,
};
pub(in crate::features) use prompt_runtime::{
    credential_prompt_id, credential_prompt_target, credential_text_input_id,
    keyboard_interactive_prompt_id, keyboard_interactive_prompt_target,
    keyboard_interactive_text_input_id, prompt_input_setup, sftp_duplicate_prompt_id,
    uuid_like_prompt_id,
};
pub(in crate::features) use state::{
    PendingSessionStart, SavedConnectionStartOptions, SessionCatalogTransferBundle,
    SessionFeatureFocus, SessionFeatureState, SessionStartEventRequest, SessionStartTabPlacement,
};
