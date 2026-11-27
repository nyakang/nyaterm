use std::collections::HashMap;
use std::sync::Arc;

use nyaterm_files::ftp::FtpFileService;

/// Connected FTP services keyed by session id.
///
/// The service owns a blocking control connection, so it is created and
/// dropped by background workers and only shared with file-browser jobs.
#[derive(Default)]
pub(in crate::features) struct FtpFeatureState {
    services: HashMap<String, FtpSession>,
}

struct FtpSession {
    service: Arc<FtpFileService>,
    _factory: Arc<nyaterm_files::ftp::pool::ConnectionFactory>,
    _maintenance: gpui::Task<()>,
}
impl Drop for FtpSession {
    fn drop(&mut self) {
        self.service.disconnect();
    }
}

impl FtpFeatureState {
    pub(in crate::features) fn insert_service(
        &mut self,
        session_id: String,
        service: Arc<FtpFileService>,
        factory: Arc<nyaterm_files::ftp::pool::ConnectionFactory>,
        maintenance: gpui::Task<()>,
    ) {
        if let Some(previous) = self.services.insert(
            session_id,
            FtpSession {
                service,
                _factory: factory,
                _maintenance: maintenance,
            },
        ) {
            previous.service.disconnect();
        }
    }

    pub(in crate::features) fn service(&self, session_id: &str) -> Option<Arc<FtpFileService>> {
        self.services
            .get(session_id)
            .map(|entry| entry.service.clone())
    }

    pub(in crate::features) fn remove_session(
        &mut self,
        session_id: &str,
    ) -> Option<Arc<FtpFileService>> {
        let service = self.services.remove(session_id)?;
        service.service.disconnect();
        Some(service.service.clone())
    }
}
