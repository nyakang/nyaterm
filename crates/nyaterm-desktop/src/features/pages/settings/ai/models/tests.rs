use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    Context, IntoElement, Modifiers, Render, ScrollHandle, TestAppContext, Window, div, point,
    prelude::*, px, size,
};
use nyaterm_core::ai::{AiBackendKind, AiModelConfigItem, AiModelSource};
use nyaterm_ui::{NyaIconButton, NyaScrollable as _, NyaSwitch, init_components, theme_palette};

use super::{model_row, status_dot};
use crate::features::ai::ConnectionStatus;

struct ModelRowsFixture {
    width: f32,
    scroll: ScrollHandle,
    clicks: Rc<RefCell<Vec<String>>>,
}

impl Render for ModelRowsFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let palette = theme_palette("github-dark");
        let names = [
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-6-astra",
            "gpt-6-sol",
            "gpt-6.1-sol",
            "a-very-long-model-identifier-that-must-be-truncated-without-hiding-its-actions",
        ];
        let mut rows = div().min_w_0().flex().flex_col();
        for (index, name) in names.into_iter().cycle().take(24).enumerate() {
            let id = index.to_string();
            let model = AiModelConfigItem {
                id: id.clone(),
                name: name.into(),
                backend: AiBackendKind::default(),
                provider_kind: None,
                credential_id: None,
                enabled: true,
                source: AiModelSource::Manual,
                last_seen_at: Some("2026-11-05".into()),
                supported_reasoning_efforts: None,
            };
            let mut actions = div()
                .min_w_0()
                .max_w_full()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(status_dot(palette, ConnectionStatus::Success));
            for action in ["test", "edit", "delete"] {
                let key = format!("{action}-{id}");
                let clicks = self.clicks.clone();
                let clicked_key = key.clone();
                actions = actions.child(
                    div().flex_none().debug_selector(move || key.clone()).child(
                        NyaIconButton::new(format!("{action}-{id}"), "icons/settings/link.svg")
                            .on_click(move |_, _, _| clicks.borrow_mut().push(clicked_key.clone())),
                    ),
                );
            }
            let clicks = self.clicks.clone();
            let key = format!("toggle-{id}");
            let clicked_key = key.clone();
            actions = actions.child(
                div().flex_none().debug_selector(move || key.clone()).child(
                    NyaSwitch::new(format!("toggle-{id}"))
                        .checked(true)
                        .on_click(move |_, _, _| clicks.borrow_mut().push(clicked_key.clone())),
                ),
            );
            rows = rows.child(
                model_row(palette, &model, actions).debug_selector(move || format!("row-{id}")),
            );
        }
        div()
            .w(px(self.width))
            .h(px(200.))
            .min_w_0()
            .flex()
            .flex_col()
            .vertical_scrollbar(&self.scroll)
            .child(
                div()
                    .id("rows-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(rows),
            )
    }
}

#[gpui::test]
fn model_rows_wrap_without_losing_names_controls_or_natural_height(cx: &mut TestAppContext) {
    cx.update(init_components);
    let clicks = Rc::new(RefCell::new(Vec::new()));
    let scroll = ScrollHandle::new();
    let (fixture, cx) = cx.add_window_view(|_, _| ModelRowsFixture {
        width: 700.,
        scroll: scroll.clone(),
        clicks: clicks.clone(),
    });
    cx.simulate_resize(size(px(1000.), px(720.)));
    // GPUI's debug_bounds API requires static selectors.
    let selectors: Vec<[&'static str; 7]> = (0..24)
        .map(|index| {
            [
                "row",
                "ai-model-name",
                "ai-model-actions",
                "test",
                "edit",
                "delete",
                "toggle",
            ]
            .map(|prefix| &*Box::leak(format!("{prefix}-{index}").into_boxed_str()))
        })
        .collect();
    for width in [700., 300., 200., 100., 700.] {
        fixture.update(cx, |fixture, cx| {
            fixture.width = width;
            cx.notify();
        });
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        cx.run_until_parked();
        let mut previous_bottom = px(0.);
        for selectors in &selectors {
            let row = cx.debug_bounds(selectors[0]).unwrap();
            let name = cx.debug_bounds(selectors[1]).unwrap();
            let actions = cx.debug_bounds(selectors[2]).unwrap();
            assert!(row.top() >= previous_bottom);
            assert!(name.size.width >= px(width.min(160.)));
            assert!(name.left() >= row.left() && name.right() <= row.right());
            assert!(name.top() >= row.top() && name.bottom() <= row.bottom());
            assert!(actions.left() >= row.left() && actions.right() <= row.right());
            assert!(actions.bottom() <= row.bottom());
            if width >= 700. {
                assert!(name.right() <= actions.left());
            } else {
                assert!(actions.top() >= name.bottom());
            }
            let mut previous = None;
            for selector in &selectors[3..] {
                let control = cx.debug_bounds(selector).unwrap();
                assert!(control.size.width > px(0.) && control.size.height > px(0.));
                assert!(control.left() >= actions.left() && control.right() <= actions.right());
                assert!(control.top() >= actions.top() && control.bottom() <= actions.bottom());
                if let Some(previous) = previous {
                    let previous: gpui::Bounds<gpui::Pixels> = previous;
                    assert!(
                        control.left() >= previous.right() || control.top() >= previous.bottom()
                    );
                }
                previous = Some(control);
            }
            previous_bottom = row.bottom();
        }
        assert!(
            previous_bottom > px(200.),
            "the list must scroll instead of shrinking rows"
        );
        for selector in &selectors[0][3..] {
            let control = cx.debug_bounds(selector).unwrap();
            // The first row is taller than the viewport only at the extreme 100px width.
            if control.bottom() <= px(200.) {
                cx.simulate_click(control.center(), Modifiers::default());
                cx.run_until_parked();
                assert_eq!(clicks.borrow().last().unwrap(), selector);
            }
        }
    }
    scroll.set_offset(point(px(0.), -scroll.max_offset().y));
    fixture.update(cx, |_, cx| cx.notify());
    cx.update(|window, cx| {
        _ = window.draw(cx);
    });
    cx.run_until_parked();
    let last = cx.debug_bounds("row-23").unwrap();
    assert!(last.top() >= px(0.) && last.bottom() <= px(200.));
}
