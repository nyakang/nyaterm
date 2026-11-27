pub(in crate::features) mod labels;
mod state;

pub(in crate::features) use state::FtpFeatureState;

impl crate::features::NyaTermApp {
    pub(in crate::features) fn register_ftp_service(
        &mut self,
        id: String,
        service: std::sync::Arc<nyaterm_files::ftp::FtpFileService>,
        config: &crate::models::FtpSessionConfig,
        cx: &mut gpui::Context<Self>,
    ) {
        let settings = config.config.clone();
        let login = config.login.clone();
        let factory: std::sync::Arc<nyaterm_files::ftp::pool::ConnectionFactory> =
            std::sync::Arc::new(move |cancelled| {
                nyaterm_files::ftp::FtpFileService::connect_with_cancel(
                    settings.clone(),
                    login.clone(),
                    move || cancelled(),
                )
            });
        let limit = self
            .settings
            .summary()
            .transfer_download_threads
            .max(self.settings.summary().transfer_upload_threads) as usize;
        service.configure_connections(
            std::sync::Arc::downgrade(&factory),
            limit,
            self.settings.summary().transfer_max_retries,
        );
        let weak = std::sync::Arc::downgrade(&service);
        let scheduler = self.blocking_jobs.clone();
        let maintenance = cx.spawn(async move |_, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(30))
                    .await;
                let Some(service) = weak.upgrade().filter(|s| s.is_available()) else {
                    break;
                };
                let (sender, receiver) = futures::channel::oneshot::channel();
                if scheduler
                    .submit_detached("ftp-keepalive", move |_| {
                        service.keep_alive();
                        let _ = sender.send(());
                    })
                    .is_err()
                {
                    break;
                }
                if receiver.await.is_err() {
                    break;
                }
            }
        });
        self.ftp.insert_service(id, service, factory, maintenance);
    }
}
