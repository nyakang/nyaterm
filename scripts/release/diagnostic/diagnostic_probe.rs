//! Bounded, content-free breadcrumbs for a diagnostic build only.
//!
//! UI and transport callbacks only touch atomics. The panic hook writes the
//! snapshot synchronously before resolving symbols; no render path does I/O.

use std::fmt::{self, Write};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Stage {
    SaveRequested,
    SaveJobStarted,
    SaveJobSucceeded,
    SaveApplied,
    EditorClosing,
    EditorWindowRemoving,
    SessionStartRequested,
    LocalPtyOpening,
    LocalShellSpawned,
    TerminalLayout,
    TerminalPrepaint,
    TerminalShape,
    TerminalShaped,
    TerminalPaint,
}

const STAGE_NAMES: [&str; 14] = [
    "save_requested",
    "save_job_started",
    "save_job_succeeded",
    "save_applied",
    "editor_closing",
    "editor_window_removing",
    "session_start_requested",
    "local_pty_opening",
    "local_shell_spawned",
    "terminal_layout",
    "terminal_prepaint",
    "terminal_shape",
    "terminal_shaped",
    "terminal_paint",
];

struct Probe {
    sequence: AtomicU64,
    counts: [AtomicU64; STAGE_NAMES.len()],
    first: [AtomicU64; STAGE_NAMES.len()],
    last: [AtomicU64; STAGE_NAMES.len()],
    geometry: [AtomicU32; 4],
}

impl Probe {
    const fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            counts: [const { AtomicU64::new(0) }; STAGE_NAMES.len()],
            first: [const { AtomicU64::new(0) }; STAGE_NAMES.len()],
            last: [const { AtomicU64::new(0) }; STAGE_NAMES.len()],
            geometry: [const { AtomicU32::new(0) }; 4],
        }
    }

    fn record(&self, stage: Stage) {
        let index = stage as usize;
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        self.counts[index].fetch_add(1, Ordering::Relaxed);
        let _ =
            self.first[index].compare_exchange(0, sequence, Ordering::Relaxed, Ordering::Relaxed);
        self.last[index].fetch_max(sequence, Ordering::Relaxed);
    }

    fn write_snapshot(&self, writer: &mut impl Write) -> fmt::Result {
        writeln!(
            writer,
            "\nDiagnostic-Stages (process-wide, best-effort concurrent snapshot):"
        )?;
        for (index, name) in STAGE_NAMES.iter().enumerate() {
            writeln!(
                writer,
                "{name}: count={} first={} last={}",
                self.counts[index].load(Ordering::Relaxed),
                self.first[index].load(Ordering::Relaxed),
                self.last[index].load(Ordering::Relaxed)
            )?;
        }
        for (index, name) in ["font_size", "cell_width", "cell_height", "scale_factor"]
            .iter()
            .enumerate()
        {
            writeln!(
                writer,
                "{name}: {}",
                f32::from_bits(self.geometry[index].load(Ordering::Relaxed))
            )?;
        }
        Ok(())
    }
}

static PROBE: Probe = Probe::new();

pub fn record(stage: Stage) {
    PROBE.record(stage);
}

pub fn geometry(font_size: f32, cell_width: f32, cell_height: f32, scale_factor: f32) {
    for (slot, value) in
        PROBE
            .geometry
            .iter()
            .zip([font_size, cell_width, cell_height, scale_factor])
    {
        slot.store(value.to_bits(), Ordering::Relaxed);
    }
}

pub fn write_snapshot(writer: &mut impl Write) -> fmt::Result {
    PROBE.write_snapshot(writer)
}

#[cfg(test)]
mod tests {
    use crate::diagnostic_probe::{Probe, Stage};

    #[test]
    fn repeated_paints_preserve_earlier_stages_without_a_growing_buffer() {
        let probe = Probe::new();
        probe.record(Stage::SaveRequested);
        probe.record(Stage::LocalShellSpawned);
        for _ in 0..10_000 {
            probe.record(Stage::TerminalPaint);
        }
        let mut report = String::new();
        probe.write_snapshot(&mut report).unwrap();
        assert!(report.contains("save_requested: count=1 first=1 last=1"));
        assert!(report.contains("local_shell_spawned: count=1 first=2 last=2"));
        assert!(report.contains("terminal_paint: count=10000 first=3 last=10002"));
        assert!(report.len() < 2048);
    }
}
