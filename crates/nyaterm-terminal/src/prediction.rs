//! Presentation-only speculative cells. They never modify the terminal parser or history.
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::{RenderCell, StyledSpan, TerminalSnapshot};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PredictedCell {
    pub row: usize,
    pub col: usize,
    pub cell: RenderCell,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalPrediction {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<PredictedCell>,
    pub cursor: Option<(usize, usize)>,
}

/// Apply only at the rendering boundary. All readers retain the confirmed snapshot.
pub fn painted_snapshot(snapshot: Arc<TerminalSnapshot>) -> Arc<TerminalSnapshot> {
    let Some(prediction) = snapshot.prediction.as_ref() else {
        return snapshot;
    };
    if snapshot.display_offset != 0
        || snapshot.cols != prediction.cols
        || snapshot.viewport_rows != prediction.rows
        || snapshot.display_rows.is_some()
    {
        return snapshot;
    }
    let mut painted = snapshot.as_ref().clone();
    let row_offset = painted.row_count().saturating_sub(painted.viewport_rows);
    let rows = Arc::make_mut(&mut painted.row_data);
    for patch in &prediction.cells {
        let Some(row) = rows.get_mut(row_offset + patch.row) else {
            continue;
        };
        if patch.col >= row.cells.len() {
            continue;
        }
        let row = Arc::make_mut(row);
        row.cells[patch.col] = patch.cell.clone();
        row.revision = row.revision.wrapping_add(1);
        // Render cache signatures must reflect speculative glyphs and attributes.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        row.signature.hash(&mut hasher);
        patch.col.hash(&mut hasher);
        format!("{:?}", patch.cell).hash(&mut hasher);
        row.signature = hasher.finish();
        row.text = row
            .cells
            .iter()
            .filter(|cell| cell.width > 0)
            .map(RenderCell::text)
            .collect();
        row.styled_spans = row
            .cells
            .iter()
            .filter(|cell| cell.width > 0)
            .map(|cell| StyledSpan {
                text: cell.text().to_owned(),
                style: cell.style,
            })
            .collect();
    }
    if let Some((row, col)) = prediction.cursor {
        painted.cursor.row = row_offset + row;
        painted.cursor.col = col;
    }
    painted.prediction = None;
    Arc::new(painted)
}

#[cfg(test)]
mod tests {
    use super::{PredictedCell, TerminalPrediction, painted_snapshot};
    use crate::{CellText, TerminalScreen};
    use std::sync::Arc;

    #[test]
    fn predictions_change_only_paint_and_have_distinct_cache_signatures() {
        let mut screen = TerminalScreen::default();
        screen.resize(10, 2);
        screen.advance(b"confirmed");
        let original = screen.snapshot();
        let confirmed = original.row_data[0].text.clone();
        let mut cell = original.row_data[0].cells[0].clone();
        cell.text = CellText::from("X");
        let mut snapshot = original.clone();
        snapshot.prediction = Some(Arc::new(TerminalPrediction {
            cols: 10,
            rows: 2,
            cells: vec![PredictedCell {
                row: 0,
                col: 0,
                cell: cell.clone(),
            }],
            cursor: Some((0, 1)),
        }));
        let snapshot = Arc::new(snapshot);
        let painted = painted_snapshot(snapshot.clone());
        assert_eq!(snapshot.row_data[0].text, confirmed);
        assert_eq!(screen.snapshot().row_data[0].text, confirmed);
        assert!(painted.row_data[0].text.starts_with('X'));
        cell.text = CellText::from("Y");
        let mut second = original;
        second.prediction = Some(Arc::new(TerminalPrediction {
            cols: 10,
            rows: 2,
            cells: vec![PredictedCell {
                row: 0,
                col: 0,
                cell,
            }],
            cursor: None,
        }));
        assert_ne!(
            painted.row_data[0].signature,
            painted_snapshot(Arc::new(second)).row_data[0].signature
        );
    }

    #[test]
    fn prediction_is_rejected_after_geometry_changes() {
        let mut snapshot = TerminalScreen::default().snapshot();
        snapshot.prediction = Some(Arc::new(TerminalPrediction {
            cols: 1,
            rows: 1,
            ..Default::default()
        }));
        let snapshot = Arc::new(snapshot);
        assert!(Arc::ptr_eq(&snapshot, &painted_snapshot(snapshot.clone())));
    }
}
