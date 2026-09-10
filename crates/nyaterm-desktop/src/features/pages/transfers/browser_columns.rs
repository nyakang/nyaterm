use gpui::{Context, MouseDownEvent, MouseMoveEvent};

use crate::features::NyaTermApp;
use crate::models::TransferBrowserSortColumn;

impl NyaTermApp {
    pub(in crate::features::pages::transfers) fn start_transfer_browser_column_resize(
        &mut self,
        column: TransferBrowserSortColumn,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.transfer
            .start_browser_column_resize(column, event.position.x);
        cx.notify();
    }

    pub(in crate::features) fn update_transfer_browser_column_resize(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        if self.transfer.update_browser_column_resize(event.position.x) {
            self.defer_transfer_panel_snapshot_flush(cx);
            cx.notify();
        }
    }

    pub(in crate::features) fn finish_transfer_browser_column_resize(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.transfer.finish_browser_column_resize() {
            self.defer_transfer_panel_snapshot_flush(cx);
            cx.notify();
        }
    }
}
