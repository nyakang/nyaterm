use std::time::{Duration, Instant};

use super::TabReorderMotion;

fn strip(widths: &[f32]) -> (TabReorderMotion, Instant) {
    let now = Instant::now();
    let mut motion = TabReorderMotion::default();
    let order: Vec<_> = (0..widths.len()).map(|i| i.to_string()).collect();
    motion.configure(order.clone(), order);
    let mut x = 0.;
    for (index, width) in widths.iter().enumerate() {
        motion.measure(&index.to_string(), x, *width, now);
        x += width;
    }
    (motion, now)
}

fn order(motion: &TabReorderMotion, source: &str) -> Vec<String> {
    motion.preview(source).unwrap().to_vec()
}

#[test]
fn unequal_tabs_reorder_repeatedly_in_both_directions_without_changing_business_order() {
    let (mut motion, now) = strip(&[120., 200., 150., 90.]);
    motion.begin("0", 30.);
    motion.update(600., now);
    assert_eq!(order(&motion, "0"), ["1", "2", "3", "0"]);
    motion.update(-20., now);
    assert_eq!(order(&motion, "0"), ["0", "1", "2", "3"]);
    motion.update(330., now);
    assert_eq!(order(&motion, "0"), ["1", "2", "0", "3"]);
    assert_eq!(motion.order, ["0", "1", "2", "3"]);
}

#[test]
fn hysteresis_keeps_the_target_stable_near_the_swap_boundary() {
    let (mut motion, now) = strip(&[120., 200.]);
    motion.begin("0", 60.);
    motion.update(169., now);
    assert_eq!(order(&motion, "0"), ["1", "0"]);
    motion.update(155., now);
    assert_eq!(order(&motion, "0"), ["1", "0"]);
    motion.update(151., now);
    assert_eq!(order(&motion, "0"), ["0", "1"]);
}

#[test]
fn retargeting_mid_animation_preserves_the_current_painted_position() {
    let (mut motion, now) = strip(&[120., 200.]);
    motion.begin("0", 60.);
    motion.update(250., now);
    let halfway = now + Duration::from_millis(75);
    let before = motion.offset("1", halfway).0;
    assert!(before < 0. && before > -120.);
    motion.update(0., halfway);
    assert_eq!(motion.offset("1", halfway).0, before);
    assert_eq!(
        motion.offset("1", halfway + Duration::from_millis(150)),
        (0., false)
    );
}

#[test]
fn cancel_or_leave_restores_slots_and_reentry_can_preview_again() {
    let (mut motion, now) = strip(&[120., 200.]);
    motion.begin("0", 60.);
    motion.update(250., now);
    motion.suspend(now);
    assert_eq!(order(&motion, "0"), ["0", "1"]);
    motion.update(250., now);
    assert_eq!(order(&motion, "0"), ["1", "0"]);
    motion.finish(now);
    assert!(motion.preview("0").is_none());
    assert_eq!(
        motion.offset("1", now + Duration::from_millis(150)),
        (0., false)
    );
}

#[test]
fn paused_drop_keeps_the_settled_neighbor_in_place_despite_a_delayed_layout() {
    for (source, pointer, expected) in [("0", 250., ["1", "0"]), ("1", 0., ["1", "0"])] {
        for delay_ms in [0, 16, 50, 150, 300] {
            let (mut motion, now) = strip(&[120., 200.]);
            motion.begin(source, 60.);
            motion.update(pointer, now);
            assert_eq!(order(&motion, source), expected);
            let released = now + Duration::from_millis(500);
            motion.animate_frame(released);
            let neighbor = if source == "0" { "1" } else { "0" };
            let destination = if neighbor == "1" { 0. } else { 200. };
            assert_eq!(
                motion.geometry[neighbor].0 + motion.offset(neighbor, released).0,
                destination
            );
            motion.commit(released);
            assert!(motion.preview(source).is_none());
            // Both the drop handler and hover cleanup can finish the drag.
            motion.finish(released);
            let frame = released + Duration::from_millis(delay_ms);
            motion.finish(frame);
            motion.animate_frame(frame);
            motion.configure(vec!["1".into(), "0".into()], vec!["1".into(), "0".into()]);
            motion.measure("1", 0., 200., frame);
            motion.measure("0", 200., 120., frame);
            assert_eq!(
                motion.offset(neighbor, frame),
                (0., false),
                "source {source}, layout delay {delay_ms} ms"
            );
            assert_eq!(motion.target_offset(neighbor), 0.);
            let settled = frame + Duration::from_millis(150);
            assert!(!motion.animate_frame(settled));
            assert!(motion.motions.is_empty());
        }
    }
}

#[test]
fn dropping_mid_animation_preserves_each_tabs_position_until_the_new_layout() {
    let (mut motion, now) = strip(&[120., 200., 150.]);
    motion.begin("0", 60.);
    motion.update(450., now);
    motion.update(250., now + Duration::from_millis(75));
    assert_eq!(order(&motion, "0"), ["1", "0", "2"]);
    let released = now + Duration::from_millis(100);
    let before = ["0", "1", "2"].map(|id| {
        let (base, width) = motion.geometry[id];
        (id, base + motion.offset(id, released).0, width)
    });
    // Tab 2 is still returning from its earlier preview, although its final
    // layout origin is unchanged. It must also resume and settle after commit.
    assert!(before[2].1 < 320.);
    motion.set_pointer(Some(250.));
    motion.commit(released);
    assert!(motion.pointer().is_none());
    assert!(
        motion
            .advance_scroll(0., 200., 0., 270., released)
            .is_none()
    );
    let frame = released + Duration::from_millis(200);
    motion.finish(frame);
    motion.animate_frame(frame);
    let committed = vec!["1".into(), "0".into(), "2".into()];
    motion.configure(committed.clone(), committed);
    for ((id, position, width), destination) in before.into_iter().zip([200., 0., 320.]) {
        motion.measure(id, destination, width, frame);
        assert!((destination + motion.offset(id, frame).0 - position).abs() < 0.001);
        assert_eq!(motion.target_offset(id), 0.);
        let mut distance = (position - destination).abs();
        for elapsed in [16, 50, 100, 150] {
            let next = motion
                .offset(id, frame + Duration::from_millis(elapsed))
                .0
                .abs();
            assert!(next <= distance);
            distance = next;
        }
        assert_eq!(distance, 0.);
    }
    assert!(!motion.animate_frame(frame + Duration::from_millis(150)));
    assert!(motion.motions.is_empty());
}

#[test]
fn membership_change_invalidates_the_preview() {
    let (mut motion, now) = strip(&[120., 200.]);
    motion.begin("0", 60.);
    motion.update(250., now);
    motion.configure(vec!["1".into()], vec!["1".into()]);
    assert!(motion.preview("0").is_none());
}

#[test]
fn edge_scroll_continues_without_mouse_moves_and_stops_at_the_content_boundary() {
    let (mut motion, now) = strip(&[120., 200.]);
    motion.begin("0", 60.);
    motion.set_pointer(Some(199.));
    let first = motion.advance_scroll(0., 200., 0., 120., now).unwrap();
    let second = motion
        .advance_scroll(0., 200., first, 120., now + Duration::from_millis(16))
        .unwrap();
    assert!(second < first);
    assert!(
        motion
            .advance_scroll(0., 200., -120., 120., now + Duration::from_millis(32))
            .is_none()
    );
    motion.suspend(now + Duration::from_millis(32));
    assert!(
        motion
            .advance_scroll(0., 200., second, 120., now + Duration::from_millis(48))
            .is_none()
    );
}
