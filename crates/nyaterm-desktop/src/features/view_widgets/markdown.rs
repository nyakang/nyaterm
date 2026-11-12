use crate::theme::ThemePalette;
use gpui::IntoElement;

pub(in crate::features) fn markdown_content_view(
    palette: ThemePalette,
    content: &str,
) -> impl IntoElement {
    nyaterm_ui::markdown::lightweight::markdown_content_view(
        palette,
        content,
        crate::features::shell::gpui_code_font_family().into(),
    )
}
