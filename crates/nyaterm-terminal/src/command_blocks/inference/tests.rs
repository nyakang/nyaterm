use crate::command_blocks::CommandBoundarySource;
use crate::command_navigation::{CommandSubmissionEvidence, command_submission_evidence};
use crate::{TerminalLineId, TerminalScreen};
use nyaterm_core::terminal::input_tracker::TerminalInputState;

fn submission(screen: &TerminalScreen) -> CommandSubmissionEvidence {
    command_submission_evidence(&screen.snapshot(), &TerminalInputState::new(), b"\r").unwrap()
}

fn submitted() -> TerminalScreen {
    let mut screen = TerminalScreen::new(80, 12);
    screen.advance(b"user@host:~$ echo hi");
    assert!(screen.note_command_submission(submission(&screen)));
    screen
}

fn submission_with(screen: &TerminalScreen, command: &str) -> CommandSubmissionEvidence {
    CommandSubmissionEvidence {
        anchor: screen.snapshot().row(0).unwrap().line_id.unwrap(),
        command: command.into(),
        prompt: Some("$".into()),
    }
}

#[test]
fn confirmed_echo_and_return_prompt_make_a_foldable_region_without_shell_authorization() {
    let mut screen = submitted();
    screen.advance(b"\r\none\r\ntwo\r\nuser@host:/tmp$ ");
    let snapshot = screen.snapshot();
    let block = &snapshot.command_blocks[0];
    assert_eq!(block.source, CommandBoundarySource::ConfirmedPrompt);
    assert_eq!(block.rows, 0..3);
    assert_eq!(block.output, 1..3);
    assert_eq!(block.output_lines, 2);
    assert!(block.can_collapse(3));
    assert!(snapshot.row(0).unwrap().inferred_input);
    assert!(
        snapshot
            .rows()
            .iter()
            .all(|row| row.shell_input.is_none() && row.command_mark.is_none())
    );
    assert!(snapshot.shell_input_anchor.is_none());
    assert!(!screen.shell_integration_enabled());
    assert!(!screen.take_effects().shell_command_finished);
    let id = block.id;
    screen.set_collapsed_commands([id].into());
    let snapshot = screen.snapshot();
    assert!(snapshot.display_rows.as_ref().unwrap().hidden(1));
    assert_eq!(snapshot.row(1).unwrap().text, "one");
}

#[test]
fn split_echo_ansi_wide_characters_and_soft_wraps_have_identical_boundaries() {
    let data = "echo \x1b[32m中文\x1b[0m\r\nout\r\n$ ".as_bytes();
    let mut expected = None;
    for size in [1, 2, 5, data.len()] {
        let mut screen = TerminalScreen::new(8, 12);
        screen.advance(b"$ ");
        screen.note_command_submission(submission_with(&screen, "echo 中文"));
        for chunk in data.chunks(size) {
            screen.advance(chunk);
        }
        let snapshot = screen.snapshot();
        let block = &snapshot.command_blocks[0];
        assert!(block.complete);
        assert_eq!(block.output_lines, 1);
        let shape = (block.rows.clone(), block.output.clone());
        assert_eq!(shape.1, 2..3);
        if let Some(expected) = &expected {
            assert_eq!(&shape, expected);
        }
        expected = Some(shape);
        assert!(snapshot.row(0).unwrap().inferred_input);
        assert!(snapshot.row(1).unwrap().inferred_input);
    }
}

#[test]
fn missing_echo_other_identity_and_output_prompt_examples_do_not_finish_a_command() {
    for tail in [
        "\r\nout\r\nroot@host:~$ ",
        "\r\nout\r\nuser@other:~$ ",
        "\r\n$ example\r\n# example\r\nstill running",
        "\r\nout\r\nuser@host:~$ example",
    ] {
        let mut screen = submitted();
        screen.advance(tail.as_bytes());
        assert!(!screen.snapshot().command_blocks[0].complete);
    }
    let mut screen = TerminalScreen::new(80, 12);
    screen.advance(b"$ ");
    screen.note_command_submission(submission_with(&screen, "echo hi"));
    screen.advance(b"\r\nout\r\n$ ");
    assert!(!screen.snapshot().command_blocks[0].can_collapse(2));
    assert!(!screen.snapshot().row(0).unwrap().inferred_input);
}

#[test]
fn prompt_like_output_is_revoked_and_the_next_submission_freezes_the_real_boundary() {
    let mut screen = submitted();
    screen.advance(b"\r\nout\r\nuser@host:~$ ");
    let id = screen.snapshot().command_blocks[0].id;
    screen.set_collapsed_commands([id].into());
    screen.advance(b"\r\nmore output");
    assert!(!screen.snapshot().command_blocks[0].complete);
    assert!(screen.snapshot().display_rows.is_none());
    screen.advance(b"\r\nuser@host:~$ second");
    screen.note_command_submission(submission(&screen));
    let snapshot = screen.snapshot();
    assert_eq!(snapshot.command_blocks[0].id, id);
    assert!(snapshot.command_blocks[0].complete);
    assert_eq!(snapshot.command_blocks[0].output, 1..4);
    screen.advance(b"\r\nsecond output\r\nuser@host:~$ ");
    assert_eq!(screen.snapshot().command_blocks.len(), 2);
    assert!(
        screen
            .snapshot()
            .command_blocks
            .iter()
            .all(|block| block.complete)
    );
}

#[test]
fn zero_output_and_interactive_submissions_are_not_foldable() {
    let mut screen = submitted();
    screen.advance(b"\r\nuser@host:~$ ");
    assert!(screen.snapshot().command_blocks[0].output.is_empty());
    for command in ["cat", "python", "redis-cli", "vim"] {
        let mut screen = TerminalScreen::new(80, 12);
        screen.advance(format!("$ {command}").as_bytes());
        screen.note_command_submission(submission(&screen));
        screen.advance(b"\r\n$ \r\nout\r\n$ ");
        assert!(
            !screen.snapshot().command_blocks[0].can_collapse(3),
            "{command}"
        );
    }
}

#[test]
fn late_osc_marks_keep_identity_and_authoritative_boundaries() {
    let mut screen = submitted();
    let id = screen.snapshot().command_blocks[0].id;
    screen.advance(
        b"\r\n\x1b]133;C\x07out\r\n\x1b]133;D;0\x07\x1b]133;A\x07user@host:~$ \x1b]133;B\x07",
    );
    let snapshot = screen.snapshot();
    assert_eq!(snapshot.command_blocks[0].id, id);
    assert_eq!(snapshot.command_blocks[0].output, 1..2);
    assert!(snapshot.command_blocks[0].complete);
    assert_eq!(
        snapshot.command_blocks[0].source,
        CommandBoundarySource::ConfirmedPrompt
    );
}

#[test]
fn inference_resumes_in_a_session_that_previously_emitted_osc() {
    let mut screen = TerminalScreen::new(80, 12);
    screen.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07su\r\n\x1b]133;C\x07root@host:~# echo hi");
    screen.note_command_submission(submission(&screen));
    screen.advance(b"\r\nhi\r\nroot@host:~# ");
    let snapshot = screen.snapshot();
    let block = snapshot.command_blocks.last().unwrap();
    assert!(block.can_collapse(snapshot.cursor.row));
    assert_eq!(block.output_lines, 1);
}

#[test]
fn background_cache_and_reflow_preserve_confirmed_input_and_block_identity() {
    let mut screen = TerminalScreen::new(80, 12);
    screen.advance(b"user@host:~$ echo hi");
    let before = screen.snapshot();
    screen.note_command_submission(submission(&screen));
    let after = screen.snapshot();
    assert!(!before.row(0).unwrap().inferred_input);
    assert!(after.row(0).unwrap().inferred_input);
    assert!(!std::sync::Arc::ptr_eq(&before.rows()[0], &after.rows()[0]));
    screen.advance(b"\r\nhi\r\nuser@host:~$ ");
    let id = screen.snapshot().command_blocks[0].id;
    screen.resize(10, 12);
    let snapshot = screen.snapshot();
    assert_eq!(snapshot.command_blocks[0].id, id);
    assert_eq!(snapshot.command_blocks[0].output_lines, 1);
    assert!(snapshot.command_blocks[0].complete);
    assert!(
        screen
            .viewport_snapshot(screen.scrollback_len())
            .row(0)
            .unwrap()
            .inferred_input
    );
    screen.advance(b"\r\nlate output");
    assert!(!screen.snapshot().command_blocks[0].complete);
}

#[test]
fn reset_clear_alternate_screen_and_eviction_drop_pending_inference() {
    for action in ["clear", "reset", "alternate", "mouse", "region", "stream"] {
        let mut screen = submitted();
        match action {
            "clear" => screen.clear_except_input(),
            "reset" => screen.advance(b"\x1bc"),
            "alternate" => screen.advance(b"\x1b[?1049h\x1b[?1049l"),
            "mouse" => screen.advance(b"\x1b[?1000h"),
            "region" => screen.advance(b"\x1b[2;5r"),
            "stream" => screen.reset_stream_state(),
            _ => unreachable!(),
        }
        screen.advance(b"\r\nout\r\nuser@host:~$ ");
        assert!(screen.primary_lines.inference.is_none(), "{action}");
    }
    let mut screen = submitted();
    screen.set_scrollback_limit(0);
    screen.advance("\r\n".repeat(30).as_bytes());
    assert!(screen.primary_lines.inference.is_none());

    let mut screen = submitted();
    screen.advance("\r\nout".repeat(20).as_bytes());
    assert!(screen.primary_lines.inference.is_some());
    screen.set_scrollback_limit(0);
    assert!(screen.primary_lines.inference.is_none());
}

#[test]
fn inference_cost_is_independent_of_history_and_stale_epochs_are_rejected() {
    let mut costs = Vec::new();
    for history in [0, 1000, 10000] {
        let mut screen = TerminalScreen::new(80, 12);
        screen.advance("history\r\n".repeat(history).as_bytes());
        screen.advance(b"user@host:~$ echo hi");
        screen.note_command_submission(submission(&screen));
        screen.inference_cells_visited = 0;
        screen.advance(b"\r\nhi\r\nuser@host:~$ ");
        costs.push(screen.inference_cells_visited);
        assert_eq!(screen.snapshot().command_blocks[0].output_lines, 1);
        let mut stale = submission_with(&screen, "echo stale");
        stale.anchor = TerminalLineId {
            epoch: u64::MAX,
            logical_line: 0,
        };
        assert!(!screen.note_command_submission(stale));
    }
    assert!(costs.iter().all(|cost| *cost == costs[0] && *cost <= 8192));
}

#[test]
fn coalesced_late_output_invalidates_a_fold_even_when_the_final_prompt_matches() {
    let mut screen = submitted();
    screen.advance(b"\r\nout\r\nuser@host:~$ ");
    let before = screen.snapshot().command_blocks[0].clone();
    screen.set_collapsed_commands([before.id].into());
    screen.advance(b"\r\nlate output\r\nuser@host:~$ ");
    let after = screen.snapshot().command_blocks[0].clone();
    assert_eq!(after.id, before.id);
    assert!(after.complete);
    assert_ne!(after.collapse_revision, before.collapse_revision);
    assert!(screen.snapshot().display_rows.is_none());
    assert_eq!(after.output_lines, 3);
}

#[test]
fn typed_input_may_extend_a_prompt_but_program_output_cannot() {
    for local_input in [false, true] {
        let mut screen = submitted();
        screen.advance(b"\r\nout\r\nuser@host:~$ ");
        if local_input {
            screen.note_command_input_activity();
        }
        screen.advance(b"more text");
        assert_eq!(screen.snapshot().command_blocks[0].complete, local_input);
    }
}

#[test]
fn erased_input_revokes_its_background_and_provisional_fold() {
    let mut screen = submitted();
    screen.advance(b"\r\nout\r\nuser@host:~$ ");
    screen.advance(b"\x1b[1;1H\x1b[2K");
    assert!(!screen.snapshot().row(0).unwrap().inferred_input);
    assert!(!screen.snapshot().command_blocks[0].complete);
    assert!(screen.primary_lines.inference.is_none());
}

#[test]
fn control_multiline_and_oversized_submissions_never_create_output_boundaries() {
    for command in [
        "echo hi\nnext".to_owned(),
        "echo\x1btest".to_owned(),
        "x".repeat(4097),
    ] {
        let mut screen = TerminalScreen::new(80, 12);
        screen.advance(b"$ ");
        screen.note_command_submission(submission_with(&screen, &command));
        screen.advance(b"\r\nout\r\n$ ");
        assert!(!screen.snapshot().command_blocks[0].can_collapse(2));
        assert!(screen.primary_lines.inference.is_none());
    }
}

#[test]
fn shell_continuation_input_is_never_hidden_as_command_output() {
    for command in ["echo 'open", "echo \\", "cat <<EOF", "for x in a b"] {
        let mut screen = TerminalScreen::new(80, 12);
        screen.advance(format!("$ {command}").as_bytes());
        screen.note_command_submission(submission(&screen));
        screen.advance(b"\r\n> additional input\r\nresult\r\n$ ");
        assert!(!screen.snapshot().command_blocks[0].can_collapse(3));
        assert!(screen.primary_lines.inference.is_none());
        screen.advance(b"echo next");
        screen.note_command_submission(submission(&screen));
        screen.advance(b"\r\nnext\r\n$ ");
        assert!(!screen.snapshot().command_blocks[0].can_collapse(5));
        assert!(screen.snapshot().command_blocks[1].can_collapse(5));
    }
}

#[test]
fn cancelled_evidence_cannot_be_closed_by_the_next_navigation_anchor() {
    for action in ["stream", "alternate", "mouse", "region", "erased"] {
        let mut screen = submitted();
        screen.advance(b"\r\nout\r\nuser@host:~$ ");
        match action {
            "stream" => screen.reset_stream_state(),
            "alternate" => screen.advance(b"\x1b[?1049h\x1b[?1049l"),
            "mouse" => screen.advance(b"\x1b[?1000h"),
            "region" => screen.advance(b"\x1b[2;5r"),
            "erased" => screen.advance(b"\x1b[1;1H\x1b[2K"),
            _ => unreachable!(),
        }
        screen.advance(b"\x1b[?1000l\x1b[r\x1b[4;1Huser@host:~$ echo next");
        screen.note_command_submission(submission(&screen));
        screen.advance(b"\r\nnext\r\nuser@host:~$ ");
        assert!(
            screen.snapshot().command_blocks[0].output.is_empty(),
            "{action}"
        );
        assert!(
            !screen.snapshot().command_blocks[0].can_collapse(5),
            "{action}"
        );
    }
}

#[test]
fn a_new_shell_identity_cannot_confirm_the_previous_shell_output() {
    let mut screen = submitted();
    screen.advance(b"\r\nout\r\nroot@host:~# echo next");
    screen.note_command_submission(submission(&screen));
    screen.advance(b"\r\nnext\r\nroot@host:~# ");
    let snapshot = screen.snapshot();
    assert!(!snapshot.command_blocks[0].can_collapse(4));
    assert!(snapshot.command_blocks[1].can_collapse(4));
}

#[test]
fn osc_input_without_an_output_marker_uses_the_weaker_echo_evidence() {
    let mut screen = TerminalScreen::new(80, 12);
    screen.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07echo hi");
    let id = screen.snapshot().command_blocks[0].id;
    screen.note_command_submission(submission(&screen));
    screen.advance(b"\r\nhi\r\n\x1b]133;D;0\x07$ ");
    let snapshot = screen.snapshot();
    assert_eq!(snapshot.command_blocks[0].id, id);
    assert_eq!(snapshot.command_blocks[0].output, 1..2);
    assert_eq!(
        snapshot.command_blocks[0].source,
        CommandBoundarySource::ConfirmedPrompt
    );
    assert!(snapshot.command_blocks[0].can_collapse(2));
}

#[test]
fn unchanged_mismatching_wrapped_echo_is_cached_and_a_tail_rewrite_rechecks_it() {
    let mut screen = TerminalScreen::new(20, 12);
    let command = format!("echo {}", "a".repeat(90));
    screen.advance(format!("$ {command}x").as_bytes());
    let tail = screen.snapshot().cursor;
    screen.note_command_submission(submission_with(&screen, &command));
    screen.advance(b"\r\nout");
    screen.inference_cells_visited = 0;
    for _ in 0..20 {
        screen.advance(b"\rout\x1b[K");
    }
    assert_eq!(screen.inference_cells_visited, 0);
    assert!(!screen.snapshot().command_blocks[0].complete);

    screen.advance(format!("\x1b[{};{}H\x1b[K", tail.row + 1, tail.col).as_bytes());
    screen.advance(format!("\x1b[{};4H\r\n$ ", tail.row + 2).as_bytes());
    assert!(screen.inference_cells_visited > 0);
    assert!(screen.snapshot().command_blocks[0].complete);
    assert_eq!(screen.snapshot().command_blocks[0].output_lines, 1);
}

#[test]
fn unchanged_output_does_not_rescan_but_rewritten_continuation_revokes_inference() {
    let mut screen = submitted();
    screen.advance(b"\r\nout");
    screen.inference_cells_visited = 0;
    for _ in 0..20 {
        screen.advance(b"\rout\x1b[K");
    }
    assert_eq!(screen.inference_cells_visited, 0);
    screen.advance(b"\r\nuser@host:~$ ");
    assert!(screen.snapshot().command_blocks[0].complete);
    screen.advance(b"\x1b[2;1H> \x1b[K\x1b[3;14H");
    assert!(screen.primary_lines.inference.is_none());
    assert!(!screen.snapshot().command_blocks[0].complete);
    assert!(screen.snapshot().command_blocks[0].output.is_empty());
}

#[test]
fn cursor_only_movement_revokes_a_cached_prompt_without_rescanning_its_text() {
    let mut screen = TerminalScreen::new(80, 12);
    screen.advance(b"$ echo hi");
    screen.note_command_submission(submission(&screen));
    screen.advance(b"\r\nhi\r\n$ ");
    let before = screen.snapshot().command_blocks[0].clone();
    screen.set_collapsed_commands([before.id].into());
    screen.inference_cells_visited = 0;
    screen.advance(b"\x1b[1G");
    let after = screen.snapshot().command_blocks[0].clone();
    assert!(!after.complete);
    assert_ne!(before.collapse_revision, after.collapse_revision);
    assert!(screen.snapshot().display_rows.is_none());
    // Only the first nonblank cell is inspected for the cursor-end check.
    assert_eq!(screen.inference_cells_visited, 1);
    screen.advance(b"\x1b[3G");
    assert!(screen.snapshot().command_blocks[0].complete);
    assert_eq!(
        screen.snapshot().command_blocks[0].collapse_revision,
        after.collapse_revision
    );
}

#[test]
fn width_reflow_invalidates_the_cached_echo_range_before_new_output() {
    let mut screen = TerminalScreen::new(80, 12);
    screen.advance(b"$ echo abcdefghijklmnopqrstuvwxyz");
    screen.note_command_submission(submission(&screen));
    screen.resize(10, 12);
    screen.advance(b"\r\nout\r\n$ ");
    let snapshot = screen.snapshot();
    assert!(snapshot.command_blocks[0].complete);
    assert_eq!(snapshot.command_blocks[0].output_lines, 1);
    let history = screen.viewport_snapshot(screen.scrollback_len());
    assert!(history.rows()[..4].iter().all(|row| row.inferred_input));
}

#[test]
fn wrap_only_changes_on_the_preceding_row_revoke_a_cached_return_prompt() {
    let mut screen = TerminalScreen::new(20, 12);
    screen.advance(b"$ echo hi");
    screen.note_command_submission(submission(&screen));
    screen.advance(b"\r\naaaaaaaaaaaaaaaaaaaa\r\n$ ");
    let before = screen.snapshot();
    assert!(before.command_blocks[0].complete);
    // Rewrite identical characters while changing only the preceding WRAPLINE.
    screen.advance(b"\x1b[2;20Ha$");
    let after = screen.snapshot();
    assert_eq!(before.row(1).unwrap().text, after.row(1).unwrap().text);
    assert_eq!(before.row(2).unwrap().text, after.row(2).unwrap().text);
    assert!(!after.command_blocks[0].complete);
}

fn note_submission(screen: &mut TerminalScreen) -> bool {
    let evidence =
        command_submission_evidence(&screen.snapshot(), &TerminalInputState::new(), b"\r")
            .expect("prompt must be recognised");
    screen.note_command_submission(evidence)
}

#[test]
fn commands_in_a_su_root_shell_without_integration_stay_foldable_for_unix_prompt_styles() {
    for (user_prompt, root_prompt) in [
        ("user@host:~$ ", "root@host:~# "),
        (
            "Jakubs-MacBook-Pro:~ jakub$ ",
            "Jakubs-MacBook-Pro:~ root# ",
        ),
        ("bash-3.2$ ", "bash-3.2# "),
        ("[jakub@rocky ~]$ ", "[root@rocky ~]# "),
        ("[jakub@rocky ~]$ ", "[root@rocky /var/log]# "),
    ] {
        let mut screen = TerminalScreen::new(100, 40);
        // The original shell has integration; `sudo su -` starts a root shell without it.
        screen.advance(
            format!(
                "\x1b]133;A\x07{user_prompt}\x1b]133;B\x07ls\r\n\x1b]133;C\x07a\r\nb\r\nc\r\n\x1b]133;D;0\x07\x1b]133;A\x07{user_prompt}\x1b]133;B\x07sudo su -\r\n\x1b]133;C\x07Password:\r\n{root_prompt}ls"
            )
            .as_bytes(),
        );
        assert!(note_submission(&mut screen), "{root_prompt}");
        screen.advance(format!("\r\nx\r\ny\r\nz\r\n{root_prompt}ls -la").as_bytes());
        assert!(note_submission(&mut screen), "{root_prompt}");
        screen.advance(format!("\r\nq\r\nr\r\ns\r\n{root_prompt}").as_bytes());
        let snapshot = screen.snapshot();
        let inferred: Vec<_> = snapshot
            .command_blocks
            .iter()
            .filter(|block| {
                block.source == CommandBoundarySource::ConfirmedPrompt && block.output_lines > 0
            })
            .collect();
        assert_eq!(inferred.len(), 2, "{root_prompt}");
        assert!(
            inferred
                .iter()
                .all(|block| block.complete && block.output_lines == 3 && block.can_collapse(3)),
            "{root_prompt}"
        );
    }
}
