use gpui::{
    AnyElement, App, Bounds, Decorations, Display, Element, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, Pixels, Point, Position, Size, Style, Window, point, px,
};

/// Measures before placing, avoiding a first frame at an uncorrected position.
pub(super) struct PointerPositioner {
    pointer: Point<Pixels>,
    child: AnyElement,
}

impl PointerPositioner {
    pub(super) fn new(pointer: Point<Pixels>, child: impl IntoElement) -> Self {
        Self {
            pointer,
            child: child.into_any_element(),
        }
    }
}

pub(super) fn available_bounds(window: &Window) -> Bounds<Pixels> {
    let inset = window.client_inset().unwrap_or(px(0.));
    let mut bounds = Bounds::new(Point::default(), window.viewport_size());
    if let Decorations::Client { tiling } = window.window_decorations() {
        if !tiling.left {
            bounds.origin.x += inset;
            bounds.size.width -= inset;
        }
        if !tiling.right {
            bounds.size.width -= inset;
        }
        if !tiling.top {
            bounds.origin.y += inset;
            bounds.size.height -= inset;
        }
        if !tiling.bottom {
            bounds.size.height -= inset;
        }
    }
    // On tiny viewports, reduce the margin instead of creating negative limits.
    let margin_x = px(8.).min((bounds.size.width / 2.).max(px(0.)));
    let margin_y = px(8.).min((bounds.size.height / 2.).max(px(0.)));
    bounds.origin += point(margin_x, margin_y);
    bounds.size.width = (bounds.size.width - margin_x * 2.).max(px(0.));
    bounds.size.height = (bounds.size.height - margin_y * 2.).max(px(0.));
    bounds
}

fn pointer_bounds(
    pointer: Point<Pixels>,
    size: Size<Pixels>,
    viewport: Bounds<Pixels>,
) -> Bounds<Pixels> {
    fn axis(pointer: Pixels, size: Pixels, min: Pixels, max: Pixels, gap: Pixels) -> Pixels {
        let after = max - pointer - gap;
        let before = pointer - gap - min;
        let preferred = if size <= after || (size > before && after >= before) {
            pointer + gap
        } else {
            pointer - gap - size
        };
        preferred.min(max - size).max(min)
    }
    Bounds::new(
        point(
            axis(
                pointer.x,
                size.width,
                viewport.left(),
                viewport.right(),
                px(12.),
            ),
            axis(
                pointer.y,
                size.height,
                viewport.top(),
                viewport.bottom(),
                px(16.),
            ),
        ),
        size,
    )
}

impl Element for PointerPositioner {
    type RequestLayoutState = LayoutId;
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, LayoutId) {
        let child_layout = self.child.request_layout(window, cx);
        let layout = window.request_layout(
            Style {
                position: Position::Absolute,
                display: Display::Flex,
                ..Style::default()
            },
            [child_layout],
            cx,
        );
        (layout, child_layout)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child_layout: &mut LayoutId,
        window: &mut Window,
        cx: &mut App,
    ) {
        let size = window.layout_bounds(*child_layout).size;
        let resolved = pointer_bounds(self.pointer, size, available_bounds(window));
        let offset = resolved.origin - bounds.origin;
        window.with_element_offset(point(offset.x.round(), offset.y.round()), |window| {
            self.child.prepaint(window, cx);
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut LayoutId,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, point, px, size};

    use super::pointer_bounds;

    #[test]
    fn pointer_card_flips_each_axis_and_clamps_when_neither_side_fits() {
        let viewport = Bounds::new(point(px(8.), px(8.)), size(px(784.), px(584.)));
        for (pointer, expected) in [
            ((100., 100.), (112., 116.)),
            ((750., 100.), (438., 116.)),
            ((100., 550.), (112., 354.)),
            ((750., 550.), (438., 354.)),
            ((400., 300.), (412., 316.)),
        ] {
            let result = pointer_bounds(
                point(px(pointer.0), px(pointer.1)),
                size(px(300.), px(180.)),
                viewport,
            );
            assert_eq!(result.origin, point(px(expected.0), px(expected.1)));
            assert!(viewport.contains(&result.origin));
            assert!(viewport.contains(&result.bottom_right()));
        }
        let result = pointer_bounds(point(px(400.), px(300.)), viewport.size, viewport);
        assert_eq!(result, viewport);
    }

    #[test]
    fn tiny_viewport_and_outside_pointer_have_ordered_limits() {
        let viewport = Bounds::new(point(px(8.), px(8.)), size(px(40.), px(20.)));
        for pointer in [point(px(-100.), px(-100.)), point(px(1000.), px(1000.))] {
            assert_eq!(pointer_bounds(pointer, viewport.size, viewport), viewport);
        }
    }
}
