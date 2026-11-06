use nyaterm_transport::SessionManager;
use std::sync::Arc;

pub(crate) mod ssh_connections;
use self::ssh_connections::SshConnectionPool;

/// Process-level owner for transport runtimes shared by every workspace window.
///
/// Presentation, focus and prompt state remain window-local. The manager is
/// shared so moving a tab only changes ownership and never tears down its PTY,
/// SSH, Telnet or serial transport.
#[derive(Clone)]
pub struct SessionHub {
    manager: Arc<SessionManager>,
    ssh_connections: SshConnectionPool,
}

impl SessionHub {
    pub fn new() -> Self {
        Self {
            manager: Arc::new(SessionManager::with_ssh_services(
                nyaterm_transport::ssh_services::SshSessionServices {
                    file_session_probe: Some(Arc::new(
                        nyaterm_files::session_probe::SftpSessionProbe,
                    )),
                },
            )),
            ssh_connections: SshConnectionPool::default(),
        }
    }

    pub(crate) fn ssh_connections(&self) -> SshConnectionPool {
        self.ssh_connections.clone()
    }

    pub fn manager(&self) -> Arc<SessionManager> {
        Arc::clone(&self.manager)
    }
}

impl Default for SessionHub {
    fn default() -> Self {
        Self::new()
    }
}
