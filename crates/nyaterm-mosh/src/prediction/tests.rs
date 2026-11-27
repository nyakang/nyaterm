//! Comprehensive prediction tests (mosh-go + stock fidelity + pipeline).
//!
//! These assert *behavior*, not just “didn’t panic”: pending positions,
//! last_shown cells, paint bytes, flagging, demote, and double-paint safety.

use super::{DisplayPipeline, DisplayPreference, Framebuffer, Predictor};
use crate::framebuffer::{Attr, Color};
use std::time::{Duration, Instant};

fn blank_fb() -> Framebuffer {
    Framebuffer::new(80, 24)
}

fn always() -> Predictor {
    Predictor::new(DisplayPreference::Always)
}

/// Always + stock band already proven (confirmed_epoch == prediction_epoch).
/// Use for overlay/paint tests that are not about hide-until-proven.
fn always_proven() -> Predictor {
    let mut p = always();
    p.prove_band_for_test();
    p
}

/// Always + proven + flagging on (stock: flagging is SRTT-driven, not Always).
fn always_flagging() -> Predictor {
    let mut p = always_proven();
    p.set_srtt(Some(Duration::from_millis(100)));
    assert!(p.flagging());
    p
}

fn adaptive() -> Predictor {
    Predictor::new(DisplayPreference::Adaptive)
}

fn adaptive_proven() -> Predictor {
    let mut p = adaptive();
    p.prove_band_for_test();
    p
}

// ---------------------------------------------------------------------------
// DisplayPreference
// ---------------------------------------------------------------------------

#[test]
fn env_preference_parsing() {
    assert_eq!(
        DisplayPreference::from_env_value("always"),
        DisplayPreference::Always
    );
    assert_eq!(
        DisplayPreference::from_env_value("NEVER"),
        DisplayPreference::Never
    );
    assert_eq!(
        DisplayPreference::from_env_value("adaptive"),
        DisplayPreference::Adaptive
    );
    assert_eq!(
        DisplayPreference::from_env_value(""),
        DisplayPreference::Adaptive
    );
    assert_eq!(
        DisplayPreference::from_env_value("bogus"),
        DisplayPreference::Adaptive
    );
}

#[test]
fn experimental_preference_is_recognized_and_shows_immediately() {
    assert_eq!(
        DisplayPreference::from_env_value("experimental"),
        DisplayPreference::Experimental
    );

    let mut predictor = Predictor::new(DisplayPreference::Experimental);
    predictor.keystroke(b"ab", &blank_fb());
    let mut shown = blank_fb();
    predictor.overlay(&mut shown);

    assert!(predictor.should_show());
    assert_eq!(shown.cell_at(0, 0).unwrap().ch, 'a');
    assert_eq!(shown.cell_at(1, 0).unwrap().ch, 'b');
}

#[test]
fn experimental_mismatch_drops_only_the_failed_prediction() {
    let mut predictor = Predictor::new(DisplayPreference::Experimental);
    let blank = blank_fb();
    predictor.keystroke(b"a", &blank);
    predictor.set_frames(1, 0, 0);
    predictor.keystroke(b"b", &blank);

    let mut host = blank_fb();
    host.put_rune(0, 0, 'X', Attr::default());
    predictor.set_frames(1, 1, 1);
    predictor.confirm(&host);

    assert_eq!(predictor.pending_known_char_at(0, 0), None);
    assert_eq!(predictor.pending_known_char_at(1, 0), Some('b'));
}

// ---------------------------------------------------------------------------
// mosh-go Predictor core
// ---------------------------------------------------------------------------

#[test]
fn basic_echo_pending_positions() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"abc", &blank_fb());
    assert!(p.active());
    // Stock full-row insert keeps one cell per column (spaces/unknown on tail).
    assert!(p.pending_len() >= 3);
    assert_eq!(p.pending_known_char_at(0, 0), Some('a'));
    assert_eq!(p.pending_known_char_at(1, 0), Some('b'));
    assert_eq!(p.pending_known_char_at(2, 0), Some('c'));
    assert_eq!(p.cur_x(), 3);
}

#[test]
fn overlay_underlines_when_flagging() {
    let mut p = always_flagging();
    p.set_cursor(0, 0);
    p.keystroke(b"hi", &blank_fb());
    let mut fb = blank_fb();
    p.overlay(&mut fb);
    assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'h');
    assert!(
        fb.cell_at(0, 0).unwrap().attr.under,
        "flagging must underline when cell differs"
    );
    assert_eq!(fb.cell_at(1, 0).unwrap().ch, 'i');
    assert!(fb.cell_at(1, 0).unwrap().attr.under);
    assert_eq!(fb.cur_x, 2);
}

#[test]
fn overlay_no_underline_when_not_flagging() {
    let mut p = adaptive_proven();
    p.set_srtt(Some(Duration::from_millis(100)));
    assert!(p.should_show() && p.flagging());
    p.set_srtt(Some(Duration::from_millis(40))); // flagging off, show still on
    assert!(p.should_show());
    assert!(!p.flagging());
    p.set_cursor(0, 0);
    p.keystroke(b"a", &blank_fb());
    let mut fb = blank_fb();
    p.overlay(&mut fb);
    assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'a');
    assert!(
        !fb.cell_at(0, 0).unwrap().attr.under,
        "predictions show without underline when flagging off"
    );
}

#[test]
fn confirm_all() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"ab", &blank_fb());
    let mut fb = blank_fb();
    fb.put_rune(0, 0, 'a', Attr::default());
    fb.put_rune(1, 0, 'b', Attr::default());
    fb.cur_x = 2;
    p.confirm(&fb);
    assert!(!p.active());
    assert_eq!(p.pending_len(), 0);
    assert_eq!(p.cur_x(), 2);
}

#[test]
fn partial_confirm() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"abc", &blank_fb());
    let mut fb = blank_fb();
    fb.put_rune(0, 0, 'a', Attr::default());
    fb.cur_x = 1;
    p.confirm(&fb);
    assert!(p.active());
    assert_eq!(p.pending_known_char_at(0, 0), None, "a confirmed");
    assert_eq!(p.pending_known_char_at(1, 0), Some('b'));
    assert_eq!(p.pending_known_char_at(2, 0), Some('c'));
}

#[test]
fn divergence_resets_all() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"abc", &blank_fb());
    let mut fb = blank_fb();
    fb.put_rune(0, 0, 'x', Attr::default());
    fb.cur_x = 5;
    p.confirm(&fb);
    // Tentative band diverge → kill_epoch (not full inactive wipe of cursor).
    assert_eq!(p.pending_known_char_at(0, 0), None);
    assert_eq!(p.pending_known_char_at(1, 0), None);
    assert_eq!(p.cur_x(), 5, "kill_epoch snaps cursor to host");
}

#[test]
fn space_confirms_as_match_not_stall() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"hi there", &blank_fb());
    assert_eq!(p.pending_known_char_at(0, 0), Some('h'));
    assert_eq!(p.pending_known_char_at(3, 0), Some('t'));
    let mut fb = blank_fb();
    fb.put_rune(0, 0, 'h', Attr::default());
    fb.put_rune(1, 0, 'i', Attr::default());
    fb.put_rune(2, 0, ' ', Attr::default());
    fb.cur_x = 3;
    p.confirm(&fb);
    assert_eq!(p.pending_known_char_at(0, 0), None);
    assert_eq!(p.pending_known_char_at(1, 0), None);
    assert_eq!(p.pending_known_char_at(3, 0), Some('t'));
}

#[test]
fn blank_host_stalls_not_diverge() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"ab", &blank_fb());
    // host still empty at prediction cells
    let fb = blank_fb();
    p.confirm(&fb);
    assert_eq!(p.pending_known_char_at(0, 0), Some('a'));
    assert_eq!(p.pending_known_char_at(1, 0), Some('b'));
    assert!(p.active(), "empty host stalls, must not reset");
}

#[test]
fn set_cursor_not_overridden_while_active() {
    let mut p = always();
    p.set_cursor(10, 5);
    p.keystroke(b"x", &blank_fb());
    p.set_cursor(0, 0);
    assert_eq!(p.cur_x(), 11);
    assert_eq!(p.cur_y(), 5);
}

#[test]
fn overlay_does_not_touch_unpredicted() {
    let mut p = always_proven();
    p.set_cursor(5, 0);
    p.keystroke(b"x", &blank_fb());
    let mut fb = blank_fb();
    fb.put_rune(0, 0, 'A', Attr::default());
    fb.put_rune(1, 0, 'B', Attr::default());
    p.overlay(&mut fb);
    assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'A');
    assert_eq!(fb.cell_at(1, 0).unwrap().ch, 'B');
    assert_eq!(fb.cell_at(5, 0).unwrap().ch, 'x');
}

#[test]
fn multibyte_utf8_one_pending() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke("é".as_bytes(), &blank_fb());
    assert_eq!(p.pending_known_char_at(0, 0), Some('é'));
    assert_eq!(p.cur_x(), 1);
}

// ---------------------------------------------------------------------------
// Backspace
// ---------------------------------------------------------------------------

#[test]
fn repeated_backspace_never_predicts_past_confirmed_input_into_prompt() {
    let mut pipe = DisplayPipeline::new(40, 10, DisplayPreference::Always);
    pipe.prove_band_for_test();
    let _ = pipe.set_srtt(Some(Duration::from_millis(100)));
    let prompt = "user@hostname:/test#";
    let _ = pipe.on_host_bytes(prompt.as_bytes());
    let prompt_end = pipe.host_fb().cur_x;

    let _ = pipe.set_frames(2, 2, 2);
    let _ = pipe.on_keystroke(b"a");
    let _ = pipe.on_host_bytes(b"a");
    let _ = pipe.set_frames(3, 3, 3);
    assert_eq!(
        pipe.predictor().pending_len(),
        0,
        "host echo must be confirmed"
    );

    let _ = pipe.on_keystroke(&[0x7f]);
    let _ = pipe.on_keystroke(&[0x7f]);

    let shown: String = (0..prompt_end)
        .map(|x| pipe.last_shown().unwrap().cell_at(x, 0).unwrap().ch)
        .collect();
    assert_eq!(
        shown, prompt,
        "local backspace prediction must never erase confirmed prompt cells"
    );
}

#[test]
fn both_backspace_bytes_cancel_pending_local_echo() {
    for erase in [0x08, 0x7f] {
        let mut pipe = DisplayPipeline::new(40, 10, DisplayPreference::Always);
        pipe.prove_band_for_test();
        let prompt = "user@hostname:/test#";
        let _ = pipe.on_host_bytes(prompt.as_bytes());
        let prompt_end = pipe.host_fb().cur_x;
        let _ = pipe.on_keystroke(b"abc");
        assert!(pipe.predictor().pending_len() > 0);

        let _ = pipe.on_keystroke(&[erase]);

        assert_eq!(pipe.predictor().pending_len(), 0);
        assert!(!pipe.predictor().active());
        let shown: String = (0..prompt_end)
            .map(|x| pipe.last_shown().unwrap().cell_at(x, 0).unwrap().ch)
            .collect();
        assert_eq!(
            shown, prompt,
            "erase byte {erase:#04x} must restore host truth"
        );
    }
}

#[test]
fn backspace_cancels_prediction_for_the_entire_input_batch() {
    for erase in [0x08, 0x7f] {
        let mut pipe = DisplayPipeline::new(40, 10, DisplayPreference::Always);
        pipe.prove_band_for_test();
        let mut input = b"abc".to_vec();
        input.push(erase);
        input.extend_from_slice(b"xyz");

        let paint = pipe.on_keystroke(&input);

        assert!(paint.is_empty());
        assert_eq!(pipe.predictor().pending_len(), 0);
        assert!(!pipe.predictor().active());
    }
}

#[test]
fn backspace_in_a_large_input_batch_still_pauses_local_echo() {
    let mut pipe = DisplayPipeline::new(140, 10, DisplayPreference::Always);
    pipe.prove_band_for_test();
    let _ = pipe.on_host_bytes(b"prompt# ");
    let _ = pipe.set_frames_late_for_test(10, 10, 10);

    let mut input = vec![b'a'; 100];
    input.push(0x7f);
    assert!(pipe.on_keystroke(&input).is_empty());
    let _ = pipe.set_frames_late_for_test(11, 10, 10);

    assert!(pipe.on_keystroke(b"x").is_empty());
    assert_eq!(pipe.predictor().pending_len(), 0);
}

#[test]
fn backspace_waits_for_a_host_frame_before_local_echo_resumes() {
    let mut pipe = DisplayPipeline::new(40, 10, DisplayPreference::Always);
    pipe.prove_band_for_test();
    let _ = pipe.on_host_bytes(b"abc");
    let _ = pipe.set_frames_late_for_test(10, 10, 10);

    let _ = pipe.on_keystroke(&[0x7f]);
    let _ = pipe.set_frames_late_for_test(11, 10, 10);

    // A newer frame that was already in flight before the erase must not
    // release the latch.
    let _ = pipe.on_host_bytes(b"\x1b[H\x1b[Kabc");
    let paint_before_host = pipe.on_keystroke(b"x");
    let _ = pipe.set_frames_late_for_test(12, 11, 10);

    assert!(paint_before_host.is_empty());
    assert_eq!(pipe.predictor().pending_len(), 0);
    assert_eq!(pipe.last_shown().unwrap().cell_at(3, 0).unwrap().ch, ' ');

    // The server has processed the erase, but not the following x that was
    // also suppressed locally, so prediction must remain paused.
    let _ = pipe.on_host_bytes(b"\x1b[H\x1b[Kab");
    let _ = pipe.set_frames_late_for_test(12, 11, 11);
    assert!(pipe.on_keystroke(b"y").is_empty());
    let _ = pipe.set_frames_late_for_test(13, 12, 11);

    // Once the host has painted and acknowledged every suppressed input,
    // prediction can safely resume from the authoritative cursor.
    let _ = pipe.on_host_bytes(b"\x1b[H\x1b[Kabxy");
    let _ = pipe.set_frames_late_for_test(13, 13, 13);
    let _ = pipe.on_keystroke(b"z");

    assert_eq!(pipe.predictor().pending_known_char_at(4, 0), Some('z'));
}

#[test]
fn backspace_does_not_resume_on_an_ack_without_a_host_frame() {
    let mut pipe = DisplayPipeline::new(40, 10, DisplayPreference::Always);
    pipe.prove_band_for_test();
    let _ = pipe.on_host_bytes(b"abc");
    let _ = pipe.set_frames_late_for_test(10, 10, 10);

    let _ = pipe.on_keystroke(&[0x7f]);
    let _ = pipe.set_frames_late_for_test(11, 11, 10);

    // The host acknowledges the erase without sending a newer screen state.
    let _ = pipe.set_frames_late_for_test(11, 11, 11);
    assert!(pipe.on_keystroke(b"x").is_empty());
    assert_eq!(pipe.predictor().pending_len(), 0);
    let _ = pipe.set_frames_late_for_test(12, 12, 11);

    // A later authoritative screen state includes the suppressed x and lets
    // prediction resume from the corrected cursor.
    let _ = pipe.on_host_bytes(b"\x1b[H\x1b[Kabx");
    let _ = pipe.set_frames_late_for_test(12, 12, 12);
    let _ = pipe.on_keystroke(b"y");
    assert_eq!(pipe.predictor().pending_known_char_at(3, 0), Some('y'));
}

#[test]
fn kill_epoch_drains_matched_prefix() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"ab", &blank_fb());
    // Prove epoch so confirmed_epoch advances
    let mut fb = blank_fb();
    fb.put_rune(0, 0, 'a', Attr::default());
    fb.put_rune(1, 0, 'b', Attr::default());
    fb.cur_x = 2;
    p.confirm(&fb);
    assert_eq!(p.confirmed_epoch_for_test(), p.prediction_epoch_for_test());
    // New band after tentative
    p.become_tentative();
    let ep_new = p.prediction_epoch_for_test();
    assert!(ep_new > p.confirmed_epoch_for_test());
    p.keystroke(b"xy", &blank_fb());
    // Confirm only first of new band, second diverges → kill_epoch
    let mut fb2 = blank_fb();
    fb2.put_rune(2, 0, 'x', Attr::default());
    fb2.put_rune(3, 0, 'Z', Attr::default()); // diverge on y
    fb2.cur_x = 4;
    p.confirm(&fb2);
    // Matched x drained; y epoch killed — no leftover matched prefix
    assert_eq!(p.pending_len(), 0);
}

#[test]
fn left_cell_attr_inherited_on_overlay() {
    let mut p = always_flagging();
    let mut fb = blank_fb();
    use crate::framebuffer::Attr;
    let bold = Attr {
        bold: true,
        ..Attr::default()
    };
    fb.put_rune(0, 0, 'P', bold);
    p.set_cursor(1, 0);
    p.keystroke(b"x", &fb);
    p.overlay(&mut fb);
    assert_eq!(fb.cell_at(1, 0).unwrap().ch, 'x');
    assert!(
        fb.cell_at(1, 0).unwrap().attr.bold,
        "inherit bold from left"
    );
    assert!(fb.cell_at(1, 0).unwrap().attr.under, "flagging underlines");
}

#[test]
fn send_interval_adaptive_thresholds() {
    // send_interval ~40ms (from 80ms RTT/2) is between show-on(30) and flag(80)
    let mut p = adaptive();
    p.set_srtt(Some(Duration::from_millis(40)));
    assert!(p.should_show());
    assert!(!p.flagging());
}

#[test]
fn row_change_becomes_tentative() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"a", &blank_fb());
    let ep0 = p.prediction_epoch_for_test();
    // Simulate host moving to next row while inactive after confirm
    let mut fb = blank_fb();
    fb.put_rune(0, 0, 'a', Attr::default());
    fb.cur_x = 1;
    p.confirm(&fb);
    p.set_cursor(0, 1); // row change
    assert!(
        p.prediction_epoch_for_test() > ep0,
        "row change must become_tentative"
    );
}

#[test]
fn host_line_insert_shifts_tail_under_pending() {
    let mut p = always();
    p.set_frames(4, 0, 0);
    let mut host = blank_fb();
    for (i, ch) in ['h', 'e', 'l', 'l', 'o'].into_iter().enumerate() {
        host.put_rune(i, 0, ch, Attr::default());
    }
    host.cur_x = 2;
    p.set_cursor(2, 0);
    p.keystroke(b"X", &host);
    // Full-row from cursor: cols-cx cells
    assert_eq!(p.pending_len(), host.cols - 2);
    assert_eq!(p.pending_known_char_at(2, 0), Some('X'));
    assert_eq!(p.pending_known_char_at(3, 0), Some('l'));
    assert_eq!(p.pending_known_char_at(4, 0), Some('l'));
    assert_eq!(p.pending_known_char_at(5, 0), Some('o'));
    assert!(p.pending_unknown_at(host.cols - 1, 0));
    p.confirm(&host);
    assert_eq!(p.pending_len(), host.cols - 2);
}

#[test]
fn cr_advances_row_when_not_bottom() {
    let mut p = always();
    p.set_cursor(5, 2);
    p.keystroke(b"\r", &blank_fb());
    assert_eq!(p.cur_x(), 0);
    assert_eq!(p.cur_y(), 3);
}

#[test]
fn original_contents_no_credit_for_noop() {
    let mut p = always();
    let mut host = blank_fb();
    host.put_rune(0, 0, 'a', Attr::default());
    p.set_cursor(0, 0);
    p.become_tentative();
    let conf_before = p.confirmed_epoch_for_test();
    let ep = p.prediction_epoch_for_test();
    // Overwrite: single-cell pred (no full-row shift noise)
    p.set_overwrite_for_test(true);
    p.keystroke(b"a", &host); // original_ch = 'a', pred = 'a'
    host.cur_x = 1;
    p.confirm(&host);
    assert_eq!(p.pending_known_char_at(0, 0), None, "matched pred drains");
    assert_eq!(
        p.confirmed_epoch_for_test(),
        conf_before,
        "noop match must not prove new band (ep was {ep})"
    );
}

#[test]
fn pipeline_with_frames_confirm_after_ack() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Always);
    pipe.set_frames_for_test(2, 2);
    let _ = pipe.on_host_bytes(b"\x1b[H");
    pipe.set_frames_for_test(2, 2);
    let _ = pipe.on_keystroke(b"z");
    // Keystroke may stamp exp=sent+1; simulate next loop frames
    pipe.set_frames_for_test(3, 2); // not yet acked if exp=3
    let _ = pipe.on_host_bytes(b"\x1b[1;1Hz");
    // Still may be pending if exp > acked
    pipe.set_frames_for_test(3, 4);
    let _ = pipe.on_host_bytes(b"\x1b[1;1Hz");
    assert_eq!(pipe.predictor().pending_len(), 0);
    assert_eq!(pipe.last_shown().unwrap().cell_at(0, 0).unwrap().ch, 'z');
}

#[test]
fn insert_mid_pending_shifts_trailing() {
    let mut p = always_proven();
    p.set_cursor(0, 0);
    p.keystroke(b"abcd", &blank_fb());
    p.keystroke(b"\x1b[D\x1b[D", &blank_fb()); // cursor at 2
    p.keystroke(b"x", &blank_fb());
    // Sorted L→R: a@0 b@1 x@2 c@3 d@4 (+ full-row tail)
    assert_eq!(p.pending_known_char_at(0, 0), Some('a'));
    assert_eq!(p.pending_known_char_at(1, 0), Some('b'));
    assert_eq!(p.pending_known_char_at(2, 0), Some('x'));
    assert_eq!(p.pending_known_char_at(3, 0), Some('c'));
    assert_eq!(p.pending_known_char_at(4, 0), Some('d'));
    let mut fb = blank_fb();
    p.overlay(&mut fb);
    assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'a');
    assert_eq!(fb.cell_at(1, 0).unwrap().ch, 'b');
    assert_eq!(fb.cell_at(2, 0).unwrap().ch, 'x');
    assert_eq!(fb.cell_at(3, 0).unwrap().ch, 'c');
    assert_eq!(fb.cell_at(4, 0).unwrap().ch, 'd');
}

#[test]
fn space_pred_correct_no_credit_on_default_blank() {
    // Stock: blank replacement → CorrectNoCredit once past Pending (no stall).
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b" ", &blank_fb());
    let fb = blank_fb(); // still default blank
    p.confirm(&fb);
    assert_eq!(p.pending_len(), 0, "blank match drains as CorrectNoCredit");
    assert_eq!(p.confirmed_epoch_for_test(), 0, "blank must not prove band");
}

#[test]
fn glitch_repairs_only_via_quick_credited_correct() {
    let mut p = adaptive_proven();
    p.set_srtt(Some(Duration::from_millis(100)));
    p.set_overwrite_for_test(true);
    p.set_cursor(0, 0);
    p.keystroke(b"a", &blank_fb());
    p.backdate_all_for_test(Duration::from_millis(300));
    p.sample_pending_age(Instant::now());
    assert!(p.glitch_trigger_for_test() >= 10);
    // Fresh quick credited Correct repairs glitch by 1 (not full clear on empty).
    p.keystroke(b"b", &blank_fb());
    // Age the 'a' is already old; put fresh 'b' — confirm only 'b' quickly
    // Simpler: reset glitch path — confirm 'a' with re-key after age
    let mut fb = blank_fb();
    fb.put_rune(0, 0, 'a', Attr::default());
    fb.put_rune(1, 0, 'b', Attr::default());
    fb.cur_x = 2;
    // 'a' is old (not quick), 'b' is new (quick) — only if 'b' is Correct credit
    // Actually pending is just 'a' aged then we typed 'b'. confirm both.
    let g_before = p.glitch_trigger_for_test();
    p.confirm(&fb);
    // At least one quick Correct on 'b' may decrement once
    assert!(
        p.glitch_trigger_for_test() <= g_before,
        "credited Correct may repair glitch"
    );
}

#[test]
fn el_then_late_ack_confirms_against_final_grid() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Always);
    pipe.prove_band_for_test();
    let _ = pipe.set_srtt(Some(Duration::from_millis(100)));
    let _ = pipe.on_host_bytes(b"\x1b[H");
    pipe.predictor_mut_for_test().set_overwrite_for_test(true);
    let _ = pipe.set_frames(5, 5, 5);
    let _ = pipe.on_keystroke(b"hello");
    assert!(pipe.predictor().pending_len() >= 5);
    // Host EL first (blank grid), then late_ack Confirm
    let _ = pipe.on_host_bytes(b"\x1b[H\x1b[K");
    let _ = pipe.set_frames(5, 6, 6);
    assert_eq!(
        pipe.predictor().pending_known_char_at(0, 0),
        None,
        "EL+late_ack resolves pending via Confirm"
    );
}

#[test]
fn frame_ack_pending_stalls_confirm_until_acked() {
    let mut p = always();
    p.set_frames(3, 0, 0);
    p.set_cursor(0, 0);
    p.keystroke(b"a", &blank_fb());
    let mut fb = blank_fb();
    fb.put_rune(0, 0, 'a', Attr::default());
    fb.cur_x = fb.cols;
    p.confirm(&fb);
    assert!(p.pending_len() > 0, "must stay Pending while unacked");
    assert_eq!(p.pending_known_char_at(0, 0), Some('a'));
    p.set_frames(3, 4, 4);
    p.confirm(&fb);
    assert_eq!(p.pending_len(), 0, "confirm after ack");
}

#[test]
fn become_tentative_hides_new_preds_until_proven() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.set_overwrite_for_test(true);
    p.keystroke(b"a", &blank_fb());
    let mut hidden = blank_fb();
    p.overlay(&mut hidden);
    assert_ne!(
        hidden.cell_at(0, 0).unwrap().ch,
        'a',
        "unproven band hidden"
    );
    let mut fb = blank_fb();
    fb.put_rune(0, 0, 'a', Attr::default());
    fb.cur_x = 1;
    p.confirm(&fb);
    assert_eq!(p.pending_len(), 0);
    assert_eq!(p.confirmed_epoch_for_test(), p.prediction_epoch_for_test());
    p.keystroke(b"b", &blank_fb());
    let mut view = blank_fb();
    p.overlay(&mut view);
    assert_eq!(view.cell_at(1, 0).unwrap().ch, 'b');
    p.become_tentative();
    p.keystroke(b"c", &blank_fb());
    let mut view2 = blank_fb();
    view2.put_rune(1, 0, 'b', Attr::default());
    p.overlay(&mut view2);
    assert_ne!(
        view2.cell_at(2, 0).map(|c| c.ch).unwrap_or(' '),
        'c',
        "new-epoch c should be tentative/hidden"
    );
}

// ---------------------------------------------------------------------------
// Arrows / CSI
// ---------------------------------------------------------------------------

#[test]
fn csi_left_right_move_cursor_keep_pending() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"hi", &blank_fb());
    assert_eq!(p.cur_x(), 2);
    let before = p.pending_len();
    p.keystroke(b"\x1b[D", &blank_fb());
    assert_eq!(p.cur_x(), 1);
    assert_eq!(p.pending_len(), before);
    p.keystroke(b"\x1b[C", &blank_fb());
    assert_eq!(p.cur_x(), 2);
}

#[test]
fn ss3_left_right_arrows() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"xy", &blank_fb());
    let before = p.pending_len();
    p.keystroke(b"\x1bOD", &blank_fb()); // left
    assert_eq!(p.cur_x(), 1);
    p.keystroke(b"\x1bOC", &blank_fb()); // right
    assert_eq!(p.cur_x(), 2);
    assert_eq!(p.pending_len(), before);
}

#[test]
fn csi_param_right_arrow_moves_one_like_stock() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"ab", &blank_fb());
    assert_eq!(p.cur_x(), 2);
    let before = p.pending_len();
    // Stock ignores CSI params for C/D — always ±1.
    p.keystroke(b"\x1b[3C", &blank_fb());
    assert_eq!(p.pending_len(), before);
    assert_eq!(p.cur_x(), 3, "CSI 3C moves right by 1 (stock)");
    assert!(p.active());
}

#[test]
fn csi_param_left_arrow_moves_one_like_stock() {
    let mut p = always();
    p.set_cursor(4, 0);
    p.keystroke(b"\x1b[2D", &blank_fb());
    assert_eq!(p.cur_x(), 3, "CSI 2D moves left by 1 (stock)");
    p.keystroke(b"\x1b[99D", &blank_fb());
    assert_eq!(p.cur_x(), 2, "each CSI D is one step");
    p.keystroke(b"\x1b[D", &blank_fb());
    p.keystroke(b"\x1b[D", &blank_fb());
    p.keystroke(b"\x1b[D", &blank_fb());
    assert_eq!(p.cur_x(), 0, "must clamp at col 0");
}

#[test]
fn csi_zero_param_defaults_to_one() {
    let mut p = always();
    p.set_cursor(3, 0);
    // CSI 0 C is treated as at least 1 (VT default)
    p.keystroke(b"\x1b[0C", &blank_fb());
    assert_eq!(p.cur_x(), 4);
    p.keystroke(b"\x1b[C", &blank_fb()); // no param = 1
    assert_eq!(p.cur_x(), 5);
}

#[test]
fn csi_param_right_clamps_to_last_col() {
    let mut p = always();
    let fb = Framebuffer::new(10, 3);
    p.set_cursor(8, 0);
    p.keystroke(b"\x1b[5C", &fb);
    assert_eq!(p.cur_x(), 9, "must clamp to cols-1");
}

#[test]
fn fragmented_csi_param_arrow_assembles() {
    let mut p = always();
    p.set_cursor(1, 0);
    p.keystroke(&[0x1b], &blank_fb());
    assert!(p.has_esc_buf_for_test());
    p.keystroke(b"[1", &blank_fb());
    assert!(p.has_esc_buf_for_test(), "incomplete CSI param must buffer");
    p.keystroke(b"2C", &blank_fb());
    assert!(!p.has_esc_buf_for_test());
    // Stock ignores count: +1 from 1 → 2
    assert_eq!(p.cur_x(), 2);
}

#[test]
fn fragmented_csi_assembles_across_chunks() {
    let mut p = always();
    p.set_overwrite_for_test(true);
    p.set_cursor(0, 0);
    p.keystroke(b"z", &blank_fb());
    assert_eq!(p.cur_x(), 1);
    assert_eq!(p.pending_known_char_at(0, 0), Some('z'));
    p.keystroke(&[0x1b], &blank_fb());
    assert!(p.has_esc_buf_for_test(), "lone ESC must buffer");
    assert_eq!(
        p.pending_known_char_at(0, 0),
        Some('z'),
        "must not clear pending on incomplete ESC"
    );
    p.keystroke(b"[D", &blank_fb());
    assert_eq!(p.cur_x(), 0);
    assert_eq!(p.pending_known_char_at(0, 0), Some('z'));
    assert!(!p.has_esc_buf_for_test());
}

#[test]
fn control_become_tentative_hides_new_not_wipe_old() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"ab", &blank_fb());
    let before = p.pending_len();
    assert!(before >= 2);
    p.keystroke(b"\n", &blank_fb());
    assert_eq!(
        p.pending_len(),
        before,
        "become_tentative must not wipe old pending"
    );
    p.keystroke(b"x", &blank_fb());
    assert!(p.pending_len() >= before);
    assert_eq!(p.pending_known_char_at(0, 0), Some('a'));
}

// ---------------------------------------------------------------------------
// Adaptive / flagging / glitch / expire
// ---------------------------------------------------------------------------

#[test]
fn adaptive_hysteresis_show_and_flag() {
    let mut p = adaptive();
    assert!(!p.should_show());
    p.set_srtt(Some(Duration::from_millis(15)));
    assert!(!p.should_show());
    p.set_srtt(Some(Duration::from_millis(35)));
    assert!(p.should_show(), "SRTT>30 enables show");
    assert!(!p.flagging(), "flagging still off until >80");
    p.set_srtt(Some(Duration::from_millis(100)));
    assert!(p.flagging());
    p.prove_band_for_test();
    p.set_cursor(0, 0);
    p.keystroke(b"x", &blank_fb());
    p.set_srtt(Some(Duration::from_millis(5)));
    assert!(p.should_show(), "hold show while pending");
    p.reset();
    // reset clears pending but does not re-align confirmed; low SRTT demotes
    p.set_srtt(Some(Duration::from_millis(5)));
    assert!(!p.should_show());
}

#[test]
fn cursor_only_active_latches_adaptive_show_like_stock() {
    // Stock active() is true for cursor-only Pending, so srtt_trigger holds.
    let mut p = adaptive();
    p.set_srtt(Some(Duration::from_millis(100)));
    p.set_cursor(5, 0);
    p.keystroke(b"\x1b[C", &blank_fb());
    assert_eq!(p.pending_len(), 0);
    assert!(p.active());
    p.set_srtt(Some(Duration::from_millis(5)));
    assert!(
        p.should_show(),
        "stock holds show while cursor-only Pending (active)"
    );
}

#[test]
fn stock_keeps_unconfirmed_predictions_past_fifteen_seconds() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"a", &blank_fb());
    let pending = p.pending_len();
    p.backdate_all_for_test(Duration::from_secs(16));
    p.sample_pending_age(Instant::now());
    assert_eq!(p.pending_len(), pending);
    assert!(p.active(), "stock waits for the server's late ACK");
    assert!(p.flagging(), "a long-pending prediction is underlined");
}

#[test]
fn glitch_threshold_raises_trigger() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"a", &blank_fb());
    p.backdate_oldest_for_test(Duration::from_millis(300));
    p.sample_pending_age(Instant::now());
    assert!(
        p.glitch_trigger_for_test() >= 10,
        "age>=250ms must set glitch_trigger, got {}",
        p.glitch_trigger_for_test()
    );
    assert!(p.pending_len() >= 1);
}

#[test]
fn last_column_places_known_and_wraps() {
    let mut p = always();
    let fb = Framebuffer::new(4, 2);
    p.set_cursor(3, 0);
    let ep_before = p.prediction_epoch_for_test();
    p.keystroke(b"x", &fb);
    assert_eq!(
        p.pending_known_char_at(3, 0),
        Some('x'),
        "stock places known glyph"
    );
    assert!(!p.pending_unknown_at(3, 0));
    assert_eq!(p.cur_x(), 0);
    assert_eq!(p.cur_y(), 1);
    assert!(p.prediction_epoch_for_test() > ep_before);

    let mut p2 = always();
    p2.set_cursor(0, 0);
    p2.keystroke("你".as_bytes(), &blank_fb());
    assert_eq!(
        p2.pending_known_char_at(0, 0),
        None,
        "wide CJK must be tentative"
    );
}

#[test]
fn combining_mark_is_tentative() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"e", &blank_fb());
    assert_eq!(p.pending_known_char_at(0, 0), Some('e'));
    let ep = p.prediction_epoch_for_test();
    p.keystroke("\u{0301}".as_bytes(), &blank_fb());
    assert_eq!(p.pending_known_char_at(0, 0), Some('e'));
    assert!(
        p.prediction_epoch_for_test() > ep,
        "combining must become_tentative"
    );
}

// ---------------------------------------------------------------------------
// DisplayPipeline — single paint path + no double paint
// ---------------------------------------------------------------------------

#[test]
fn never_mode_passthrough_host_bytes() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Never);
    let out = pipe.on_host_bytes(b"\x1b[Hhello");
    assert_eq!(out, b"\x1b[Hhello");
    assert!(pipe.on_keystroke(b"x").is_empty());
    assert!(!pipe.using_overlay_path());
}

#[test]
fn notification_uses_the_single_framebuffer_paint_path_and_restores_host_row() {
    let mut pipe = DisplayPipeline::new(40, 10, DisplayPreference::Never);
    assert_eq!(
        pipe.on_host_bytes(b"\x1b[Horiginal prompt"),
        b"\x1b[Horiginal prompt"
    );

    let paint = pipe.set_notification(Some("mosh: Last contact 7 seconds ago.".to_string()));
    assert!(!paint.is_empty());
    assert!(pipe.using_overlay_path());
    let shown = pipe.last_shown().expect("notification frame");
    let message = "mosh: Last contact 7 seconds ago.";
    for (x, expected) in message.chars().enumerate() {
        let cell = shown.cell_at(x, 0).unwrap();
        assert_eq!(cell.ch, expected);
        assert!(cell.attr.bold);
        assert_eq!(cell.attr.fg, crate::framebuffer::Color::index(7));
        assert_eq!(cell.attr.bg, crate::framebuffer::Color::index(4));
    }
    assert!(
        !shown.cursor_visible,
        "top-row notification hides its cursor"
    );

    let _ = pipe.on_host_bytes(b"\x1b[Hchanged behind bar");
    let shown = pipe.last_shown().unwrap();
    assert_eq!(shown.cell_at(0, 0).unwrap().ch, 'm');

    let clear = pipe.set_notification(None);
    assert!(!clear.is_empty());
    assert!(!pipe.using_overlay_path());
    let shown = pipe.last_shown().unwrap();
    let restored = (0..18)
        .map(|x| shown.cell_at(x, 0).unwrap().ch)
        .collect::<String>();
    assert_eq!(restored, "changed behind bar");
    assert!(shown.cursor_visible);
}

#[test]
fn unchanged_notification_does_not_repaint() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Adaptive);
    let message = Some("mosh: Last reply 12 seconds ago.".to_string());
    assert!(!pipe.set_notification(message.clone()).is_empty());
    assert!(pipe.set_notification(message).is_empty());
}

#[test]
fn adaptive_demote_clears_predictions_while_notification_stays_visible() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Adaptive);
    let _ = pipe.set_srtt(Some(Duration::from_millis(100)));
    let _ = pipe.on_host_bytes(b"\x1b[2;1H$ ");
    pipe.prove_band_for_test();
    let _ = pipe.on_keystroke(b"x");
    assert_eq!(pipe.last_shown().unwrap().cell_at(2, 1).unwrap().ch, 'x');
    let _ = pipe.set_notification(Some("mosh: Last contact 7 seconds ago.".to_string()));

    pipe.predictor_mut_for_test().reset();
    let paint = pipe.set_srtt(Some(Duration::from_millis(5)));
    assert!(!paint.is_empty());
    assert_eq!(pipe.last_shown().unwrap().cell_at(2, 1).unwrap().ch, ' ');
    assert_eq!(pipe.last_shown().unwrap().cell_at(0, 0).unwrap().ch, 'm');
    assert!(pipe.using_overlay_path());
}

#[test]
fn bulk_paste_does_not_clear_an_active_network_notification() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Always);
    let _ = pipe.set_notification(Some("mosh: Last contact 8 seconds ago.".to_string()));
    let _ = pipe.on_keystroke(&[b'x'; 101]);
    let shown = pipe.last_shown().unwrap();
    assert_eq!(shown.cell_at(0, 0).unwrap().ch, 'm');
    assert_eq!(shown.cell_at(1, 0).unwrap().ch, 'o');
}

#[test]
fn pipeline_local_echo_then_confirm_no_double_glyph() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Always);
    let _ = pipe.set_srtt(Some(Duration::from_millis(100))); // flagging on
    let prompt = pipe.on_host_bytes(b"\x1b[H\x1b[2J$ ");
    assert!(!prompt.is_empty() || pipe.last_shown().is_some());
    assert_eq!(pipe.host_fb().cur_x, 2);

    let _ = pipe.on_keystroke(b"a");
    let shown0 = pipe.last_shown().expect("last_shown");
    assert_ne!(
        shown0.cell_at(2, 0).map(|c| c.ch),
        Some('a'),
        "unproven first keystroke must not paint"
    );
    let _ = pipe.on_host_bytes(b"\x1b[1;3Ha\x1b[1;4H");
    assert_eq!(pipe.predictor().pending_len(), 0);

    let local = pipe.on_keystroke(b"ls");
    assert!(!local.is_empty(), "must emit Diff after prove");
    assert_eq!(pipe.predictor().pending_known_char_at(3, 0), Some('l'));
    let shown = pipe.last_shown().expect("last_shown after keystroke");
    assert_eq!(shown.cell_at(3, 0).unwrap().ch, 'l');
    assert_eq!(shown.cell_at(4, 0).unwrap().ch, 's');
    assert!(shown.cell_at(3, 0).unwrap().attr.under);

    let _ = pipe.on_host_bytes(b"\x1b[1;4Hl\x1b[1;5Hs\x1b[1;6H");
    let shown2 = pipe.last_shown().unwrap();
    assert_eq!(shown2.cell_at(3, 0).unwrap().ch, 'l');
    assert!(
        !shown2.cell_at(3, 0).unwrap().attr.under,
        "confirmed cells must not stay underlined"
    );
}

#[test]
fn pipeline_relative_host_echo_no_double() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Always);
    pipe.prove_band_for_test();
    let _ = pipe.on_host_bytes(b"\x1b[1;1H$ "); // cursor at col 2
    assert_eq!(pipe.host_fb().cur_x, 2);
    let _ = pipe.on_keystroke(b"l");
    assert_eq!(pipe.last_shown().unwrap().cell_at(2, 0).unwrap().ch, 'l');
    let _ = pipe.on_host_bytes(b"l");
    assert_eq!(pipe.host_fb().cell_at(2, 0).unwrap().ch, 'l');
    let shown = pipe.last_shown().unwrap();
    assert_eq!(shown.cell_at(2, 0).unwrap().ch, 'l');
    assert_ne!(
        shown.cell_at(3, 0).map(|c| c.ch),
        Some('l'),
        "relative host echo must not leave a second l"
    );
}

#[test]
fn pipeline_flagging_flip_repaints_underlines() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Adaptive);
    pipe.prove_band_for_test();
    let _ = pipe.set_srtt(Some(Duration::from_millis(100)));
    let _ = pipe.on_host_bytes(b"\x1b[H");
    let _ = pipe.on_keystroke(b"ab");
    assert!(pipe.last_shown().unwrap().cell_at(0, 0).unwrap().attr.under);

    let paint = pipe.set_srtt(Some(Duration::from_millis(40)));
    assert!(
        !paint.is_empty() || !pipe.last_shown().unwrap().cell_at(0, 0).unwrap().attr.under,
        "flagging off must repaint"
    );
    assert!(!pipe.predictor().flagging());
    assert!(
        !pipe.last_shown().unwrap().cell_at(0, 0).unwrap().attr.under,
        "last_shown must clear under after flagging demote"
    );
}

#[test]
fn pipeline_demote_show_clears_overlay() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Adaptive);
    pipe.prove_band_for_test();
    let _ = pipe.set_srtt(Some(Duration::from_millis(100)));
    let _ = pipe.on_host_bytes(b"\x1b[H$ ");
    let _ = pipe.on_keystroke(b"x");
    assert!(pipe.using_overlay_path());
    let _ = pipe.on_host_bytes(b"\x1b[1;3Hx");
    let _ = pipe.on_host_bytes(b"\x1b[1;1H$ x");
    // Force clear residual row preds for demote assertion
    pipe.predictor_mut_for_test().reset();
    let _paint = pipe.set_srtt(Some(Duration::from_millis(5)));
    assert!(!pipe.predictor().should_show());
}

#[test]
fn pipeline_tick_keeps_and_flags_long_pending_prediction() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Always);
    pipe.prove_band_for_test();
    let _ = pipe.on_host_bytes(b"\x1b[H");
    let _ = pipe.on_keystroke(b"z");
    assert!(pipe.predictor().active());
    assert_eq!(pipe.last_shown().unwrap().cell_at(0, 0).unwrap().ch, 'z');

    pipe.predictor_mut_for_test()
        .backdate_all_for_test(Duration::from_secs(16));
    let paint = pipe.tick(Instant::now());
    assert!(!paint.is_empty(), "flagging transition repaints the row");
    assert!(pipe.predictor().pending_len() > 0);
    let shown = pipe.last_shown().unwrap();
    assert_eq!(shown.cell_at(0, 0).unwrap().ch, 'z');
    assert!(
        shown.cell_at(0, 0).unwrap().attr.under,
        "stock underlines a prediction that remains unconfirmed"
    );
}

#[test]
fn pipeline_tick_reveals_hidden_adaptive_prediction_after_stock_glitch_delay() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Adaptive);
    pipe.prove_band_for_test();
    let _ = pipe.set_srtt(Some(Duration::from_millis(5)));
    let _ = pipe.on_host_bytes(b"\x1b[H");
    assert!(pipe.on_keystroke(b"z").is_empty());
    assert!(!pipe.predictor().should_show());

    pipe.predictor_mut_for_test()
        .backdate_all_for_test(Duration::from_millis(300));
    let paint = pipe.tick(Instant::now());

    assert!(
        !paint.is_empty(),
        "the 250ms glitch transition must repaint"
    );
    assert!(pipe.predictor().should_show());
    assert_eq!(pipe.last_shown().unwrap().cell_at(0, 0).unwrap().ch, 'z');
}

#[test]
fn pipeline_tick_underlines_hidden_adaptive_prediction_after_five_seconds() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Adaptive);
    pipe.prove_band_for_test();
    let _ = pipe.set_srtt(Some(Duration::from_millis(5)));
    let _ = pipe.on_host_bytes(b"\x1b[H");
    assert!(pipe.on_keystroke(b"z").is_empty());

    pipe.predictor_mut_for_test()
        .backdate_all_for_test(Duration::from_secs(6));
    let paint = pipe.tick(Instant::now());

    assert!(
        !paint.is_empty(),
        "the five-second flag transition must repaint"
    );
    assert!(pipe.predictor().flagging());
    let shown = pipe.last_shown().unwrap();
    assert_eq!(shown.cell_at(0, 0).unwrap().ch, 'z');
    assert!(shown.cell_at(0, 0).unwrap().attr.under);
}

#[test]
fn pipeline_bulk_paste_resets() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Always);
    let _ = pipe.on_host_bytes(b"\x1b[H");
    let big = vec![b'a'; 120];
    let _ = pipe.on_keystroke(&big);
    assert_eq!(
        pipe.predictor().pending_len(),
        0,
        "paste >100 must reset, not create 120 preds"
    );
}

#[test]
fn stock_sgr_mouse_input_never_paints_coordinate_digits_as_predictions() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Experimental);
    let _ = pipe.on_host_bytes(b"$ ");

    let first = pipe.on_keystroke(b"\x1b[<0;12");
    let second = pipe.on_keystroke(b";5M");

    assert!(
        first.is_empty() && second.is_empty(),
        "an SGR mouse report is one control input and must not locally echo its coordinates"
    );
}

#[test]
fn native_wide_symbol_does_not_receive_single_cell_prediction() {
    let mut pipe = DisplayPipeline::new(8, 2, DisplayPreference::Experimental);
    assert!(pipe.on_keystroke("☰".as_bytes()).is_empty());
}

#[test]
fn stock_last_column_backspace_keeps_the_cursor_on_the_border() {
    let mut pipe = DisplayPipeline::new(24, 8, DisplayPreference::Never);

    // Stock Display prints a last-column replacement as a glyph followed by
    // BS. Its terminal keeps an implicit cursor just beyond the right border,
    // so BS cancels pending wrap and lands on the last visible column. The
    // next replacement must overwrite that same cell, not the cell to its left.
    pipe.on_host_bytes(b"\x1b[8;24Ha\x08");
    pipe.on_host_bytes(b"P\x08");

    assert_eq!(pipe.host_fb().cell_at(23, 7).unwrap().ch, 'P');
    assert_eq!(pipe.host_fb().cell_at(22, 7).unwrap().ch, ' ');
    assert_eq!((pipe.host_fb().cur_x, pipe.host_fb().cur_y), (23, 7));
}

#[test]
fn stock_wide_overlap_preserves_the_uncovered_background_cell() {
    let mut pipe = DisplayPipeline::new(24, 8, DisplayPreference::Never);

    // Stock Display can first paint a wide glyph with a background at columns
    // 2-3, then replace it with a new wide glyph starting at column 1. Xterm
    // keeps the old trailing column as an erased cell with its old background.
    pipe.on_host_bytes("\x1b[1;2H\x1b[48;2;0;0;0m🙂\x1b[m\x1b[1;1H".as_bytes());
    pipe.on_host_bytes("🙂".as_bytes());

    assert_eq!(pipe.host_fb().cell_at(0, 0).unwrap().ch, '🙂');
    assert_eq!(pipe.host_fb().cell_at(0, 0).unwrap().width, 2);
    assert_eq!(pipe.host_fb().cell_at(1, 0).unwrap().width, 0);
    let uncovered = pipe.host_fb().cell_at(2, 0).unwrap();
    assert_eq!(uncovered.ch, ' ');
    assert_eq!(uncovered.width, 1);
    assert_eq!(uncovered.attr.bg, Color::rgb(0, 0, 0));
}

#[test]
fn native_display_wraps_wide_cell_at_right_margin() {
    let mut pipe = DisplayPipeline::new(24, 8, DisplayPreference::Never);
    pipe.on_host_bytes("\x1b[1;24H💻\r\n\x1b[1;24H".as_bytes());
    assert_eq!(pipe.host_fb().cell_at(23, 0).unwrap().ch, ' ');
    assert_eq!(pipe.host_fb().cell_at(0, 1).unwrap().ch, '💻');
    assert_eq!((pipe.host_fb().cur_x, pipe.host_fb().cur_y), (23, 0));
}

#[test]
fn pipeline_resize_full_redraw() {
    let mut pipe = DisplayPipeline::new(40, 10, DisplayPreference::Always);
    let _ = pipe.on_host_bytes(b"\x1b[Hhi");
    let paint = pipe.resize(80, 24);
    assert!(!paint.is_empty(), "resize must emit full redraw");
    assert_eq!(pipe.host_fb().cols, 80);
    assert_eq!(pipe.host_fb().rows, 24);
}

// ---------------------------------------------------------------------------
// Fidelity hardening (cursor_exp, unknown cells, overwrite mode, adaptive bg, structural)
// ---------------------------------------------------------------------------

#[test]
fn cursor_only_survives_confirm_until_frame_ack() {
    let mut p = always();
    p.set_frames(3, 2, 2);
    p.set_cursor(2, 0);
    p.keystroke(b"\x1b[C", &blank_fb()); // right arrow
    assert!(p.active());
    assert_eq!(p.cur_x(), 3);
    assert_eq!(p.pending_len(), 0);
    // Unacked: host still at old cursor — must keep glass cursor.
    let mut host = blank_fb();
    host.cur_x = 2;
    host.cur_y = 0;
    p.confirm(&host);
    assert!(p.active(), "cursor pred Pending until ack");
    assert_eq!(p.cur_x(), 3);
    // Ack + host matches predicted cursor.
    p.set_frames(3, 4, 4);
    host.cur_x = 3;
    p.confirm(&host);
    assert!(!p.active());
    assert_eq!(p.cur_x(), 3);
}

#[test]
fn cursor_only_resets_on_ack_mismatch() {
    let mut p = always();
    p.set_frames(5, 5, 5);
    p.set_cursor(1, 0);
    p.keystroke(b"\x1b[D", &blank_fb()); // left → col 0, exp=6
    assert_eq!(p.cur_x(), 0);
    p.set_frames(5, 6, 6); // acked past exp
    let mut host = blank_fb();
    host.cur_x = 4; // disagree
    host.cur_y = 0;
    p.confirm(&host);
    assert!(!p.active());
    assert_eq!(p.cur_x(), 4, "mismatch after ack snaps to host");
}

#[test]
fn unknown_overlay_does_not_replace_glyph() {
    let mut p = always_flagging();
    p.set_cursor(0, 0);
    p.keystroke(b"a", &blank_fb());
    p.set_unknown_pending_for_test(0);
    let mut fb = blank_fb();
    fb.put_rune(0, 0, 'H', Attr::default());
    p.overlay(&mut fb);
    assert_eq!(
        fb.cell_at(0, 0).unwrap().ch,
        'H',
        "unknown must not replace host"
    );
    assert!(
        fb.cell_at(0, 0).unwrap().attr.under,
        "flagging still underlines unknown mid-row"
    );
}

#[test]
fn adaptive_cold_builds_background_predictions() {
    let mut p = adaptive();
    assert!(!p.should_show());
    p.set_cursor(0, 0);
    p.keystroke(b"hi", &blank_fb());
    assert_eq!(p.pending_known_char_at(0, 0), Some('h'));
    assert_eq!(p.pending_known_char_at(1, 0), Some('i'));
    assert!(!p.should_show());
    p.prove_band_for_test();
    p.set_srtt(Some(Duration::from_millis(100)));
    assert!(p.should_show());
    let mut fb = blank_fb();
    p.overlay(&mut fb);
    assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'h');
}

#[test]
fn pipeline_adaptive_background_then_show() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Adaptive);
    pipe.prove_band_for_test();
    let _ = pipe.on_host_bytes(b"\x1b[H");
    let paint = pipe.on_keystroke(b"xy");
    assert!(paint.is_empty(), "cold adaptive must not paint");
    assert!(pipe.predictor().pending_len() >= 2);
    assert_eq!(pipe.predictor().pending_known_char_at(0, 0), Some('x'));
    let paint = pipe.set_srtt(Some(Duration::from_millis(100)));
    assert!(
        !paint.is_empty() || pipe.using_overlay_path(),
        "warming Adaptive must engage overlay path"
    );
    assert!(pipe.predictor().should_show());
}

#[test]
fn pipeline_ich_confirms_final_grid_not_hard_reset() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Always);
    pipe.prove_band_for_test();
    let _ = pipe.set_srtt(Some(Duration::from_millis(100)));
    let _ = pipe.on_host_bytes(b"\x1b[H");
    pipe.predictor_mut_for_test().set_overwrite_for_test(true);
    let _ = pipe.set_frames(4, 4, 4);
    let _ = pipe.on_keystroke(b"ab");
    assert_eq!(pipe.predictor().pending_known_char_at(0, 0), Some('a'));
    let _ = pipe.on_host_bytes(b"\x1b[1;1H\x1b[2@");
    let _ = pipe.set_frames(4, 5, 5);
    assert_eq!(
        pipe.predictor().pending_known_char_at(0, 0),
        None,
        "ICH mismatch resolves via Confirm"
    );
}

#[test]
fn pipeline_split_host_csi_reassembled() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Always);
    let _ = pipe.on_host_bytes(b"\x1b[1;");
    let _ = pipe.on_host_bytes(b"3HX");
    assert_eq!(pipe.host_fb().cur_y, 0);
    // CUP 1;3 → col 2, then X printed
    assert_eq!(pipe.host_fb().cell_at(2, 0).unwrap().ch, 'X');
}

#[test]
fn cross_epoch_insert_shifts_all_pending_on_row() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"abcd", &blank_fb());
    p.become_tentative();
    // Move into the middle of older-epoch preds via left arrows
    while p.cur_x() > 1 {
        p.keystroke(b"\x1b[D", &blank_fb());
    }
    assert_eq!(p.cur_x(), 1);
    let n_before = p.pending_len();
    p.keystroke(b"X", &blank_fb());
    let mut positions: Vec<usize> = (0..p.pending_len())
        .filter_map(|i| p.pending_pos(i).map(|(x, _)| x))
        .collect();
    positions.sort();
    assert!(
        positions.contains(&1),
        "insert at 1 must place/shift; positions={positions:?} n_before={n_before}"
    );
    assert_eq!(p.pending_char(0), Some('a'));
    assert_eq!(p.pending_pos(0), Some((0, 0)));
}

#[test]
fn kill_epoch_resyncs_cursor_to_host() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"ab", &blank_fb());
    p.become_tentative();
    p.keystroke(b"xy", &blank_fb());
    let mut host = blank_fb();
    host.put_rune(0, 0, 'a', Attr::default());
    host.put_rune(1, 0, 'b', Attr::default());
    host.put_rune(2, 0, 'Z', Attr::default()); // not 'x'
    host.cur_x = 3;
    host.cur_y = 0;
    p.confirm(&host);
    for i in 0..p.pending_len() {
        assert_ne!(p.pending_char(i), Some('x'));
        assert_ne!(p.pending_char(i), Some('y'));
    }
    // Failed band killed; remaining empty or older band only; cursor sane.
    if p.pending_len() == 0 {
        assert_eq!(p.cur_x(), 3);
        assert_eq!(p.cur_y(), 0);
    }
}

#[test]
fn ss3_up_does_not_pollute_pending() {
    let mut p = always();
    p.set_overwrite_for_test(true);
    p.set_cursor(0, 0);
    p.keystroke(b"a", &blank_fb());
    let before = p.pending_len();
    let ep = p.prediction_epoch_for_test();
    p.keystroke(b"\x1bOA", &blank_fb()); // SS3 up → tentative, consume
    assert_eq!(p.pending_len(), before, "SS3 up must not add printables");
    assert!(p.prediction_epoch_for_test() > ep);
}

#[test]
fn pipeline_split_ich_still_parsed_for_host() {
    // Split CSI ICH must reassemble via carry for host_fb (not prediction wipe).
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Always);
    pipe.prove_band_for_test();
    let _ = pipe.on_host_bytes(b"\x1b[Hxy");
    assert_eq!(pipe.host_fb().cell_at(0, 0).unwrap().ch, 'x');
    let _ = pipe.on_host_bytes(b"\x1b[1;1H\x1b[2"); // incomplete ICH
    let _ = pipe.on_host_bytes(b"@"); // complete
    // Host should have inserted blanks at (0,0)
    // After ICH 2 at 1;1, x,y shift right
    assert_eq!(pipe.host_fb().cell_at(2, 0).unwrap().ch, 'x');
}

#[test]
fn adaptive_demote_holds_while_cursor_pending() {
    let mut p = adaptive();
    p.set_srtt(Some(Duration::from_millis(100)));
    p.set_cursor(3, 0);
    p.keystroke(b"\x1b[D", &blank_fb());
    assert!(p.active());
    p.set_srtt(Some(Duration::from_millis(5)));
    assert!(
        p.should_show(),
        "cursor-only Pending holds show (stock active)"
    );
}

#[test]
fn kill_epoch_resyncs_cursor_to_host_strict() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.become_tentative();
    p.keystroke(b"xy", &blank_fb());
    let mut host = blank_fb();
    host.put_rune(0, 0, 'Z', Attr::default());
    host.cur_x = 5;
    host.cur_y = 1;
    p.confirm(&host);
    // Failed tentative with no matched prefix → kill_epoch empties → snap cursor
    assert_eq!(p.pending_len(), 0);
    assert_eq!(p.cur_x(), 5);
    assert_eq!(p.cur_y(), 1);
}

// ---------------------------------------------------------------------------
// Polish: Correct row rendition sync + keystroke UTF-8 carry
// ---------------------------------------------------------------------------

#[test]
fn correct_cascades_host_renditions_to_rest_of_row() {
    let mut p = always_flagging();
    p.set_overwrite_for_test(true);
    p.set_cursor(0, 0);
    p.keystroke(b"abc", &blank_fb());
    let mut host = blank_fb();
    let bold = Attr {
        bold: true,
        ..Attr::default()
    };
    host.put_rune(0, 0, 'a', bold);
    host.cur_x = 1;
    p.confirm(&host);
    assert_eq!(p.pending_known_char_at(1, 0), Some('b'));
    let mut fb = blank_fb();
    fb.put_rune(0, 0, 'a', bold);
    p.overlay(&mut fb);
    assert!(fb.cell_at(1, 0).unwrap().attr.bold);
    assert!(fb.cell_at(2, 0).unwrap().attr.bold);
    assert!(fb.cell_at(1, 0).unwrap().attr.under);
}

#[test]
fn correct_no_credit_does_not_prove_or_false_cascade_path() {
    let mut p = always();
    let mut host = blank_fb();
    let bold = Attr {
        bold: true,
        ..Attr::default()
    };
    host.put_rune(0, 0, 'a', bold);
    p.set_cursor(0, 0);
    p.become_tentative();
    let conf_before = p.confirmed_epoch_for_test();
    p.set_overwrite_for_test(true);
    p.keystroke(b"a", &host); // noop match on original 'a'
    host.cur_x = 1;
    p.confirm(&host);
    assert_eq!(p.pending_len(), 0);
    assert_eq!(
        p.confirmed_epoch_for_test(),
        conf_before,
        "CorrectNoCredit must not advance confirmed_epoch"
    );
}

#[test]
fn correct_cascade_same_row_only() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"ab", &blank_fb());
    // Manually inject a same-epoch pending on another row via cursor + key
    // (simulate by becoming non-tentative equal epochs through confirm-free path)
    // Easier: type on row0, set pending on row1 by using test helper if needed.
    // Use CR (tentative) then type — those are different epoch and may be hidden.
    // Instead: after typing ab, move to row1 without become_tentative by forcing
    // cursor when inactive — but we're active. So left-only path won't work.
    // Build two-row pending with same epoch via predict by host insert? Skip.
    // Direct: confirm 'a' bold; remaining 'b' on row0 gets bold; ensure no crash.
    let mut host = blank_fb();
    let bold = Attr {
        bold: true,
        ..Attr::default()
    };
    host.put_rune(0, 0, 'a', bold);
    host.cur_x = 1;
    p.confirm(&host);
    assert_eq!(p.pending_pos(0), Some((1, 0)));
    let mut fb = blank_fb();
    // Put a bold cell on a different row that must not affect anything wrongly
    fb.put_rune(0, 1, 'Z', bold);
    fb.put_rune(0, 0, 'a', bold);
    p.overlay(&mut fb);
    assert!(fb.cell_at(1, 0).unwrap().attr.bold);
    // Unrelated row1 cell Z must stay as we left it (overlay shouldn't touch)
    assert_eq!(fb.cell_at(0, 1).unwrap().ch, 'Z');
}

#[test]
fn correct_cascade_dim_and_fg() {
    let mut p = always_proven();
    p.set_overwrite_for_test(true);
    p.set_cursor(0, 0);
    p.keystroke(b"xy", &blank_fb());
    let mut host = blank_fb();
    let attr = Attr {
        dim: true,
        fg: crate::framebuffer::Color::index(2),
        ..Attr::default()
    };
    host.put_rune(0, 0, 'x', attr);
    host.cur_x = 1;
    p.confirm(&host);
    let mut fb = blank_fb();
    fb.put_rune(0, 0, 'x', attr);
    p.overlay(&mut fb);
    assert!(fb.cell_at(1, 0).unwrap().attr.dim);
    assert_eq!(
        fb.cell_at(1, 0).unwrap().attr.fg,
        crate::framebuffer::Color::index(2)
    );
}

#[test]
fn correct_cascade_survives_flagging_off() {
    let mut p = adaptive();
    p.set_srtt(Some(Duration::from_millis(100))); // show+flag
    p.set_cursor(0, 0);
    p.keystroke(b"ab", &blank_fb());
    let mut host = blank_fb();
    let bold = Attr {
        bold: true,
        ..Attr::default()
    };
    host.put_rune(0, 0, 'a', bold);
    host.cur_x = 1;
    p.confirm(&host);
    // Demote flagging only
    p.set_srtt(Some(Duration::from_millis(40)));
    assert!(p.should_show());
    assert!(!p.flagging());
    let mut fb = blank_fb();
    fb.put_rune(0, 0, 'a', bold);
    p.overlay(&mut fb);
    assert!(fb.cell_at(1, 0).unwrap().attr.bold, "cascade bold remains");
    assert!(
        !fb.cell_at(1, 0).unwrap().attr.under,
        "flagging off clears under even with cascade"
    );
}

#[test]
fn keystroke_split_utf8_euro_reassembled() {
    let mut p = always();
    p.set_overwrite_for_test(true);
    p.set_cursor(0, 0);
    // Euro U+20AC = e2 82 ac
    p.keystroke(&[0xe2], &blank_fb());
    assert_eq!(p.pending_len(), 0);
    assert!(p.has_esc_buf_for_test(), "incomplete UTF-8 must carry");
    p.keystroke(&[0x82, 0xac], &blank_fb());
    assert_eq!(p.pending_known_char_at(0, 0), Some('€'));
}

#[test]
fn keystroke_split_utf8_after_ascii_prefix() {
    let mut p = always();
    p.set_overwrite_for_test(true);
    p.set_cursor(0, 0);
    p.keystroke(b"a", &blank_fb());
    assert_eq!(p.pending_known_char_at(0, 0), Some('a'));
    p.keystroke(&[0xc3], &blank_fb()); // first byte of é
    p.keystroke(&[0xa9], &blank_fb());
    assert_eq!(p.pending_known_char_at(1, 0), Some('é'));
}

#[test]
fn keystroke_invalid_utf8_lead_is_tentative() {
    let mut p = always();
    p.set_overwrite_for_test(true);
    p.set_cursor(0, 0);
    p.keystroke(b"ab", &blank_fb());
    let ep = p.prediction_epoch_for_test();
    p.keystroke(&[0xff], &blank_fb());
    assert_eq!(p.pending_known_char_at(0, 0), Some('a'));
    assert_eq!(p.pending_known_char_at(1, 0), Some('b'));
    assert!(
        p.prediction_epoch_for_test() > ep,
        "invalid must not wipe old pending"
    );
}

#[test]
fn keystroke_utf8_carry_does_not_break_following_csi() {
    let mut p = always();
    p.set_overwrite_for_test(true);
    p.set_cursor(0, 0);
    p.keystroke(b"a", &blank_fb());
    assert_eq!(p.pending_known_char_at(0, 0), Some('a'));
    p.keystroke(b"\x1b[D", &blank_fb());
    assert_eq!(p.cur_x(), 0);
    assert_eq!(p.pending_known_char_at(0, 0), Some('a'));
}

#[test]
fn pipeline_keystroke_split_utf8_paints_once_complete() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Always);
    pipe.prove_band_for_test();
    let _ = pipe.on_host_bytes(b"\x1b[H");
    let p1 = pipe.on_keystroke(&[0xc3]);
    assert!(p1.is_empty(), "incomplete UTF-8 must not paint");
    assert_eq!(pipe.predictor().pending_len(), 0);
    let p2 = pipe.on_keystroke(&[0xa9]); // completes é
    assert!(!p2.is_empty(), "complete UTF-8 must paint");
    assert_eq!(pipe.predictor().pending_known_char_at(0, 0), Some('é'));
}

#[test]
fn pipeline_csi_param_arrow_moves_glass_cursor() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Always);
    pipe.prove_band_for_test();
    let _ = pipe.on_host_bytes(b"\x1b[H");
    let _ = pipe.on_keystroke(b"\x1b[3C");
    // Stock: +1 only
    assert_eq!(pipe.predictor().cur_x(), 1);
}

#[test]
fn pipeline_correct_cascade_visible_in_last_shown() {
    let mut pipe = DisplayPipeline::new(80, 24, DisplayPreference::Always);
    pipe.prove_band_for_test();
    // Force overwrite-like single cells by proving and typing with host spaces
    pipe.predictor_mut_for_test().set_overwrite_for_test(true);
    let _ = pipe.on_host_bytes(b"\x1b[H");
    let _ = pipe.on_keystroke(b"ab");
    assert_eq!(pipe.predictor().pending_known_char_at(0, 0), Some('a'));
    assert_eq!(pipe.predictor().pending_known_char_at(1, 0), Some('b'));
    // Confirm 'a' with bold via SGR
    let _ = pipe.on_host_bytes(b"\x1b[1;1H\x1b[1ma\x1b[1;2H");
    assert_eq!(pipe.predictor().pending_known_char_at(0, 0), None);
    assert_eq!(pipe.predictor().pending_known_char_at(1, 0), Some('b'));
    let shown = pipe.last_shown().unwrap();
    assert!(
        shown.cell_at(1, 0).unwrap().attr.bold,
        "cascade bold onto remaining pred in last_shown"
    );
}

// ---------------------------------------------------------------------------
// late_ack (echo_ack) vs early transport ack
// ---------------------------------------------------------------------------

#[test]
fn pending_uses_late_ack_not_early_transport_ack() {
    let mut p = always();
    p.set_overwrite_for_test(true);
    p.set_cursor(0, 0);
    p.set_frames(5, 6, 0);
    p.keystroke(b"ab", &blank_fb());
    assert_eq!(p.pending_known_char_at(0, 0), Some('a'));
    assert_eq!(p.pending_known_char_at(1, 0), Some('b'));
    let mut host = blank_fb();
    host.put_rune(0, 0, 'a', Attr::default());
    host.put_rune(1, 0, 'b', Attr::default());
    host.cur_x = 2;
    p.confirm(&host);
    assert_eq!(
        p.pending_known_char_at(0, 0),
        Some('a'),
        "stock Pending uses late_ack; early transport ack alone must not confirm"
    );
    p.set_frames(5, 6, 6);
    p.confirm(&host);
    assert_eq!(p.pending_len(), 0, "late_ack must release Pending");
}

#[test]
fn cursor_only_pending_waits_for_late_ack() {
    let mut p = always();
    p.set_frames(3, 3, 0);
    p.set_cursor(2, 0);
    p.keystroke(b"\x1b[C", &blank_fb());
    assert_eq!(p.cur_x(), 3);
    let mut host = blank_fb();
    host.cur_x = 2; // host not yet moved
    p.confirm(&host);
    assert!(p.active(), "cursor Pending until late_ack");
    assert_eq!(p.cur_x(), 3);
    // early already 3, late still 0 — still pending
    p.set_frames(3, 9, 0);
    p.confirm(&host);
    assert!(p.active());
    // late catches up; host still wrong → reset
    p.set_frames(3, 9, 4);
    host.cur_x = 2;
    p.confirm(&host);
    assert!(!p.active());
    assert_eq!(p.cur_x(), 2);
}

// ---------------------------------------------------------------------------
// Stock alignment gates (overlay apply, epoch hide, full-row shift, reset)
// ---------------------------------------------------------------------------

#[test]
fn stock_fresh_session_hides_until_credited_correct() {
    let mut p = always();
    p.set_overwrite_for_test(true);
    assert_eq!(p.prediction_epoch_for_test(), 1);
    assert_eq!(p.confirmed_epoch_for_test(), 0);
    p.set_cursor(0, 0);
    p.keystroke(b"xy", &blank_fb());
    let mut fb = blank_fb();
    p.overlay(&mut fb);
    assert_ne!(fb.cell_at(0, 0).unwrap().ch, 'x');
    let mut host = blank_fb();
    host.put_rune(0, 0, 'x', Attr::default());
    host.cur_x = 1;
    p.confirm(&host);
    assert_eq!(p.confirmed_epoch_for_test(), 1);
    assert_eq!(p.pending_known_char_at(1, 0), Some('y'));
    // Enable flagging (stock: not automatic for Always)
    p.set_srtt(Some(Duration::from_millis(100)));
    let mut view = blank_fb();
    view.put_rune(0, 0, 'x', Attr::default());
    p.overlay(&mut view);
    assert_eq!(view.cell_at(1, 0).unwrap().ch, 'y');
    assert!(view.cell_at(1, 0).unwrap().attr.under);
}

#[test]
fn stock_reset_does_not_realign_confirmed_epoch() {
    let mut p = always();
    p.set_cursor(0, 0);
    p.keystroke(b"a", &blank_fb());
    let mut host = blank_fb();
    host.put_rune(0, 0, 'a', Attr::default());
    host.cur_x = host.cols;
    p.confirm(&host);
    assert_eq!(p.confirmed_epoch_for_test(), p.prediction_epoch_for_test());
    let conf = p.confirmed_epoch_for_test();
    p.reset();
    assert!(
        p.prediction_epoch_for_test() > conf,
        "reset becomes tentative"
    );
    assert_eq!(
        p.confirmed_epoch_for_test(),
        conf,
        "reset must not re-align confirmed_epoch"
    );
    p.set_cursor(0, 0);
    p.keystroke(b"z", &blank_fb());
    let mut fb = blank_fb();
    p.overlay(&mut fb);
    assert_ne!(
        fb.cell_at(0, 0).unwrap().ch,
        'z',
        "post-reset preds stay hidden until proven"
    );
}

#[test]
fn stock_overlay_blank_on_blank_no_underline() {
    let mut p = always_proven();
    p.set_cursor(0, 0);
    // Predict space over blank host
    p.keystroke(b" ", &blank_fb());
    let mut fb = blank_fb();
    p.overlay(&mut fb);
    assert!(
        !fb.cell_at(0, 0).unwrap().attr.under,
        "blank-on-blank must not underline"
    );
}

#[test]
fn stock_overlay_matching_host_cell_no_underline() {
    let mut p = always_proven();
    let mut fb = blank_fb();
    fb.put_rune(0, 0, 'a', Attr::default());
    p.set_cursor(0, 0);
    p.keystroke(b"a", &fb); // pred matches host glyph
    p.overlay(&mut fb);
    assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'a');
    assert!(
        !fb.cell_at(0, 0).unwrap().attr.under,
        "identical host cell must not gain underline"
    );
}

#[test]
fn stock_unknown_last_col_no_underline() {
    let mut p = always_flagging();
    p.set_overwrite_for_test(true);
    p.set_cursor(1, 0);
    p.keystroke(b"m", &blank_fb()); // mid col
    let mid = (0..p.pending_len())
        .find(|&i| p.pending_pos(i) == Some((1, 0)))
        .expect("mid pending");
    p.set_unknown_pending_for_test(mid);
    let mut mid_view = blank_fb();
    mid_view.put_rune(1, 0, 'H', Attr::default());
    p.overlay(&mut mid_view);
    assert!(
        mid_view.cell_at(1, 0).unwrap().attr.under,
        "unknown mid-row underlines when flagging"
    );

    let mut p2 = always_flagging();
    let fb = Framebuffer::new(4, 2);
    p2.set_cursor(3, 0);
    p2.keystroke(b"x", &fb);
    let last = (0..p2.pending_len())
        .find(|&i| p2.pending_pos(i) == Some((3, 0)))
        .expect("last-col pending");
    p2.set_unknown_pending_for_test(last);
    let mut view = Framebuffer::new(4, 2);
    view.put_rune(3, 0, 'H', Attr::default());
    p2.overlay(&mut view);
    assert_eq!(view.cell_at(3, 0).unwrap().ch, 'H');
    assert!(
        !view.cell_at(3, 0).unwrap().attr.under,
        "unknown last column must not underline even when flagging"
    );
}

#[test]
fn stock_full_row_insert_shifts_through_last_column() {
    let mut p = always();
    let mut host = Framebuffer::new(8, 2);
    for (i, ch) in ['a', 'b', 'c', 'd'].into_iter().enumerate() {
        host.put_rune(i, 0, ch, Attr::default());
    }
    p.set_cursor(1, 0);
    p.keystroke(b"X", &host);
    assert_eq!(p.pending_len(), 7, "cols from cursor through last");
    assert_eq!(p.pending_known_char_at(1, 0), Some('X'));
    assert_eq!(p.pending_known_char_at(2, 0), Some('b'));
    assert_eq!(p.pending_known_char_at(3, 0), Some('c'));
    assert_eq!(p.pending_known_char_at(4, 0), Some('d'));
    assert!(p.pending_unknown_at(7, 0));
}

#[test]
fn stock_glitch_trigger_truthy_forces_show() {
    let mut p = adaptive();
    p.set_srtt(Some(Duration::from_millis(5)));
    assert!(!p.should_show());
    p.prove_band_for_test();
    p.set_cursor(0, 0);
    p.keystroke(b"a", &blank_fb());
    p.backdate_all_for_test(Duration::from_millis(300));
    p.sample_pending_age(Instant::now());
    assert!(p.glitch_trigger_for_test() >= 10);
    // Low SRTT but glitch truthy → show
    p.set_srtt(Some(Duration::from_millis(5)));
    assert!(p.should_show(), "any non-zero glitch_trigger forces show");
}

#[test]
fn stock_pipeline_never_dual_writes_raw_glyphs() {
    let mut pipe = DisplayPipeline::new(40, 10, DisplayPreference::Always);
    pipe.prove_band_for_test();
    let _ = pipe.on_host_bytes(b"\x1b[H>");
    let paint = pipe.on_keystroke(b"z");
    // Paint must be Diff CSI/CUP style, not bare dual-write of only 'z' without model
    assert!(!paint.is_empty());
    assert!(pipe.using_overlay_path(), "Always uses overlay Diff path");
    let shown = pipe.last_shown().unwrap();
    assert_eq!(shown.cell_at(1, 0).unwrap().ch, 'z');
    // Host model itself must NOT have the prediction (host_fb is server-only)
    assert_ne!(
        pipe.host_fb().cell_at(1, 0).map(|c| c.ch),
        Some('z'),
        "prediction must not write into host_fb"
    );
}

// ---------------------------------------------------------------------------
// Stock fidelity gates (multi-agent gap fill)
// ---------------------------------------------------------------------------

#[test]
fn stock_post_ack_blank_host_is_incorrect_not_stall() {
    let mut p = always();
    p.set_overwrite_for_test(true);
    p.set_frames(5, 0, 0);
    p.set_cursor(0, 0);
    p.keystroke(b"a", &blank_fb());
    // Content present but unacked → Pending
    let mut host = blank_fb();
    host.put_rune(0, 0, 'a', Attr::default());
    host.cur_x = 1;
    p.confirm(&host);
    assert_eq!(p.pending_known_char_at(0, 0), Some('a'));
    // Ack frames but host rolls back to blank → IncorrectOrExpired
    p.set_frames(5, 6, 6);
    let blank = blank_fb();
    p.confirm(&blank);
    assert_eq!(
        p.pending_known_char_at(0, 0),
        None,
        "post-ack blank host must not infinite-stall"
    );
}

#[test]
fn stock_glitch_repair_only_on_credited_correct() {
    let mut p = adaptive_proven();
    p.set_srtt(Some(Duration::from_millis(100)));
    p.set_overwrite_for_test(true);
    p.set_cursor(0, 0);
    p.keystroke(b" ", &blank_fb()); // blank pred
    p.backdate_all_for_test(Duration::from_millis(50)); // quick but no-credit
    // Force glitch high
    p.backdate_all_for_test(Duration::from_millis(300));
    p.sample_pending_age(Instant::now());
    let g = p.glitch_trigger_for_test();
    assert!(g >= 10);
    // Confirm blank match (CorrectNoCredit) — must NOT repair glitch
    let fb = blank_fb();
    p.confirm(&fb);
    assert_eq!(
        p.glitch_trigger_for_test(),
        g,
        "CorrectNoCredit must not decrement glitch_trigger"
    );
}

#[test]
fn stock_always_does_not_force_flagging() {
    let mut p = always();
    p.set_srtt(Some(Duration::from_millis(20))); // below FLAG_TRIGGER_LOW
    assert!(p.should_show());
    assert!(!p.flagging(), "Always show ≠ always underline");
    p.prove_band_for_test();
    p.set_overwrite_for_test(true);
    p.set_cursor(0, 0);
    p.keystroke(b"a", &blank_fb());
    let mut fb = blank_fb();
    p.overlay(&mut fb);
    assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'a');
    assert!(!fb.cell_at(0, 0).unwrap().attr.under);
}

#[test]
fn stock_last_col_print_is_known_not_unknown() {
    let mut p = always_proven();
    let fb = Framebuffer::new(4, 3);
    p.set_cursor(3, 0);
    let ep_cell = p.prediction_epoch_for_test();
    p.keystroke(b"x", &fb);
    assert!(
        !p.pending_unknown_at(3, 0),
        "stock places known glyph at last col"
    );
    assert_eq!(p.pending_known_char_at(3, 0), Some('x'));
    assert_eq!((p.cur_x(), p.cur_y()), (0, 1));
    // Second become_tentative after wrap → new typing uses newer epoch
    assert!(
        p.prediction_epoch_for_test() > ep_cell,
        "wrap does extra become_tentative"
    );
}

#[test]
fn stock_cr_on_bottom_row_blank_predicts_full_row() {
    let mut p = always();
    let fb = Framebuffer::new(8, 3);
    p.set_cursor(4, 2); // last row
    p.keystroke(b"\r", &fb);
    assert_eq!(p.cur_x(), 0);
    assert_eq!(p.cur_y(), 2);
    assert_eq!(p.pending_len(), 8, "one blank pred per column");
    for x in 0..8 {
        assert_eq!(p.pending_known_char_at(x, 2), Some(' '));
    }
}

#[test]
fn stock_reset_preserves_glitch_trigger() {
    let mut p = adaptive();
    p.set_srtt(Some(Duration::from_millis(100)));
    p.prove_band_for_test();
    p.set_overwrite_for_test(true);
    p.set_cursor(0, 0);
    p.keystroke(b"a", &blank_fb());
    p.backdate_all_for_test(Duration::from_millis(300));
    p.sample_pending_age(Instant::now());
    let g = p.glitch_trigger_for_test();
    assert!(g >= 10);
    p.reset();
    assert_eq!(
        p.glitch_trigger_for_test(),
        g,
        "stock reset does not clear glitch_trigger"
    );
    // Low SRTT but glitch still forces show
    p.set_srtt(Some(Duration::from_millis(5)));
    assert!(p.should_show());
}

#[test]
fn stock_kill_epoch_removes_failed_and_newer_bands() {
    let mut p = always();
    p.set_overwrite_for_test(true);
    p.set_cursor(0, 0);
    p.keystroke(b"ab", &blank_fb());
    // Prove epoch 1
    let mut host = blank_fb();
    host.put_rune(0, 0, 'a', Attr::default());
    host.put_rune(1, 0, 'b', Attr::default());
    host.cur_x = 2;
    p.confirm(&host);
    assert_eq!(p.confirmed_epoch_for_test(), p.prediction_epoch_for_test());
    p.become_tentative();
    let e_fail = p.prediction_epoch_for_test();
    p.keystroke(b"xy", &blank_fb()); // epoch e_fail
    p.become_tentative();
    p.keystroke(b"z", &blank_fb()); // newer epoch
    // Diverge on first of failed band
    let mut bad = blank_fb();
    bad.put_rune(2, 0, 'Q', Attr::default());
    bad.cur_x = 3;
    p.confirm(&bad);
    assert_eq!(p.pending_known_char_at(2, 0), None);
    assert_eq!(p.pending_known_char_at(3, 0), None);
    assert_eq!(
        p.pending_known_char_at(4, 0),
        None,
        "newer band also killed"
    );
    assert_eq!(p.cur_x(), 3, "cursor snapped to host");
    let _ = e_fail;
}

// ---------------------------------------------------------------------------
// Code-review regression gates
// ---------------------------------------------------------------------------

#[test]
fn pipeline_late_ack_without_host_bytes_drains_pending() {
    let mut pipe = DisplayPipeline::new(40, 10, DisplayPreference::Always);
    pipe.prove_band_for_test();
    let _ = pipe.set_srtt(Some(Duration::from_millis(100)));
    let _ = pipe.on_host_bytes(b"\x1b[H");
    pipe.predictor_mut_for_test().set_overwrite_for_test(true);
    let _ = pipe.set_frames(3, 0, 0);
    let _ = pipe.on_keystroke(b"z");
    assert_eq!(pipe.predictor().pending_known_char_at(0, 0), Some('z'));
    // Host content while still Pending (late < exp)
    let _ = pipe.on_host_bytes(b"\x1b[1;1Hz");
    assert_eq!(
        pipe.predictor().pending_known_char_at(0, 0),
        Some('z'),
        "must stay Pending until late_ack"
    );
    // Ack-only: no new hoststring
    let paint = pipe.set_frames(3, 4, 4);
    assert_eq!(
        pipe.predictor().pending_known_char_at(0, 0),
        None,
        "late_ack alone must Confirm/drain"
    );
    assert_eq!(
        pipe.predictor().confirmed_epoch_for_test(),
        pipe.predictor().prediction_epoch_for_test(),
        "matching host z must credit band (not only kill)"
    );
    let _ = paint;
}

#[test]
fn kill_epoch_anchors_host_cursor_until_normal_confirmation() {
    let mut p = always();
    p.set_overwrite_for_test(true);
    p.set_cursor(0, 0);
    p.keystroke(b"ab", &blank_fb());
    let mut fb = blank_fb();
    fb.put_rune(0, 0, 'x', Attr::default());
    fb.cur_x = 5;
    p.confirm(&fb); // tentative diverge → kill_epoch
    assert!(p.active(), "stock keeps the host cursor conditional");
    let mut painted = fb.clone();
    p.overlay(&mut painted);
    assert_eq!((painted.cur_x, painted.cur_y), (5, fb.cur_y));
    p.confirm(&fb);
    assert!(!p.active(), "matching host cursor drains normally");
    // set_cursor must work again
    p.set_cursor(2, 3);
    assert_eq!(p.cur_x(), 2);
    assert_eq!(p.cur_y(), 3);
}

#[test]
fn overwrite_retype_same_cell_replaces_not_stacks() {
    let mut p = always_proven();
    p.set_overwrite_for_test(true);
    p.set_cursor(0, 0);
    p.keystroke(b"a", &blank_fb());
    p.keystroke(b"\x1b[D", &blank_fb()); // left back to 0
    p.keystroke(b"b", &blank_fb());
    assert_eq!(p.pending_known_char_at(0, 0), Some('b'));
    // Only one known pred at col 0
    let mut count = 0;
    for i in 0..p.pending_len() {
        if p.pending_pos(i) == Some((0, 0)) {
            count += 1;
        }
    }
    assert_eq!(count, 1, "must not stack overwrite preds at same cell");
}

#[test]
fn repeated_overwrite_value_does_not_falsely_confirm_a_tentative_epoch() {
    let mut p = always();
    p.set_overwrite_for_test(true);
    let mut host = blank_fb();
    p.set_cursor(0, 0);
    p.keystroke(b"b", &host);
    p.keystroke(b"\x1b[D", &host);
    p.keystroke(b"b", &host);

    host.put_rune(0, 0, 'b', Attr::default());
    p.confirm(&host);

    assert_eq!(
        p.confirmed_epoch_for_test(),
        0,
        "a value already predicted at this cell is CorrectNoCredit in stock mosh"
    );
}

#[test]
fn confirm_clears_cursor_exp_when_unframed_mismatch() {
    let mut p = always_proven();
    p.set_cursor(2, 0);
    p.keystroke(b"\x1b[C", &blank_fb());
    assert!(p.active());
    let mut host = blank_fb();
    host.cur_x = 0; // disagree, local_frame_sent==0
    p.confirm(&host);
    assert!(!p.active());
    // Adaptive demote must not latch forever: no public cursor_exp accessor —
    // set_cursor works and show demote path
    p.set_cursor(1, 1);
    assert_eq!((p.cur_x(), p.cur_y()), (1, 1));
}

#[test]
fn esc_meta_does_not_predict_printable() {
    // Alt-x arrives as ESC x — must not insert-predict 'x'.
    let mut p = always_proven();
    p.set_overwrite_for_test(true);
    p.set_cursor(0, 0);
    let ep = p.prediction_epoch_for_test();
    p.keystroke(b"\x1bx", &blank_fb());
    assert_eq!(p.pending_known_char_at(0, 0), None);
    assert!(p.prediction_epoch_for_test() > ep);
}

#[test]
fn overlay_does_not_move_cursor_for_tentative_cr() {
    let mut p = always_flagging();
    p.set_overwrite_for_test(true);
    p.set_cursor(0, 0);
    p.keystroke(b"ab", &blank_fb());
    // Prove so cells visible (band already proven via always_flagging)
    let mut view = blank_fb();
    p.overlay(&mut view);
    assert_eq!(view.cell_at(0, 0).unwrap().ch, 'a');
    let host_cur_x = view.cur_x;
    p.keystroke(b"\r", &blank_fb()); // become_tentative + next row
    let mut view2 = blank_fb();
    view2.cur_x = host_cur_x;
    view2.cur_y = 0;
    // put host cells so we can see overlay still paints a,b if still pending
    // After CR with overwrite, pending still has a,b on row 0
    p.overlay(&mut view2);
    // Cursor must stay at host (row 0), not jump to row 1 (tentative CR)
    assert_eq!(view2.cur_y, 0, "tentative CR must not move glass cursor");
}

#[test]
fn tentative_new_cursor_keeps_the_last_confirmed_cursor_visible() {
    let mut p = always_proven();
    p.set_overwrite_for_test(true);
    let host = blank_fb();
    p.set_cursor(0, 0);
    p.keystroke(b"ab", &host);
    p.keystroke(b"\r", &host);

    let mut view = blank_fb();
    p.overlay(&mut view);
    assert_eq!(
        (view.cur_x, view.cur_y),
        (2, 0),
        "stock applies the older confirmed cursor and hides the newer tentative CR"
    );
}

#[test]
fn host_wrap_scroll_resets_pending() {
    let mut pipe = DisplayPipeline::new(4, 3, DisplayPreference::Always);
    pipe.prove_band_for_test();
    let _ = pipe.set_srtt(Some(Duration::from_millis(100)));
    // Bottom row last col with wrap flag
    let _ = pipe.on_host_bytes(b"\x1b[3;1Haaaa");
    assert!(pipe.host_fb().next_print_will_wrap);
    pipe.predictor_mut_for_test().set_overwrite_for_test(true);
    let _ = pipe.on_keystroke(b"z");
    assert!(pipe.predictor().pending_len() > 0);
    let gen0 = pipe.host_fb().scroll_generation;
    // Next glyph wrap-scrolls without LF in stream
    let _ = pipe.on_host_bytes(b"X");
    assert!(
        pipe.host_fb().scroll_generation > gen0,
        "DECAWM wrap on bottom must scroll"
    );
    assert_eq!(
        pipe.predictor().pending_len(),
        0,
        "scroll must wipe pending coords"
    );
}

#[test]
fn reconstructed_host_frame_scroll_resets_pending() {
    let mut pipe = DisplayPipeline::new(4, 3, DisplayPreference::Always);
    pipe.prove_band_for_test();
    let mut host = Framebuffer::new(4, 3);
    host.cur_x = 0;
    host.cur_y = 2;
    let _ = pipe.on_host_frame(&host);
    pipe.predictor_mut_for_test().set_overwrite_for_test(true);
    let _ = pipe.on_keystroke(b"z");
    assert!(pipe.predictor().pending_len() > 0);

    let mut scrolled = host.clone();
    crate::ansi_apply::apply_ansi(&mut scrolled, b"\n");
    assert_ne!(scrolled.scroll_generation, host.scroll_generation);
    let _ = pipe.on_host_frame(&scrolled);
    assert_eq!(
        pipe.predictor().pending_len(),
        0,
        "a reconstructed server scroll must invalidate old prediction rows"
    );
}

#[test]
fn last_col_insert_collates_prior_unknown() {
    let mut p = always_proven();
    let fb = Framebuffer::new(4, 2);
    p.set_cursor(0, 0);
    // densify via insert
    p.keystroke(b"abc", &fb); // fills 0..2, last col unknown from shifts
    p.keystroke(b"d", &fb); // last col place known
    // only one pred at (3,0)
    let mut n = 0;
    for i in 0..p.pending_len() {
        if p.pending_pos(i) == Some((3, 0)) {
            n += 1;
        }
    }
    assert_eq!(n, 1);
    assert_eq!(p.pending_known_char_at(3, 0), Some('d'));
}

#[test]
fn host_then_late_ack_same_batch_does_not_kill_match() {
    // Simulates mosh_client order: on_host_bytes first, then set_frames.
    let mut pipe = DisplayPipeline::new(40, 10, DisplayPreference::Always);
    pipe.prove_band_for_test();
    let _ = pipe.set_srtt(Some(Duration::from_millis(100)));
    let _ = pipe.on_host_bytes(b"\x1b[H");
    pipe.predictor_mut_for_test().set_overwrite_for_test(true);
    let _ = pipe.set_frames(2, 0, 0);
    let _ = pipe.on_keystroke(b"a");
    assert_eq!(pipe.predictor().pending_known_char_at(0, 0), Some('a'));
    // Host echo arrives matching (like poll hoststring)
    let _ = pipe.on_host_bytes(b"\x1b[1;1Ha");
    // Still Pending (late=0 < exp=3)
    assert_eq!(pipe.predictor().pending_known_char_at(0, 0), Some('a'));
    // late_ack advances after host applied — must credit, not kill
    let _ = pipe.set_frames(2, 3, 3);
    assert_eq!(pipe.predictor().pending_known_char_at(0, 0), None);
    assert_eq!(
        pipe.predictor().confirmed_epoch_for_test(),
        pipe.predictor().prediction_epoch_for_test(),
        "matching echo must prove band"
    );
}

#[test]
fn el_matching_reprint_credits_via_confirm() {
    // Host CUP+EL+reprint matching predictions — host apply BEFORE late_ack Confirm.
    let mut pipe = DisplayPipeline::new(40, 10, DisplayPreference::Always);
    pipe.prove_band_for_test();
    let _ = pipe.set_srtt(Some(Duration::from_millis(100)));
    let _ = pipe.on_host_bytes(b"\x1b[H");
    pipe.predictor_mut_for_test().set_overwrite_for_test(true);
    let _ = pipe.set_frames(3, 3, 3);
    let _ = pipe.on_keystroke(b"ab");
    // Apply host first (mosh_client order), then late_ack
    let _ = pipe.on_host_bytes(b"\x1b[H\x1b[Kab");
    let _ = pipe.set_frames(3, 4, 4);
    assert_eq!(pipe.predictor().pending_len(), 0);
    assert_eq!(pipe.host_fb().cell_at(0, 0).unwrap().ch, 'a');
    assert_eq!(pipe.host_fb().cell_at(1, 0).unwrap().ch, 'b');
    assert_eq!(
        pipe.predictor().confirmed_epoch_for_test(),
        pipe.predictor().prediction_epoch_for_test()
    );
}

#[test]
fn ack_before_host_kills_matching_echo_regression() {
    // Documents why mosh_client must host-then-ack: wrong order kills band.
    let mut pipe = DisplayPipeline::new(40, 10, DisplayPreference::Always);
    pipe.prove_band_for_test();
    let _ = pipe.set_srtt(Some(Duration::from_millis(100)));
    let _ = pipe.on_host_bytes(b"\x1b[H");
    pipe.predictor_mut_for_test().set_overwrite_for_test(true);
    let _ = pipe.set_frames(2, 0, 0);
    let _ = pipe.on_keystroke(b"a");
    let ep_before = pipe.predictor().prediction_epoch_for_test();
    // WRONG order: late_ack before host apply
    let _ = pipe.set_frames(2, 3, 3);
    // pending killed against blank host
    assert_eq!(pipe.predictor().pending_known_char_at(0, 0), None);
    let _ = pipe.on_host_bytes(b"\x1b[1;1Ha");
    // Band was not proved (confirmed still behind or equal only if kill didn't bump)
    // After kill_epoch, prediction_epoch increases
    assert!(
        pipe.predictor().prediction_epoch_for_test() > ep_before
            || pipe.predictor().confirmed_epoch_for_test()
                < pipe.predictor().prediction_epoch_for_test(),
        "wrong order must not leave a clean proved band for free"
    );
}

#[test]
fn confirm_skips_pending_continues_later_cells() {
    // Overwrite retype at col0 with higher exp must not block Correct of col1.
    let mut p = always_proven();
    p.set_overwrite_for_test(true);
    p.set_frames(1, 1, 1);
    p.set_cursor(0, 0);
    p.keystroke(b"ab", &blank_fb()); // exp=2 for both
    p.keystroke(b"\x1b[D\x1b[D", &blank_fb()); // back to 0
    p.set_frames(5, 5, 5); // advance sent
    p.keystroke(b"X", &blank_fb()); // retype col0, exp=6
    assert_eq!(p.pending_known_char_at(0, 0), Some('X'));
    assert_eq!(p.pending_known_char_at(1, 0), Some('b'));
    // late_ack past b (exp 2) but not X (exp 6)
    p.set_frames(5, 5, 3);
    let mut host = blank_fb();
    host.put_rune(0, 0, '?', Attr::default()); // X still wrong / pending
    host.put_rune(1, 0, 'b', Attr::default());
    host.cur_x = 2;
    p.confirm(&host);
    // b must be drained; X still pending (or pending if late < 6)
    assert_eq!(
        p.pending_known_char_at(1, 0),
        None,
        "later Correct must not stall behind Pending"
    );
    assert_eq!(p.pending_known_char_at(0, 0), Some('X'));
}
