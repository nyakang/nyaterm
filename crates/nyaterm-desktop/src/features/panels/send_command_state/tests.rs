use std::sync::atomic::Ordering;

use gpui::TestAppContext;

use crate::send_command::{SendCommandControlFocus, SendCommandDataType, SendCommandMode};

use super::SendCommandFeatureState;

fn send_command_state(cx: &TestAppContext) -> SendCommandFeatureState {
    cx.update(SendCommandFeatureState::new)
}

#[test]
fn send_tabs_keep_payload_options_and_identity_independent() {
    let cx = TestAppContext::single();
    let mut state = send_command_state(&cx);
    cx.update(|cx| {
        let first = state.active_id();
        state.apply_draft("first".into());
        state.set_target(crate::send_command::SendCommandTarget::AllSessions);
        state.apply_control_input(SendCommandControlFocus::Count, "∞".into());
        let hex = state.presentation(cx).hex;
        hex.update(cx, |hex, cx| {
            hex.set_bytes(vec![1, 2], cx);
            hex.select_all(cx);
        });
        let second = state.add_tab(cx);
        assert_ne!(first, second);
        assert_eq!(state.presentation(cx).draft, "");
        assert_eq!(state.presentation(cx).count_input, "1");
        assert_eq!(state.presentation(cx).interval_input, "1.00");
        assert_eq!(
            state.target(),
            &crate::send_command::SendCommandTarget::Current
        );
        assert_ne!(state.presentation(cx).hex.entity_id(), hex.entity_id());
        state.apply_draft("second".into());
        state.rename_tab(first, "primary");
        state.switch_tab(first);
        assert_eq!(state.presentation(cx).draft, "first");
        assert_eq!(state.presentation(cx).count_input, "∞");
        assert_eq!(state.tab_name(first), Some("primary"));
        assert!(hex.read(cx).document().selection().is_some());
        assert!(state.tab_has_draft(first, cx));
        state.switch_tab(second);
        assert_eq!(state.presentation(cx).draft, "second");
    });
}

#[test]
fn send_tabs_close_right_neighbor_then_left_without_reusing_numbers() {
    let cx = TestAppContext::single();
    let mut state = send_command_state(&cx);
    cx.update(|cx| {
        let first = state.active_id();
        let second = state.add_tab(cx);
        let third = state.add_tab(cx);
        state.switch_tab(second);
        assert!(state.close_tabs(&[second]));
        assert_eq!(state.active_id(), third);
        assert!(state.close_tabs(&[third]));
        assert_eq!(state.active_id(), first);
        assert!(!state.close_tabs(&[first]));
        let fourth = state.add_tab(cx);
        assert_eq!(state.tab_name(fourth), Some("4"));
        assert!(state.close_tabs(&[first]));
        assert_eq!(state.active_id(), fourth);
        assert_eq!(state.tab_labels().len(), 1);
    });
}

#[test]
fn duplicated_send_tabs_copy_provenance_but_not_entities_or_runs() {
    let cx = TestAppContext::single();
    let mut state = send_command_state(&cx);
    cx.update(|cx| {
        let first = state.active_id();
        state.fill_plugin_draft(
            "plugin".into(),
            nyaterm_core::command_draft::DraftOrigin::Plugin {
                plugin_id: "p".into(),
                action_id: "a".into(),
                revision: 1,
            },
            true,
        );
        let source = state.presentation(cx);
        source
            .hex
            .update(cx, |hex, cx| hex.set_bytes(vec![0xff], cx));
        state.begin_send(1);
        state.set_run_id(42);
        let copy = state.duplicate_tab(first, cx).unwrap();
        let snapshot = state.presentation(cx);
        assert_ne!(copy, first);
        assert_eq!(snapshot.draft, source.draft);
        assert_eq!(snapshot.provenance, source.provenance);
        assert_ne!(snapshot.hex.entity_id(), source.hex.entity_id());
        assert!(!snapshot.sending);
        assert!(state.tab_is_sending(first));
        assert_eq!(state.run_id(), None);
        snapshot.hex.update(cx, |hex, cx| hex.clear(cx));
        assert_eq!(source.hex.read(cx).document().bytes(), &[0xff]);
        assert!(!state.can_close_tabs(&[first]));
        assert!(state.can_close_tabs(&[copy]));
        state.finish_send(first, 42);
    });
}

#[test]
fn send_completion_is_bound_to_source_and_rejects_stale_run_ids() {
    let cx = TestAppContext::single();
    let mut state = send_command_state(&cx);
    cx.update(|cx| {
        let first = state.active_id();
        state.apply_draft("first".into());
        state.begin_send(1);
        state.set_run_id(10);
        let second = state.add_tab(cx);
        state.apply_draft("second".into());
        assert!(!state.clear_sent_tab(second, 10, true, true, "second", cx));
        assert!(!state.clear_sent_tab(first, 9, true, true, "first", cx));
        assert!(!state.clear_sent_tab(first, 10, true, false, "first", cx));
        assert!(state.clear_sent_tab(first, 10, true, true, "first", cx));
        assert_eq!(state.presentation(cx).draft, "second");
        state.finish_send(first, 10);
        state.begin_send(1);
        state.set_run_id(11);
        assert!(!state.matches_run(first, 10));
        assert!(state.matches_run(second, 11));
        assert!(!state.clear_sent_tab(first, 10, true, true, "", cx));
        state.finish_send(second, 11);
    });
}

#[test]
fn removed_send_tab_releases_hex_entity_and_ignores_late_input() {
    let cx = TestAppContext::single();
    let mut state = send_command_state(&cx);
    let first = state.active_id();
    for _ in 0..100 {
        let weak = cx.update(|cx| {
            let id = state.add_tab(cx);
            let weak = state.presentation(cx).hex.downgrade();
            assert!(state.close_tabs(&[id]));
            assert!(!state.apply_tab_draft(id, "late".into()));
            assert_eq!(state.active_id(), first);
            weak
        });
        cx.run_until_parked();
        assert!(weak.upgrade().is_none());
    }
    assert_eq!(state.tab_labels().len(), 1);
}

#[test]
fn explicit_targets_have_one_owner_and_cannot_change_during_send() {
    use crate::send_command::{SendCommandSelection, SendCommandTarget};
    use nyaterm_transport::SessionKind;
    let cx = TestAppContext::single();
    let mut state = send_command_state(&cx);
    state.set_target(SendCommandTarget::Sessions(SendCommandSelection::default()));
    state.toggle_target_session("one".into(), SessionKind::Ssh);
    state.toggle_target_session("two".into(), SessionKind::Telnet);
    let target = state.target().clone();
    state.set_target_picker_open(true);
    state.begin_send(2);
    state.set_run_id(1);
    assert!(!state.target_picker_open());
    state.toggle_target_session("one".into(), SessionKind::Ssh);
    assert!(!state.set_target(SendCommandTarget::Current));
    assert_eq!(state.target(), &target);
    state.finish_send(state.active_id(), 1);
    assert!(state.set_target(SendCommandTarget::Current));
    state.set_target(SendCommandTarget::Sessions(SendCommandSelection::default()));
    assert_eq!(
        state.target(),
        &SendCommandTarget::Sessions(SendCommandSelection::default())
    );
}

#[test]
fn composer_statistics_refresh_on_edits_clears_and_hex_decoding() {
    let cx = TestAppContext::single();
    let mut state = send_command_state(&cx);
    cx.update(|cx| {
        state.apply_draft("中\r\nlast".into());
        let stats = state
            .payload_statistics(Some(nyaterm_transport::SessionKind::Ssh), cx)
            .unwrap();
        assert_eq!((stats.units, stats.bytes), (2, 9));
        state.set_mode(SendCommandMode::Character);
        let stats = state
            .payload_statistics(Some(nyaterm_transport::SessionKind::Ssh), cx)
            .unwrap();
        assert_eq!((stats.units, stats.bytes), (6, 8));
        state.clear_draft(cx);
        assert_eq!(state.payload_statistics(None, cx).unwrap().units, 0);
        state.fill_plugin_draft(
            "next".into(),
            nyaterm_core::command_draft::DraftOrigin::User,
            true,
        );
        assert_eq!(state.payload_statistics(None, cx).unwrap().units, 4);
        state.set_data_type(SendCommandDataType::Hex);
        state
            .active_mut()
            .composer
            .hex
            .update(cx, |hex, cx| hex.set_bytes(b"back\n".to_vec(), cx));
        state.decode_utf8(cx).unwrap();
        let stats = state.payload_statistics(None, cx).unwrap();
        assert_eq!((stats.units, stats.bytes), (5, 5));
    });
}

#[test]
fn send_command_owner_keeps_data_and_mode_compatible() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = send_command_state(&cx);

    assert_eq!(
        state.set_data_type(SendCommandDataType::Hex),
        Some("0.02".to_string())
    );
    let presentation = cx.update(|cx| state.presentation(cx));
    assert_eq!(presentation.data_type, SendCommandDataType::Hex);
    assert_eq!(presentation.mode, SendCommandMode::Byte);

    assert_eq!(
        state.set_mode(SendCommandMode::Packet),
        Some("0.00".to_string())
    );
    assert_eq!(
        state.set_data_type(SendCommandDataType::Text),
        Some("1.00".to_string())
    );
    assert_eq!(
        cx.update(|cx| state.presentation(cx).mode),
        SendCommandMode::Line
    );
}

#[test]
fn send_command_owner_normalizes_control_edits_and_infinity_count() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = send_command_state(&cx);

    assert!(state.apply_control_input(SendCommandControlFocus::Count, "∞".to_string()));
    assert_eq!(
        state.synced_control_input(SendCommandControlFocus::Count),
        "∞"
    );
    assert!(state.apply_control_input(SendCommandControlFocus::Count, "25".to_string()));
    assert_eq!(
        state.synced_control_input(SendCommandControlFocus::Count),
        "25"
    );

    assert!(state.apply_control_input(SendCommandControlFocus::Interval, "999".to_string()));
    cx.update(|cx| assert!(state.payload_statistics(None, cx).is_err()));
    assert_eq!(
        state.synced_control_input(SendCommandControlFocus::Interval),
        "1.00"
    );
    assert!(state.apply_control_input(SendCommandControlFocus::Interval, "0.001".to_string()));
    assert_eq!(
        state.synced_control_input(SendCommandControlFocus::Interval),
        "0.001"
    );
}

#[test]
fn drafts_and_modes_survive_switching_and_clear_only_sent_payload() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    cx.update(|cx| {
        let mut state = SendCommandFeatureState::new(cx);
        state.apply_draft("AT+CSQ".to_string());
        state.set_mode(SendCommandMode::Character);
        state.set_data_type(SendCommandDataType::Hex);
        state
            .active_mut()
            .composer
            .hex
            .update(cx, |hex, cx| hex.paste("01 FF", cx));
        state.set_mode(SendCommandMode::Packet);
        state.set_data_type(SendCommandDataType::Text);
        assert_eq!(state.presentation(cx).draft, "AT+CSQ");
        assert_eq!(state.presentation(cx).mode, SendCommandMode::Character);
        state.set_data_type(SendCommandDataType::Hex);
        assert_eq!(state.presentation(cx).draft, "01 FF");
        assert_eq!(state.presentation(cx).mode, SendCommandMode::Packet);
        assert!(!state.clear_sent_draft(false, true, "01 FF", cx));
        assert!(!state.clear_sent_draft(true, false, "01 FF", cx));
        state
            .active_mut()
            .composer
            .hex
            .update(cx, |hex, cx| hex.paste("AB", cx));
        assert!(!state.clear_sent_draft(true, true, "01 FF", cx));
        assert!(state.clear_sent_draft(true, true, "01 FF AB", cx));
        state.set_data_type(SendCommandDataType::Text);
        assert_eq!(state.presentation(cx).draft, "AT+CSQ");
    });
}

#[test]
fn explicit_utf8_conversion_validates_before_replacing_text() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    cx.update(|cx| {
        let mut state = SendCommandFeatureState::new(cx);
        state.apply_draft("中a".to_string());
        assert!(state.convert_to_hex(cx));
        assert_eq!(
            state.active_mut().composer.hex.read(cx).document().bytes(),
            "中a".as_bytes()
        );
        state
            .active_mut()
            .composer
            .hex
            .update(cx, |hex, cx| hex.set_bytes(vec![255], cx));
        assert!(state.decode_utf8(cx).is_err());
        state.set_data_type(SendCommandDataType::Text);
        assert_eq!(state.presentation(cx).draft, "中a");
        state.set_data_type(SendCommandDataType::Hex);
        state
            .active_mut()
            .composer
            .hex
            .update(cx, |hex, cx| hex.set_bytes(b"Hello".to_vec(), cx));
        assert!(state.decode_utf8(cx).is_ok());
        assert_eq!(state.presentation(cx).draft, "Hello");
    });
}

#[test]
fn send_command_owner_finishes_progress_and_releases_cancel_atomically() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = send_command_state(&cx);

    let run = state.begin_send(3);
    state.set_run_id(1);
    assert!(!run.infinite);
    assert_eq!(run.rounds, 1);
    assert!(state.is_sending());
    assert!(!state.set_target(crate::send_command::SendCommandTarget::AllSessions));
    assert_eq!(state.set_data_type(SendCommandDataType::Hex), None);
    state.set_progress_round(state.active_id(), 1, 1);
    state.complete_progress_units(state.active_id(), 1, 1);
    state.complete_progress_units(state.active_id(), 1, 1);
    assert!(state.request_cancel());
    assert!(run.cancel.load(Ordering::SeqCst));

    let progress = state.finish_send(state.active_id(), 1).unwrap();
    assert_eq!(progress.completed, 2);
    assert_eq!(progress.total, 3);
    assert_eq!(progress.round, 1);
    assert!(!state.is_sending());
    assert!(!state.request_cancel());
}

#[test]
fn send_tabs_are_isolated_between_workspace_owners() {
    let cx = TestAppContext::single();
    cx.update(|cx| {
        let mut first = SendCommandFeatureState::new(cx);
        let second = SendCommandFeatureState::new(cx);
        assert_ne!(
            first.presentation(cx).hex.entity_id(),
            second.presentation(cx).hex.entity_id()
        );
        first.apply_draft("private to window one".into());
        first.add_tab(cx);
        assert_eq!(first.tab_labels().len(), 2);
        assert_eq!(second.tab_labels().len(), 1);
        assert!(second.presentation(cx).draft.is_empty());
        assert!(!second.is_sending());
    });
}

#[test]
fn concurrent_send_tabs_keep_progress_cancel_and_stale_completion_independent() {
    let cx = TestAppContext::single();
    let mut state = send_command_state(&cx);
    cx.update(|cx| {
        let first = state.active_id();
        let first_run = state.begin_send(3);
        state.set_run_id(10);
        let second = state.add_tab(cx);
        let second_run = state.begin_send(5);
        state.set_run_id(11);
        state.set_progress_round(first, 10, 1);
        state.complete_progress_units(first, 10, 2);
        state.complete_progress_units(second, 11, 1);
        assert_eq!(state.presentation(cx).completed, 1);
        assert_eq!(state.presentation(cx).total, 5);
        assert_eq!(state.progress_result(first, 10).unwrap().completed, 2);
        assert!(state.request_cancel());
        assert!(second_run.cancel.load(Ordering::SeqCst));
        assert!(!first_run.cancel.load(Ordering::SeqCst));
        assert!(!state.can_close_tabs(&[first]));
        assert!(!state.can_close_tabs(&[second]));
        assert!(state.finish_send(second, 11).is_some());
        assert!(state.can_close_tabs(&[second]));
        assert!(state.tab_is_sending(first));
        state.begin_send(7);
        state.set_run_id(12);
        state.complete_progress_units(second, 11, 100);
        state.set_progress_round(second, 11, 100);
        assert!(state.finish_send(second, 11).is_none());
        assert_eq!(state.presentation(cx).completed, 0);
        assert_eq!(state.presentation(cx).round, 0);
        assert!(state.matches_run(second, 12));
        state.finish_send(first, 10);
        assert!(state.tab_is_sending(second));
        state.finish_send(second, 12);
        assert!(!state.is_sending());
    });
}
