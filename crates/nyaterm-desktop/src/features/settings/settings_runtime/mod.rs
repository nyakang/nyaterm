use gpui::Context;

use crate::features::NyaTermApp;

mod helpers;

mod draft;
mod general_interaction;
mod quit;
mod recording_transfer;
mod search_engines;
mod tab;
mod terminal_remote;

pub(in crate::features) use general_interaction::SettingsSaveKind;

impl NyaTermApp {
    pub(in crate::features) fn open_external_url_for_ui(
        &mut self,
        url: &str,
        cx: &mut Context<Self>,
    ) {
        let url = url.trim();
        match url::Url::parse(url) {
            Ok(parsed) if matches!(parsed.scheme(), "http" | "https" | "mailto") => {
                cx.open_url(parsed.as_str());
                self.shell.set_status(format!("opened URL: {url}"));
            }
            _ => self
                .shell
                .set_status("failed to open URL: unsupported or invalid URL".to_string()),
        }
        cx.notify();
    }

    pub(in crate::features) fn open_documentation(&mut self, cx: &mut Context<Self>) {
        const DOCS_URL: &str = "https://nyaterm.app/docs/";
        self.open_external_url_for_ui(DOCS_URL, cx);
    }
}
