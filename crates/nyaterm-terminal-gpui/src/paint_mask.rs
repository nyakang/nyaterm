//! Rectangular exclusions for overlays that replace terminal content.

use gpui::{App, Bounds, ContentMask, Pixels, Window, bounds, point, px, size};

/// Split the viewport into disjoint rectangles, so translucent primitives are
/// never submitted twice for the same pixel and the underlying surface survives.
fn outside_regions(viewport: Bounds<Pixels>, hole: Bounds<Pixels>) -> [Bounds<Pixels>; 4] {
    let hole = viewport.intersect(&hole);
    [
        bounds(
            viewport.origin,
            size(viewport.size.width, hole.top() - viewport.top()),
        ),
        bounds(
            point(viewport.left(), hole.bottom()),
            size(viewport.size.width, viewport.bottom() - hole.bottom()),
        ),
        bounds(
            point(viewport.left(), hole.top()),
            size(hole.left() - viewport.left(), hole.size.height),
        ),
        bounds(
            point(hole.right(), hole.top()),
            size(viewport.right() - hole.right(), hole.size.height),
        ),
    ]
}

pub(crate) fn paint_outside(
    exclusion: Option<Bounds<Pixels>>,
    primitive: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
    mut paint: impl FnMut(&mut Window, &mut App),
) {
    let viewport = window.content_mask().bounds;
    let Some(hole) =
        exclusion.filter(|hole| hole.intersects(&viewport) && hole.intersects(&primitive))
    else {
        paint(window, cx);
        return;
    };
    for region in outside_regions(viewport, hole) {
        if region.size.width > px(0.)
            && region.size.height > px(0.)
            && region.intersects(&primitive)
        {
            window.with_content_mask(Some(ContentMask { bounds: region }), |window| {
                paint(window, cx)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{bounds, point, px, size};

    use super::outside_regions;

    #[test]
    fn exclusion_preserves_each_outside_pixel_once_including_clipped_edges() {
        let viewport = bounds(point(px(10.), px(20.)), size(px(12.), px(8.)));
        for (x, y, w, h) in [
            (13., 22., 4., 3.),
            (8., 18., 6., 5.),
            (18., 25., 9., 7.),
            (0., 0., 40., 40.),
        ] {
            let hole = bounds(point(px(x), px(y)), size(px(w), px(h)));
            let regions = outside_regions(viewport, hole);
            for x in 10..22 {
                for y in 20..28 {
                    let pixel = point(px(x as f32 + 0.5), px(y as f32 + 0.5));
                    assert_eq!(
                        regions
                            .iter()
                            .filter(|region| region.contains(&pixel))
                            .count(),
                        usize::from(!hole.contains(&pixel))
                    );
                }
            }
        }
    }
}
