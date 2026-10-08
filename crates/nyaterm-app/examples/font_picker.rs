//! Manual Windows QA for localized font names and independently styled samples.
//! Run with `cargo run -p nyaterm-app --example font_picker`.

use gpui::{
    App, AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, TitlebarOptions, Window, WindowBounds, WindowOptions, div, font, px, rgb, size,
    svg,
};
use nyaterm_app::assets::NyaTermAssets;
use nyaterm_ui::{
    NyaSelect, NyaSelectOption, NyaSelectState, apply_component_theme, nya_root, theme_palette,
};

struct FontPicker {
    select: Entity<NyaSelectState>,
}

impl Render for FontPicker {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0xf2f5f8))
            .p_6()
            .flex()
            .flex_col()
            .gap_4()
            .child("字体名称与字形预览 / Font names and glyph samples")
            .child(
                div()
                    .w(px(480.))
                    .h(px(32.))
                    .child(NyaSelect::new(&self.select)),
            )
            .child(
                div()
                    .flex()
                    .gap_4()
                    .child(
                        svg()
                            .size(px(24.))
                            .path("icons/brand/server.svg")
                            .text_color(rgb(0x60a5fa)),
                    )
                    .child(
                        svg()
                            .size(px(24.))
                            .path("icons/conn/folder.svg")
                            .text_color(rgb(0xfe9a00))
                            .opacity(0.7),
                    )
                    .child(
                        svg()
                            .size(px(24.))
                            .path("icons/session/folder-open.svg")
                            .text_color(rgb(0xfe9a00))
                            .opacity(0.7),
                    ),
            )
    }
}

fn main() {
    gpui_platform::application()
        .with_assets(NyaTermAssets)
        .run(|cx: &mut App| {
            apply_component_theme(
                theme_palette("github-light"),
                font("Microsoft YaHei UI"),
                px(16.),
                cx,
            );
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(620.), px(450.)),
                        cx,
                    ))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("NyaTerm Font Picker QA".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| FontPicker {
                        select: cx.new(|cx| {
                            NyaSelectState::new(
                                cx,
                                [
                                    "Arial",
                                    "Consolas",
                                    "宋体",
                                    "微软雅黑",
                                    "Microsoft Himalaya",
                                ]
                                .map(|family| {
                                    NyaSelectOption::new(family, family).font_family(family)
                                })
                                .to_vec(),
                                Some("宋体".to_string()),
                            )
                            .searchable(true)
                        }),
                    });
                    cx.new(|cx| nya_root(view, window, cx))
                },
            )
            .expect("open font picker QA window");
            cx.activate(true);
        });
}
