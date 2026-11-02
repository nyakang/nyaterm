//! Shared GPUI theme tokens and reusable presentation widgets for NyaTerm.

mod app_menu_bar;
mod button;
pub mod chat;
mod child_window;
mod command;
mod dialog;
mod document_editor;
pub mod document_snapshot;
pub mod document_syntax;
pub mod hex_editor;
mod hover_card;
mod input;
mod input_focus;
mod markdown;
mod menu;
pub mod notification;
mod number_input;
pub mod plot;
mod popover;
mod root;
mod selectable_text;
mod selection;
mod settings;
mod sizing;
pub mod tab_close;
mod tabs;
mod theme;
mod theme_bridge;
mod tooltip;
mod widgets;
pub mod window_chrome;

pub use app_menu_bar::{NyaAppMenu, NyaAppMenuBar};
pub use button::{NyaButton, NyaButtonVariant, NyaIconButton};
pub use child_window::{ChildWindowSlot, activate_child_window};
pub use command::{NyaCommand, NyaCommandIndex, NyaCommandItem, NyaCommandState};
pub use dialog::{
    NyaConfirmDialog, NyaDialog, NyaDialogFooter, NyaDialogHandle, NyaDialogWindowExt,
};
pub use document_editor::{NyaDocumentEditor, NyaDocumentEditorEvent, NyaDocumentEditorState};
pub use gpui_kit::component::color_picker::{
    ColorPicker as NyaColorPicker, ColorPickerEvent as NyaColorPickerEvent,
    ColorPickerState as NyaColorPickerState,
};
pub use gpui_kit::component::input::{
    Copy as NyaCopy, Cut as NyaCut, Paste as NyaPaste, Redo as NyaRedo, SelectAll as NyaSelectAll,
    Undo as NyaUndo,
};
pub use gpui_kit::component::kbd::Kbd as NyaKbd;
pub use gpui_kit::component::scroll::ScrollableElement as NyaScrollable;
pub use gpui_kit::component::scroll::ScrollbarAxis as NyaScrollbarAxis;
pub use gpui_kit::component::slider::{
    Slider as NyaSlider, SliderEvent as NyaSliderEvent, SliderState as NyaSliderState,
    SliderValue as NyaSliderValue,
};
pub use gpui_kit::component::tag::Tag as NyaTag;
pub use hover_card::NyaHoverCard;
pub use input::{
    NyaInput, NyaInputEvent, NyaInputKind, NyaInputLabels, NyaInputShell, NyaInputState,
    NyaSearchInput, NyaTextArea, set_nya_input_label_provider,
};
pub use markdown::NyaMarkdown;
pub use menu::{
    NyaContextMenu, NyaDropdownMenu, NyaMenuAnchor, NyaMenuItem, nya_popup_is_active,
    nya_popup_menu_is_active,
};
pub use number_input::{
    NyaNumberInput, NyaNumberInputEvent, NyaNumberInputOptions, NyaNumberInputState, NyaNumberStep,
};
pub use popover::{NyaPopover, NyaPopoverAlign, NyaPopoverPlacement};
pub use root::{NyaRoot, NyaWindowHandle, nya_root, set_nya_root_transparent};
pub use selectable_text::NyaSelectableText;
pub use selection::{
    NyaCheckbox, NyaRadioGroup, NyaSelect, NyaSelectEvent, NyaSelectOption, NyaSelectState,
    NyaSwitch,
};
pub use settings::{NyaSettingsLayout, NyaSettingsNavGroup, NyaSettingsNavItem};
pub use sizing::NYA_FORM_CONTROL_HEIGHT_PX;
pub use tabs::{NyaTabItem, NyaTabs, NyaTabsVariant};
pub use theme::{
    APPEARANCE_THEME_IDS, ThemePalette, appearance_theme_label, available_themes,
    palette_from_theme, resolve_theme_palette, theme_palette,
};
pub use theme_bridge::apply_component_theme;
pub use tooltip::NyaTooltip;
pub use widgets::{
    NyaHorizontalScrollbar, NyaScrollArea, NyaUniformListScrollbar, capability_line, empty_panel,
    empty_panel_with_icon, mode_button, section_header, small_button, status_pill, svg_icon_button,
};

#[cfg(test)]
mod tests {
    /// NyaTerm localises `gpui-kit`'s own widget strings by setting one
    /// process-wide locale, which only works because both crates read the same
    /// `rust_i18n` global and `gpui-kit` ships the locales NyaTerm offers.
    /// This pins the whole chain without mutating the global, which parallel tests
    /// would race.
    #[test]
    fn gpui_component_shares_the_rust_i18n_locale_and_ships_simplified_chinese() {
        assert_eq!(&*gpui_kit::component::locale(), &*rust_i18n::locale());

        let english = gpui_kit::component::_rust_i18n_try_translate("en", "Calendar.month.January");
        let chinese =
            gpui_kit::component::_rust_i18n_try_translate("zh-CN", "Calendar.month.January");
        assert_eq!(english.as_deref(), Some("January"));
        assert_eq!(chinese.as_deref(), Some("一月"));
    }
}
