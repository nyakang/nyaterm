use nyaterm_remote_desktop::{DirtyRect, RemoteFrameEvent};

use crate::frame::{DamageTracker, SurfaceError, SurfaceFormat, SurfaceLayout};

fn pixels(frame: RemoteFrameEvent) -> Vec<u8> {
    match frame {
        RemoteFrameEvent::Bitmap { pixels, .. } => pixels.to_vec(),
        _ => panic!("not a bitmap"),
    }
}

#[test]
fn padded_surface_crops_only_the_changed_pixels_and_makes_xrgb_opaque() {
    let layout = SurfaceLayout {
        width: 2,
        height: 2,
        stride: 12,
        format: SurfaceFormat::Xrgb32,
    };
    let source = [
        1, 2, 3, 0, 4, 5, 6, 0, 99, 99, 99, 99, 7, 8, 9, 0, 10, 11, 12, 0,
    ];
    let rect = DirtyRect {
        x: 1,
        y: 0,
        width: 1,
        height: 2,
    };
    assert_eq!(
        pixels(layout.bitmap(&source, rect, 1).unwrap()),
        [4, 5, 6, 255, 10, 11, 12, 255]
    );
}

#[test]
fn negative_stride_surface_has_the_same_logical_orientation() {
    let layout = SurfaceLayout {
        width: 1,
        height: 2,
        stride: -4,
        format: SurfaceFormat::Argb32,
    };
    let source = [4, 5, 6, 255, 1, 2, 3, 255];
    assert_eq!(
        pixels(
            layout
                .bitmap(
                    &source,
                    DirtyRect {
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 2
                    },
                    0
                )
                .unwrap()
        ),
        [1, 2, 3, 255, 4, 5, 6, 255]
    );
}

#[test]
fn rgb555_and_rgb565_convert_full_intensity_channels_correctly() {
    for (format, values) in [
        (SurfaceFormat::Rgb555, [0x7c00u16, 0x03e0, 0x001f]),
        (SurfaceFormat::Rgb565, [0xf800, 0x07e0, 0x001f]),
    ] {
        let source: Vec<_> = values.into_iter().flat_map(u16::to_le_bytes).collect();
        let layout = SurfaceLayout {
            width: 3,
            height: 1,
            stride: 6,
            format,
        };
        assert_eq!(
            pixels(
                layout
                    .bitmap(
                        &source,
                        DirtyRect {
                            x: 0,
                            y: 0,
                            width: 3,
                            height: 1
                        },
                        0
                    )
                    .unwrap()
            ),
            [0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255]
        );
    }
}

#[test]
fn malformed_surfaces_and_rectangles_fail_before_allocating() {
    let layout = SurfaceLayout {
        width: 2,
        height: 2,
        stride: 8,
        format: SurfaceFormat::Argb32,
    };
    let full = DirtyRect {
        x: 0,
        y: 0,
        width: 2,
        height: 2,
    };
    assert_eq!(
        layout.bitmap(&[0; 15], full, 0).unwrap_err(),
        SurfaceError::Truncated
    );
    assert_eq!(
        SurfaceLayout {
            stride: i32::MIN,
            ..layout
        }
        .source_bytes(),
        Err(SurfaceError::Dimensions)
    );
    assert_eq!(
        SurfaceLayout {
            stride: 7,
            ..layout
        }
        .source_bytes(),
        Err(SurfaceError::Stride)
    );
    assert_eq!(
        SurfaceLayout {
            width: u32::MAX,
            ..layout
        }
        .source_bytes(),
        Err(SurfaceError::Dimensions)
    );
    assert_eq!(
        layout
            .bitmap(
                &[0; 16],
                DirtyRect {
                    x: u32::MAX,
                    ..full
                },
                0
            )
            .unwrap_err(),
        SurfaceError::Rectangle
    );
}

#[test]
fn repeated_damage_coalesces_and_large_frames_are_split_under_the_budget() {
    let mut tracker = DamageTracker::new(3840, 2160).unwrap();
    tracker.mark_full();
    for _ in 0..1000 {
        tracker
            .mark(DirtyRect {
                x: 20,
                y: 30,
                width: 10,
                height: 20,
            })
            .unwrap();
    }
    let mut area = 0;
    while let Some(rect) = tracker.take_chunk(1024 * 1024).unwrap() {
        assert!(rect.width as usize * rect.height as usize * 4 <= 1024 * 1024);
        area += u64::from(rect.width) * u64::from(rect.height);
    }
    assert_eq!(area, 3840 * 2160);
    assert!(tracker.is_empty());
}

#[test]
fn saturated_damage_list_becomes_a_complete_refresh() {
    let mut tracker = DamageTracker::new(128, 128).unwrap();
    for x in 0..33 {
        tracker
            .mark(DirtyRect {
                x: x * 3,
                y: 0,
                width: 1,
                height: 1,
            })
            .unwrap();
    }
    assert_eq!(
        tracker.take_chunk(128 * 128 * 4).unwrap(),
        Some(DirtyRect {
            x: 0,
            y: 0,
            width: 128,
            height: 128
        })
    );
    assert!(tracker.is_empty());
}

#[test]
fn rejected_output_can_be_requeued_without_losing_pixels() {
    let mut tracker = DamageTracker::new(100, 100).unwrap();
    tracker.mark_full();
    let first = tracker.take_chunk(4000).unwrap().unwrap();
    tracker.mark(first).unwrap();
    assert_eq!(
        tracker.take_chunk(40_000).unwrap(),
        Some(DirtyRect {
            x: 0,
            y: 0,
            width: 100,
            height: 100
        })
    );
}

#[test]
fn too_small_output_budget_keeps_pending_damage() {
    let mut tracker = DamageTracker::new(100, 100).unwrap();
    tracker.mark_full();
    assert_eq!(tracker.take_chunk(399), Err(SurfaceError::ChunkBudget));
    assert!(!tracker.is_empty());
}
