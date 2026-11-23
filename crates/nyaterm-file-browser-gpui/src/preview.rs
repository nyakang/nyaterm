use gpui::{IntoElement, SharedString, div, img, prelude::*, px, rgb};
use nyaterm_preview::delimited::PreviewDelimited;
use nyaterm_ui::{NyaScrollable, ThemePalette};
use rust_i18n::t;
pub const PDF_MIN_ZOOM: f32 = 0.5;
pub const PDF_MAX_ZOOM: f32 = 3.0;
pub struct ImagePresentation<'a> {
    pub id: &'a str,
    pub image: std::sync::Arc<gpui::RenderImage>,
    pub width: u32,
    pub height: u32,
    pub zoom: f32,
    pub fit_to_window: bool,
}
pub fn session_badge_element(palette: ThemePalette, session: &str) -> gpui::AnyElement {
    if session.trim().is_empty() {
        return div().into_any_element();
    }
    div()
        .flex_none()
        .px_1p5()
        .py_0p5()
        .rounded_sm()
        .bg(rgb(palette.surface_elevated))
        .text_color(rgb(palette.text))
        .max_w(px(180.))
        .truncate()
        .child(session.to_string())
        .into_any_element()
}

pub fn format_size_and_time(size: Option<u64>, modified_at: Option<u32>) -> String {
    let size = nyaterm_ui::presentation::format_file_size(size);
    let time = crate::formatting::format_sftp_modified(modified_at);
    match (size.is_empty(), time == "-") {
        (true, true) => String::new(),
        (false, true) => size,
        (true, false) => time,
        (false, false) => format!("{size}  ·  {time}"),
    }
}

pub fn delimited_headers(data: &PreviewDelimited) -> Vec<String> {
    if data.first_row_is_header() {
        data.header_row().unwrap_or_default().to_vec()
    } else {
        (1..=data.column_count())
            .map(|column| format!("Column {column}"))
            .collect()
    }
}

pub fn delimited_row(
    palette: &ThemePalette,
    data: &PreviewDelimited,
    position: usize,
    column_count: usize,
) -> gpui::AnyElement {
    let Some(row) = data.row(position) else {
        return div().into_any_element();
    };
    let mut body_row = div().flex().flex_row().bg(if position.is_multiple_of(2) {
        rgb(palette.bg)
    } else {
        rgb(palette.section_header)
    });
    for col in 0..column_count {
        let cell = row.get(col).cloned().unwrap_or_default();
        body_row = body_row.child(
            div()
                .min_w(px(120.))
                .flex_1()
                .px_2()
                .py_1()
                .border_r_1()
                .border_color(rgb(palette.surface_elevated))
                .text_size(px(11.))
                .text_color(rgb(palette.text))
                .child(cell),
        );
    }
    body_row.into_any_element()
}

pub fn centered_message(palette: ThemePalette, message: SharedString) -> gpui::AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(palette.text_muted))
        .child(message)
        .into_any_element()
}

pub fn error_message(palette: ThemePalette, message: &str) -> gpui::AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p_4()
        .text_color(rgb(palette.danger))
        .child(message.to_string())
        .into_any_element()
}

pub fn text_surface_body(surface: impl IntoElement) -> gpui::AnyElement {
    div().size_full().child(surface).into_any_element()
}

pub fn json_body(
    palette: ThemePalette,
    surface: impl IntoElement,
    parse_error: Option<&str>,
) -> gpui::AnyElement {
    let mut column = div().size_full().flex().flex_col().min_h_0();
    if let Some(error) = parse_error {
        column = column.child(
            div()
                .flex_none()
                .px_3()
                .py_2()
                .bg(rgb(palette.surface_elevated))
                .border_b_1()
                .border_color(rgb(palette.danger))
                .text_size(px(12.))
                .text_color(rgb(palette.danger))
                .child(t!("filePreview.jsonInvalid", error = error).to_string()),
        );
    }
    column
        .child(div().flex_1().min_h_0().child(surface))
        .into_any_element()
}

pub fn markdown_body(palette: ThemePalette, text: &str, scroll_id: &str) -> gpui::AnyElement {
    div()
        .id(SharedString::from(format!("preview-md-{scroll_id}")))
        .size_full()
        .overflow_y_scrollbar()
        .p_4()
        .child(nyaterm_ui::markdown::lightweight::markdown_content_view(
            palette,
            text,
            nyaterm_ui::presentation::gpui_code_font_family().into(),
        ))
        .into_any_element()
}

pub fn image_body(palette: ThemePalette, image: ImagePresentation<'_>) -> gpui::AnyElement {
    let (width, height) =
        fitted_dimensions(image.width, image.height, image.zoom, image.fit_to_window);
    div()
        .id(SharedString::from(format!("preview-img-{}", image.id)))
        .size_full()
        .overflow_scrollbar()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(palette.bg))
        .child(
            img(image.image.clone())
                .w(px(width))
                .h(px(height))
                .flex_none(),
        )
        .into_any_element()
}

pub fn pdf_body(palette: ThemePalette, page: Option<ImagePresentation<'_>>) -> gpui::AnyElement {
    let Some(page) = page else {
        // The active page is still rasterizing (or failed and will retry).
        return centered_message(palette, t!("filePreview.renderingPage").into());
    };
    let zoom = page.zoom.clamp(PDF_MIN_ZOOM, PDF_MAX_ZOOM);
    let (width, height) = fitted_dimensions(page.width, page.height, zoom, page.fit_to_window);
    div()
        .id(SharedString::from(format!("preview-pdf-{}", page.id)))
        .size_full()
        .overflow_scrollbar()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(palette.bg))
        .child(
            div()
                .flex_none()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.surface))
                .child(
                    img(page.image.clone())
                        .w(px(width))
                        .h(px(height))
                        .flex_none(),
                ),
        )
        .into_any_element()
}

pub fn fitted_dimensions(width: u32, height: u32, zoom: f32, fit_to_window: bool) -> (f32, f32) {
    let natural_w = width.max(1) as f32;
    let natural_h = height.max(1) as f32;
    if fit_to_window {
        // Fit inside the document window's content area (roughly 1080x620 after
        // chrome/toolbar/status). Scale down only; never enlarge past natural.
        let max_w = 1040.0;
        let max_h = 600.0;
        let scale = (max_w / natural_w).min(max_h / natural_h).min(1.0);
        ((natural_w * scale).max(1.0), (natural_h * scale).max(1.0))
    } else {
        ((natural_w * zoom).max(1.0), (natural_h * zoom).max(1.0))
    }
}

#[cfg(test)]
mod delimited_tests {
    use super::delimited_headers;
    use nyaterm_preview::delimited::PreviewDelimited;

    #[test]
    fn headers_preserve_empty_ragged_and_synthesized_labels() {
        let mut data = PreviewDelimited::new(
            vec![vec!["name".into()], vec!["a".into(), "1".into()]],
            false,
        );
        assert_eq!(delimited_headers(&data), ["name"]);
        data.toggle_header();
        assert_eq!(delimited_headers(&data), ["Column 1", "Column 2"]);
        let mut empty = PreviewDelimited::new(Vec::new(), false);
        assert!(delimited_headers(&empty).is_empty());
        empty.toggle_header();
        assert_eq!(delimited_headers(&empty), ["Column 1"]);
    }
}
