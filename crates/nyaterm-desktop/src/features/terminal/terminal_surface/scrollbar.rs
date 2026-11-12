use crate::features::NyaTermApp;
use gpui::{Entity, IntoElement, prelude::*};

#[derive(Clone, Debug, PartialEq)]
pub(in crate::features) struct TerminalScrollbarDragState {
    pub session_id: Option<String>,
    pub grab_offset_y: f32,
}

pub(in crate::features) fn terminal_scrollbar_track_bounds_tracker(
    entity: Entity<NyaTermApp>,
    session_id: Option<String>,
) -> impl IntoElement {
    let tracked_entity = entity.clone();
    let tracked_session_id = session_id.clone();
    gpui::canvas(
        move |bounds, window, cx| {
            let visible = window.content_mask().bounds.intersect(&bounds);
            if visible.size.width <= gpui::px(0.) || visible.size.height <= gpui::px(0.) {
                return;
            }
            if tracked_entity
                .read(cx)
                .terminal_scrollbar_track_bounds_for_session(tracked_session_id.as_deref())
                == Some(visible)
            {
                return;
            }
            let entity = entity.clone();
            let session_id = session_id.clone();
            cx.defer(move |cx| {
                entity.update(cx, |this, _cx| {
                    this.remember_terminal_scrollbar_track_bounds_for_session(
                        session_id.as_deref(),
                        visible,
                    );
                });
            });
        },
        |_bounds, _state, _window, _cx| {},
    )
    .absolute()
    .inset_0()
    .size_full()
}
