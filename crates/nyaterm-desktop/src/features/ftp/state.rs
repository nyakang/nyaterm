use std::collections::HashMap;
use std::sync::Arc;

use nyaterm_files::ftp::FtpFileService;

/// Connected FTP services keyed by session id.
///
/// The service owns a blocking control connection, so it is created and
/// dropped by background workers and only shared with file-browser jobs.
#[derive(Default)]
pub(in crate::features) struct FtpFeatureState {
    services: HashMap<String, Arc<FtpFileService>>,
}

impl FtpFeatureState {
    pub(in crate::features) fn insert_service(
        &mut self,
        session_id: String,
        service: Arc<FtpFileService>,
    ) {
        if let Some(previous) = self.services.insert(session_id, service) {
            previous.disconnect();
        }
    }

    pub(in crate::features) fn service(&self, session_id: &str) -> Option<Arc<FtpFileService>> {
        self.services.get(session_id).cloned()
    }

    pub(in crate::features) fn remove_session(
        &mut self,
        session_id: &str,
    ) -> Option<Arc<FtpFileService>> {
        let service = self.services.remove(session_id)?;
        service.disconnect();
        Some(service)
    }
}
