use std::sync::Arc;

use gpui::{
    AnyElement, Context, FontWeight, IntoElement, ListHorizontalSizingBehavior, MouseButton,
    MouseDownEvent, SharedString, canvas, div, prelude::*, px, rgb, uniform_list,
};
use nyaterm_core::{
    AssetDisplayLabels, AssetFilterKey, AssetRecord, AssetSortDirection, AssetViewMode,
    SavedConnection, StartWorkspaceMode, build_group_path, format_accelerators,
    format_asset_address, format_bytes, format_cpu_summary, format_disk_summary,
};
use nyaterm_ui::{NyaScrollable, NyaSearchInput, NyaSelect};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::assets::{ASSET_CARD_ROW_HEIGHT, ASSET_TABLE_ROW_HEIGHT, AssetColumn};
use crate::features::formatting::format_last_used_ms;
use crate::features::icons::resolve_connection_icon;
use crate::features::view_widgets::connection_type_icon;

const ASSET_CARD_MIN_WIDTH: f32 = 300.;
const ASSET_CARD_GAP: f32 = 8.;
const ASSET_CARD_HORIZONTAL_PADDING: f32 = 24.;

fn responsive_asset_card_columns(viewport_width: f32) -> usize {
    if !viewport_width.is_finite() || viewport_width <= 0. {
        return 1;
    }
    let available = (viewport_width - ASSET_CARD_HORIZONTAL_PADDING).max(0.);
    (((available + ASSET_CARD_GAP) / (ASSET_CARD_MIN_WIDTH + ASSET_CARD_GAP)).floor() as usize)
        .clamp(1, 3)
}

impl NyaTermApp {
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
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(self.shell_surface_color(self.terminal_theme_palette().terminal_bg))
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
        let groups = self.connection_state.groups().to_vec();
        self.start_workspace.sync_group_options(&groups, cx);
        let labels = AssetDisplayLabels {
            none: t!("assets.none").to_string(),
            not_applicable: t!("assets.notApplicable").to_string(),
            local_machine: t!("assets.localMachine").to_string(),
        };
        let records: Arc<[AssetRecord]> = self
            .start_workspace
            .records(
                self.connection_state.connections(),
                self.connection_state.groups(),
                &labels,
                &t!("assets.title"),
            )
            .into();
        let search = self.start_workspace.search_field();
        let group_select = self.start_workspace.group_select();
        let list_mode = self.start_workspace.view_mode() == AssetViewMode::List;
        let count = records.len();

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
                    .h(px(38.))
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
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_muted))
                            .child(t!("assets.items", count = count)),
                    ),
            )
            .child(body)
            .into_any_element()
    }

    fn asset_filter_buttons(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div().flex().items_center().gap_1();
        let all_active = self.start_workspace.filters().is_empty();
        row = row.child(self.asset_filter_button(None, t!("assets.all"), all_active, cx));
        for (filter, label) in [
            (AssetFilterKey::Linux, t!("assets.linux")),
            (AssetFilterKey::Windows, t!("assets.windows")),
            (AssetFilterKey::Gpu, t!("assets.gpu")),
            (AssetFilterKey::Npu, t!("assets.npu")),
        ] {
            row = row.child(self.asset_filter_button(
                Some(filter),
                label,
                self.start_workspace.filters().contains(&filter),
                cx,
            ));
        }
        row.into_any_element()
    }

    fn asset_filter_button(
        &self,
        filter: Option<AssetFilterKey>,
        label: impl Into<SharedString>,
        active: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let id = match filter {
            Some(AssetFilterKey::Linux) => "linux",
            Some(AssetFilterKey::Windows) => "windows",
            Some(AssetFilterKey::Gpu) => "gpu",
            Some(AssetFilterKey::Npu) => "npu",
            None => "all",
        };
        div()
            .id(SharedString::from(format!("asset-filter-{id}")))
            .h(px(28.))
            .px_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(if active {
                palette.primary
            } else {
                palette.border
            }))
            .flex()
            .items_center()
            .cursor_pointer()
            .text_size(px(11.))
            .text_color(rgb(if active {
                palette.primary
            } else {
                palette.text_muted
            }))
            .hover(|this| this.bg(rgb(palette.hover)))
            .on_click(cx.listener(move |this, _, _, cx| {
                match filter {
                    Some(filter) => this.start_workspace.toggle_filter(filter),
                    None => this.start_workspace.clear_filters(),
                }
                cx.notify();
            }))
            .child(label.into())
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
        let table_width = self.start_workspace.table_width();
        let scroll = self.start_workspace.list_scroll().clone();
        let mut header = div()
            .h(px(34.))
            .w(px(table_width))
            .flex_none()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(palette.section_header));
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
                .w(px(92.))
                .px_2()
                .text_right()
                .text_size(px(11.))
                .text_color(rgb(palette.text_muted))
                .child(t!("assets.actions")),
        );

        let row_records = records.clone();
        let list = uniform_list(
            "asset-table-rows",
            records.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|index| row_records.get(index).cloned())
                    .map(|record| this.asset_table_row(record, &labels, &widths, table_width, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .flex_1()
        .min_h_0()
        .track_scroll(&scroll);

        div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_x_scrollbar()
            .child(
                div()
                    .w(px(table_width))
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
                            .child(list)
                            .vertical_scrollbar(&scroll),
                    ),
            )
            .into_any_element()
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
        record: AssetRecord,
        labels: &AssetDisplayLabels,
        widths: &[f32; 7],
        table_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
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
        let mut row = div()
            .h(px(ASSET_TABLE_ROW_HEIGHT))
            .w(px(table_width))
            .flex_none()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(palette.border))
            .hover(|this| this.bg(rgb(palette.hover)))
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
        row.child(self.asset_row_actions(connection, cx))
            .into_any_element()
    }

    fn asset_row_actions(&self, connection: SavedConnection, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.theme_palette();
        let edit = connection.clone();
        div()
            .w(px(92.))
            .flex_none()
            .px_1()
            .flex()
            .items_center()
            .justify_end()
            .gap_1()
            .child(
                div()
                    .id(SharedString::from(format!(
                        "asset-connect-{}",
                        connection.id
                    )))
                    .h(px(26.))
                    .px_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .text_size(px(10.))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.start_saved_connection(connection.clone(), window, cx);
                    }))
                    .child(t!("savedConnections.connect")),
            )
            .child(
                div()
                    .id(SharedString::from(format!("asset-edit-{}", edit.id)))
                    .h(px(26.))
                    .px_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .text_size(px(10.))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_connection_editor(Some(edit.id.clone()), None, false, window, cx);
                    }))
                    .child(t!("savedConnections.edit")),
            )
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
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .p_3()
            .child(width_tracker)
            .child(list)
            .vertical_scrollbar(&scroll)
            .into_any_element()
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
        let asset = connection.asset.as_ref();
        let icon_def = resolve_connection_icon(connection.icon.as_deref(), connection.kind_label());
        let custom_icon = connection
            .icon
            .as_ref()
            .and_then(|id| self.connection_state.custom_icons.images.get(id))
            .cloned();
        div()
            .w_full()
            .min_w_0()
            .h(px(190.))
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(palette.surface))
            .flex()
            .flex_col()
            .child(
                div().px_3().py_2().min_w_0().child(
                    div()
                        .min_w_0()
                        .flex()
                        .items_start()
                        .gap_2()
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
                    .h(px(38.))
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
mod tests {
    use super::responsive_asset_card_columns;

    #[test]
    fn asset_card_columns_follow_available_viewport_width() {
        assert_eq!(responsive_asset_card_columns(f32::NAN), 1);
        assert_eq!(responsive_asset_card_columns(631.), 1);
        assert_eq!(responsive_asset_card_columns(632.), 2);
        assert_eq!(responsive_asset_card_columns(939.), 2);
        assert_eq!(responsive_asset_card_columns(940.), 3);
        assert_eq!(responsive_asset_card_columns(1920.), 3);
    }
}
