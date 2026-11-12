use super::{FileAccessPurpose, FilePrivilegeStatus, RemoteFilesSession};
use nyaterm_files::remote_file::RemoteFileService;
use nyaterm_files::sftp::sudo::FileAccessMode;
use nyaterm_transport::{
    SshHostKey, SshHostKeyDecision, SshHostKeyVerifier, SshMultiplexHandle, SshSessionConfig,
    open_ssh_multiplex_handle,
};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::{ChannelId, server};
use std::sync::Arc;

struct Peer;
impl server::Handler for Peer {
    type Error = russh::Error;
    async fn auth_none(&mut self, _: &str) -> Result<server::Auth, Self::Error> {
        Ok(server::Auth::Accept)
    }
    async fn exec_request(
        &mut self,
        id: ChannelId,
        _: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        session.channel_failure(id)?;
        session.close(id)?;
        Ok(())
    }
}
struct Verify;
impl SshHostKeyVerifier for Verify {
    fn verify(&self, _: &SshHostKey) -> Result<SshHostKeyDecision, String> {
        Ok(SshHostKeyDecision::Accept)
    }
}

pub(in crate::features) struct Connection {
    pub(in crate::features) config: SshSessionConfig,
    pub(in crate::features) handle: SshMultiplexHandle,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.handle.disconnect();
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}
pub(in crate::features) fn connection() -> Connection {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let worker = crate::thread_owner::spawn_joinable("file-sudo-fixture", move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let key = russh::keys::PrivateKey::new(
                KeypairData::Ed25519(Ed25519Keypair::from_seed(&[33; 32])),
                "desktop-sudo-fixture",
            )
            .unwrap();
            let config = Arc::new(server::Config {
                keys: vec![key],
                auth_rejection_time: std::time::Duration::ZERO,
                ..Default::default()
            });
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let (socket, _) = listener.accept().await.unwrap();
            let session = server::run_stream(config, socket, Peer).await.unwrap();
            let _ = session.await;
        });
    })
    .unwrap();
    let config = SshSessionConfig {
        host: "127.0.0.1".into(),
        port,
        username: "test".into(),
        allow_none_auth: true,
        host_key_verifier: Some(Arc::new(Verify)),
        ..Default::default()
    };
    let handle = open_ssh_multiplex_handle(config.clone()).unwrap();
    Connection {
        config,
        handle,
        worker: Some(worker),
    }
}
fn session(connection: &Connection) -> RemoteFilesSession {
    RemoteFilesSession::new(
        RemoteFileService::with_multiplex(connection.config.clone(), connection.handle.clone())
            .unwrap(),
    )
}

#[test]
fn automation_stays_normal_during_authorization_elevation_and_revocation() {
    let connection = connection();
    let mut files = session(&connection);
    let old_drag = files.weak();
    let (generation, lease) = files.begin(connection.handle.clone(), None).unwrap();
    assert_eq!(files.status(), FilePrivilegeStatus::Authorizing);
    assert!(files.service(FileAccessPurpose::Interactive).is_err());
    assert_eq!(
        files
            .service(FileAccessPurpose::Automation)
            .unwrap()
            .access_mode(),
        FileAccessMode::LoginUser
    );
    let candidate =
        RemoteFileService::with_sudo(connection.config.clone(), connection.handle.clone(), lease)
            .unwrap();
    assert!(files.finish(generation, Some(candidate)));
    assert!(old_drag.upgrade().is_none());
    let old_root = files.service(FileAccessPurpose::Interactive).unwrap();
    assert_eq!(old_root.access_mode(), FileAccessMode::SudoRoot);
    assert_eq!(
        files
            .service(FileAccessPurpose::Automation)
            .unwrap()
            .access_mode(),
        FileAccessMode::LoginUser
    );
    let (generation, grant) = files.disable().unwrap();
    assert!(!old_root.authorization_is_valid());
    assert!(files.service(FileAccessPurpose::Interactive).is_err());
    grant.close();
    assert!(files.finish_disable(generation));
    assert_eq!(files.status(), FilePrivilegeStatus::Normal);
    assert_eq!(
        files
            .service(FileAccessPurpose::Interactive)
            .unwrap()
            .access_mode(),
        FileAccessMode::LoginUser
    );
}

#[test]
fn cancel_failure_and_late_result_cannot_replace_the_current_identity() {
    let connection = connection();
    let mut files = session(&connection);
    let (first, lease) = files
        .begin(connection.handle.clone(), Some("test-only".into()))
        .unwrap();
    let candidate = RemoteFileService::with_sudo(
        connection.config.clone(),
        connection.handle.clone(),
        lease.clone(),
    )
    .unwrap();
    files.cancel(first);
    assert!(lease.is_revoked());
    let (second, lease) = files.begin(connection.handle.clone(), None).unwrap();
    assert_ne!(first, second);
    assert!(!files.finish(first, Some(candidate)));
    assert_eq!(files.status(), FilePrivilegeStatus::Authorizing);
    assert!(files.finish(second, None));
    assert!(lease.is_revoked());
    assert_eq!(files.status(), FilePrivilegeStatus::Normal);
}

#[test]
fn session_transfer_moves_one_grant_and_cancels_pending_authorization() {
    let connection = connection();
    let mut files = session(&connection);
    let (generation, lease) = files.begin(connection.handle.clone(), None).unwrap();
    let candidate = RemoteFileService::with_sudo(
        connection.config.clone(),
        connection.handle.clone(),
        lease.clone(),
    )
    .unwrap();
    files.finish(generation, Some(candidate));
    let mut source = Some(files);
    let mut target = source.take().unwrap();
    target.cancel_pending();
    assert!(source.is_none());
    assert!(!lease.is_revoked());
    assert_eq!(target.status(), FilePrivilegeStatus::Elevated);
    drop(target);
    assert!(lease.is_revoked());
    let mut pending = session(&connection);
    let (generation, lease) = pending.begin(connection.handle.clone(), None).unwrap();
    pending.cancel_pending();
    assert!(lease.is_revoked());
    assert!(!pending.finish(generation, None));
    assert_eq!(pending.status(), FilePrivilegeStatus::Normal);
}

#[test]
fn disconnect_invalidates_saved_service_clones_and_new_session_has_no_grant() {
    let connection = connection();
    let mut files = session(&connection);
    let (generation, lease) = files.begin(connection.handle.clone(), None).unwrap();
    files.finish(
        generation,
        Some(
            RemoteFileService::with_sudo(
                connection.config.clone(),
                connection.handle.clone(),
                lease,
            )
            .unwrap(),
        ),
    );
    let old = files.service(FileAccessPurpose::Interactive).unwrap();
    connection.handle.disconnect().unwrap();
    assert_eq!(files.status(), FilePrivilegeStatus::Unavailable);
    assert!(files.service(FileAccessPurpose::Interactive).is_err());
    assert!(!old.authorization_is_valid());
    let fresh = RemoteFilesSession::new(RemoteFileService::new(connection.config.clone()));
    assert_eq!(fresh.status(), FilePrivilegeStatus::Normal);
}
