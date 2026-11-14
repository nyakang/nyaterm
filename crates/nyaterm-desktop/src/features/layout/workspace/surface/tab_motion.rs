use std::{cell::RefCell, rc::Rc, time::Instant};

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, LayoutId,
    Pixels, ScrollHandle, Window, point, px,
};

use crate::features::shell::tab_reorder::TabReorderMotion;

/// Layout stays in business order; both painting and hitboxes follow the
/// preview offsets. This also measures tabs clipped by horizontal scrolling.
pub(super) struct TabMotionElement {
    pub child: AnyElement,
    pub id: String,
    pub motion: Rc<RefCell<TabReorderMotion>>,
    pub scroll: ScrollHandle,
}

impl Element for TabMotionElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let now = Instant::now();
        let offset = {
            let mut motion = self.motion.borrow_mut();
            motion.measure(
                &self.id,
                f32::from(bounds.origin.x - self.scroll.offset().x),
                f32::from(bounds.size.width),
                now,
            );
            motion.offset(&self.id, now).0
        };
        let offset = if cx.reduce_motion() {
            self.motion.borrow().target_offset(&self.id)
        } else {
            offset
        };
        window.with_element_offset(point(px(offset), px(0.)), |window| {
            self.child.prepaint(window, cx)
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

pub(super) struct TabStripViewportElement {
    pub child: AnyElement,
    pub motion: Rc<RefCell<TabReorderMotion>>,
    pub scroll: ScrollHandle,
}

impl Element for TabStripViewportElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let mut scrolling = false;
        if cx.has_active_drag() {
            let mut motion = self.motion.borrow_mut();
            let now = Instant::now();
            if let Some(next) = motion.advance_scroll(
                f32::from(bounds.left()),
                f32::from(bounds.right()),
                f32::from(self.scroll.offset().x),
                f32::from(self.scroll.max_offset().x),
                now,
            ) {
                self.scroll.set_offset(point(px(next), px(0.)));
                scrolling = true;
            }
            if let Some(pointer) = motion.pointer() {
                motion.update(pointer - f32::from(self.scroll.offset().x), now);
            }
        }
        self.child.prepaint(window, cx);
        let animating = self.motion.borrow_mut().animate_frame(Instant::now());
        if scrolling || (animating && !cx.reduce_motion()) {
            window.request_animation_frame();
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}
