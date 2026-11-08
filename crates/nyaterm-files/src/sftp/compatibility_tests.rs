use super::{
    OpenSftpConnection, OpenSftpSession, SFTP_COMPATIBILITY_CACHE, SftpService, open_sftp_session,
    sftp_error_category, sftp_error_invalidates_compatibility_session,
};
use nyaterm_transport::{SftpSettings, SshSessionConfig};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{Status, StatusCode, Version};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

struct Peer {
    initializations: Arc<AtomicUsize>,
    renames: Arc<AtomicUsize>,
}

impl russh_sftp::server::Handler for Peer {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        self.initializations.fetch_add(1, Ordering::SeqCst);
        Ok(Version::new())
    }

    async fn rename(
        &mut self,
        id: u32,
        _oldpath: String,
        _newpath: String,
    ) -> Result<Status, Self::Error> {
        self.renames.fetch_add(1, Ordering::SeqCst);
        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: String::new(),
            language_tag: "en".into(),
        })
    }
}

fn seeded_service(
    ssh_compatibility_mode: bool,
) -> (SftpService, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let service = SftpService::new(SshSessionConfig {
        ssh_compatibility_mode,
        sftp: SftpSettings {
            compatibility_mode: true,
            ..Default::default()
        },
        ..Default::default()
    });
    let state = service.compatibility.as_ref().unwrap();
    let initializations = Arc::new(AtomicUsize::new(0));
    let renames = Arc::new(AtomicUsize::new(0));
    let peer = Peer {
        initializations: initializations.clone(),
        renames: renames.clone(),
    };
    let session = state
        .block_on(async move {
            let (client, server) = tokio::io::duplex(4096);
            tokio::spawn(russh_sftp::server::run(server, peer));
            Ok(OpenSftpSession {
                sftp: Arc::new(SftpSession::new(client).await?),
                connection: Arc::new(Mutex::new(Some(OpenSftpConnection::Multiplex))),
                persistent: true,
            })
        })
        .unwrap();
    *state.cache.lock().unwrap() = Some(session);
    (service, initializations, renames)
}

#[test]
fn repeated_operations_and_clones_reuse_one_live_peer_session() {
    for enabled in [false, true] {
        let (service, initializations, renames) = seeded_service(enabled);
        service.rename_path("/a", "/b").unwrap();
        service.clone().rename_path("/b", "/c").unwrap();
        assert_eq!(initializations.load(Ordering::SeqCst), 1);
        assert_eq!(renames.load(Ordering::SeqCst), 2);
        assert!(
            service
                .compatibility
                .as_ref()
                .unwrap()
                .cache
                .lock()
                .unwrap()
                .is_some()
        );
    }
}

#[test]
fn destination_scope_selects_its_own_session_and_restores_source_scope() {
    let (source, _, _) = seeded_service(true);
    let (destination, _, _) = seeded_service(true);
    let source_state = source.compatibility.as_ref().unwrap();
    let destination_cache = destination.compatibility.as_ref().unwrap().cache.clone();
    let source_session = source_state
        .cache
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .sftp
        .clone();
    let destination_session = destination_cache
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .sftp
        .clone();
    source_state
        .block_on(async move {
            let config = SshSessionConfig::default();
            let first = open_sftp_session(&config, None).await?;
            assert!(Arc::ptr_eq(&first.sftp, &source_session));
            let second = SFTP_COMPATIBILITY_CACHE
                .scope(Some(destination_cache), open_sftp_session(&config, None))
                .await?;
            assert!(Arc::ptr_eq(&second.sftp, &destination_session));
            let last = open_sftp_session(&config, None).await?;
            assert!(Arc::ptr_eq(&last.sftp, &source_session));
            Ok(())
        })
        .unwrap();
}

#[test]
fn closed_session_is_evicted_without_retrying_the_operation() {
    let (service, _, renames) = seeded_service(true);
    let state = service.compatibility.as_ref().unwrap();
    let session = state.cache.lock().unwrap().as_ref().unwrap().sftp.clone();
    // Make the close/write race deterministic: a request queued immediately after the close
    // sentinel may time out before the writer task has dropped its receiver.
    session.set_timeout(0);
    state
        .block_on(async move {
            session.close().await?;
            Ok(())
        })
        .unwrap();
    let error = service.rename_path("/a", "/b").unwrap_err();
    assert!(
        state.cache.lock().unwrap().is_none(),
        "closed session stayed cached after {error:?} (category {})",
        sftp_error_category(&error)
    );
    assert_eq!(renames.load(Ordering::SeqCst), 0);
}

#[test]
fn diagnostic_categories_never_include_server_error_messages() {
    let error = anyhow::Error::from(russh_sftp::client::error::Error::Status(Status {
        id: 1,
        status_code: StatusCode::Failure,
        error_message: "server-controlled-secret".into(),
        language_tag: "en".into(),
    }));
    assert_eq!(sftp_error_category(&error), "operation_failed");
}

#[test]
fn closed_or_timed_out_requests_invalidate_compatibility_sessions() {
    let dropped = anyhow::Error::from(russh_sftp::client::error::Error::UnexpectedBehavior(
        "sender dropped".into(),
    ));
    assert!(sftp_error_invalidates_compatibility_session(&dropped));
    assert_eq!(sftp_error_category(&dropped), "stream_closed");

    let timeout = anyhow::Error::from(russh_sftp::client::error::Error::Timeout);
    assert!(sftp_error_invalidates_compatibility_session(&timeout));
    assert_eq!(sftp_error_category(&timeout), "timeout");
}

#[test]
fn compatibility_download_limits_do_not_change_normal_mode_policy() {
    let options = crate::sftp_transfer_types::SftpTransferOptions::default()
        .with_download_threads(4)
        .with_directory_upload_threads(8);
    let normal = SftpService::new(SshSessionConfig::default());
    let normal_options = normal.effective_transfer_options(options.clone());
    assert_eq!(normal_options.download_threads(), 4);
    assert_eq!(normal_options.directory_upload_threads(), 8);
    let (compatible, _, _) = seeded_service(true);
    let compatibility_options = compatible.effective_transfer_options(options);
    assert_eq!(compatibility_options.download_threads(), 1);
    assert_eq!(compatibility_options.directory_upload_threads(), 1);
}
