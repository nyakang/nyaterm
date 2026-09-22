//! Lifecycle state for one secondary (child) window.
//!
//! Five features open a child window: settings, the connection editor, the quick
//! command editor, the remote text editor, and the external-editor sync prompt.
//! Each used to carry its own `Option<NyaWindowHandle>` plus an `open_pending`
//! bool, with four different vocabularies for the same four transitions.
//! `ChildWindowSlot` is that state, once.
//!
//! The pending flag exists because a window is opened from a deferred callback
//! rather than inline: `cx.open_window` inside an entity update would re-enter
//! the app entity that update already borrows. Between the request and the
//! deferred open there is a gap in which a second request must not start a
//! second OS window, and [`ChildWindowSlot::begin_open`] is what closes it.

use gpui::{App, Entity};

use crate::root::NyaWindowHandle;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ChildWindowSlot {
    window: Option<NyaWindowHandle>,
    open_pending: bool,
}

impl ChildWindowSlot {
    pub fn handle(&self) -> Option<NyaWindowHandle> {
        self.window
    }

    pub fn is_open(&self) -> bool {
        self.window.is_some()
    }

    pub fn is_pending(&self) -> bool {
        self.open_pending
    }

    pub fn is_open_or_pending(&self) -> bool {
        self.is_open() || self.is_pending()
    }

    /// Claim the right to open the window.
    ///
    /// Returns `false` when a window is already open or an open is already in
    /// flight, in which case the caller must not open one.
    pub fn begin_open(&mut self) -> bool {
        if self.is_open_or_pending() {
            return false;
        }
        self.open_pending = true;
        true
    }

    /// Give up a claim without having opened anything.
    pub fn cancel_open(&mut self) {
        self.open_pending = false;
    }

    pub fn finish_open(&mut self, handle: NyaWindowHandle) {
        self.window = Some(handle);
        self.open_pending = false;
    }

    /// `cx.open_window` failed: drop both the claim and any stale handle.
    pub fn fail_open(&mut self) {
        self.window = None;
        self.open_pending = false;
    }

    pub fn clear(&mut self) {
        self.window = None;
        self.open_pending = false;
    }

    /// Clear only if `handle` is still the one this slot owns.
    ///
    /// A deferred callback can outlive the window it captured, so it must not
    /// clear a newer window that replaced it. Returns whether anything changed.
    pub fn clear_if(&mut self, handle: NyaWindowHandle) -> bool {
        if self.window != Some(handle) {
            return false;
        }
        self.window = None;
        true
    }
}

/// Raise an already-open child window, dropping the handle if it is stale.
///
/// The raise is deferred because `handle.update` re-enters `App` while `owner`
/// is still borrowed by the update this is called from. Closing over the handle
/// means the deferred callback can clear a window that was replaced in the
/// meantime, which is why it clears through [`ChildWindowSlot::clear_if`].
///
/// This is the whole of "a window we think is open may already be gone": GPUI
/// reports that only as an error from `handle.update`, and nothing observes a
/// child window's release.
///
/// `slot` returns an `Option` because a slot can be keyed by domain state -- the
/// external-editor prompts hold one per prompt id -- and by the time the deferred
/// callback runs that state may be gone. Returning `None` then is the honest
/// answer, and keeps the accessor from having to insert an entry to hand back.
///
/// There is no "draw attention" variant, and there should not need to be: the
/// windows that must not be worked around are `WindowKind::Dialog`, so the
/// platform stops input from reaching their owner in the first place. A window
/// the user cannot reach cannot ask to be flashed.
pub fn activate_child_window<T: 'static>(
    owner: &Entity<T>,
    handle: NyaWindowHandle,
    slot: impl Fn(&mut T) -> Option<&mut ChildWindowSlot> + 'static,
    cx: &mut App,
) {
    let owner = owner.clone();
    cx.defer(move |cx| {
        let raised = handle.update(cx, |_, window, _| window.activate_window());
        if raised.is_err() {
            owner.update(cx, |owner, cx| {
                if slot(owner).is_some_and(|slot| slot.clear_if(handle)) {
                    cx.notify();
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use gpui::AppContext as _;

    use super::ChildWindowSlot;

    #[test]
    fn a_fresh_slot_is_neither_open_nor_pending() {
        let slot = ChildWindowSlot::default();
        assert!(!slot.is_open());
        assert!(!slot.is_pending());
        assert!(!slot.is_open_or_pending());
        assert!(slot.handle().is_none());
    }

    #[test]
    fn begin_open_claims_once_and_refuses_a_second_claim() {
        let mut slot = ChildWindowSlot::default();
        assert!(slot.begin_open());
        assert!(slot.is_pending());
        assert!(slot.is_open_or_pending());
        // The second request must not start a second OS window.
        assert!(!slot.begin_open());
    }

    #[test]
    fn cancel_and_fail_release_the_claim_so_a_later_open_can_proceed() {
        let mut slot = ChildWindowSlot::default();
        assert!(slot.begin_open());
        slot.cancel_open();
        assert!(!slot.is_pending());
        assert!(slot.begin_open());

        slot.fail_open();
        assert!(!slot.is_open_or_pending());
        assert!(slot.begin_open());
    }

    #[test]
    fn clear_releases_a_pending_claim_as_well_as_the_handle() {
        let mut slot = ChildWindowSlot::default();
        assert!(slot.begin_open());
        slot.clear();
        assert!(!slot.is_open_or_pending());
    }

    /// A deferred callback captures the handle it was created for. If the window
    /// it captured was replaced in the meantime, clearing must be a no-op, or the
    /// callback tears down a window that is still on screen.
    #[gpui::test]
    fn clear_if_only_clears_the_handle_it_was_given(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let first = cx.add_window(|window, cx| {
            let view = cx.new(|_| SlotWindowFixture);
            crate::nya_root(view, window, cx)
        });
        let second = cx.add_window(|window, cx| {
            let view = cx.new(|_| SlotWindowFixture);
            crate::nya_root(view, window, cx)
        });
        assert_ne!(first, second);

        let mut slot = ChildWindowSlot::default();
        assert!(slot.begin_open());
        slot.finish_open(first);
        assert!(slot.is_open());
        assert!(!slot.is_pending(), "finishing an open ends the claim");
        assert_eq!(slot.handle(), Some(first));

        assert!(
            !slot.clear_if(second),
            "a stale handle must not clear the slot"
        );
        assert_eq!(slot.handle(), Some(first));

        assert!(slot.clear_if(first));
        assert!(!slot.is_open());
        // Already cleared: a second callback for the same window changes nothing.
        assert!(!slot.clear_if(first));
    }

    struct SlotOwnerFixture {
        slot: ChildWindowSlot,
    }

    impl gpui::Render for SlotOwnerFixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::div()
        }
    }

    /// A window that has already gone away is only ever discovered by trying to
    /// use it: nothing observes a child window's release, so the failed raise is
    /// the only signal, and it has to leave the slot empty.
    #[gpui::test]
    async fn raising_a_window_that_is_already_gone_clears_the_slot(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.add_window(|window, cx| {
            let view = cx.new(|_| SlotWindowFixture);
            crate::nya_root(view, window, cx)
        });
        let owner = cx.new(|_| SlotOwnerFixture {
            slot: ChildWindowSlot::default(),
        });
        owner.update(cx, |owner, _| owner.slot.finish_open(window));

        window
            .update(cx, |_, window, _| window.remove_window())
            .expect("the window is still open here");
        cx.run_until_parked();

        cx.update(|cx| {
            super::activate_child_window(
                &owner,
                window,
                |owner: &mut SlotOwnerFixture| Some(&mut owner.slot),
                cx,
            );
        });
        cx.run_until_parked();

        owner.update(cx, |owner, _| {
            assert!(
                !owner.slot.is_open(),
                "the handle of a window that is gone should be dropped"
            );
        });
    }

    struct SlotWindowFixture;

    impl gpui::Render for SlotWindowFixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::div()
        }
    }
}
