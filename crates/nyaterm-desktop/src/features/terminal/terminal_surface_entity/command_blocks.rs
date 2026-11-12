use super::{TerminalSurface, terminal_surface_visible_rows_for_viewport};
use crate::features::NyaTermApp;
use crate::models::terminal::command_blocks::{CommandBlockAction, TerminalCommandBlockState};
use gpui::{
    AnyElement, Context, IntoElement, MouseButton, SharedString, WeakEntity, div, prelude::*, px,
    rgb,
};
use nyaterm_terminal::TerminalSnapshot;
use nyaterm_terminal_gpui::command_blocks::{COMMAND_RAIL_WIDTH, command_rail_row};

impl NyaTermApp {
    pub(in crate::features) fn terminal_command_block_action(
        &mut self,
        session_id: &str,
        action: CommandBlockAction,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.terminal.view.views.get_mut(session_id) else {
            return;
        };
        if view.protocol_state.alternate_screen || view.protocol_state.mouse_reporting {
            return;
        }
        let Some(snapshot) = view.frame_snapshot.as_ref() else {
            return;
        };
        let source_start = snapshot
            .total_rows
            .saturating_sub(snapshot.display_offset)
            .saturating_sub(snapshot.row_count());
        let cursor = source_start.saturating_add(snapshot.cursor.row);
        let old_index = nyaterm_terminal::display_rows::TerminalDisplayRows::new(
            snapshot.history.scrollback_len + snapshot.viewport_rows,
            snapshot.logical_lines.clone(),
            &snapshot.command_blocks,
            &view.command_blocks.collapsed,
            cursor,
        );
        let old_top = old_index.top(view.scroll_offset, snapshot.viewport_rows);
        let anchor = old_index.source_row(old_top);
        let changed = view
            .command_blocks
            .apply(action, &snapshot.command_blocks, cursor);
        if changed {
            let ids = view.command_blocks.collapsed.clone();
            let next_index = nyaterm_terminal::display_rows::TerminalDisplayRows::new(
                snapshot.history.scrollback_len + snapshot.viewport_rows,
                snapshot.logical_lines.clone(),
                &snapshot.command_blocks,
                &ids,
                cursor,
            );
            let new_top = next_index.display_row(anchor);
            let offset = next_index.physical_offset(
                next_index
                    .rows()
                    .saturating_sub(snapshot.viewport_rows)
                    .saturating_sub(new_top),
                snapshot.viewport_rows,
            );
            view.scroll_offset = offset;
            view.scrollback_snapshots.clear();
            view.scrollback_action_links.clear();
            view.pending_snapshot_offsets.clear();
            view.priority_pending_snapshot_offsets.clear();
            self.terminal
                .view
                .frame_pipeline
                .set_collapsed_commands(session_id.to_owned(), ids);
            self.clear_terminal_selection(cx);
            self.request_terminal_frame_snapshot_for_user_scroll(session_id, offset);
        }
        self.notify_terminal_surface_only(Some(session_id), cx);
    }
}

impl TerminalSurface {
    pub(in crate::features) fn set_command_blocks(
        &mut self,
        state: TerminalCommandBlockState,
    ) -> bool {
        if self.command_blocks == state {
            return false;
        }
        if self.command_blocks.collapsed != state.collapsed {
            self.clear_retained_scroll_state();
        }
        self.command_blocks = state;
        true
    }

    pub(super) fn command_rail(
        &self,
        snapshot: &TerminalSnapshot,
        y_offset: f32,
        height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut rail = div()
            .absolute()
            .right_0()
            .top_0()
            .w(px(COMMAND_RAIL_WIDTH))
            .h_full()
            .child(
                div()
                    .absolute()
                    .left(px(8.0))
                    .top_0()
                    .w(px(1.0))
                    .h_full()
                    .bg(rgb(self.palette.border)),
            );
        if self.protocol_state.alternate_screen || self.protocol_state.mouse_reporting {
            return rail.into_any_element();
        }
        let source_start = snapshot
            .total_rows
            .saturating_sub(snapshot.display_offset)
            .saturating_sub(snapshot.row_count());
        let count = self
            .painted_unwrapped
            .as_ref()
            .map_or(snapshot.row_count(), |layout| layout.viewport_rows);
        for display in
            terminal_surface_visible_rows_for_viewport(self.viewport_rows, count, y_offset, height)
        {
            let source = self
                .painted_unwrapped
                .as_ref()
                .map_or(display, |layout| layout.first_source_row(display));
            let absolute = source_start + source;
            let index = snapshot
                .command_blocks
                .partition_point(|block| block.rows.start <= absolute);
            let Some(block) = index
                .checked_sub(1)
                .and_then(|i| snapshot.command_blocks.get(i))
                .filter(|block| block.rows.contains(&absolute))
            else {
                continue;
            };
            let head = absolute == block.rows.start;
            let summary = self
                .painted_unwrapped
                .as_ref()
                .is_some_and(|layout| layout.summary(display) == Some(block.id));
            let next_absolute = self
                .painted_unwrapped
                .as_ref()
                .map_or(absolute + 1, |layout| {
                    source_start + layout.first_source_row(display + 1)
                });
            let tail = block.complete && (summary || next_absolute >= block.rows.end);
            let color = if self.command_blocks.selected == Some(block.id) {
                self.palette.accent
            } else {
                self.palette.text_dimmed
            };
            let collapsed = self.command_blocks.collapsed.contains(&block.id);
            let cursor = source_start.saturating_add(snapshot.cursor.row);
            let enabled = block.can_collapse(cursor);
            let id = block.id;
            let session_id = self.session_id.clone();
            let app = self.app.clone();
            rail = rail.child(
                command_rail_row(
                    height,
                    head,
                    tail,
                    collapsed,
                    color,
                    self.palette.terminal_bg,
                )
                .id(SharedString::from(format!("command-rail-{display}")))
                .absolute()
                .top(px(y_offset + display as f32 * height))
                .left_0()
                .when(!head || enabled, |element| element.cursor_pointer())
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    if let Some(app) = app.as_ref().and_then(WeakEntity::upgrade) {
                        app.update(cx, |app, cx| {
                            app.terminal_command_block_action(
                                &session_id,
                                if head && enabled {
                                    CommandBlockAction::Toggle(id)
                                } else {
                                    CommandBlockAction::Select(id)
                                },
                                cx,
                            )
                        });
                    }
                }),
            );
        }
        let _ = cx;
        rail.into_any_element()
    }

    pub(super) fn command_summaries(
        &self,
        snapshot: &TerminalSnapshot,
        height: f32,
    ) -> Vec<AnyElement> {
        let Some(layout) = self.painted_unwrapped.as_ref() else {
            return Vec::new();
        };
        let mut summaries = Vec::new();
        for row in 0..layout.viewport_rows {
            let Some(id) = layout.summary(row) else {
                continue;
            };
            let Some(block) = snapshot
                .command_blocks
                .binary_search_by_key(&id, |block| block.id)
                .ok()
                .map(|index| &snapshot.command_blocks[index])
            else {
                continue;
            };
            let app = self.app.clone();
            let session_id = self.session_id.clone();
            summaries.push(
                div()
                    .id(SharedString::from(format!("command-summary-{row}")))
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(row as f32 * height))
                    .h(px(height))
                    .line_height(px(height))
                    .text_color(rgb(self.palette.text_dimmed))
                    .cursor_pointer()
                    .child(
                        rust_i18n::t!("terminalCommandOutputCollapsed", count = block.output_lines)
                            .to_string(),
                    )
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        cx.stop_propagation();
                        if let Some(app) = app.as_ref().and_then(WeakEntity::upgrade) {
                            app.update(cx, |app, cx| {
                                app.terminal_command_block_action(
                                    &session_id,
                                    CommandBlockAction::Toggle(id),
                                    cx,
                                )
                            });
                        }
                    })
                    .into_any_element(),
            );
        }
        summaries
    }
}

#[cfg(test)]
mod tests {
    use crate::features::NyaTermApp;
    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::TerminalFrameEvent;
    use crate::test_support::{TestAppCleanup, TestConfigDir};
    use gpui::{
        AppContext, Context, Entity, IntoElement, Modifiers, MouseButton, ParentElement, Render,
        Styled, TestAppContext, Window, div, point, px,
    };
    use std::collections::VecDeque;

    struct Host {
        app: Entity<NyaTermApp>,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let canvas = self.app.update(cx, |app, cx| {
                app.terminal_canvas_for("s1".into(), cx).into_any_element()
            });
            div().size_full().flex().child(canvas)
        }
    }

    #[gpui::test]
    fn command_blocks_button_consumes_click_and_search_reveals_hidden_output(
        cx: &mut TestAppContext,
    ) {
        let dir = TestConfigDir::new("nyaterm-command-block-click");
        let _cleanup = TestAppCleanup::new(cx);
        cx.update(nyaterm_ui::init_components);
        let app = app_with_visible_local_session(cx, dir.path(), "s1");
        let id = app.update(cx, |app, _| {
            let text = "$ \x1b]133;B\x07echo hi\r\n\x1b]133;C\x07one\r\ntwo\r\n\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07";
            app.terminal.seed_session_view("s1".into(), text.into(), "UTF-8");
            let pipeline = &app.terminal.view.frame_pipeline;
            pipeline.seed_session("s1", text, "UTF-8", 1000);
            pipeline.request_snapshot("s1", 0, false, Default::default());
            pipeline.flush_for_test();
            let mut events = VecDeque::new();
            pipeline.drain_events_into(&mut events, usize::MAX);
            let snapshot = events.into_iter().filter_map(|event| match event {
                TerminalFrameEvent::Snapshot(frame) => Some(frame.snapshot), _ => None,
            }).next_back().unwrap();
            let id = snapshot.command_blocks[0].id;
            let view = app.terminal.view.views.get_mut("s1").unwrap();
            view.frame_snapshot = Some(snapshot);
            view.render_degraded = false;
            view.output_burst_bytes = 0;
            let mut settings = app.settings.summary().clone();
            settings.terminal_show_line_numbers = false;
            settings.terminal_show_timestamps = false;
            app.settings.replace_summary(settings);
            id
        });
        let (_, vcx) = cx.add_window_view({
            let app = app.clone();
            move |window, cx| {
                let host = cx.new(|_| Host { app });
                nyaterm_ui::nya_root(host, window, cx)
            }
        });
        vcx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let position = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let geometry = app
                    .terminal_hit_test_geometry_for_session(Some("s1"), cx)
                    .unwrap();
                point(
                    geometry.bounds.left() - px(11.0),
                    geometry.bounds.top() + px(geometry.cell_h * 0.5),
                )
            })
        });
        vcx.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        vcx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
        vcx.update(|_, cx| app.update(cx, |app, cx| {
            let view = &app.terminal.view.views["s1"];
            assert_eq!(view.command_blocks.selected, Some(id));
            assert!(view.command_blocks.collapsed.contains(&id));
            assert!(app.terminal.selection.selection.is_none());
            app.terminal.view.frame_pipeline.flush_for_test();
            let mut events = VecDeque::new();
            app.terminal.view.frame_pipeline.drain_events_into(&mut events, usize::MAX);
            assert!(events.iter().any(|event| matches!(event, TerminalFrameEvent::Snapshot(frame)
                if frame.snapshot.display_rows.as_ref().is_some_and(|index| index.summary(1) == Some(id)))));
            app.reveal_terminal_absolute_line(1, cx);
            assert!(!app.terminal.view.views["s1"].command_blocks.collapsed.contains(&id));
        }));
    }
}
