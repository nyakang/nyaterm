#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProcessDisplayMode {
    Compact,
    Narrow,
    Medium,
    Wide,
}

pub(crate) fn process_display_mode(panel_width: f32) -> ProcessDisplayMode {
    // Width is the outer panel width, including its content padding.
    if panel_width > 0. && panel_width < 280. {
        ProcessDisplayMode::Compact
    } else if panel_width > 0. && panel_width < 430. {
        ProcessDisplayMode::Narrow
    } else if panel_width > 0. && panel_width < 600. {
        ProcessDisplayMode::Medium
    } else {
        ProcessDisplayMode::Wide
    }
}

pub(crate) fn process_row_height_px(mode: ProcessDisplayMode) -> f32 {
    match mode {
        ProcessDisplayMode::Compact => 78.,
        ProcessDisplayMode::Narrow => 60.,
        _ => 40.,
    }
}
