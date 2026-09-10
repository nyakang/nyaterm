use nyaterm_core::SessionsConfig;

use crate::features::NyaTermApp;

impl NyaTermApp {
    /// The one place a store reply becomes the visible connection catalog.
    ///
    /// Ten handlers used to reach into `connection_state` and spread the same two
    /// fields themselves. They differ in what they do around it -- some clear list
    /// runtime state first, some finish an editor save afterwards -- so this funnel
    /// deliberately covers only the catalog swap and leaves that ordering alone.
    ///
    /// It is not where the panel snapshot is rebuilt. That happens once per store
    /// reply in `submit_store_request`, after the whole handler body has run, so a
    /// handler that mutates *after* the swap still flushes fresh state.
    pub(in crate::features) fn apply_loaded_sessions(
        &mut self,
        sessions: SessionsConfig,
        cx: &mut gpui::Context<Self>,
    ) {
        self.update_custom_icons(sessions.custom_icons, cx);
        self.connection_state
            .replace_loaded(sessions.connections, sessions.groups);
    }
}
