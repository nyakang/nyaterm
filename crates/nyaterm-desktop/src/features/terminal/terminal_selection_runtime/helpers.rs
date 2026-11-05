use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    Bounds, Context, ElementInputHandler, EntityInputHandler, IntoElement, Pixels, Point,
    UTF16Selection, Window, prelude::*, px,
};

use crate::features::NyaTermApp;

fn terminal_input_selection() -> UTF16Selection {
    UTF16Selection {
        range: 0..0,
        reversed: false,
    }
}

fn terminal_visible_surface_bounds(
    bounds: Bounds<Pixels>,
    content_mask: Bounds<Pixels>,
) -> Option<Bounds<Pixels>> {
    let visible = content_mask.intersect(&bounds);
    (visible.size.width > px(0.) && visible.size.height > px(0.)).then_some(visible)
}

/// Invisible canvas child that records the terminal output bounds for selection hit-testing.
pub(in crate::features) fn terminal_bounds_tracker(
    entity: gpui::Entity<NyaTermApp>,
    session_id: Option<String>,
    active: bool,
) -> impl IntoElement {
    terminal_bounds_tracker_with_exclusion(entity, session_id, active, Rc::default())
}

/// Frame-local geometry shared by the grid and its composition overlay. The
/// overlay resolves it during prepaint, before either element starts painting.
pub(in crate::features) fn terminal_bounds_tracker_with_exclusion(
    entity: gpui::Entity<NyaTermApp>,
    session_id: Option<String>,
    active: bool,
    exclusion: Rc<Cell<Option<Bounds<Pixels>>>>,
) -> impl IntoElement {
    let input_entity = entity.clone();
    let bounds_entity = input_entity.clone();
    let tracked_session_id = session_id.clone();
    gpui::canvas(
        move |bounds, window, cx| {
            exclusion.set(None);
            let bounds = terminal_visible_surface_bounds(bounds, window.content_mask().bounds)?;
            let ime = if active {
                bounds_entity
                    .read(cx)
                    .terminal_ime_layout(bounds, window, cx)
            } else {
                None
            };
            exclusion.set(ime.as_ref().map(|layout| layout.bounds));
            let scale_factor = window.scale_factor();
            if bounds_entity
                .read(cx)
                .terminal_surface_bounds_tracking_is_current(
                    session_id.as_deref(),
                    bounds,
                    scale_factor,
                )
            {
                return ime;
            }
            // Defer mutation so we never re-enter the entity while layout/prepaint is running.
            let entity = entity.clone();
            let session_id = tracked_session_id.clone();
            cx.defer(move |cx| {
                entity.update(cx, |this, cx| {
                    this.remember_terminal_surface_bounds_for_session_and_sync(
                        session_id.as_deref(),
                        bounds,
                        scale_factor,
                        cx,
                    );
                });
            });
            ime
        },
        move |bounds, ime, window, cx| {
            if !active {
                return;
            }
            let Some(bounds) =
                terminal_visible_surface_bounds(bounds, window.content_mask().bounds)
            else {
                return;
            };
            let focus = input_entity.read(cx).terminal.input.focus.clone();
            window.handle_input(&focus, ElementInputHandler::new(bounds, input_entity), cx);
            if let Some(ime) = ime {
                ime.paint(window, cx);
            }
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

impl EntityInputHandler for NyaTermApp {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        if !self
            .terminal
            .input
            .ime
            .is_active_for(self.session.active_id())
        {
            return None;
        }
        let (text, range) = self.terminal.input.ime.text_for_range(range);
        *adjusted_range = Some(range);
        Some(text)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        // GPUI's IME contract needs a valid insertion range even when there is
        // no marked text. This is also what lets CJK candidate windows anchor to
        // the terminal cursor instead of treating the surface as non-editable.
        let mut selection = terminal_input_selection();
        if self
            .terminal
            .input
            .ime
            .is_active_for(self.session.active_id())
        {
            selection.range = self.terminal.input.ime.selection();
        }
        Some(selection)
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        if !self
            .terminal
            .input
            .ime
            .is_active_for(self.session.active_id())
        {
            return None;
        }
        let len = self.terminal.input.ime.text().encode_utf16().count();
        (len > 0).then_some(0..len)
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_terminal_composition(cx);
        window.notify_text_input_changed(&self.terminal.input.focus);
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_terminal_composition(cx);
        self.commit_terminal_text(text, cx);
        window.notify_text_input_changed(&self.terminal.input.focus);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session_id) = self.session.active_id_owned() else {
            return;
        };
        self.terminal
            .input
            .ime
            .replace(&session_id, range, new_text, new_selected_range);
        self.refresh_terminal_composition_surface(&session_id, cx);
        window.notify_text_input_changed(&self.terminal.input.focus);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(
            self.terminal_ime_layout(element_bounds, window, cx)
                .map(|layout| layout.range_bounds(range_utf16))
                .unwrap_or_else(|| self.terminal_ime_anchor_bounds(element_bounds, cx)),
        )
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self
            .session
            .active_id()
            .and_then(|id| self.terminal.layout.session_surface_bounds.get(id).copied());
        Some(
            bounds
                .and_then(|bounds| self.terminal_ime_layout(bounds, window, cx))
                .map_or(0, |layout| layout.character_index(point)),
        )
    }

    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal.input.ime.select(range);
        if let Some(session_id) = self.terminal.input.ime.session_id().map(str::to_owned) {
            self.refresh_terminal_composition_surface(&session_id, cx);
        }
        window.notify_text_selection_changed(&self.terminal.input.focus);
        cx.notify();
    }
}

impl NyaTermApp {
    fn refresh_terminal_composition_surface(&mut self, session_id: &str, cx: &mut Context<Self>) {
        self.sync_terminal_surface_paint(session_id, cx);
        if let Some(surface) = self.terminal.view.surfaces.get(session_id) {
            // Text/selection changes do not change the remote snapshot or cursor
            // policy, but must invalidate the entity's cached preedit paint.
            surface.update(cx, |_, cx| cx.notify());
        }
    }

    pub(in crate::features) fn clear_terminal_composition(&mut self, cx: &mut Context<Self>) {
        if let Some(session_id) = self.terminal.input.ime.clear() {
            self.refresh_terminal_composition_surface(&session_id, cx);
            cx.notify();
        }
    }

    pub(in crate::features) fn clear_terminal_composition_for_session(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.terminal.input.ime.session_id() == Some(session_id) {
            self.clear_terminal_composition(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, px};

    use super::{terminal_input_selection, terminal_visible_surface_bounds};

    #[test]
    fn terminal_input_selection_keeps_a_valid_insertion_point() {
        let selection = terminal_input_selection();

        assert_eq!(selection.range, 0..0);
        assert!(!selection.reversed);
    }

    #[test]
    fn terminal_surface_bounds_follow_the_visible_content_mask() {
        let bounds = Bounds::new(
            gpui::point(px(10.), px(20.)),
            gpui::size(px(600.), px(4_000.)),
        );
        let content_mask = Bounds::new(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(1_280.), px(800.)),
        );

        assert_eq!(
            terminal_visible_surface_bounds(bounds, content_mask),
            Some(Bounds::new(
                gpui::point(px(10.), px(20.)),
                gpui::size(px(600.), px(780.))
            ))
        );
    }

    #[test]
    fn terminal_surface_bounds_exclude_scrollbar_column() {
        let outer_width = 610.;
        let text_width = outer_width
            - crate::features::terminal::terminal_surface::TERMINAL_SCROLLBAR_COLUMN_WIDTH;
        let bounds = Bounds::new(
            gpui::point(px(10.), px(20.)),
            gpui::size(px(text_width), px(400.)),
        );
        let content_mask = Bounds::new(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(1_280.), px(800.)),
        );

        assert_eq!(
            terminal_visible_surface_bounds(bounds, content_mask),
            Some(Bounds::new(
                gpui::point(px(10.), px(20.)),
                gpui::size(px(text_width), px(400.))
            ))
        );
    }

    #[test]
    fn terminal_surface_bounds_skip_fully_clipped_surfaces() {
        let bounds = Bounds::new(
            gpui::point(px(10.), px(900.)),
            gpui::size(px(600.), px(400.)),
        );
        let content_mask = Bounds::new(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(1_280.), px(800.)),
        );

        assert_eq!(terminal_visible_surface_bounds(bounds, content_mask), None);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum SmartSelectionEdge {
    Start,
    End,
}
