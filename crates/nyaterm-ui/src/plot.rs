//! NyaTerm's plotting boundary. Callers provide data and theme colors without
//! depending on gpui-kit's plotting types or cache lifecycle.

use std::f32::consts::TAU;
use std::sync::Arc;

use gpui::{App, Bounds, ElementId, Hsla, Pixels, Window, point, px, size};
use gpui_kit::component::plot::{
    Curve, Grid, IntoPlot, PathCaches, Plot,
    scale::{Scale, ScaleLinear},
    shape::{Arc as PlotArc, ArcData, Line},
};

#[derive(Clone, Copy)]
pub struct NyaLinePoint {
    pub x: f64,
    /// Missing values break the line instead of implying zero traffic or
    /// connecting across an interval in which the interface did not exist.
    pub value: Option<f64>,
}

pub struct NyaLineSeries {
    pub points: Arc<[NyaLinePoint]>,
    pub color: Hsla,
}

/// Nonnegative series sharing a time domain and a zero-based value domain.
#[derive(IntoPlot)]
pub struct NyaLinePlot {
    id: ElementId,
    series: Vec<NyaLineSeries>,
    grid_color: Hsla,
    x_domain: [f64; 2],
    max_value: f64,
}

impl NyaLinePlot {
    pub fn new(id: impl Into<ElementId>, series: Vec<NyaLineSeries>, grid_color: Hsla) -> Self {
        let mut x_min = f64::INFINITY;
        let mut x_max = f64::NEG_INFINITY;
        let mut max_value = 1.0_f64;
        for point in series.iter().flat_map(|series| series.points.iter()) {
            if point.x.is_finite() {
                x_min = x_min.min(point.x);
                x_max = x_max.max(point.x);
                if let Some(value) = point.value.filter(|value| value.is_finite()) {
                    max_value = max_value.max(value);
                }
            }
        }
        let x_domain = if x_min < x_max {
            [x_min, x_max]
        } else if x_min.is_finite() {
            [x_min, x_min + 1.]
        } else {
            [0., 1.]
        };
        Self {
            id: id.into(),
            series,
            grid_color,
            x_domain,
            max_value,
        }
    }

    pub fn max_value(&self) -> f64 {
        self.max_value
    }

    /// Fix the horizontal domain without stretching a short series to fill it.
    /// Invalid ranges leave the automatically inferred domain unchanged.
    pub fn x_range(mut self, range: [f64; 2]) -> Self {
        if range.iter().all(|value| value.is_finite()) && range[0] < range[1] {
            self.x_domain = range;
            self.max_value = self
                .series
                .iter()
                .flat_map(|series| series.points.iter())
                .filter(|point| self.contains_x(point.x))
                .filter_map(|point| point.value.filter(|value| value.is_finite()))
                .fold(1., f64::max);
        }
        self
    }

    pub fn y_max(mut self, maximum: f64) -> Self {
        if maximum.is_finite() && maximum > 0. {
            self.max_value = maximum;
        }
        self
    }

    fn contains_x(&self, x: f64) -> bool {
        x.is_finite() && x >= self.x_domain[0] && x <= self.x_domain[1]
    }
}

impl Plot for NyaLinePlot {
    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn interactive(&self) -> bool {
        false
    }

    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        // Inset both axes so strokes at zero and the maximum stay inside the card.
        let bounds = Bounds::new(
            bounds.origin + point(px(4.), px(4.)),
            size(
                (bounds.size.width - px(8.)).max(px(1.)),
                (bounds.size.height - px(8.)).max(px(1.)),
            ),
        );
        let width = bounds.size.width.as_f32();
        let height = bounds.size.height.as_f32();
        let x = ScaleLinear::new(self.x_domain, [0., width]);
        let y = ScaleLinear::new([0., self.max_value], [height, 0.]);
        Grid::new()
            .y([height / 2., height])
            .stroke(self.grid_color)
            .paint(&bounds, window);
        let caches = PathCaches::for_paint("nya-lines", window, cx);
        caches.update(cx, |caches, _| {
            let mut slot = 0;
            for series in &self.series {
                for segment in series
                    .points
                    .split(|point| {
                        !self.contains_x(point.x)
                            || point.value.is_none_or(|value| !value.is_finite())
                    })
                    .filter(|segment| !segment.is_empty())
                {
                    let x = x.clone();
                    let y = y.clone();
                    let line = Line::new()
                        .data(segment.iter().copied())
                        .x(move |point| x.tick(&point.x))
                        .y(move |point| point.value.and_then(|value| y.tick(&value.max(0.))))
                        .curve(Curve::Linear)
                        .stroke(series.color)
                        .stroke_width(px(1.5));
                    line.paint_cached(&bounds, caches.slot(slot), window);
                    slot += 1;
                }
            }
        });
    }
}

#[derive(IntoPlot)]
pub struct NyaRingGauge {
    id: ElementId,
    ratio: f32,
    track: Hsla,
    accent: Hsla,
}

impl NyaRingGauge {
    pub fn new(id: impl Into<ElementId>, ratio: f64, track: Hsla, accent: Hsla) -> Self {
        Self {
            id: id.into(),
            ratio: if ratio.is_finite() {
                ratio.clamp(0., 1.) as f32
            } else {
                0.
            },
            track,
            accent,
        }
    }
}

impl Plot for NyaRingGauge {
    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn interactive(&self) -> bool {
        false
    }

    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let outer = (bounds.size.width.min(bounds.size.height).as_f32() / 2. - 0.5).max(0.);
        let arc = PlotArc::new()
            .inner_radius((outer - 5.).max(0.))
            .outer_radius(outer);
        let caches = PathCaches::for_paint("nya-ring", window, cx);
        caches.update(cx, |caches, _| {
            arc.paint_cached(
                &ArcData::new(&(), 0, 1., 0., TAU),
                self.track,
                &bounds,
                caches.slot(0),
                window,
            );
            if self.ratio > 0. {
                arc.paint_cached(
                    &ArcData::new(&(), 0, self.ratio, 0., TAU * self.ratio),
                    self.accent,
                    &bounds,
                    caches.slot(1),
                    window,
                );
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{NyaLinePlot, NyaLinePoint, NyaLineSeries};
    use gpui::rgb;

    fn plot() -> NyaLinePlot {
        NyaLinePlot::new(
            "test",
            vec![NyaLineSeries {
                color: rgb(0x3b82f6).into(),
                points: vec![
                    NyaLinePoint {
                        x: -3.,
                        value: Some(40.),
                    },
                    NyaLinePoint {
                        x: 0.,
                        value: Some(80.),
                    },
                ]
                .into(),
            }],
            rgb(0x777777).into(),
        )
    }

    #[test]
    fn fixed_domain_does_not_stretch_two_samples_and_default_stays_automatic() {
        assert_eq!(plot().x_domain, [-3., 0.]);
        let fixed = plot().x_range([-60., 0.]).y_max(100.);
        assert_eq!(fixed.x_domain, [-60., 0.]);
        assert_eq!(fixed.max_value(), 100.);
        assert_eq!(3. / (fixed.x_domain[1] - fixed.x_domain[0]), 0.05);
    }

    #[test]
    fn out_of_window_points_are_not_painted_or_used_for_automatic_y_range() {
        let plot = plot().x_range([-60., -1.]);
        assert!(plot.contains_x(-3.));
        assert!(!plot.contains_x(0.));
        assert!(!plot.contains_x(f64::NAN));
        assert_eq!(plot.max_value(), 40.);
    }

    #[test]
    fn invalid_explicit_domains_leave_safe_defaults() {
        let plot = plot()
            .x_range([1., 1.])
            .x_range([f64::NAN, f64::INFINITY])
            .y_max(0.)
            .y_max(f64::NAN);
        assert_eq!(plot.x_domain, [-3., 0.]);
        assert_eq!(plot.max_value(), 80.);
    }
}
