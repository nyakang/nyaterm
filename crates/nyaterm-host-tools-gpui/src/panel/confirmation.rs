//! The caller presents confirmation; the request retains its original panel and host.
use super::HostToolsPanel;
use gpui::{App, WeakEntity, Window};
use rust_i18n::t;
use std::cell::Cell;
use std::rc::Rc;

pub type HostToolConfirmationHandler = Rc<dyn Fn(HostToolConfirmation, &mut Window, &mut App)>;

/// A one-shot approval bound to the panel and host that requested it.
/// Dropping it is cancellation; it owns no session or credential.
pub struct HostToolConfirmation {
    panel: WeakEntity<HostToolsPanel>,
    epoch: u64,
    generation: u64,
    pid: u32,
    force: bool,
    consumed: Cell<bool>,
}
impl HostToolConfirmation {
    pub(super) fn new(
        panel: WeakEntity<HostToolsPanel>,
        epoch: u64,
        generation: u64,
        pid: u32,
        force: bool,
    ) -> Self {
        Self {
            panel,
            epoch,
            generation,
            pid,
            force,
            consumed: Cell::new(false),
        }
    }
    pub fn title(&self) -> String {
        t!(if self.force {
            "hostTools.killTitle"
        } else {
            "hostTools.stopTitle"
        })
        .to_string()
    }
    pub fn description(&self) -> String {
        t!(
            if self.force {
                "hostTools.confirmKill"
            } else {
                "hostTools.confirmStop"
            },
            pid = self.pid
        )
        .to_string()
    }
    pub fn confirm_label(&self) -> String {
        t!(if self.force {
            "hostTools.killProcess"
        } else {
            "hostTools.stopProcess"
        })
        .to_string()
    }
    /// Ignores repeated approval, hidden/destroyed panels and obsolete hosts.
    pub fn approve(&self, cx: &mut App) {
        if self.consumed.replace(true) {
            return;
        }
        let _ = self.panel.update(cx, |panel, cx| {
            if panel.can_refresh() && panel.confirmation_generation == self.generation {
                panel.kill_pid_for_epoch(self.epoch, self.pid, self.force, cx);
            }
        });
    }
}
