use std::rc::Rc;

use gpui::{
    Anchor, App, ClickEvent, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce,
    SharedString, Styled, Window, div, prelude::FluentBuilder as _, px, rgb,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{Cancel, Confirm};
use gpui_kit::component::menu::{
    ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuAppearance, PopupMenuItem,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
};

type MenuClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type ContextMenuItemsBuilder = Rc<dyn Fn(&mut Window, &mut App) -> Vec<NyaMenuItem>>;

const NYA_MENU_WIDTH: f32 = 220.;
const NYA_MENU_ICON_SLOT_WIDTH: f32 = 24.;
const NYA_MENU_ITEM_GAP: f32 = 8.;
const NYA_SUBMENU_OVERLAP: f32 = 8.;
const NYA_MENU_ICON_OPTICAL_OFFSET_Y: f32 = 1.;

/// `PopupMenu` chooses a submenu's side from the parent menu's `max_width` and
/// origin. Its probe does not include the parent menu's own width, so an exact
/// 220px parent opened near the right edge can still choose the right side and
/// leave its child outside the window. Widen only that probe when the pointer
/// is in the collision zone; the parent's 220px minimum keeps its normal visual
/// width, while the component sees enough reserved space to flip the child.
fn submenu_direction_probe_width(
    items: &[NyaMenuItem],
    parent_min_width: Option<Pixels>,
    pointer_x: Pixels,
    window_width: Pixels,
) -> Option<Pixels> {
    let child_width = items
        .iter()
        .filter_map(|item| item.children().map(|_| item.submenu_min_width))
        .map(|width| width.unwrap_or(px(NYA_MENU_WIDTH)))
        .max_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal))?;
    let parent_width = parent_min_width.unwrap_or(px(NYA_MENU_WIDTH));
    let parent_and_child = parent_width + child_width - px(NYA_SUBMENU_OVERLAP);
    (pointer_x + parent_and_child > window_width).then_some(parent_and_child)
}

pub(crate) fn nya_popup_menu_appearance() -> PopupMenuAppearance {
    PopupMenuAppearance::new()
        .row_height(px(28.))
        .font_size(px(12.))
        .icon_size(px(16.))
        .icon_slot_width(px(NYA_MENU_ICON_SLOT_WIDTH))
        .horizontal_padding(px(8.))
        .item_gap(px(NYA_MENU_ITEM_GAP))
        .content_padding(px(4.))
        .row_gap(px(0.))
        .separator_thickness(px(1.))
        .separator_vertical_margin(px(4.))
        .separator_horizontal_margin(px(0.))
        .disabled_opacity(0.5)
        .item_radius(px(4.))
}

fn resolved_menu_widths(
    min_width: Option<Pixels>,
    max_width: Option<Pixels>,
) -> (Option<Pixels>, Option<Pixels>) {
    if min_width.is_none() && max_width.is_none() {
        let width = px(NYA_MENU_WIDTH);
        (Some(width), Some(width))
    } else {
        (min_width, max_width)
    }
}

pub(crate) fn style_nya_popup_menu(
    menu: PopupMenu,
    min_width: Option<Pixels>,
    max_width: Option<Pixels>,
) -> PopupMenu {
    let (min_width, max_width) = resolved_menu_widths(min_width, max_width);
    menu.appearance(nya_popup_menu_appearance())
        .when_some(min_width, |menu, width| menu.min_w(width))
        .when_some(max_width, |menu, width| menu.max_w(width))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NyaMenuAnchor {
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
}

impl NyaMenuAnchor {
    fn component_anchor(self) -> Anchor {
        match self {
            Self::TopLeft => Anchor::TopLeft,
            Self::TopRight => Anchor::TopRight,
            Self::BottomLeft => Anchor::BottomLeft,
            Self::BottomRight => Anchor::BottomRight,
        }
    }
}

#[derive(Clone)]
enum NyaMenuItemKind {
    Action,
    ActionBar(Vec<NyaMenuItem>),
    Label,
    Separator,
    Submenu(Vec<NyaMenuItem>),
}

#[derive(Clone)]
pub struct NyaMenuItem {
    kind: NyaMenuItemKind,
    label: SharedString,
    icon_path: Option<SharedString>,
    icon_color: Option<u32>,
    shortcut: Option<SharedString>,
    disabled: bool,
    checked: bool,
    danger: bool,
    label_indent: Option<Pixels>,
    submenu_min_width: Option<Pixels>,
    submenu_max_height: Option<Pixels>,
    submenu_scrollable: Option<bool>,
    on_click: Option<MenuClickHandler>,
}

impl NyaMenuItem {
    pub fn action(label: impl Into<SharedString>) -> Self {
        Self {
            kind: NyaMenuItemKind::Action,
            label: label.into(),
            icon_path: None,
            icon_color: None,
            shortcut: None,
            disabled: false,
            checked: false,
            danger: false,
            label_indent: None,
            submenu_min_width: None,
            submenu_max_height: None,
            submenu_scrollable: None,
            on_click: None,
        }
    }

    pub fn label(label: impl Into<SharedString>) -> Self {
        Self {
            kind: NyaMenuItemKind::Label,
            ..Self::action(label)
        }
    }

    /// A compact row of commands inside a context menu. Each command keeps its
    /// own disabled state and click handler.
    pub fn action_bar(items: [Self; 5]) -> Self {
        Self {
            kind: NyaMenuItemKind::ActionBar(items.into()),
            ..Self::action("")
        }
    }

    pub fn separator() -> Self {
        Self {
            kind: NyaMenuItemKind::Separator,
            ..Self::action("")
        }
    }

    pub fn submenu(label: impl Into<SharedString>, items: Vec<Self>) -> Self {
        Self {
            kind: NyaMenuItemKind::Submenu(items),
            ..Self::action(label)
        }
    }

    pub fn icon(mut self, icon_path: impl Into<SharedString>) -> Self {
        self.icon_path = Some(icon_path.into());
        self
    }

    pub fn icon_color(mut self, color: u32) -> Self {
        self.icon_color = Some(color);
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn label_indent(mut self, indent: Pixels) -> Self {
        self.label_indent = Some(indent);
        self
    }

    pub fn submenu_min_width(mut self, width: Pixels) -> Self {
        self.submenu_min_width = Some(width);
        self
    }

    pub fn submenu_max_height(mut self, height: Pixels) -> Self {
        self.submenu_max_height = Some(height);
        self
    }

    pub fn submenu_scrollable(mut self, scrollable: bool) -> Self {
        self.submenu_scrollable = Some(scrollable);
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    #[doc(hidden)]
    pub fn test_label(&self) -> &str {
        self.label.as_ref()
    }

    #[doc(hidden)]
    pub fn test_presentation(&self) -> (String, Option<String>, Option<String>, bool, bool, bool) {
        (
            self.label.to_string(),
            self.icon_path.as_ref().map(ToString::to_string),
            self.shortcut.as_ref().map(ToString::to_string),
            self.disabled,
            self.checked,
            self.danger,
        )
    }

    #[doc(hidden)]
    pub fn children(&self) -> Option<&[NyaMenuItem]> {
        match &self.kind {
            NyaMenuItemKind::Submenu(items) => Some(items.as_slice()),
            _ => None,
        }
    }

    fn is_action_bar(&self) -> bool {
        matches!(self.kind, NyaMenuItemKind::ActionBar(_))
    }

    #[doc(hidden)]
    pub fn test_icon_color(&self) -> Option<u32> {
        self.icon_color
    }

    #[doc(hidden)]
    pub fn test_submenu_layout(
        &self,
    ) -> (Option<Pixels>, Option<Pixels>, Option<bool>, Option<Pixels>) {
        (
            self.submenu_min_width,
            self.submenu_max_height,
            self.submenu_scrollable,
            self.label_indent,
        )
    }

    pub(crate) fn append_to(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut gpui::Context<PopupMenu>,
    ) -> PopupMenu {
        match &self.kind {
            NyaMenuItemKind::Separator => menu.separator(),
            NyaMenuItemKind::Label => menu.label(self.label.clone()),
            NyaMenuItemKind::Action => menu.item(self.popup_item(cx)),
            NyaMenuItemKind::ActionBar(items) => {
                let items = items.clone();
                let menu_id = cx.entity().entity_id();
                menu.item(PopupMenuItem::element(move |_, cx| {
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        // PopupMenu reserves an icon slot for the other rows.
                        .ml(px(-(NYA_MENU_ICON_SLOT_WIDTH + NYA_MENU_ITEM_GAP)))
                        .h(px(48.))
                        .children(items.iter().enumerate().map(|(index, item)| {
                            let label = item.label.clone();
                            let on_click = item.on_click.clone();
                            let on_confirm = on_click.clone();
                            let mut button = Button::new(format!("menu-{menu_id}-action-{index}"))
                                .ghost()
                                .compact()
                                .accessibility_label(label.clone())
                                .tooltip(label.clone())
                                .disabled(item.disabled)
                                .flex_1()
                                .min_w_0()
                                .h(px(48.))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .gap_1()
                                        .children(
                                            item.component_icon(cx)
                                                .map(|icon| icon.with_size(px(16.))),
                                        )
                                        .child(
                                            div()
                                                .w_full()
                                                .text_center()
                                                .text_size(px(10.))
                                                .child(label),
                                        ),
                                );
                            if item.danger {
                                button = button.text_color(cx.theme().danger);
                            }
                            if let Some(on_click) = on_click {
                                button = button.on_click(move |event, window, cx| {
                                    cx.stop_propagation();
                                    window.dispatch_action(Box::new(Cancel), cx);
                                    on_click(event, window, cx);
                                });
                            }
                            div()
                                .flex()
                                .items_center()
                                .flex_1()
                                .min_w_0()
                                .when(index > 0, |this| {
                                    this.child(
                                        div()
                                            .flex_none()
                                            .w(px(1.))
                                            .h(px(24.))
                                            .bg(cx.theme().border),
                                    )
                                })
                                .when_some(
                                    on_confirm.filter(|_| !item.disabled),
                                    |this, handler| {
                                        this.on_action(move |_: &Confirm, window, cx| {
                                            cx.stop_propagation();
                                            window.dispatch_action(Box::new(Cancel), cx);
                                            handler(&ClickEvent::default(), window, cx);
                                        })
                                    },
                                )
                                .child(button)
                        }))
                }))
            }
            NyaMenuItemKind::Submenu(items) => {
                let items = items.clone();
                let min_width = self.submenu_min_width;
                let max_height = self.submenu_max_height;
                let scrollable = self.submenu_scrollable;
                menu.submenu_with_icon(
                    self.component_icon(cx),
                    self.label.clone(),
                    window,
                    cx,
                    move |menu, window, cx| {
                        let menu = style_nya_popup_menu(menu, min_width, None)
                            .when_some(max_height, |menu, height| menu.max_h(height))
                            .scrollable(popup_menu_should_scroll(&items, scrollable));
                        items
                            .iter()
                            .fold(menu, |menu, item| item.append_to(menu, window, cx))
                    },
                )
            }
        }
    }

    fn popup_item(&self, cx: &App) -> PopupMenuItem {
        let mut item = if self.danger || self.shortcut.is_some() || self.label_indent.is_some() {
            let label = self.label.clone();
            let shortcut = self.shortcut.clone();
            let danger = self.danger;
            let label_indent = self.label_indent;
            PopupMenuItem::element(move |_, cx| {
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .text_color(if danger {
                        cx.theme().danger
                    } else {
                        cx.theme().foreground
                    })
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .when_some(label_indent, |this, indent| this.pl(indent))
                            .child(label.clone()),
                    )
                    .when_some(shortcut.clone(), |this, shortcut| {
                        this.child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(shortcut),
                        )
                    })
            })
        } else {
            PopupMenuItem::new(self.label.clone())
        };

        item = item
            .when_some(self.component_icon(cx), |item, icon| item.icon(icon))
            .disabled(self.disabled)
            .checked(self.checked);
        if let Some(on_click) = self.on_click.clone() {
            item = item.on_click(move |event, window, cx| on_click(event, window, cx));
        }
        item
    }

    fn component_icon(&self, cx: &App) -> Option<Icon> {
        let icon = if let Some(path) = self.icon_path.clone() {
            Icon::default().path(path)
        } else if self.checked {
            Icon::new(IconName::Check)
        } else {
            return None;
        };

        let icon = if self.danger {
            icon.text_color(cx.theme().danger)
        } else if let Some(color) = self.icon_color {
            icon.text_color(rgb(color))
        } else {
            icon.text_color(cx.theme().muted_foreground)
        };

        // GPUI centers the icon box against the text line box, but the visible
        // glyphs in our UI fonts sit slightly below that line box's midpoint.
        // Offset the icon ink without changing layout so the two look centered.
        Some(icon.relative().top(px(NYA_MENU_ICON_OPTICAL_OFFSET_Y)))
    }
}

#[derive(IntoElement)]
pub struct NyaDropdownMenu {
    id: SharedString,
    label: Option<SharedString>,
    icon_path: Option<SharedString>,
    icon_size: Option<Pixels>,
    tooltip: Option<SharedString>,
    selected: bool,
    disabled: bool,
    anchor: NyaMenuAnchor,
    min_width: Option<Pixels>,
    max_width: Option<Pixels>,
    max_height: Option<Pixels>,
    scrollable: Option<bool>,
    items: DropdownItemsBuilder,
    on_trigger: Option<MenuClickHandler>,
}

/// Built when the menu opens, not when the trigger renders.
///
/// A menu whose items are values has to rebuild every item -- and every click
/// handler hanging off one -- on every frame that draws the trigger, however
/// rarely anyone opens it.
type DropdownItemsBuilder = Rc<dyn Fn(&mut Window, &mut App) -> Vec<NyaMenuItem>>;

impl NyaDropdownMenu {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: None,
            icon_path: None,
            icon_size: None,
            tooltip: None,
            selected: false,
            disabled: false,
            anchor: NyaMenuAnchor::default(),
            min_width: None,
            max_width: None,
            max_height: None,
            scrollable: None,
            items: Rc::new(|_, _| Vec::new()),
            on_trigger: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn icon(mut self, icon_path: impl Into<SharedString>) -> Self {
        self.icon_path = Some(icon_path.into());
        self
    }

    pub fn icon_size(mut self, size: Pixels) -> Self {
        self.icon_size = Some(size);
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn anchor(mut self, anchor: NyaMenuAnchor) -> Self {
        self.anchor = anchor;
        self
    }

    pub fn min_width(mut self, width: Pixels) -> Self {
        self.min_width = Some(width);
        self
    }

    pub fn max_width(mut self, width: Pixels) -> Self {
        self.max_width = Some(width);
        self
    }

    pub fn max_height(mut self, height: Pixels) -> Self {
        self.max_height = Some(height);
        self
    }

    pub fn scrollable(mut self, scrollable: bool) -> Self {
        self.scrollable = Some(scrollable);
        self
    }

    /// Build the items when the menu opens.
    ///
    /// Prefer this wherever the items are derived from application state: the
    /// trigger renders on every frame, and this keeps that frame free of the
    /// items and their handlers.
    pub fn items_dynamic(
        mut self,
        builder: impl Fn(&mut Window, &mut App) -> Vec<NyaMenuItem> + 'static,
    ) -> Self {
        self.items = Rc::new(builder);
        self
    }

    pub fn items(mut self, items: impl IntoIterator<Item = NyaMenuItem>) -> Self {
        let items: Vec<_> = items.into_iter().collect();
        self.items = Rc::new(move |_, _| items.clone());
        self
    }

    pub fn on_trigger(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_trigger = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for NyaDropdownMenu {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut trigger = Button::new(self.id).ghost().small();
        if let Some(label) = self.label {
            trigger = trigger.label(label);
        }
        if let Some(icon_path) = self.icon_path {
            let icon = Icon::default()
                .path(icon_path)
                .when_some(self.icon_size, |icon, size| icon.with_size(size));
            trigger = trigger.icon(icon);
        }
        if let Some(tooltip) = self.tooltip {
            trigger = trigger.tooltip(tooltip);
        }
        if let Some(on_trigger) = self.on_trigger {
            trigger = trigger.on_click(move |event, window, cx| on_trigger(event, window, cx));
        }
        trigger = trigger.selected(self.selected);

        let items_builder = self.items;
        let min_width = self.min_width;
        let max_width = self.max_width;
        let max_height = self.max_height;
        let scrollable = self.scrollable;
        trigger.disabled(self.disabled).dropdown_menu_with_anchor(
            self.anchor.component_anchor(),
            move |menu, window, cx| {
                let items = items_builder(window, cx);
                let menu = style_nya_popup_menu(menu, min_width, max_width)
                    .when_some(max_height, |menu, height| menu.max_h(height))
                    .scrollable(popup_menu_should_scroll(&items, scrollable));
                items
                    .iter()
                    .fold(menu, |menu, item| item.append_to(menu, window, cx))
            },
        )
    }
}

/// A right-click menu attached to an element.
///
/// Exactly one of these may cover any given point. Nesting a second one inside
/// the first opens both on a single right-click: `ContextMenu` arms a plain
/// hitbox-gated mouse listener, and the outer element's listener is registered
/// last and so runs first, before the inner one - a `cx.stop_propagation()` on
/// the inner element cannot prevent it. Only the menu that receives the item
/// click dismisses, and the one left open re-focuses itself on every layout
/// pass. Anything opened afterwards therefore loses focus on the next frame, and
/// because a dialog is dismissed by dispatching `Cancel`/`Confirm` along the
/// focused element's path, its close, cancel and confirm controls all stop
/// responding for the rest of the session.
///
/// For a list, put the single menu on the list and aim it with
/// [`NyaContextMenu::new_dynamic`]: reset the target from a capture-phase
/// handler on the list, and re-aim it from a capture-phase handler on each row.
/// Capture runs outermost first, so the row's handler wins, and it runs before
/// the menu is built regardless of who stops the bubble.
#[derive(IntoElement)]
pub struct NyaContextMenu<E>
where
    E: InteractiveElement + ParentElement + Styled + IntoElement + 'static,
{
    element: E,
    items_builder: ContextMenuItemsBuilder,
    enabled: bool,
    min_width: Option<Pixels>,
}

impl<E> NyaContextMenu<E>
where
    E: InteractiveElement + ParentElement + Styled + IntoElement + 'static,
{
    pub fn new(element: E, items: impl IntoIterator<Item = NyaMenuItem>) -> Self {
        let items: Vec<_> = items.into_iter().collect();
        Self {
            element,
            items_builder: Rc::new(move |_, _| items.clone()),
            enabled: true,
            min_width: None,
        }
    }

    pub fn new_dynamic(
        element: E,
        items_builder: impl Fn(&mut Window, &mut App) -> Vec<NyaMenuItem> + 'static,
    ) -> Self {
        Self {
            element,
            items_builder: Rc::new(items_builder),
            enabled: true,
            min_width: None,
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn min_width(mut self, width: Pixels) -> Self {
        self.min_width = Some(width);
        self
    }

    #[doc(hidden)]
    pub fn test_min_width(&self) -> Option<Pixels> {
        self.min_width
    }
}

const AUTO_SCROLL_ITEM_THRESHOLD: usize = 20;

fn popup_menu_should_scroll(items: &[NyaMenuItem], override_value: Option<bool>) -> bool {
    override_value.unwrap_or_else(|| {
        items.len() > AUTO_SCROLL_ITEM_THRESHOLD
            && items.iter().all(|item| item.children().is_none())
    })
}

impl<E> RenderOnce for NyaContextMenu<E>
where
    E: InteractiveElement + ParentElement + Styled + IntoElement + 'static,
{
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        if !self.enabled {
            return self.element.into_any_element();
        }
        let items_builder = self.items_builder;
        let min_width = self.min_width;
        self.element
            .context_menu(move |menu, window, cx| {
                let items = items_builder(window, cx);
                let min_width = if items.iter().any(NyaMenuItem::is_action_bar) {
                    Some(px(256.))
                } else {
                    min_width
                };
                let direction_probe_width = submenu_direction_probe_width(
                    &items,
                    min_width,
                    window.mouse_position().x,
                    window.bounds().size.width,
                );
                let menu = style_nya_popup_menu(menu, min_width, None)
                    .when_some(direction_probe_width, |menu, width| menu.max_w(width));
                // PopupMenu paints a vertical scrollbar whenever `scrollable` is
                // true, even when every item fits. Keep short context menus clean;
                // only long flat menus need the bounded scrolling behavior.
                let menu = menu.scrollable(popup_menu_should_scroll(&items, None));
                items
                    .iter()
                    .fold(menu, |menu, item| item.append_to(menu, window, cx))
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{
        Context, IntoElement, MouseButton, MouseDownEvent, Render, SharedString, TestAppContext,
        VisualTestContext, Window, div, point, prelude::*, px, uniform_list,
    };

    use super::{
        NyaContextMenu, NyaMenuItem, NyaMenuItemKind, popup_menu_should_scroll,
        resolved_menu_widths, submenu_direction_probe_width,
    };

    struct DynamicContextMenuFixture {
        target: Rc<Cell<u8>>,
        current_triggered: Rc<Cell<bool>>,
        parent_triggered: Rc<Cell<bool>>,
        entry_triggered: Rc<Cell<bool>>,
    }

    impl Render for DynamicContextMenuFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let target_for_capture = self.target.clone();
            let target_for_row = self.target.clone();
            let target_for_menu = self.target.clone();
            let current_triggered = self.current_triggered.clone();
            let parent_triggered = self.parent_triggered.clone();
            let entry_triggered = self.entry_triggered.clone();
            NyaContextMenu::new_dynamic(
                div()
                    .id("dynamic-context-menu")
                    .size(px(100.))
                    .capture_any_mouse_down(move |event, _, _| {
                        if event.button == MouseButton::Right {
                            target_for_capture.set(0);
                        }
                    })
                    .child(
                        uniform_list(
                            "dynamic-context-menu-rows",
                            2,
                            cx.processor(move |_, range: std::ops::Range<usize>, _, _| {
                                range
                                    .map(|index| {
                                        let target_for_row = target_for_row.clone();
                                        div()
                                            .id(SharedString::from(format!(
                                                "dynamic-context-menu-row-{index}"
                                            )))
                                            .w_full()
                                            .h(px(30.))
                                            .on_mouse_down(MouseButton::Right, move |_, _, _| {
                                                target_for_row.set(index as u8 + 1)
                                            })
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .h(px(60.)),
                    ),
                move |_, _| match target_for_menu.get() {
                    1 => {
                        let parent_triggered = parent_triggered.clone();
                        vec![NyaMenuItem::action("Parent").on_click(move |_, _, _| {
                            parent_triggered.set(true);
                        })]
                    }
                    2 => {
                        let entry_triggered = entry_triggered.clone();
                        vec![NyaMenuItem::action("Entry").on_click(move |_, _, _| {
                            entry_triggered.set(true);
                        })]
                    }
                    _ => {
                        let current_triggered = current_triggered.clone();
                        vec![NyaMenuItem::action("Current").on_click(move |_, _, _| {
                            current_triggered.set(true);
                        })]
                    }
                },
            )
        }
    }

    #[test]
    fn menu_item_builders_preserve_behavior_flags() {
        let item = NyaMenuItem::action("Delete")
            .icon("icons/net/delete.svg")
            .icon_color(0x336699)
            .disabled(true)
            .checked(true)
            .danger();

        assert!(matches!(item.kind, NyaMenuItemKind::Action));
        assert_eq!(item.label.as_ref(), "Delete");
        assert_eq!(
            item.icon_path.as_ref().map(|path| path.as_ref()),
            Some("icons/net/delete.svg")
        );
        assert_eq!(item.shortcut, None);
        assert_eq!(item.test_icon_color(), Some(0x336699));
        assert!(item.disabled);
        assert!(item.checked);
        assert!(item.danger);
    }

    struct ActionBarFixture {
        invoked: Rc<Cell<u8>>,
    }

    impl Render for ActionBarFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let invoked = self.invoked.clone();
            NyaContextMenu::new_dynamic(
                div().id("action-bar-context-menu").size(px(100.)),
                move |_, _| {
                    let invoked = invoked.clone();
                    vec![
                        NyaMenuItem::action_bar([
                            NyaMenuItem::action("Cut").disabled(true),
                            NyaMenuItem::action("Copy").on_click(move |_, _, _| {
                                invoked.set(1);
                            }),
                            NyaMenuItem::action("Paste").disabled(true),
                            NyaMenuItem::action("Rename").disabled(true),
                            NyaMenuItem::action("Delete").disabled(true),
                        ]),
                        NyaMenuItem::separator(),
                        NyaMenuItem::action("Open"),
                    ]
                },
            )
        }
    }

    #[test]
    fn action_bar_preserves_each_command_state() {
        let bar = NyaMenuItem::action_bar([
            NyaMenuItem::action("Cut").disabled(true),
            NyaMenuItem::action("Copy"),
            NyaMenuItem::action("Paste").disabled(true),
            NyaMenuItem::action("Rename"),
            NyaMenuItem::action("Delete").danger(),
        ]);
        let NyaMenuItemKind::ActionBar(items) = &bar.kind else {
            panic!("expected action bar");
        };
        assert_eq!(items.len(), 5);
        assert_eq!(
            items
                .iter()
                .map(|item| item.label.as_ref())
                .collect::<Vec<_>>(),
            ["Cut", "Copy", "Paste", "Rename", "Delete"]
        );
        assert!(items[0].disabled);
        assert!(items[2].disabled);
        assert!(items[4].danger);
        assert!(bar.is_action_bar());
    }

    #[test]
    fn popup_menus_scroll_only_for_long_flat_item_lists() {
        let short = vec![NyaMenuItem::action("Open"), NyaMenuItem::action("Delete")];
        assert!(!popup_menu_should_scroll(&short, None));

        let long = (0..21)
            .map(|index| NyaMenuItem::action(format!("Item {index}")))
            .collect::<Vec<_>>();
        assert!(popup_menu_should_scroll(&long, None));

        let mut long_with_submenu = long.clone();
        long_with_submenu.push(NyaMenuItem::submenu(
            "Move",
            vec![NyaMenuItem::action("Group")],
        ));
        assert!(!popup_menu_should_scroll(&long_with_submenu, None));

        assert!(popup_menu_should_scroll(&short, Some(true)));
        assert!(!popup_menu_should_scroll(&long, Some(false)));
    }

    #[test]
    fn context_menu_retains_optional_minimum_width() {
        let menu = NyaContextMenu::new(div(), [NyaMenuItem::action("Open")]).min_width(px(200.));

        assert_eq!(menu.test_min_width(), Some(px(200.)));
    }

    #[test]
    fn standard_menu_width_is_220_unless_a_caller_sets_a_constraint() {
        assert_eq!(
            resolved_menu_widths(None, None),
            (Some(px(220.)), Some(px(220.)))
        );
        assert_eq!(
            resolved_menu_widths(Some(px(208.)), None),
            (Some(px(208.)), None)
        );
        assert_eq!(
            resolved_menu_widths(None, Some(px(180.))),
            (None, Some(px(180.)))
        );
    }

    /// The SFTP browser context menu pins itself to 208px (the Tauri `w-52` /
    /// `min-w-[200px]` equivalent) so item labels and shortcut hints do not
    /// reflow between rows when the menu opens. Lock that exact value.
    #[test]
    fn context_menu_supports_the_sftp_browser_minimum_width() {
        let menu = NyaContextMenu::new(div(), [NyaMenuItem::action("Preview")]).min_width(px(208.));

        assert_eq!(menu.test_min_width(), Some(px(208.)));
    }

    #[test]
    fn context_submenu_reserves_parent_and_child_width_near_right_edge() {
        let items = vec![NyaMenuItem::submenu(
            "Move to category",
            vec![NyaMenuItem::action("Category")],
        )];

        // Matches the reported narrow-window geometry: the menu opens around
        // x=48 in a 381px content area, where two 220px menus cannot fit right.
        assert_eq!(
            submenu_direction_probe_width(&items, None, px(48.), px(381.)),
            Some(px(432.))
        );
        assert_eq!(
            submenu_direction_probe_width(&items, None, px(48.), px(900.)),
            None
        );
        assert_eq!(
            submenu_direction_probe_width(&[NyaMenuItem::action("Open")], None, px(360.), px(381.)),
            None
        );
    }

    #[test]
    fn context_submenu_uses_configured_parent_and_child_widths() {
        let items = vec![
            NyaMenuItem::submenu(
                "Move",
                vec![NyaMenuItem::action("Folder").label_indent(px(10.))],
            )
            .submenu_min_width(px(176.))
            .submenu_max_height(px(288.))
            .submenu_scrollable(true),
        ];

        assert_eq!(
            submenu_direction_probe_width(&items, Some(px(160.)), px(48.), px(381.)),
            None
        );
        assert_eq!(
            submenu_direction_probe_width(&items, Some(px(160.)), px(48.), px(370.)),
            Some(px(328.))
        );
        assert_eq!(
            items[0].test_submenu_layout(),
            (Some(px(176.)), Some(px(288.)), Some(true), None)
        );
        assert_eq!(
            items[0].children().expect("submenu")[0].test_submenu_layout(),
            (None, None, None, Some(px(10.)))
        );
    }

    #[test]
    fn submenu_retains_nested_items() {
        let item = NyaMenuItem::submenu(
            "Move",
            vec![NyaMenuItem::action("Group A"), NyaMenuItem::separator()],
        );

        let NyaMenuItemKind::Submenu(items) = item.kind else {
            panic!("expected submenu");
        };
        assert_eq!(items.len(), 2);
    }

    #[gpui::test]
    fn action_bar_enabled_button_can_be_used_with_keyboard(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let invoked = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view({
            let invoked = invoked.clone();
            move |_, _| ActionBarFixture {
                invoked: invoked.clone(),
            }
        });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: point(px(10.), px(10.)),
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        cx.update(|window, cx| window.focus_next(cx));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(invoked.get(), 1);
    }

    #[gpui::test]
    fn dynamic_context_menu_uses_the_latest_uniform_list_target(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let target = Rc::new(Cell::new(0));
        let current_triggered = Rc::new(Cell::new(false));
        let parent_triggered = Rc::new(Cell::new(false));
        let entry_triggered = Rc::new(Cell::new(false));
        let (_, cx) = cx.add_window_view({
            let target = target.clone();
            let current_triggered = current_triggered.clone();
            let parent_triggered = parent_triggered.clone();
            let entry_triggered = entry_triggered.clone();
            move |_, _| DynamicContextMenuFixture {
                target: target.clone(),
                current_triggered: current_triggered.clone(),
                parent_triggered: parent_triggered.clone(),
                entry_triggered: entry_triggered.clone(),
            }
        });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });

        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: point(px(10.), px(10.)),
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        cx.simulate_keystrokes("down enter");
        cx.run_until_parked();

        assert!(parent_triggered.get());
        assert!(!entry_triggered.get());
        assert!(!current_triggered.get());

        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: point(px(10.), px(40.)),
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        cx.simulate_keystrokes("down enter");
        cx.run_until_parked();

        assert!(entry_triggered.get());
        assert!(!current_triggered.get());

        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: point(px(10.), px(80.)),
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        cx.simulate_keystrokes("down enter");
        cx.run_until_parked();

        assert!(current_triggered.get());
    }
}
