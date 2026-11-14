use std::{ops::Range, sync::Arc};

use gpui::{
    AnyElement, App, Bounds, Context, FontWeight, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, Pixels, Point, Rgba, ScrollHandle, SharedString, StatefulInteractiveElement,
    UniformListDecoration, WeakEntity, Window, canvas, div, prelude::*, px, rgb, uniform_list,
};
use nyaterm_core::{
    AssetDisplayLabels, AssetFilterKey, AssetRecord, AssetSortDirection, AssetViewMode,
    SavedConnection, StartWorkspaceMode, build_group_path, format_accelerators,
    format_asset_address, format_bytes, format_cpu_summary, format_disk_summary,
};
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaCheckbox, NyaContextMenu, NyaHorizontalScrollbar,
    NyaIconButton, NyaScrollable, NyaSearchInput, NyaSelect, NyaUniformListScrollbar, ThemePalette,
};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::assets::{
    ASSET_CARD_ROW_HEIGHT, ASSET_SELECT_COLUMN_WIDTH, ASSET_TABLE_ACTIONS_WIDTH,
    ASSET_TABLE_ROW_HEIGHT, AssetColumn,
};
use crate::features::formatting::format_last_used_ms;
use crate::features::shell::WorkspaceTabDragPayload;
use crate::features::view_widgets::connection_type_icon;
use nyaterm_ui::icons::resolve_connection_icon;

const ASSET_CARD_MIN_WIDTH: f32 = 300.;
const ASSET_CARD_GAP: f32 = 8.;
const ASSET_CARD_HORIZONTAL_PADDING: f32 = 24.;
const ASSET_SCROLLBAR_TRACK_WIDTH: f32 = 12.;

struct AssetRowActionsDecoration {
    records: Arc<[AssetRecord]>,
    app: WeakEntity<NyaTermApp>,
    horizontal_scroll: ScrollHandle,
    /// The row fill for each parity. The pinned action column must repeat the
    /// trim of the row it covers or a color seam shows at the column boundary.
    row_backgrounds: [Rgba; 2],
    border: u32,
}

impl AssetRowActionsDecoration {
    fn new(
        records: Arc<[AssetRecord]>,
        app: WeakEntity<NyaTermApp>,
        horizontal_scroll: ScrollHandle,
        row_backgrounds: [Rgba; 2],
        border: u32,
    ) -> Self {
        Self {
            records,
            app,
            horizontal_scroll,
            row_backgrounds,
            border,
        }
    }
}

impl UniformListDecoration for AssetRowActionsDecoration {
    fn compute(
        &self,
        visible_range: Range<usize>,
        _bounds: Bounds<Pixels>,
        _scroll_offset: Point<Pixels>,
        item_height: Pixels,
        _item_count: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let viewport_width = self.horizontal_scroll.bounds().size.width;
        let horizontal_offset = self.horizontal_scroll.offset().x;
        let action_left = asset_action_left_in_decoration(viewport_width, horizontal_offset);
        let mut layer = div().relative().size_full();

        for index in visible_range {
            let Some(record) = self.records.get(index) else {
                continue;
            };
            let connection = record.connection.clone();
            let edit = connection.clone();
            let connect_app = self.app.clone();
            let edit_app = self.app.clone();
            let menu_app = self.app.clone();
            let menu_id = connection.id.clone();
            let selector = format!("asset-actions-{}", connection.id);

            let background = self
                .app
                .upgrade()
                .map(|app| {
                    let app = app.read(cx);
                    let palette = app.theme_palette();
                    if app.start_workspace.is_asset_selected(&record.connection.id) {
                        app.shell_surface_color(asset_selected_fill(palette))
                    } else if app.start_workspace.is_asset_hovered(&record.connection.id) {
                        app.shell_surface_color(asset_row_fills(palette).hover)
                    } else {
                        self.row_backgrounds[index % 2]
                    }
                })
                .unwrap_or(self.row_backgrounds[index % 2]);
            let hover_app = self.app.clone();
            let hover_id = record.connection.id.clone();
            layer = layer.child(
                div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .capture_any_mouse_down(move |event: &MouseDownEvent, _, cx| {
                        if event.button == MouseButton::Right
                            && let Some(app) = menu_app.upgrade()
                        {
                            app.update(cx, |this, _| {
                                this.start_workspace
                                    .set_context_connection_id(Some(menu_id.clone()));
                            });
                        }
                    })
                    .absolute()
                    .left(action_left)
                    .top(item_height * index)
                    .h(item_height)
                    .w(px(ASSET_TABLE_ACTIONS_WIDTH))
                    .pl_1()
                    .pr_4()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(self.border))
                    .bg(background)
                    .on_hover(move |hovered, _, cx| {
                        if let Some(app) = hover_app.upgrade() {
                            app.update(cx, |app, cx| {
                                if app.start_workspace.set_asset_hovered(&hover_id, *hovered) {
                                    cx.notify();
                                }
                            });
                        }
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(asset_action_button(
                        format!("asset-connect-{}", connection.id),
                        "icons/conn/connect.svg",
                        t!("savedConnections.connect"),
                        self.border,
                        move |_, window, cx| {
                            cx.stop_propagation();
                            let Some(app) = connect_app.upgrade() else {
                                return;
                            };
                            app.update(cx, |this, cx| {
                                this.start_saved_connections_by_ids(
                                    std::slice::from_ref(&connection.id),
                                    window,
                                    cx,
                                );
                            });
                        },
                    ))
                    .child(asset_action_button(
                        format!("asset-edit-{}", edit.id),
                        "icons/edit.svg",
                        t!("savedConnections.edit"),
                        self.border,
                        move |_, window, cx| {
                            cx.stop_propagation();
                            let Some(app) = edit_app.upgrade() else {
                                return;
                            };
                            app.update(cx, |this, cx| {
                                this.edit_saved_connection_by_id(&edit.id, window, cx);
                            });
                        },
                    )),
            );
        }

        layer.into_any_element()
    }
}

fn asset_selected_fill(palette: ThemePalette) -> u32 {
    let mix = if palette.is_dark() { 0.28 } else { 0.16 };
    let channel = |shift: u32| {
        let base = ((palette.bg >> shift) & 255u32) as f32;
        let accent = ((palette.primary >> shift) & 255u32) as f32;
        (base * (1. - mix) + accent * mix).round() as u32
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

fn asset_action_left_in_decoration(viewport_width: Pixels, horizontal_scroll: Pixels) -> Pixels {
    viewport_width - horizontal_scroll - px(ASSET_TABLE_ACTIONS_WIDTH)
}

/// The alternating-row fill pair used by the asset list.
///
/// A zebra stripe needs a measurable step above the base row with a clearly
/// higher hover fill. The palette's `surface` fails the first step on several
/// dark themes (tokyo-night differs by 2-3 levels, one-dark-pro by 6,
/// github-dark by 9), so dark themes derive the stripe from
/// `surface_elevated` instead. Light themes keep the established `surface`
/// tint so the stripe stays subtle.
///
/// This lives beside the list instead of in `ThemePalette` because the shared
/// theme layer is owned by the input/theme work; move it there once the palette
/// exposes a first-class alternating-row token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AssetRowFills {
    stripe: u32,
    hover: u32,
}

/// Contrast each zebra step must clear. Dark themes split the base-to-raised
/// range evenly when that range is wide enough; narrower ranges get an explicit
/// stripe step and a hover fill lifted above the raised surface.
const ASSET_ROW_TARGET_CONTRAST: f32 = 1.13;

fn asset_row_fills(palette: ThemePalette) -> AssetRowFills {
    if !palette.is_dark() {
        return AssetRowFills {
            stripe: palette.surface,
            hover: palette.surface_elevated,
        };
    }
    let base = palette.bg;
    let mut ceiling = palette.surface_elevated;
    if relative_luminance(ceiling) <= relative_luminance(base) {
        // Custom themes may point the raised surface at or below the base; lift
        // the ceiling toward the theme text colour instead of inventing a colour.
        ceiling = mix_rgb(base, palette.text, 0.12);
    }
    let balanced = contrast_ratio(base, ceiling).sqrt();
    let stripe = if balanced >= ASSET_ROW_TARGET_CONTRAST {
        // Equal perceptual steps: the stripe sits at the geometric mean of the
        // two luminances, so hover stays the raised surface colour.
        let target = (((relative_luminance(base) + 0.05) * (relative_luminance(ceiling) + 0.05))
            .sqrt())
            - 0.05;
        mix_at_luminance(base, ceiling, target)
    } else {
        mix_at_luminance(
            base,
            ceiling,
            ASSET_ROW_TARGET_CONTRAST * (relative_luminance(base) + 0.05) - 0.05,
        )
    };
    let hover = if contrast_ratio(stripe, ceiling) >= ASSET_ROW_TARGET_CONTRAST {
        ceiling
    } else {
        let target = ASSET_ROW_TARGET_CONTRAST * (relative_luminance(stripe) + 0.05) - 0.05;
        mix_at_luminance(ceiling, palette.text, target)
    };
    AssetRowFills { stripe, hover }
}

/// Mixes `from` toward `to` until the blend reaches `target` relative luminance.
/// Falls back to `to` when the range cannot reach it.
fn mix_at_luminance(from: u32, to: u32, target: f32) -> u32 {
    if relative_luminance(to) <= target {
        return to;
    }
    let (mut low, mut high) = (0.0f32, 1.0f32);
    for _ in 0..12 {
        let mid = (low + high) / 2.;
        if relative_luminance(mix_rgb(from, to, mid)) < target {
            low = mid;
        } else {
            high = mid;
        }
    }
    mix_rgb(from, to, high)
}

fn relative_luminance(color: u32) -> f32 {
    let channel = |shift: u32| {
        let value = ((color >> shift) & 0xff) as f32 / 255.;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.045) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

fn contrast_ratio(a: u32, b: u32) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn mix_rgb(a: u32, b: u32, t: f32) -> u32 {
    let mix = |shift: u32| {
        let from = ((a >> shift) & 0xff) as f32;
        let to = ((b >> shift) & 0xff) as f32;
        (from + (to - from) * t).round().clamp(0., 255.) as u32
    };
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

fn asset_action_button(
    id: impl Into<SharedString>,
    icon_path: &'static str,
    tooltip: impl Into<SharedString>,
    border: u32,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let id = id.into();
    let selector = id.to_string();
    div()
        .id(SharedString::from(format!("{id}-shell")))
        .debug_selector(move || selector.clone())
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .border_1()
        .border_color(rgb(border))
        .child(
            NyaIconButton::new(id, icon_path)
                .icon_size(px(15.))
                .tooltip(tooltip)
                .on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    on_click(event, window, cx);
                }),
        )
        .into_any_element()
}

fn responsive_asset_card_columns(viewport_width: f32) -> usize {
    if !viewport_width.is_finite() || viewport_width <= 0. {
        return 1;
    }
    let available = (viewport_width - ASSET_CARD_HORIZONTAL_PADDING).max(0.);
    (((available + ASSET_CARD_GAP) / (ASSET_CARD_MIN_WIDTH + ASSET_CARD_GAP)).floor() as usize)
        .clamp(1, 3)
}

impl NyaTermApp {
    fn asset_item(
        &self,
        connection_id: &str,
        view: &str,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let menu_id = connection_id.to_string();
        let connect_id = connection_id.to_string();
        let select_id = connection_id.to_string();
        let hover_id = connection_id.to_string();
        let selector = format!("asset-{view}-{connection_id}");
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .capture_any_mouse_down(cx.listener(move |this, event: &MouseDownEvent, _, _| {
                if event.button == MouseButton::Right {
                    this.start_workspace
                        .set_context_connection_id(Some(menu_id.clone()));
                }
            }))
            .on_hover(cx.listener(move |this, hovered, _, cx| {
                if this.start_workspace.set_asset_hovered(&hover_id, *hovered) {
                    cx.notify();
                }
            }))
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    let clicks = match event {
                        gpui::ClickEvent::Mouse(mouse) if mouse.up.button == MouseButton::Left => {
                            event.click_count()
                        }
                        _ => return,
                    };
                    if clicks == 2 {
                        cx.stop_propagation();
                        this.start_saved_connections_by_ids(
                            std::slice::from_ref(&connect_id),
                            window,
                            cx,
                        );
                        return;
                    }
                    cx.stop_propagation();
                    let modifiers = event.modifiers();
                    let additive = modifiers.control || modifiers.platform;
                    this.apply_asset_selection_click(&select_id, additive, modifiers.shift, cx);
                }),
            )
    }

    /// A row/card leading checkbox. Its own mouse-down is swallowed so the
    /// surrounding row click handler cannot turn the toggle into a selection.
    fn asset_select_checkbox(
        &self,
        connection_id: &str,
        view: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = format!("asset-select-{view}-{connection_id}");
        let selector = id.clone();
        let target = connection_id.to_string();
        let checked = self.start_workspace.is_asset_selected(connection_id);
        div()
            .id(SharedString::from(format!("{id}-shell")))
            .debug_selector(move || selector.clone())
            .w(px(ASSET_SELECT_COLUMN_WIDTH))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(NyaCheckbox::new(id).checked(checked).on_click(cx.listener(
                move |this, _: &bool, _, cx| {
                    cx.stop_propagation();
                    this.toggle_asset_selection(&target, cx);
                },
            )))
            .into_any_element()
    }

    /// Header checkbox that selects or clears everything currently visible.
    fn asset_select_all_checkbox(
        &self,
        checked: bool,
        header: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self
            .start_workspace
            .visible_selected_count(self.start_workspace.visible_records());
        div()
            .id(if header {
                "asset-select-all-header-shell"
            } else {
                "asset-select-all-shell"
            })
            .debug_selector(|| "asset-select-all-shell".to_string())
            .flex_none()
            .min_h(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .when(header, |this| this.w(px(ASSET_SELECT_COLUMN_WIDTH)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                NyaCheckbox::new(if header {
                    "asset-select-all-header"
                } else {
                    "asset-select-all"
                })
                .checked(checked)
                .indeterminate(selected > 0 && !checked)
                .map(|checkbox| {
                    if header {
                        checkbox
                    } else {
                        checkbox.label(t!("assets.selectAll"))
                    }
                })
                .on_click(cx.listener(move |this, _: &bool, _, cx| {
                    cx.stop_propagation();
                    this.toggle_asset_select_all(cx);
                })),
            )
            .into_any_element()
    }

    pub(in crate::features) fn apply_asset_selection_click(
        &mut self,
        connection_id: &str,
        additive: bool,
        range: bool,
        cx: &mut Context<Self>,
    ) {
        let records = self.start_workspace.visible_records().to_vec();
        let changed = if range {
            self.start_workspace
                .select_asset_range(&records, connection_id, additive)
        } else if additive {
            self.start_workspace.toggle_asset_selection(connection_id)
        } else {
            self.start_workspace.select_asset_only(connection_id)
        };
        if changed {
            cx.notify();
        }
    }

    pub(in crate::features) fn toggle_asset_selection(
        &mut self,
        connection_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.start_workspace.toggle_asset_selection(connection_id) {
            cx.notify();
        }
    }

    pub(in crate::features) fn toggle_asset_select_all(&mut self, cx: &mut Context<Self>) {
        let records = self.start_workspace.visible_records().to_vec();
        let changed = if self.start_workspace.all_assets_selected(&records) {
            self.start_workspace.deselect_visible_assets(&records)
        } else {
            self.start_workspace.select_all_assets(&records)
        };
        if changed {
            cx.notify();
        }
    }

    pub(in crate::features) fn clear_asset_selection(&mut self, cx: &mut Context<Self>) {
        if self.start_workspace.clear_asset_selection() {
            cx.notify();
        }
    }

    fn asset_context_menu(
        &self,
        container: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let app = cx.weak_entity();
        NyaContextMenu::new_dynamic(
            container.capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
                if event.button == MouseButton::Right {
                    this.start_workspace.set_context_connection_id(None);
                }
            })),
            move |_, cx| {
                let Some(app) = app.upgrade() else {
                    return Vec::new();
                };
                app.update(cx, |this, cx| {
                    let Some(id) = this
                        .start_workspace
                        .context_connection_id()
                        .map(str::to_string)
                    else {
                        return Vec::new();
                    };
                    let target_ids = this.start_workspace.asset_menu_target_ids(&id);
                    this.connection_target_menu_items(id, target_ids, cx)
                })
            },
        )
        .into_any_element()
    }

    pub(in crate::features) fn empty_workspace_state(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mode = self.start_workspace.mode();
        let content = match mode {
            StartWorkspaceMode::Workbench => self.workbench_workspace_state(cx).into_any_element(),
            StartWorkspaceMode::Assets => self.asset_workspace_state(cx),
        };
        let palette = self.theme_palette();
        // Asset controls and text use the application palette, including their background.
        let background = match mode {
            StartWorkspaceMode::Workbench => self.terminal_theme_palette().terminal_bg,
            StartWorkspaceMode::Assets => palette.bg,
        };
        div()
            .id("empty-workspace-tab-drop")
            .track_focus(self.terminal.input_focus())
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(match mode {
                StartWorkspaceMode::Workbench => self.shell_terminal_surface_color(background),
                StartWorkspaceMode::Assets => self.shell_surface_color(background),
            })
            // Empty workspaces hide the tab strip, so the surface must receive
            // tab drops before the outside-release fallback creates a window.
            .drag_over::<WorkspaceTabDragPayload>(move |this, _, _, _| {
                this.border_2().border_color(rgb(palette.focus_ring))
            })
            .on_drop(
                cx.listener(|this, payload: &WorkspaceTabDragPayload, _, cx| {
                    this.accept_session_tab_drop(payload, cx);
                    if payload.source_workspace_id != this.workspace_id {
                        this.request_tab_tree_move(
                            payload,
                            nyaterm_core::MoveTabPlacement::Append,
                            cx,
                        );
                    }
                }),
            )
            .child(content)
            .child(
                div()
                    .absolute()
                    .top(px(12.))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .h(px(32.))
                            .flex()
                            .items_center()
                            .gap_1()
                            .p(px(2.))
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(palette.border))
                            .bg(self.shell_surface_color(palette.surface))
                            .child(self.start_workspace_mode_button(
                                StartWorkspaceMode::Workbench,
                                t!("assets.workbench"),
                                cx,
                            ))
                            .child(self.start_workspace_mode_button(
                                StartWorkspaceMode::Assets,
                                t!("assets.assets"),
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }

    fn start_workspace_mode_button(
        &self,
        mode: StartWorkspaceMode,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let active = self.start_workspace.mode() == mode;
        div()
            .id(SharedString::from(format!(
                "start-workspace-mode-{}",
                mode.as_str()
            )))
            .h(px(26.))
            .px_3()
            .rounded_sm()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_size(px(11.))
            .font_weight(FontWeight(if active { 600. } else { 500. }))
            .text_color(rgb(if active {
                palette.primary
            } else {
                palette.text_muted
            }))
            .bg(if active {
                rgb(palette.hover)
            } else {
                rgb(palette.surface)
            })
            .hover(|this| this.bg(rgb(palette.hover)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.start_workspace.set_mode(mode) {
                    this.persist_ui_layout();
                    cx.notify();
                }
            }))
            .child(label.into())
            .into_any_element()
    }

    fn asset_workspace_state(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.theme_palette();
        self.start_workspace.sync_group_options_for_catalog(
            self.connection_state.catalog_revisions().1,
            self.connection_state.groups(),
            cx,
        );
        let labels = AssetDisplayLabels {
            none: t!("assets.none").to_string(),
            not_applicable: t!("assets.notApplicable").to_string(),
            local_machine: t!("assets.localMachine").to_string(),
        };
        let records: Arc<[AssetRecord]> = self.start_workspace.records(
            self.connection_state.catalog_revisions(),
            self.connection_state.connections(),
            self.connection_state.groups(),
            &labels,
            &t!("assets.title"),
        );
        let search = self.start_workspace.search_field();
        let group_select = self.start_workspace.group_select();
        let list_mode = self.start_workspace.view_mode() == AssetViewMode::List;
        let count = records.len();
        let selected_count = self.start_workspace.selected_asset_count();
        let all_selected = self.start_workspace.all_assets_selected(&records);

        let body = if records.is_empty() {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.))
                .text_color(rgb(palette.text_muted))
                .child(t!("assets.noResults"))
                .into_any_element()
        } else if list_mode {
            self.asset_table(records, labels, cx)
        } else {
            self.asset_cards(records, labels, cx)
        };

        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .pt(px(54.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .text_color(rgb(palette.text))
            .child(
                div()
                    .flex_none()
                    .px_5()
                    .pb_3()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .h(px(36.))
                            .w_full()
                            .child(NyaSearchInput::new("asset-workspace-search", &search)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(self.asset_filter_buttons(cx))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(self.asset_view_button(
                                        AssetViewMode::List,
                                        t!("assets.list"),
                                        cx,
                                    ))
                                    .child(self.asset_view_button(
                                        AssetViewMode::Cards,
                                        t!("assets.cards"),
                                        cx,
                                    )),
                            ),
                    ),
            )
            .child(
                div()
                    .min_h(px(38.))
                    .py_1()
                    .flex_wrap()
                    .flex_none()
                    .px_5()
                    .border_t_1()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .bg(self.shell_surface_color(palette.section_header))
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().w(px(280.)).child(NyaSelect::new(&group_select)))
                    .child(self.asset_breadcrumb())
                    .child(
                        div()
                            .ml_auto()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(self.asset_select_all_checkbox(all_selected, false, cx))
                            .when(selected_count > 0, |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(rgb(palette.text))
                                                .child(t!(
                                                    "assets.selectedCount",
                                                    count = selected_count,
                                                    hidden =
                                                        self.start_workspace.hidden_selected_count(
                                                            self.start_workspace.visible_records()
                                                        )
                                                )),
                                        )
                                        .child(
                                            NyaButton::new(
                                                "asset-batch-edit",
                                                t!("assets.batchEdit"),
                                            )
                                            .small()
                                            .variant(NyaButtonVariant::Primary)
                                            .on_click(
                                                cx.listener(|this, _, window, cx| {
                                                    this.open_asset_batch_edit(window, cx);
                                                }),
                                            ),
                                        )
                                        .child(
                                            NyaButton::new(
                                                "asset-select-clear",
                                                t!("assets.clearSelection"),
                                            )
                                            .small()
                                            .on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    this.clear_asset_selection(cx);
                                                }),
                                            ),
                                        ),
                                )
                            })
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(palette.text_muted))
                                    .child(t!("assets.items", count = count)),
                            ),
                    ),
            )
            .child(body)
            .into_any_element()
    }

    fn asset_filter_buttons(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div()
            .id("asset-tag-filters")
            .min_w_0()
            .flex_1()
            .max_h(px(96.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1();
        let all_active = self.start_workspace.filters().is_empty();
        row = row.child(self.asset_filter_button(None, t!("assets.all"), all_active, cx));
        let mut tags = self
            .connection_state
            .connections()
            .iter()
            .flat_map(|connection| connection.tags.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>();
        tags.extend(
            self.start_workspace
                .filters()
                .iter()
                .filter_map(|filter| match filter {
                    AssetFilterKey::Tag(tag) => Some(tag.clone()),
                    _ => None,
                }),
        );
        for label in tags {
            let filter = AssetFilterKey::Tag(label.clone());
            row = row.child(self.asset_filter_button(
                Some(filter.clone()),
                label,
                self.start_workspace.filters().contains(&filter),
                cx,
            ));
        }
        row.overflow_y_scrollbar().into_any_element()
    }

    fn asset_filter_button(
        &self,
        filter: Option<AssetFilterKey>,
        label: impl Into<SharedString>,
        active: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = match filter.as_ref() {
            Some(AssetFilterKey::Linux) => "linux".to_string(),
            Some(AssetFilterKey::Windows) => "windows".to_string(),
            Some(AssetFilterKey::Gpu) => "gpu".to_string(),
            Some(AssetFilterKey::Npu) => "npu".to_string(),
            Some(AssetFilterKey::Tag(tag)) => format!("tag-{tag}"),
            None => "all".to_string(),
        };
        nyaterm_ui::NyaButton::new(format!("asset-filter-{id}"), label)
            .small()
            .selected(active)
            .on_click(cx.listener(move |this, _, _, cx| {
                match filter.clone() {
                    Some(filter) => this.start_workspace.toggle_filter(filter),
                    None => this.start_workspace.clear_filters(),
                }
                cx.notify();
            }))
            .into_any_element()
    }

    fn asset_view_button(
        &self,
        mode: AssetViewMode,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let active = self.start_workspace.view_mode() == mode;
        let id = if mode == AssetViewMode::List {
            "list"
        } else {
            "cards"
        };
        div()
            .id(SharedString::from(format!("asset-view-{id}")))
            .h(px(28.))
            .px_2()
            .rounded_md()
            .cursor_pointer()
            .flex()
            .items_center()
            .text_size(px(11.))
            .text_color(rgb(if active {
                palette.primary
            } else {
                palette.text_muted
            }))
            .bg(if active {
                rgb(palette.hover)
            } else {
                rgb(palette.surface)
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.start_workspace.set_view_mode(mode);
                cx.notify();
            }))
            .child(label.into())
            .into_any_element()
    }

    fn asset_breadcrumb(&self) -> AnyElement {
        let palette = self.theme_palette();
        let path = build_group_path(
            self.connection_state.groups(),
            self.start_workspace.selected_group_id(),
        );
        let names = path
            .into_iter()
            .map(|part| {
                if part.id.is_none() {
                    t!("assets.title").to_string()
                } else {
                    part.name
                }
            })
            .collect::<Vec<_>>()
            .join("  /  ");
        div()
            .min_w_0()
            .flex_1()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_size(px(11.))
            .text_color(rgb(palette.text))
            .child(names)
            .into_any_element()
    }

    fn asset_table(
        &mut self,
        records: Arc<[AssetRecord]>,
        labels: AssetDisplayLabels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let widths = AssetColumn::ALL.map(|column| self.start_workspace.column_width(column));
        let min_table_width = self.start_workspace.table_width();
        let horizontal_scroll = self.start_workspace.table_horizontal_scroll().clone();
        let scroll = self.start_workspace.list_scroll().clone();
        // Zebra rows alternate the theme's own background (white in light themes,
        // the dark theme's base surface otherwise) with the derived stripe. The
        // pinned action column takes the same pair so the two stay continuous.
        let fills = asset_row_fills(palette);
        let row_backgrounds = [
            self.shell_surface_color(palette.bg),
            self.shell_surface_color(fills.stripe),
        ];
        let mut header = div()
            .h(px(34.))
            .w_full()
            .min_w(px(min_table_width))
            .flex_none()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(palette.section_header))
            .child(self.asset_select_all_checkbox(
                self.start_workspace.all_assets_selected(&records),
                true,
                cx,
            ));
        for (column, label) in [
            (AssetColumn::Name, t!("assets.name")),
            (AssetColumn::Address, t!("assets.address")),
            (AssetColumn::ConnectionTime, t!("assets.connectionTime")),
            (AssetColumn::Cpu, t!("assets.cpu")),
            (AssetColumn::Memory, t!("assets.memory")),
            (AssetColumn::Storage, t!("assets.storage")),
            (AssetColumn::Accelerators, t!("assets.accelerators")),
        ] {
            header = header.child(self.asset_header_cell(column, label, cx));
        }
        header = header.child(
            div()
                .ml_auto()
                .w(px(ASSET_TABLE_ACTIONS_WIDTH))
                .h_full()
                .flex_none(),
        );

        let row_records = records.clone();
        let row_actions = AssetRowActionsDecoration::new(
            records,
            cx.weak_entity(),
            horizontal_scroll.clone(),
            row_backgrounds,
            palette.border,
        );
        let list = uniform_list(
            "asset-table-rows",
            row_records.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|index| {
                        row_records
                            .get(index)
                            .cloned()
                            .map(|record| (index, record))
                    })
                    .map(|(index, record)| {
                        this.asset_table_row(index, record, &labels, &widths, min_table_width, cx)
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .with_decoration(row_actions)
        .w_full()
        .flex_1()
        .min_h_0()
        .track_scroll(&scroll);

        let scrolling_table = div()
            .w_full()
            .min_w(px(min_table_width))
            .h_full()
            .flex()
            .flex_col()
            .child(header)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(list),
            );

        self.asset_context_menu(
            div()
                .id("asset-table")
                .debug_selector(|| "asset-table".to_string())
                .relative()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("asset-table-horizontal-viewport")
                        .size_full()
                        .min_h_0()
                        .min_w_0()
                        .overflow_x_scroll()
                        .restrict_scroll_to_axis()
                        .track_scroll(&horizontal_scroll)
                        .child(scrolling_table),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .right_0()
                        .h(px(34.))
                        .w(px(ASSET_TABLE_ACTIONS_WIDTH))
                        .pl_2()
                        .pr_4()
                        .flex()
                        .items_center()
                        .justify_end()
                        .border_b_1()
                        .border_color(rgb(palette.border))
                        .bg(rgb(palette.section_header))
                        .text_size(px(11.))
                        .text_color(rgb(palette.text_muted))
                        .child(t!("assets.actions")),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right(px(ASSET_SCROLLBAR_TRACK_WIDTH))
                        .bottom_0()
                        .child(NyaHorizontalScrollbar::new(
                            "asset-table-horizontal-scrollbar",
                            &horizontal_scroll,
                        )),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(34.))
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .child(NyaUniformListScrollbar::new(
                            "asset-table-vertical-scrollbar",
                            &scroll,
                        )),
                ),
            cx,
        )
    }

    fn asset_header_cell(
        &self,
        column: AssetColumn,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let active = self
            .start_workspace
            .sort()
            .is_some_and(|sort| sort.key == column.sort_key());
        let indicator = self
            .start_workspace
            .sort()
            .filter(|sort| sort.key == column.sort_key())
            .map(|sort| {
                if sort.direction == AssetSortDirection::Asc {
                    " ↑"
                } else {
                    " ↓"
                }
            })
            .unwrap_or(" ↕");
        div()
            .id(SharedString::from(format!(
                "asset-sort-{}",
                column.sort_key().as_str()
            )))
            .relative()
            .w(px(self.start_workspace.column_width(column)))
            .h_full()
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .cursor_pointer()
            .text_size(px(11.))
            .font_weight(FontWeight(500.))
            .text_color(rgb(if active {
                palette.primary
            } else {
                palette.text_muted
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.start_workspace.cycle_sort(column.sort_key());
                this.persist_ui_layout();
                cx.notify();
            }))
            .child(format!("{}{indicator}", label.into()))
            .child(
                div()
                    .absolute()
                    .right(px(-3.))
                    .top(px(4.))
                    .bottom(px(4.))
                    .w(px(7.))
                    .cursor_col_resize()
                    .hover(|this| this.bg(rgb(palette.primary)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.start_workspace
                                .begin_column_resize(column, f32::from(event.position.x));
                        }),
                    ),
            )
            .into_any_element()
    }

    fn asset_table_row(
        &mut self,
        index: usize,
        record: AssetRecord,
        labels: &AssetDisplayLabels,
        widths: &[f32; 7],
        min_table_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let fills = asset_row_fills(palette);
        let connection = record.connection.clone();
        let asset = connection.asset.as_ref();
        let icon_def = resolve_connection_icon(connection.icon.as_deref(), connection.kind_label());
        let custom_icon = connection
            .icon
            .as_ref()
            .and_then(|id| self.connection_state.custom_icons.images.get(id))
            .cloned();
        let values = [
            format_asset_address(&connection, labels),
            format_last_used_ms(connection.last_used_at_ms),
            format_cpu_summary(asset, labels),
            format_bytes(asset.and_then(|value| value.memory_bytes), labels),
            format_disk_summary(asset.and_then(|value| value.disks.as_deref()), labels),
            format_accelerators(
                asset.and_then(|value| value.accelerators.as_deref()),
                labels,
                Some(2),
            ),
        ];
        let mut row = self
            .asset_item(&connection.id, "row", cx)
            .h(px(ASSET_TABLE_ROW_HEIGHT))
            .w_full()
            .min_w(px(min_table_width))
            .flex_none()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(
                if self.start_workspace.is_asset_selected(&connection.id) {
                    asset_selected_fill(palette)
                } else if self.start_workspace.is_asset_hovered(&connection.id) {
                    fills.hover
                } else if index % 2 == 1 {
                    fills.stripe
                } else {
                    palette.bg
                },
            ))
            .child(self.asset_select_checkbox(&connection.id, "row", cx))
            .child(
                div()
                    .w(px(widths[0]))
                    .flex_none()
                    .min_w_0()
                    .overflow_hidden()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(if let Some(image) = custom_icon {
                        gpui::img(image).size(px(18.)).into_any_element()
                    } else {
                        connection_type_icon(palette, icon_def, false, 18.).into_any_element()
                    })
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(11.))
                                    .text_color(rgb(palette.text))
                                    .child(connection.name.clone()),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(10.))
                                    .text_color(rgb(palette.text_dimmed))
                                    .child(record.group_path.clone()),
                            ),
                    ),
            );
        for (index, value) in values.into_iter().enumerate() {
            row = row.child(
                div()
                    .w(px(widths[index + 1]))
                    .flex_none()
                    .px_3()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text))
                    .child(value),
            );
        }
        row.into_any_element()
    }

    fn asset_row_actions(&self, connection: SavedConnection, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.theme_palette();
        let edit = connection.clone();
        let selector = format!("asset-card-actions-{}", connection.id);
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .w(px(ASSET_TABLE_ACTIONS_WIDTH))
            .flex_none()
            .pl_1()
            .pr_4()
            .flex()
            .items_center()
            .justify_end()
            .gap_1()
            .child(asset_action_button(
                format!("asset-connect-{}", connection.id),
                "icons/conn/connect.svg",
                t!("savedConnections.connect"),
                palette.border,
                cx.listener(move |this, _, window, cx| {
                    this.start_saved_connections_by_ids(
                        std::slice::from_ref(&connection.id),
                        window,
                        cx,
                    );
                }),
            ))
            .child(asset_action_button(
                format!("asset-edit-{}", edit.id),
                "icons/edit.svg",
                t!("savedConnections.edit"),
                palette.border,
                cx.listener(move |this, _, window, cx| {
                    this.edit_saved_connection_by_id(&edit.id, window, cx);
                }),
            ))
            .into_any_element()
    }

    fn asset_cards(
        &mut self,
        records: Arc<[AssetRecord]>,
        labels: AssetDisplayLabels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let columns = self.start_workspace.card_columns().max(1);
        let row_count = records.len().div_ceil(columns);
        let scroll = self.start_workspace.card_scroll().clone();
        let tracked_app = cx.weak_entity();
        let width_tracker = canvas(
            move |bounds, _window, cx| {
                let next_columns = responsive_asset_card_columns(f32::from(bounds.size.width));
                let Some(app) = tracked_app.upgrade() else {
                    return;
                };
                if app.read(cx).start_workspace.card_columns() == next_columns {
                    return;
                }
                cx.defer(move |cx| {
                    app.update(cx, |this, cx| {
                        if this.start_workspace.set_card_columns(next_columns) {
                            cx.notify();
                        }
                    });
                });
            },
            |_bounds, _state, _window, _cx| {},
        )
        .absolute()
        .inset_0()
        .size_full();
        let row_records = records.clone();
        let list = uniform_list(
            "asset-card-rows",
            row_count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|row| {
                        let start = row * columns;
                        let end = (start + columns).min(row_records.len());
                        this.asset_card_row(&row_records[start..end], &labels, columns, cx)
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .w_full()
        .flex_1()
        .min_h_0()
        .track_scroll(&scroll);
        self.asset_context_menu(
            div()
                .id("asset-cards")
                .debug_selector(|| "asset-cards".to_string())
                .relative()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .p_3()
                .child(width_tracker)
                .child(list)
                .vertical_scrollbar(&scroll),
            cx,
        )
    }

    fn asset_card_row(
        &mut self,
        records: &[AssetRecord],
        labels: &AssetDisplayLabels,
        columns: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut row = div()
            .h(px(ASSET_CARD_ROW_HEIGHT))
            .w_full()
            .flex_none()
            .grid()
            .grid_cols(columns as u16)
            .gap_2()
            .pb_2();
        for record in records {
            row = row.child(self.asset_card(record.clone(), labels, cx));
        }
        for _ in records.len()..columns {
            row = row.child(div().w_full().min_w_0());
        }
        row.into_any_element()
    }

    fn asset_card(
        &mut self,
        record: AssetRecord,
        labels: &AssetDisplayLabels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let connection = record.connection.clone();
        let footer_selector = format!("asset-card-footer-{}", connection.id);
        let asset = connection.asset.as_ref();
        let icon_def = resolve_connection_icon(connection.icon.as_deref(), connection.kind_label());
        let custom_icon = connection
            .icon
            .as_ref()
            .and_then(|id| self.connection_state.custom_icons.images.get(id))
            .cloned();
        self.asset_item(&connection.id, "card", cx)
            .w_full()
            .min_w_0()
            .h(px(190.))
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(
                if self.start_workspace.is_asset_selected(&connection.id) {
                    asset_selected_fill(palette)
                } else if self.start_workspace.is_asset_hovered(&connection.id) {
                    asset_row_fills(palette).hover
                } else {
                    palette.surface
                },
            ))
            .flex()
            .flex_col()
            .child(
                div().px_3().py_2().min_w_0().child(
                    div()
                        .min_w_0()
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(self.asset_select_checkbox(&connection.id, "card", cx))
                        .child(
                            div()
                                .size(px(30.))
                                .flex_none()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(palette.border))
                                .bg(self.shell_surface_color(palette.hover))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(if let Some(image) = custom_icon {
                                    gpui::img(image).size(px(18.)).into_any_element()
                                } else {
                                    connection_type_icon(palette, icon_def, false, 18.)
                                        .into_any_element()
                                }),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .overflow_hidden()
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight(600.))
                                        .whitespace_nowrap()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(connection.name.clone()),
                                )
                                .child(
                                    div()
                                        .mt_1()
                                        .text_size(px(10.))
                                        .text_color(rgb(palette.text_muted))
                                        .child(format_asset_address(&connection, labels)),
                                )
                                .child(
                                    div()
                                        .mt_1()
                                        .text_size(px(10.))
                                        .text_color(rgb(palette.text_dimmed))
                                        .child(record.group_path),
                                ),
                        ),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px_3()
                    .grid()
                    .grid_cols(2)
                    .gap_2()
                    .text_size(px(10.))
                    .text_color(rgb(palette.text_muted))
                    .child(format!(
                        "{}  {}",
                        t!("assets.cpu"),
                        format_cpu_summary(asset, labels)
                    ))
                    .child(format!(
                        "{}  {}",
                        t!("assets.memory"),
                        format_bytes(asset.and_then(|value| value.memory_bytes), labels)
                    ))
                    .child(format!(
                        "{}  {}",
                        t!("assets.storage"),
                        format_disk_summary(asset.and_then(|value| value.disks.as_deref()), labels)
                    ))
                    .child(format!(
                        "{}  {}",
                        t!("assets.accelerators"),
                        format_accelerators(
                            asset.and_then(|value| value.accelerators.as_deref()),
                            labels,
                            Some(1)
                        )
                    )),
            )
            .child(
                div()
                    .id(SharedString::from(footer_selector.clone()))
                    .debug_selector(move || footer_selector.clone())
                    .h(px(38.))
                    .w_full()
                    .min_w_0()
                    .flex_none()
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .flex()
                    .items_center()
                    .justify_end()
                    .child(self.asset_row_actions(connection, cx)),
            )
            .into_any_element()
    }

    pub(in crate::features) fn update_asset_column_resize(
        &mut self,
        pointer_x: f32,
        cx: &mut Context<Self>,
    ) {
        if self.start_workspace.update_column_resize(pointer_x) {
            cx.notify();
        }
    }

    pub(in crate::features) fn finish_asset_column_resize(&mut self, cx: &mut Context<Self>) {
        if self.start_workspace.finish_column_resize() {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests;
