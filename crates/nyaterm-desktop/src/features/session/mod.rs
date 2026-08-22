//! Session lifecycle, prompts, recording and file-transfer session runtimes.

use std::collections::HashMap;

use nyaterm_transport::{RemoteFileService, SshMultiplexHandle};

mod auth_runtime;
mod prompt_runtime;
mod recording_runtime;
mod session_dialog_runtime;
mod session_lifecycle;
mod session_order;
mod session_runtime;
mod session_state;
mod startup_restore_runtime;
mod state;
mod temporary_ssh_link;
mod trzsz_runtime;
mod zmodem_runtime;

#[derive(Default)]
struct SessionProtocolRuntimeState {
    zmodem: HashMap<String, zmodem_runtime::ZmodemSessionState>,
    trzsz: HashMap<String, trzsz_runtime::TrzszSessionState>,
    remote_files: HashMap<String, RemoteFileService>,
    multiplex_handles: HashMap<String, SshMultiplexHandle>,
}

impl Drop for SessionProtocolRuntimeState {
    fn drop(&mut self) {
        for (_, handle) in std::mem::take(&mut self.multiplex_handles) {
            disconnect_multiplex_handle(handle);
        }
    }
}

fn disconnect_multiplex_handle(handle: SshMultiplexHandle) {
    if let Err(error) = std::thread::Builder::new()
        .name("nyaterm-ssh-multiplex-disconnect".to_string())
        .spawn(move || {
            if let Err(error) = handle.disconnect() {
                tracing::warn!(error = %error, "failed to disconnect SSH multiplex handle");
            }
        })
    {
        tracing::warn!(error = %error, "failed to spawn SSH multiplex disconnect worker");
    }
}

pub(in crate::features) use auth_runtime::{
    AgentPromptBroker, AgentPromptRequest, AgentPromptState, CredentialPromptBroker,
    CredentialPromptRequest, CredentialPromptState, HostKeyPromptBroker, HostKeyPromptChoice,
    HostKeyPromptIssue, HostKeyPromptRequest, KeyboardInteractivePromptState,
    NativeHostKeyVerifier, NativeOtpProvider, SftpDuplicatePromptState, unix_seconds_now,
};
pub(in crate::features) use prompt_runtime::{
    credential_prompt_id, credential_prompt_target, credential_text_input_id,
    keyboard_interactive_prompt_id, keyboard_interactive_prompt_target,
    keyboard_interactive_text_input_id, sftp_duplicate_prompt_id, uuid_like_prompt_id,
};
pub(in crate::features) use state::{
    PendingSessionStart, SavedConnectionStartOptions, SessionFeatureFocus, SessionFeatureState,
    SessionStartEventRequest, SessionStartTabPlacement,
};
