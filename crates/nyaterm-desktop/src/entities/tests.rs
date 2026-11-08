use super::{OverlayStore, StartupRestoreStore};
use gpui::AppContext as _;

#[test]
fn startup_restore_store_starts_after_window_open_once() {
    let mut store = StartupRestoreStore::default();

    assert!(store.mark_started_after_window_open());
    assert!(!store.mark_started_after_window_open());
    assert!(store.started_after_window_open());
}

#[test]
fn startup_restore_store_tracks_queue_admission() {
    let mut store = StartupRestoreStore::default();
    store.set_queue(vec![
        nyaterm_core::RestorableOpenTab::with_leaf_root("one", "Local", None, None, None),
        nyaterm_core::RestorableOpenTab::with_leaf_root("two", "Local", None, None, None),
    ]);

    assert_eq!(store.queue_len(), 2);
    assert!(store.can_pump_queue(false));
    assert!(!store.can_pump_queue(true));
    assert_eq!(
        store.pop_next_tab().as_ref().map(|tab| tab.title.as_str()),
        Some("one")
    );
    assert_eq!(store.queue_len(), 1);
    assert_eq!(
        store.pop_next_tab().as_ref().map(|tab| tab.title.as_str()),
        Some("two")
    );
    assert!(!store.can_pump_queue(false));
    assert!(!store.can_finish_restore(false));
    assert!(store.mark_open_tabs_restored());
    assert!(store.can_finish_restore(false));
    assert!(!store.can_finish_restore(true));
    assert!(!store.can_pump_queue(true));
}

struct OverlayTestView;

impl gpui::Render for OverlayTestView {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        gpui::div()
    }
}

#[gpui::test]
fn overlay_store_owns_quick_switch_state(cx: &mut gpui::TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| OverlayTestView);

    cx.update(|window, cx| {
        let command_state = cx.new(|cx| nyaterm_ui::NyaCommandState::new(window, cx));
        let mut store = OverlayStore::default();

        assert!(!store.quick_switch().is_open());
        assert!(store.open_quick_switch(command_state.clone()));
        assert!(store.quick_switch().is_open());
        assert_eq!(
            store.quick_switch().command_state(),
            Some(command_state.clone())
        );
        assert_eq!(command_state.read(cx).query(cx), "");
        assert!(store.close_quick_switch());
        assert!(!store.close_quick_switch());
        assert!(!store.quick_switch().is_open());
        assert!(store.quick_switch().command_state().is_none());
    });
}
