use std::time::Duration;

use gpui::{
    AnimationExt, AnyElement, Hsla, Img, IntoElement, SharedString, Svg, div, img,
    linear_color_stop, linear_gradient, prelude::*, px, rgb, svg,
};

use super::{IconDef, file_entry_icon};
use crate::theme::ThemePalette;

/// Paint a monochrome asset from `icons/**`.
///
/// GPUI rasterizes `svg()` into an alpha mask and keeps only the coverage
/// channel, so whatever paints the file declares are discarded and `color` is the
/// only thing that reaches the screen. That is what makes one glyph usable in
/// seven tints, and it is also why a full-color logo cannot go through here.
pub fn mono_icon(path: &'static str, color: Hsla, size_px: f32) -> Svg {
    debug_assert!(
        path.starts_with("icons/"),
        "monochrome icons live under icons/: {path}"
    );
    svg()
        .size(px(size_px))
        .flex_none()
        .path(path)
        .text_color(color)
}

/// Tauri-compatible connection spinner: a dim ring with a rotating bright arc.
pub fn connection_spinner(
    animation_id: SharedString,
    color: Hsla,
    size_px: f32,
) -> impl IntoElement {
    let stroke_px = (size_px / 6.).max(1.);
    div()
        .relative()
        .size(px(size_px))
        .flex_none()
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded_full()
                .border(px(stroke_px))
                .border_color(color.opacity(0.25)),
        )
        .child(
            svg()
                .absolute()
                .inset_0()
                .size(px(size_px))
                .path("icons/conn/spinner-arc.svg")
                .text_color(color.opacity(0.75))
                .with_animation(
                    animation_id,
                    gpui::Animation::new(Duration::from_secs(1)).repeat(),
                    |svg, delta| {
                        svg.with_transformation(gpui::Transformation::rotate(gpui::percentage(
                            delta,
                        )))
                    },
                ),
        )
}

/// Paint a full-color asset from `color/**`.
///
/// Unlike [`mono_icon`] this keeps the rasterized pixels, so the asset **cannot
/// be tinted** — callers must express selection or hover with surrounding chrome
/// instead. Decoding happens off the render thread, so the first frame after a
/// cold start paints nothing; give the icon a reserved size to avoid a reflow.
pub fn color_icon(path: &'static str, size_px: f32) -> Img {
    debug_assert!(
        path.starts_with("color/"),
        "full-color icons live under color/: {path}"
    );
    img(path).size(px(size_px)).flex_none()
}

/// Theme-colored NyaTerm application icon, matching the Tauri logo composition.
pub fn nyaterm_app_icon(palette: ThemePalette, size_px: f32) -> impl IntoElement {
    div()
        .size(px(size_px))
        .flex_none()
        .overflow_hidden()
        .rounded(px(size_px * 0.1875))
        .flex()
        .items_center()
        .justify_center()
        .bg(linear_gradient(
            135.,
            linear_color_stop(rgb(palette.primary), 0.),
            linear_color_stop(rgb(palette.primary_hover), 1.),
        ))
        .child(
            svg()
                .size(px(size_px))
                .flex_none()
                .path("icons/logo.svg")
                .text_color(rgb(0xffffff)),
        )
}

/// Activity-bar icon.
pub fn activity_icon(path: &'static str, color: Hsla, size_px: f32) -> AnyElement {
    mono_icon(path, color, size_px).into_any_element()
}

/// Faded NyaTerm logo used by empty workspace (Tauri EmptyWorkspaceState).
pub fn nyaterm_logo_mark(palette: ThemePalette, size_px: f32, opacity: f32) -> impl IntoElement {
    div()
        .size(px(size_px))
        .flex_none()
        .opacity(opacity)
        .flex()
        .items_center()
        .justify_center()
        .child(mono_icon(
            "icons/logo.svg",
            rgb(palette.text_muted).into(),
            size_px,
        ))
}

/// Paint a resolved [`IconDef`], honoring whichever element the asset needs.
///
/// `selected` only reaches monochrome icons: a full-color logo cannot be tinted,
/// so selection has to be carried by the row's own background. That matches the
/// pre-GPUI UI, where these were `<img>` tags and the configured color was inert.
pub fn themed_icon(
    palette: ThemePalette,
    def: IconDef,
    selected: bool,
    size_px: f32,
) -> AnyElement {
    match def.tint(palette) {
        Some(color) => {
            let color = if selected { palette.link } else { color };
            mono_icon(def.path, rgb(color).into(), size_px).into_any_element()
        }
        None => color_icon(def.path, size_px).into_any_element(),
    }
}

/// Connection / OS icon for saved connection rows, tabs and the session header.
pub fn connection_type_icon(
    palette: ThemePalette,
    def: IconDef,
    selected: bool,
    size_px: f32,
) -> AnyElement {
    themed_icon(palette, def, selected, size_px)
}

/// File explorer entry icon, colored by kind and extension.
pub fn transfer_entry_icon(
    palette: ThemePalette,
    name: &str,
    is_directory: bool,
    is_symlink: bool,
    selected: bool,
) -> AnyElement {
    let def = file_entry_icon(name, is_directory, is_symlink, palette);
    let color = if selected {
        palette.link
    } else {
        def.tint(palette).unwrap_or(palette.text_muted)
    };
    mono_icon(def.path, rgb(color).into(), 16.).into_any_element()
}
