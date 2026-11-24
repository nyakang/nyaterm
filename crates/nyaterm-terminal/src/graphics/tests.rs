use super::{
    GRAPHICS_PENDING_LIMIT, GraphicsCursorMotion, GraphicsEvent, GraphicsIngress,
    GraphicsPlacement, GraphicsProtocol, GraphicsScreenKind, GraphicsSegment, KittyDeleteMode,
    MAX_IMAGE_BYTES, MAX_KITTY_PENDING_BYTES, MAX_KITTY_STORED_IMAGES, TerminalGraphicsState,
};

fn placement(line: i32, col: usize, width_cells: usize, height_cells: usize) -> GraphicsPlacement {
    GraphicsPlacement {
        id: 1,
        protocol: GraphicsProtocol::Kitty,
        line: i64::from(line),
        screen: GraphicsScreenKind::Primary,
        col,
        width_cells,
        height_cells,
        data: b"image".to_vec().into(),
        content_id: 1,
        source_rect: None,
        name: None,
        kitty_id: None,
        placement_id: None,
        z_index: 0,
        above_text: false,
    }
}

fn kitty_image_event(id: u32, data: Vec<u8>, place: bool) -> GraphicsEvent {
    GraphicsEvent::Image {
        protocol: GraphicsProtocol::Kitty,
        id: Some(id),
        placement_id: None,
        width_cells: Some(1),
        height_cells: Some(1),
        data,
        name: None,
        more: false,
        place,
        z_index: 0,
        above_text: false,
        cursor_motion: false,
        format: None,
        compressed: false,
        pixel_width: None,
        pixel_height: None,
        quiet: None,
        source_rect: None,
    }
}

/// A stream of SGR/CSI escapes carries no graphics introducer, so it must
/// leave the ingress whole. Splitting per escape used to cost one `Vec`
/// plus a full decode/parse round trip for every colour code a TUI paints.
#[test]
fn plain_csi_escapes_do_not_split_the_stream() {
    let mut ingress = GraphicsIngress::new();
    let seq = b"\x1b[31mred\x1b[0m \x1b[1;32mgreen\x1b[0m\x1b[2K\x1b[H";
    let segments = ingress.advance(seq);
    assert_eq!(
        segments,
        vec![GraphicsSegment::Terminal(seq.to_vec())],
        "CSI-only output should stay in a single terminal segment"
    );
}

#[test]
fn advance_with_borrows_plain_terminal_input() {
    let mut ingress = GraphicsIngress::new();
    let seq = b"\x1b[31mred\x1b[0m plain text";
    let mut terminal_segments = Vec::new();
    let mut events = 0;

    ingress.advance_with(
        seq,
        |bytes| terminal_segments.push(bytes.as_ptr()),
        |_| events += 1,
    );

    assert_eq!(events, 0);
    assert_eq!(terminal_segments, vec![seq.as_ptr()]);
}

/// Graphics sequences still split, and the plain bytes on either side —
/// escapes included — coalesce into one segment each.
#[test]
fn csi_runs_around_a_graphics_sequence_coalesce() {
    let mut ingress = GraphicsIngress::new();
    let seq = b"\x1b[31mhi\x1b[0m\x1b]1337;ClearScrollback\x07\x1b[1myo\x1b[0m";
    let segments = ingress.advance(seq);
    assert_eq!(
        segments,
        vec![
            GraphicsSegment::Terminal(b"\x1b[31mhi\x1b[0m".to_vec()),
            GraphicsSegment::Event(GraphicsEvent::ClearScrollback),
            GraphicsSegment::Terminal(b"\x1b[1myo\x1b[0m".to_vec()),
        ]
    );
}

/// A chunk ending on a bare ESC can still turn out to be a graphics
/// introducer, so it stays held until the next chunk decides.
#[test]
fn trailing_bare_escape_is_held_until_the_next_chunk() {
    let mut ingress = GraphicsIngress::new();
    assert_eq!(
        ingress.advance(b"hi\x1b"),
        vec![GraphicsSegment::Terminal(b"hi".to_vec())],
        "the dangling ESC must not be emitted yet"
    );
    assert_eq!(
        ingress.advance(b"[0mbye"),
        vec![GraphicsSegment::Terminal(b"\x1b[0mbye".to_vec())],
        "the held ESC rejoins the next chunk"
    );
}

#[test]
fn strips_iterm2_inline_and_keeps_surrounding_text() {
    let mut ingress = GraphicsIngress::new();
    // "hi" + OSC 1337 inline tiny payload + "yo"
    // base64("PNG") = UE5H — not a real png but enough for parse path
    let seq = b"hi\x1b]1337;File=name=x.png;width=4;height=2;inline=1:UE5H\x07yo";
    let segments = ingress.advance(seq);
    assert_eq!(segments[0], GraphicsSegment::Terminal(b"hi".to_vec()));
    match &segments[1] {
        GraphicsSegment::Event(GraphicsEvent::Image {
            protocol: GraphicsProtocol::ITerm2,
            width_cells: Some(4),
            height_cells: Some(2),
            data,
            ..
        }) => {
            assert_eq!(data, b"PNG");
        }
        other => panic!("unexpected segment: {other:?}"),
    }
    assert_eq!(segments[2], GraphicsSegment::Terminal(b"yo".to_vec()));
}

#[test]
fn iterm2_file_without_inline_strips_without_image() {
    let mut ingress = GraphicsIngress::new();
    let seq = b"hi\x1b]1337;File=name=x.png;width=4;height=2:UE5H\x07yo";
    let segments = ingress.advance(seq);
    assert_eq!(
        segments,
        vec![
            GraphicsSegment::Terminal(b"hi".to_vec()),
            GraphicsSegment::Terminal(b"yo".to_vec())
        ]
    );
}

#[test]
fn iterm2_clear_scrollback_emits_event() {
    let mut ingress = GraphicsIngress::new();
    let seq = b"hi\x1b]1337;ClearScrollback\x07yo";
    let segments = ingress.advance(seq);
    assert_eq!(
        segments,
        vec![
            GraphicsSegment::Terminal(b"hi".to_vec()),
            GraphicsSegment::Event(GraphicsEvent::ClearScrollback),
            GraphicsSegment::Terminal(b"yo".to_vec())
        ]
    );
}

#[test]
fn osc_1337_prefix_with_extra_digit_passes_through() {
    let mut ingress = GraphicsIngress::new();
    let seq = b"hi\x1b]13370;not-iterm\x07yo";
    let segments = ingress.advance(seq);
    let terminal: Vec<u8> = segments
        .into_iter()
        .flat_map(|segment| match segment {
            GraphicsSegment::Terminal(bytes) => bytes,
            other => panic!("unexpected graphics segment: {other:?}"),
        })
        .collect();
    assert_eq!(terminal, seq);
}

#[test]
fn strips_kitty_apc_transmit() {
    let mut ingress = GraphicsIngress::new();
    // a=t,i=1,c=8,r=4 ; base64("AB")
    let seq = b"\x1b_Ga=t,i=1,c=8,r=4;QUI=\x1b\\";
    let segments = ingress.advance(seq);
    assert_eq!(segments.len(), 1);
    match &segments[0] {
        GraphicsSegment::Event(GraphicsEvent::Image {
            protocol: GraphicsProtocol::Kitty,
            id: Some(1),
            width_cells: Some(8),
            height_cells: Some(4),
            data,
            ..
        }) => assert_eq!(data, b"AB"),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn holds_incomplete_sequence_across_chunks() {
    let mut ingress = GraphicsIngress::new();
    let first = ingress.advance(b"pre\x1b_Ga=t,i=2;");
    assert_eq!(first, vec![GraphicsSegment::Terminal(b"pre".to_vec())]);
    let second = ingress.advance(b"QUI=\x1b\\post");
    assert!(matches!(
        second[0],
        GraphicsSegment::Event(GraphicsEvent::Image { .. })
    ));
    assert_eq!(second[1], GraphicsSegment::Terminal(b"post".to_vec()));
}

#[test]
fn oversized_incomplete_graphics_sequence_passes_through() {
    let mut ingress = GraphicsIngress::new();
    let mut seq = b"pre\x1b_Ga=t,i=1;".to_vec();
    seq.resize(seq.len() + GRAPHICS_PENDING_LIMIT + 1, b'A');
    let first = ingress.advance(&seq);
    assert!(
        first
            .iter()
            .all(|segment| matches!(segment, GraphicsSegment::Terminal(_)))
    );
    let second = ingress.advance(b"QQ==\x1b\\post");
    assert!(
        second
            .iter()
            .all(|segment| matches!(segment, GraphicsSegment::Terminal(_)))
    );
    let bytes: Vec<u8> = first
        .into_iter()
        .chain(second)
        .flat_map(|segment| match segment {
            GraphicsSegment::Terminal(bytes) => bytes,
            GraphicsSegment::Event(_) => unreachable!("checked above"),
        })
        .collect();
    assert!(bytes.starts_with(&seq));
    assert!(bytes.ends_with(b"QQ==\x1b\\post"));
}

#[test]
fn sixel_dcs_is_stripped() {
    let mut ingress = GraphicsIngress::new();
    let segments = ingress.advance(b"a\x1bPq#0;2;0;0;0#1~~~\x1b\\b");
    assert_eq!(segments[0], GraphicsSegment::Terminal(b"a".to_vec()));
    match &segments[1] {
        GraphicsSegment::Event(GraphicsEvent::Image {
            protocol: GraphicsProtocol::Sixel,
            data,
            place: true,
            ..
        }) => {
            assert!(data.contains(&b'q'));
            assert!(!data.is_empty());
        }
        other => panic!("expected sixel event: {other:?}"),
    }
    assert_eq!(segments[2], GraphicsSegment::Terminal(b"b".to_vec()));
}

#[test]
fn non_sixel_dcs_with_q_passes_through() {
    let mut ingress = GraphicsIngress::new();
    let seq = b"\x1bPabcq\x1b\\";
    let segments = ingress.advance(seq);
    assert!(
        segments
            .iter()
            .all(|segment| matches!(segment, GraphicsSegment::Terminal(_)))
    );
    let bytes: Vec<u8> = segments
        .into_iter()
        .flat_map(|segment| match segment {
            GraphicsSegment::Terminal(bytes) => bytes,
            GraphicsSegment::Event(_) => unreachable!("checked above"),
        })
        .collect();
    assert_eq!(bytes, seq);
}

#[test]
fn sixel_is_rasterized_into_snapshot() {
    let mut state = TerminalGraphicsState::default();
    // Solid red 1x6 column.
    let body = b"0;0;0q#0;2;100;0;0#0~";
    state.handle(
        GraphicsEvent::Image {
            protocol: GraphicsProtocol::Sixel,
            id: None,
            placement_id: None,
            width_cells: None,
            height_cells: None,
            data: body.to_vec(),
            name: None,
            more: false,
            place: true,
            z_index: 0,
            above_text: false,
            cursor_motion: false,
            format: None,
            compressed: false,
            pixel_width: None,
            pixel_height: None,
            quiet: None,
            source_rect: None,
        },
        0,
        0,
        80,
        9,
        18,
    );
    let images = state.viewport_images(0, 24, 80);
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].protocol, GraphicsProtocol::Sixel);
    assert!(images[0].data.starts_with(b"NYAR"));
    assert!(images[0].width_cells >= 1);
    assert!(images[0].height_cells >= 1);
    let (w, h) = crate::sixel::nyar_dimensions(&images[0].data).expect("nyar");
    assert!(w >= 1 && h >= 6);
    assert_eq!(&images[0].data[12..16], &[255, 0, 0, 255]);
}

#[test]
fn viewport_images_keep_full_size_when_top_clipped() {
    let mut state = TerminalGraphicsState::default();
    state.placements.push(placement(-2, 4, 6, 5));

    let images = state.viewport_images(0, 4, 20);

    assert_eq!(images.len(), 1);
    assert_eq!(images[0].row, 0);
    assert_eq!(images[0].col, 4);
    assert_eq!(images[0].width_cells, 6);
    assert_eq!(images[0].height_cells, 3);
    assert_eq!(images[0].image_width_cells, 6);
    assert_eq!(images[0].image_height_cells, 5);
    assert_eq!(images[0].source_row_cells, 2);
    assert_eq!(images[0].source_col_cells, 0);
}

#[test]
fn viewport_images_keep_full_size_when_bottom_or_right_clipped() {
    let mut state = TerminalGraphicsState::default();
    state.placements.push(placement(2, 8, 6, 5));

    let images = state.viewport_images(0, 4, 10);

    assert_eq!(images.len(), 1);
    assert_eq!(images[0].row, 2);
    assert_eq!(images[0].col, 8);
    assert_eq!(images[0].width_cells, 2);
    assert_eq!(images[0].height_cells, 2);
    assert_eq!(images[0].image_width_cells, 6);
    assert_eq!(images[0].image_height_cells, 5);
    assert_eq!(images[0].source_row_cells, 0);
    assert_eq!(images[0].source_col_cells, 0);
}

#[test]
fn non_graphics_osc_passes_through() {
    let mut ingress = GraphicsIngress::new();
    let seq = b"\x1b]0;title\x07";
    let segments = ingress.advance(seq);
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0], GraphicsSegment::Terminal(seq.to_vec()));
}

#[test]
fn assembles_kitty_multi_chunk_payload() {
    let mut state = TerminalGraphicsState::default();
    // Two chunks: "AB" + "CD" base64 QUI= and Q0Q=
    state.handle(
        GraphicsEvent::Image {
            protocol: GraphicsProtocol::Kitty,
            id: Some(9),
            placement_id: None,
            width_cells: Some(4),
            height_cells: Some(2),
            data: b"AB".to_vec(),
            name: None,
            more: true,
            place: true,
            z_index: 0,
            above_text: false,
            cursor_motion: false,
            format: None,
            compressed: false,
            pixel_width: None,
            pixel_height: None,
            quiet: None,
            source_rect: None,
        },
        0,
        0,
        80,
        9,
        18,
    );
    assert!(state.viewport_images(0, 24, 80).is_empty());
    state.handle(
        GraphicsEvent::Image {
            protocol: GraphicsProtocol::Kitty,
            id: Some(9),
            placement_id: None,
            width_cells: None,
            height_cells: None,
            data: b"CD".to_vec(),
            name: None,
            more: false,
            place: true,
            z_index: 0,
            above_text: false,
            cursor_motion: false,
            format: None,
            compressed: false,
            pixel_width: None,
            pixel_height: None,
            quiet: None,
            source_rect: None,
        },
        0,
        1,
        80,
        9,
        18,
    );
    let images = state.viewport_images(0, 24, 80);
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].data.as_ref(), b"ABCD");
    assert_eq!(images[0].width_cells, 4);
    assert_eq!(images[0].height_cells, 2);
    assert_eq!(images[0].col, 1);
}

#[test]
fn place_only_reuses_stored_kitty_image() {
    let mut state = TerminalGraphicsState::default();
    // Transmit only (no place).
    state.handle(
        GraphicsEvent::Image {
            protocol: GraphicsProtocol::Kitty,
            id: Some(3),
            placement_id: None,
            width_cells: Some(2),
            height_cells: Some(2),
            data: b"XY".to_vec(),
            name: None,
            more: false,
            place: false,
            z_index: 0,
            above_text: false,
            cursor_motion: false,
            format: None,
            compressed: false,
            pixel_width: None,
            pixel_height: None,
            quiet: None,
            source_rect: None,
        },
        0,
        0,
        80,
        9,
        18,
    );
    assert!(state.viewport_images(0, 24, 80).is_empty());
    // Place-only at a different cursor.
    state.handle(
        GraphicsEvent::Image {
            protocol: GraphicsProtocol::Kitty,
            id: Some(3),
            placement_id: None,
            width_cells: None,
            height_cells: None,
            data: Vec::new(),
            name: None,
            more: false,
            place: true,
            z_index: 0,
            above_text: true,
            cursor_motion: false,
            format: None,
            compressed: false,
            pixel_width: None,
            pixel_height: None,
            quiet: None,
            source_rect: None,
        },
        2,
        5,
        80,
        9,
        18,
    );
    let images = state.viewport_images(0, 24, 80);
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].data.as_ref(), b"XY");
    assert_eq!(images[0].col, 5);
    assert_eq!(images[0].row, 2);
    assert!(images[0].above_text);
}

#[test]
fn kitty_store_prunes_old_unplaced_images() {
    let mut state = TerminalGraphicsState::default();
    for id in 1..=(MAX_KITTY_STORED_IMAGES as u32 + 4) {
        state.handle(
            kitty_image_event(id, vec![id as u8], false),
            0,
            0,
            80,
            9,
            18,
        );
    }

    assert_eq!(state.kitty_store.len(), MAX_KITTY_STORED_IMAGES);
    assert!(
        !state.kitty_query_ok(GraphicsScreenKind::Primary, Some(1), None),
        "old unplaced image should be evicted"
    );
    assert!(
        state.kitty_query_ok(
            GraphicsScreenKind::Primary,
            Some(MAX_KITTY_STORED_IMAGES as u32 + 4),
            None,
        ),
        "newest image should remain queryable"
    );
}

#[test]
fn kitty_store_prune_prefers_unreferenced_images() {
    let mut state = TerminalGraphicsState::default();
    state.handle(
        kitty_image_event(1, b"placed".to_vec(), true),
        0,
        0,
        80,
        9,
        18,
    );
    for id in 2..=(MAX_KITTY_STORED_IMAGES as u32 + 4) {
        state.handle(
            kitty_image_event(id, vec![id as u8], false),
            0,
            0,
            80,
            9,
            18,
        );
    }

    assert_eq!(state.kitty_store.len(), MAX_KITTY_STORED_IMAGES);
    assert!(
        state.kitty_query_ok(GraphicsScreenKind::Primary, Some(1), None),
        "store for a live placement should be retained before unplaced images"
    );
    assert_eq!(state.viewport_images(0, 24, 80).len(), 1);
}

#[test]
fn kitty_store_prune_after_placement_retains_newest_placed_image() {
    let mut state = TerminalGraphicsState::default();
    for id in 1..=(MAX_KITTY_STORED_IMAGES as u32 + 1) {
        state.handle(kitty_image_event(id, vec![id as u8], true), 0, 0, 80, 9, 18);
    }

    assert_eq!(state.kitty_store.len(), MAX_KITTY_STORED_IMAGES);
    assert!(
        !state.kitty_store.contains_key(&1),
        "oldest placement should be dropped before store pruning chooses a victim"
    );
    assert!(
        state
            .kitty_store
            .contains_key(&(MAX_KITTY_STORED_IMAGES as u32 + 1)),
        "new transmit+place payload should stay reusable after pruning"
    );
}

#[test]
fn oversized_kitty_image_is_rejected_without_truncating_or_replacing() {
    let mut state = TerminalGraphicsState::default();
    state.handle(kitty_image_event(7, vec![1], true), 0, 0, 80, 9, 18);
    let oversized = vec![7; MAX_IMAGE_BYTES + 17];
    let result = state.handle(kitty_image_event(7, oversized, false), 0, 0, 80, 9, 18);
    assert!(result.pty_writes[0].windows(5).any(|s| s == b"E2BIG"));
    assert_eq!(state.kitty_store.get(&7).unwrap().data.as_ref(), &[1]);
    assert_eq!(state.placements.len(), 1);
}

#[test]
fn oversized_kitty_multi_chunk_transfer_fails_without_partial_image() {
    let mut state = TerminalGraphicsState::default();
    state.handle(
        GraphicsEvent::Image {
            protocol: GraphicsProtocol::Kitty,
            id: Some(42),
            placement_id: None,
            width_cells: Some(1),
            height_cells: Some(1),
            data: vec![1; MAX_KITTY_PENDING_BYTES + 1],
            name: None,
            more: true,
            place: true,
            z_index: 0,
            above_text: false,
            cursor_motion: false,
            format: None,
            compressed: false,
            pixel_width: None,
            pixel_height: None,
            quiet: Some(2),
            source_rect: None,
        },
        0,
        0,
        80,
        9,
        18,
    );

    let result = state.handle(
        GraphicsEvent::Image {
            protocol: GraphicsProtocol::Kitty,
            id: Some(42),
            placement_id: None,
            width_cells: None,
            height_cells: None,
            data: b"tail".to_vec(),
            name: None,
            more: false,
            place: true,
            z_index: 0,
            above_text: false,
            cursor_motion: false,
            format: None,
            compressed: false,
            pixel_width: None,
            pixel_height: None,
            quiet: Some(2),
            source_rect: None,
        },
        0,
        0,
        80,
        9,
        18,
    );

    assert!(state.viewport_images(0, 24, 80).is_empty());
    assert!(!state.kitty_query_ok(GraphicsScreenKind::Primary, Some(42), None));
    assert!(result.pty_writes.is_empty());
}

#[test]
fn store_only_transmit_does_not_place() {
    let mut ingress = GraphicsIngress::new();
    let segments = ingress.advance(b"\x1b_Ga=t,i=1,c=2,r=2;QUI=\x1b\\");
    match &segments[0] {
        GraphicsSegment::Event(GraphicsEvent::Image { place: false, .. }) => {}
        other => panic!("expected store-only transmit: {other:?}"),
    }
    let segments = ingress.advance(b"\x1b_Ga=T,i=1,c=2,r=2,z=1,C=1;QUI=\x1b\\");
    match &segments[0] {
        GraphicsSegment::Event(GraphicsEvent::Image {
            place: true,
            above_text: true,
            cursor_motion: false,
            format: None,
            compressed: false,
            pixel_width: None,
            pixel_height: None,
            quiet: None,
            source_rect: None,
            ..
        }) => {}
        other => panic!("expected place+above: {other:?}"),
    }
}

#[test]
fn delete_all_clears_placements_and_store() {
    let mut state = TerminalGraphicsState::default();
    state.handle(
        GraphicsEvent::Image {
            protocol: GraphicsProtocol::Kitty,
            id: Some(1),
            placement_id: Some(1),
            width_cells: Some(2),
            height_cells: Some(2),
            data: b"AB".to_vec(),
            name: None,
            more: false,
            place: true,
            z_index: 0,
            above_text: false,
            cursor_motion: false,
            format: None,
            compressed: false,
            pixel_width: None,
            pixel_height: None,
            quiet: None,
            source_rect: None,
        },
        0,
        0,
        80,
        9,
        18,
    );
    assert_eq!(state.viewport_images(0, 24, 80).len(), 1);
    state.handle(
        GraphicsEvent::Delete {
            mode: KittyDeleteMode::All,
            free_data: true,
            image_id: None,
            placement_id: None,
            col: None,
            row: None,
            z: None,
        },
        0,
        0,
        80,
        9,
        18,
    );
    assert!(state.viewport_images(0, 24, 80).is_empty());
    // place-only must fail after free
    state.handle(
        GraphicsEvent::Image {
            protocol: GraphicsProtocol::Kitty,
            id: Some(1),
            placement_id: Some(2),
            width_cells: None,
            height_cells: None,
            data: Vec::new(),
            name: None,
            more: false,
            place: true,
            z_index: 0,
            above_text: false,
            cursor_motion: false,
            format: None,
            compressed: false,
            pixel_width: None,
            pixel_height: None,
            quiet: None,
            source_rect: None,
        },
        0,
        0,
        80,
        9,
        18,
    );
    assert!(state.viewport_images(0, 24, 80).is_empty());
}

#[test]
fn delete_image_without_free_keeps_store_for_replace() {
    let mut state = TerminalGraphicsState::default();
    state.handle(
        GraphicsEvent::Image {
            protocol: GraphicsProtocol::Kitty,
            id: Some(7),
            placement_id: Some(1),
            width_cells: Some(2),
            height_cells: Some(1),
            data: b"XY".to_vec(),
            name: None,
            more: false,
            place: true,
            z_index: 0,
            above_text: false,
            cursor_motion: false,
            format: None,
            compressed: false,
            pixel_width: None,
            pixel_height: None,
            quiet: None,
            source_rect: None,
        },
        0,
        0,
        80,
        9,
        18,
    );
    state.handle(
        GraphicsEvent::Delete {
            mode: KittyDeleteMode::Image,
            free_data: false,
            image_id: Some(7),
            placement_id: None,
            col: None,
            row: None,
            z: None,
        },
        0,
        0,
        80,
        9,
        18,
    );
    assert!(state.viewport_images(0, 24, 80).is_empty());
    state.handle(
        GraphicsEvent::Image {
            protocol: GraphicsProtocol::Kitty,
            id: Some(7),
            placement_id: Some(2),
            width_cells: Some(2),
            height_cells: Some(1),
            data: Vec::new(),
            name: None,
            more: false,
            place: true,
            z_index: 0,
            above_text: false,
            cursor_motion: false,
            format: None,
            compressed: false,
            pixel_width: None,
            pixel_height: None,
            quiet: None,
            source_rect: None,
        },
        1,
        3,
        80,
        9,
        18,
    );
    let images = state.viewport_images(0, 24, 80);
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].data.as_ref(), b"XY");
    assert_eq!(images[0].col, 3);
}

#[test]
fn delete_newest_and_placement_id() {
    let mut state = TerminalGraphicsState::default();
    state.handle(kitty_image_event(9, b"ZZ".to_vec(), false), 0, 0, 80, 9, 18);
    for (pid, col) in [(1u32, 0usize), (2, 4), (3, 8)] {
        state.handle(
            GraphicsEvent::Image {
                protocol: GraphicsProtocol::Kitty,
                id: Some(9),
                placement_id: Some(pid),
                width_cells: Some(2),
                height_cells: Some(1),
                data: Vec::new(),
                name: None,
                more: false,
                place: true,
                z_index: 0,
                above_text: false,
                cursor_motion: false,
                format: None,
                compressed: false,
                pixel_width: None,
                pixel_height: None,
                quiet: None,
                source_rect: None,
            },
            0,
            col,
            80,
            9,
            18,
        );
    }
    assert_eq!(state.viewport_images(0, 24, 80).len(), 3);
    state.handle(
        GraphicsEvent::Delete {
            mode: KittyDeleteMode::Newest,
            free_data: false,
            image_id: Some(9),
            placement_id: None,
            col: None,
            row: None,
            z: None,
        },
        0,
        0,
        80,
        9,
        18,
    );
    let images = state.viewport_images(0, 24, 80);
    assert_eq!(images.len(), 2);
    assert!(!images.iter().any(|i| i.col == 8));
    state.handle(
        GraphicsEvent::Delete {
            mode: KittyDeleteMode::Placement,
            free_data: false,
            image_id: Some(9),
            placement_id: Some(1),
            col: None,
            row: None,
            z: None,
        },
        0,
        0,
        80,
        9,
        18,
    );
    let images = state.viewport_images(0, 24, 80);
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].col, 4);
}

#[test]
fn delete_by_cell_column_and_z() {
    let mut state = TerminalGraphicsState::default();
    state.handle(
        GraphicsEvent::Image {
            protocol: GraphicsProtocol::Kitty,
            id: Some(1),
            placement_id: Some(1),
            width_cells: Some(3),
            height_cells: Some(2),
            data: b"AA".to_vec(),
            name: None,
            more: false,
            place: true,
            z_index: 0,
            above_text: false,
            cursor_motion: false,
            format: None,
            compressed: false,
            pixel_width: None,
            pixel_height: None,
            quiet: None,
            source_rect: None,
        },
        0,
        0,
        80,
        9,
        18,
    );
    state.handle(
        GraphicsEvent::Image {
            protocol: GraphicsProtocol::Kitty,
            id: Some(2),
            placement_id: Some(1),
            width_cells: Some(2),
            height_cells: Some(1),
            data: b"BB".to_vec(),
            name: None,
            more: false,
            place: true,
            z_index: 5,
            above_text: true,
            cursor_motion: false,
            format: None,
            compressed: false,
            pixel_width: None,
            pixel_height: None,
            quiet: None,
            source_rect: None,
        },
        0,
        10,
        80,
        9,
        18,
    );
    // Cell delete at (1-based col=2,row=1) hits first image only.
    state.handle(
        GraphicsEvent::Delete {
            mode: KittyDeleteMode::Cell,
            free_data: false,
            image_id: None,
            placement_id: None,
            col: Some(2),
            row: Some(1),
            z: None,
        },
        0,
        0,
        80,
        9,
        18,
    );
    let images = state.viewport_images(0, 24, 80);
    assert_eq!(images.len(), 1);
    assert!(images[0].above_text);
    state.handle(
        GraphicsEvent::Delete {
            mode: KittyDeleteMode::ZIndex,
            free_data: true,
            image_id: None,
            placement_id: None,
            col: None,
            row: None,
            z: Some(5),
        },
        0,
        0,
        80,
        9,
        18,
    );
    assert!(state.viewport_images(0, 24, 80).is_empty());
}

#[test]
fn parse_delete_modes_from_apc() {
    let mut ingress = GraphicsIngress::new();
    let segments = ingress.advance(b"\x1b_Ga=d,d=A\x1b\\");
    match &segments[0] {
        GraphicsSegment::Event(GraphicsEvent::Delete {
            mode: KittyDeleteMode::All,
            free_data: true,
            ..
        }) => {}
        other => panic!("expected delete all free: {other:?}"),
    }
    let segments = ingress.advance(b"\x1b_Ga=d,d=i,i=4\x1b\\");
    match &segments[0] {
        GraphicsSegment::Event(GraphicsEvent::Delete {
            mode: KittyDeleteMode::Image,
            free_data: false,
            image_id: Some(4),
            ..
        }) => {}
        other => panic!("expected delete image: {other:?}"),
    }
    let segments = ingress.advance(b"\x1b_Ga=d,d=p,i=4,p=2\x1b\\");
    match &segments[0] {
        GraphicsSegment::Event(GraphicsEvent::Delete {
            mode: KittyDeleteMode::Placement,
            placement_id: Some(2),
            image_id: Some(4),
            ..
        }) => {}
        other => panic!("expected delete placement: {other:?}"),
    }
    let segments = ingress.advance(b"\x1b_Ga=d,d=c,x=3,y=2\x1b\\");
    match &segments[0] {
        GraphicsSegment::Event(GraphicsEvent::Delete {
            mode: KittyDeleteMode::Cell,
            col: Some(3),
            row: Some(2),
            ..
        }) => {}
        other => panic!("expected delete cell: {other:?}"),
    }
}

#[test]
fn kitty_query_reports_existence() {
    let mut state = TerminalGraphicsState::default();
    let result = state.handle(
        GraphicsEvent::Query {
            image_id: Some(9),
            placement_id: None,
        },
        0,
        0,
        80,
        9,
        18,
    );
    assert_eq!(result.pty_writes.len(), 1);
    assert!(
        String::from_utf8_lossy(&result.pty_writes[0]).contains("ENOENT"),
        "{:?}",
        result.pty_writes
    );
    state.handle(
        GraphicsEvent::Image {
            protocol: GraphicsProtocol::Kitty,
            id: Some(9),
            placement_id: None,
            width_cells: Some(1),
            height_cells: Some(1),
            data: b"AB".to_vec(),
            name: None,
            more: false,
            place: false,
            z_index: 0,
            above_text: false,
            cursor_motion: false,
            format: None,
            compressed: false,
            pixel_width: None,
            pixel_height: None,
            quiet: None,
            source_rect: None,
        },
        0,
        0,
        80,
        9,
        18,
    );
    let result = state.handle(
        GraphicsEvent::Query {
            image_id: Some(9),
            placement_id: None,
        },
        0,
        0,
        80,
        9,
        18,
    );
    assert!(
        String::from_utf8_lossy(&result.pty_writes[0]).contains("OK"),
        "{:?}",
        result.pty_writes
    );
    let result = state.handle(
        GraphicsEvent::Query {
            image_id: Some(9),
            placement_id: Some(1),
        },
        0,
        0,
        80,
        9,
        18,
    );
    assert!(
        String::from_utf8_lossy(&result.pty_writes[0]).contains("ENOENT"),
        "{:?}",
        result.pty_writes
    );
    state.handle(
        GraphicsEvent::Image {
            protocol: GraphicsProtocol::Kitty,
            id: Some(9),
            placement_id: Some(1),
            width_cells: Some(1),
            height_cells: Some(1),
            data: Vec::new(),
            name: None,
            more: false,
            place: true,
            z_index: 0,
            above_text: false,
            cursor_motion: false,
            format: None,
            compressed: false,
            pixel_width: None,
            pixel_height: None,
            quiet: None,
            source_rect: None,
        },
        0,
        0,
        80,
        9,
        18,
    );
    let result = state.handle(
        GraphicsEvent::Query {
            image_id: Some(9),
            placement_id: Some(1),
        },
        0,
        0,
        80,
        9,
        18,
    );
    assert!(
        String::from_utf8_lossy(&result.pty_writes[0]).contains("OK"),
        "{:?}",
        result.pty_writes
    );
}

#[test]
fn kitty_quiet_suppresses_success_on_store() {
    let mut state = TerminalGraphicsState::default();
    let result = state.handle(
        GraphicsEvent::Image {
            protocol: GraphicsProtocol::Kitty,
            id: Some(2),
            placement_id: None,
            width_cells: Some(1),
            height_cells: Some(1),
            data: b"XY".to_vec(),
            name: None,
            more: false,
            place: false,
            z_index: 0,
            above_text: false,
            cursor_motion: false,
            format: None,
            compressed: false,
            pixel_width: None,
            pixel_height: None,
            quiet: Some(2),
            source_rect: None,
        },
        0,
        0,
        80,
        9,
        18,
    );
    assert!(result.pty_writes.is_empty());
}

#[test]
fn parse_query_action_from_apc() {
    let mut ingress = GraphicsIngress::new();
    let segments = ingress.advance(b"\x1b_Ga=q,i=5\x1b\\");
    match &segments[0] {
        GraphicsSegment::Event(GraphicsEvent::Query {
            image_id: Some(5),
            placement_id: None,
        }) => {}
        other => panic!("expected query: {other:?}"),
    }
}

#[test]
fn cursor_motion_ansi_targets_after_image() {
    let motion = GraphicsCursorMotion {
        start_col: 2,
        width_cells: 4,
        height_cells: 3,
    };
    // CUD 2 then CHA 7 (1-based col after image: 2+4+1)
    assert_eq!(motion.to_ansi(), b"\x1b[2B\x1b[7G");
    let flat = GraphicsCursorMotion {
        start_col: 0,
        width_cells: 5,
        height_cells: 1,
    };
    assert_eq!(flat.to_ansi(), b"\x1b[6G");
}

#[test]
fn place_with_cursor_motion_flag_reports_geometry() {
    let mut state = TerminalGraphicsState::default();
    let result = state.handle(
        GraphicsEvent::Image {
            protocol: GraphicsProtocol::Kitty,
            id: Some(1),
            placement_id: None,
            width_cells: Some(3),
            height_cells: Some(2),
            data: b"AB".to_vec(),
            name: None,
            more: false,
            place: true,
            z_index: 0,
            above_text: false,
            cursor_motion: true,
            format: None,
            compressed: false,
            pixel_width: None,
            pixel_height: None,
            quiet: None,
            source_rect: None,
        },
        0,
        5,
        80,
        9,
        18,
    );
    assert_eq!(
        result.cursor_motion,
        Some(GraphicsCursorMotion {
            start_col: 5,
            width_cells: 3,
            height_cells: 2,
        })
    );
}

#[test]
fn kitty_rgb24_payload_is_rasterized() {
    let mut state = TerminalGraphicsState::default();
    // 2x1 red/green RGB pixels: FF0000 00FF00
    let raw = vec![255, 0, 0, 0, 255, 0];
    let result = state.handle(
        GraphicsEvent::Image {
            protocol: GraphicsProtocol::Kitty,
            id: Some(1),
            placement_id: None,
            width_cells: Some(2),
            height_cells: Some(1),
            data: raw,
            name: None,
            more: false,
            place: true,
            z_index: 0,
            above_text: false,
            cursor_motion: false,
            format: Some(24),
            compressed: false,
            pixel_width: Some(2),
            pixel_height: Some(1),
            quiet: None,
            source_rect: None,
        },
        0,
        0,
        80,
        9,
        18,
    );
    let images = state.viewport_images(0, 24, 80);
    assert_eq!(images.len(), 1);
    assert!(images[0].data.starts_with(b"NYAR"));
    assert_eq!(&images[0].data[12..16], &[255, 0, 0, 255]);
    assert_eq!(&images[0].data[16..20], &[0, 255, 0, 255]);
    assert!(result.cursor_motion.is_none());
}

fn place_kitty_tile(state: &mut TerminalGraphicsState, pid: u32) {
    let mut event = kitty_image_event(123, Vec::new(), true);
    if let GraphicsEvent::Image { placement_id, .. } = &mut event {
        *placement_id = Some(pid);
    }
    state.handle(event, 0, 0, 80, 9, 18);
}

#[test]
#[ignore = "manual placement scaling benchmark"]
fn benchmark_kitty_placements() {
    use std::time::Instant;
    for count in [1000, 4000, 8000] {
        let mut state = TerminalGraphicsState::default();
        state.handle(kitty_image_event(123, vec![1], false), 0, 0, 80, 9, 18);
        let start = Instant::now();
        for pid in 1..=count {
            place_kitty_tile(&mut state, pid);
        }
        let insert = start.elapsed();
        let start = Instant::now();
        for pid in 1..=count {
            place_kitty_tile(&mut state, pid);
        }
        eprintln!(
            "{count} placements: insert={insert:?}, replace={:?}",
            start.elapsed()
        );
        assert_eq!(state.placements.len(), count as usize);
    }
}

#[test]
fn retransmission_clears_all_old_placements_only_after_success() {
    for place in [false, true] {
        let mut state = TerminalGraphicsState::default();
        state.handle(kitty_image_event(123, vec![1], true), 0, 0, 80, 9, 18);
        place_kitty_tile(&mut state, 1);
        state.handle_on_screen(
            kitty_image_event(123, Vec::new(), true),
            GraphicsScreenKind::Alternate,
            0,
            0,
            0,
            80,
            9,
            18,
        );
        state.handle(kitty_image_event(456, vec![2], true), 0, 0, 80, 9, 18);
        let old = std::sync::Arc::downgrade(&state.kitty_store[&123].data);
        let mut first = kitty_image_event(123, vec![3], place);
        if let GraphicsEvent::Image { more, .. } = &mut first {
            *more = true;
        }
        state.handle(first, 0, 0, 80, 9, 18);
        assert_eq!(state.placements.len(), 4);
        state.handle(kitty_image_event(123, vec![4], false), 0, 0, 80, 9, 18);
        assert!(old.upgrade().is_none());
        assert_eq!(state.placements.len(), 1 + usize::from(place));
        assert_eq!(state.kitty_store[&123].data.as_ref(), &[3, 4]);
        assert!(state.placements.iter().any(|p| p.kitty_id == Some(456)));
        place_kitty_tile(&mut state, 1);
        assert_eq!(state.placements.last().unwrap().data.as_ref(), &[3, 4]);
    }
}

#[test]
fn invalid_retransmission_preserves_old_placements() {
    let mut state = TerminalGraphicsState::default();
    state.handle(kitty_image_event(123, vec![1], true), 0, 0, 80, 9, 18);
    let mut invalid = kitty_image_event(123, vec![2], true);
    if let GraphicsEvent::Image { format, .. } = &mut invalid {
        *format = Some(999);
    }
    let result = state.handle(invalid, 0, 0, 80, 9, 18);
    assert!(result.pty_writes[0].windows(6).any(|s| s == b"EINVAL"));
    assert_eq!(state.placements.len(), 1);
    assert_eq!(state.kitty_store[&123].data.as_ref(), &[1]);
}

#[test]
fn unsupported_kitty_transfers_reply_and_respect_quiet() {
    for option in ["t=f", "t=t", "t=s", "U=1"] {
        for action in ["t", "T", "p"] {
            for quiet in 0..=2 {
                let body = format!("a={action},{option},i=42,p=7,q={quiet};AAAA");
                let event = super::parse_kitty_body(body.as_bytes()).unwrap();
                let mut state = TerminalGraphicsState::default();
                let result = state.handle(event, 0, 0, 80, 9, 18);
                assert_eq!(result.pty_writes.len(), usize::from(quiet != 2));
                if quiet != 2 {
                    assert!(result.pty_writes[0].starts_with(b"\x1b_Gi=42,p=7;ENOTSUP:"));
                }
                assert_eq!(state.placements.len(), 0);
                assert!(state.kitty_store.is_empty());
            }
        }
    }
}

#[test]
fn indexed_placements_preserve_order_and_survive_removal_and_eviction() {
    let mut state = TerminalGraphicsState::default();
    state.handle(kitty_image_event(123, vec![1], false), 0, 0, 80, 9, 18);
    for pid in 1..=8200 {
        place_kitty_tile(&mut state, pid);
    }
    assert_eq!(state.placements.len(), 8192);
    assert_eq!(
        state.placements.iter().next().unwrap().placement_id,
        Some(9)
    );
    place_kitty_tile(&mut state, 9);
    assert_eq!(state.placements.len(), 8192);
    assert_eq!(state.placements.last().unwrap().placement_id, Some(9));
    assert_eq!(
        state.placements.iter().next().unwrap().placement_id,
        Some(10)
    );
    state.clear_screen(GraphicsScreenKind::Primary);
    place_kitty_tile(&mut state, 9);
    assert_eq!(state.placements.len(), 1);
    state.retain_line_range(10, 20);
    place_kitty_tile(&mut state, 9);
    assert_eq!(state.placements.len(), 1);
}

#[test]
fn rejected_chunk_aborts_pending_transfer_and_inherits_reply_metadata() {
    let mut state = TerminalGraphicsState::default();
    let first = super::parse_kitty_body(b"a=T,i=123,p=7,q=1,m=1;AQID").unwrap();
    assert!(state.handle(first, 0, 0, 80, 9, 18).pty_writes.is_empty());
    let rejected = super::parse_kitty_body(b"t=f,m=0;AAAA").unwrap();
    let result = state.handle(rejected, 0, 0, 80, 9, 18);
    assert!(result.pty_writes[0].starts_with(b"\x1b_Gi=123,p=7;ENOTSUP:"));
    assert!(state.pending_kitty.is_none());
    assert!(state.kitty_store.is_empty());
    assert_eq!(state.placements.len(), 0);
    state.handle(kitty_image_event(123, vec![4], true), 0, 0, 80, 9, 18);
    assert_eq!(state.placements.last().unwrap().data.as_ref(), &[4]);
}

#[test]
fn placement_index_distinguishes_screens_and_anonymous_placements() {
    let mut state = TerminalGraphicsState::default();
    state.handle(kitty_image_event(123, vec![1], false), 0, 0, 80, 9, 18);
    place_kitty_tile(&mut state, 1);
    let mut event = kitty_image_event(123, Vec::new(), true);
    if let GraphicsEvent::Image { placement_id, .. } = &mut event {
        *placement_id = Some(1);
    }
    state.handle_on_screen(event, GraphicsScreenKind::Alternate, 0, 0, 0, 80, 9, 18);
    place_kitty_tile(&mut state, 1);
    assert_eq!(state.placements.len(), 2);
    for _ in 0..2 {
        state.handle(kitty_image_event(123, Vec::new(), true), 0, 0, 80, 9, 18);
    }
    assert_eq!(state.placements.len(), 4);
    state.clear_screen(GraphicsScreenKind::Alternate);
    place_kitty_tile(&mut state, 1);
    assert_eq!(state.placements.len(), 3);
}
