//! Cancellable, chunked frame search jobs.

use super::frame_events::TerminalFrameSearchEvent;
use super::frame_worker::TerminalFrameSession;
use crate::terminal::TerminalBufferMatch;
use nyaterm_terminal::{
    TerminalGridOccurrence, TerminalSearchDirection, TerminalSearchQuery, TerminalSearchRangeMode,
    terminal_cell_count,
};
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TerminalFrameSearchKey {
    pub(crate) query: String,
    pub(crate) case_sensitive: bool,
    pub(crate) regex: bool,
    pub(crate) whole_word: bool,
    pub(crate) limit: usize,
    pub(crate) request_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TerminalFrameSearchPurpose {
    Find,
    FindVisible {
        absolute_start: usize,
        absolute_end: usize,
    },
    SelectedOccurrenceVisible {
        absolute_start: usize,
        absolute_end: usize,
    },
    SelectedOccurrence,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalFrameSearchResult {
    pub(crate) key: TerminalFrameSearchKey,
    pub(crate) revision: u64,
    pub(crate) matches: Result<Arc<[TerminalBufferMatch]>, String>,
    pub(crate) occurrences: Arc<[Range<usize>]>,
    pub(crate) truncated: bool,
    pub(crate) position_fingerprint: u64,
}

impl TerminalFrameSearchResult {
    pub(crate) fn new(
        key: TerminalFrameSearchKey,
        revision: u64,
        matches: Result<Vec<TerminalBufferMatch>, String>,
    ) -> Self {
        let position_fingerprint = matches
            .as_deref()
            .map(terminal_buffer_matches_position_fingerprint)
            .unwrap_or_default();
        Self {
            key,
            revision,
            matches: matches.map(Arc::from),
            occurrences: Arc::from([]),
            truncated: false,
            position_fingerprint,
        }
    }

    pub(crate) fn new_find(
        key: TerminalFrameSearchKey,
        revision: u64,
        occurrences: Result<Vec<TerminalGridOccurrence>, String>,
    ) -> Self {
        let mut truncated = false;
        let matches = occurrences.map(|mut occurrences| {
            truncated = occurrences.len() > key.limit;
            occurrences.truncate(key.limit);
            let mut segments = Vec::new();
            let mut ranges = Vec::with_capacity(occurrences.len());
            for occurrence in occurrences {
                let start = segments.len();
                segments.extend(occurrence.segments.into_iter().map(|segment| {
                    TerminalBufferMatch {
                        line_index: segment.line_index,
                        start_col: segment.start_col,
                        end_col: segment.end_col,
                    }
                }));
                ranges.push(start..segments.len());
            }
            (segments, ranges)
        });
        match matches {
            Ok((segments, ranges)) => {
                let mut result = Self::new(key, revision, Ok(segments));
                result.occurrences = ranges.into();
                result.truncated = truncated;
                result
            }
            Err(error) => Self::new(key, revision, Err(error)),
        }
    }
}

pub(crate) fn terminal_buffer_matches_position_fingerprint(matches: &[TerminalBufferMatch]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    matches.len().hash(&mut hasher);
    for search_match in matches {
        search_match.line_index.hash(&mut hasher);
        search_match.start_col.hash(&mut hasher);
        search_match.end_col.hash(&mut hasher);
    }
    hasher.finish()
}

pub(crate) fn terminal_frame_search_result_is_current(
    result: &TerminalFrameSearchResult,
    key: &TerminalFrameSearchKey,
    revision: u64,
) -> bool {
    result.key == *key && result.revision == revision
}

pub(super) const SELECTED_OCCURRENCE_SEARCH_CHUNK_ROWS: usize = 256;

#[derive(Debug)]
pub(super) struct FindSearchJob {
    pub(super) session_id: String,
    pub(super) key: TerminalFrameSearchKey,
    pub(super) query: TerminalSearchQuery,
    pub(super) revision: u64,
    pub(super) total_rows: usize,
    pub(super) next_absolute_row: usize,
    pub(super) occurrences: Vec<TerminalGridOccurrence>,
    pub(super) started_at: Instant,
}

impl FindSearchJob {
    pub(super) fn new(
        session_id: String,
        key: TerminalFrameSearchKey,
        session: &TerminalFrameSession,
    ) -> Self {
        let query = TerminalSearchQuery {
            pattern: key.query.clone(),
            regex: key.regex,
            case_sensitive: key.case_sensitive,
            whole_word: key.whole_word,
            direction: TerminalSearchDirection::Forward,
            limit: key.limit.saturating_add(1),
        };
        Self {
            session_id,
            key,
            query,
            revision: session.revision,
            total_rows: session.screen.total_rows(),
            next_absolute_row: 0,
            occurrences: Vec::new(),
            started_at: Instant::now(),
        }
    }

    pub(super) fn event(
        self,
        occurrences: Result<Vec<TerminalGridOccurrence>, String>,
    ) -> TerminalFrameSearchEvent {
        TerminalFrameSearchEvent {
            session_id: self.session_id,
            purpose: TerminalFrameSearchPurpose::Find,
            result: TerminalFrameSearchResult::new_find(self.key, self.revision, occurrences),
            process_duration: self.started_at.elapsed(),
        }
    }

    pub(super) fn process_chunk(
        &mut self,
        session: &TerminalFrameSession,
    ) -> Result<bool, nyaterm_terminal::TerminalSearchError> {
        let end = self
            .next_absolute_row
            .saturating_add(SELECTED_OCCURRENCE_SEARCH_CHUNK_ROWS)
            .min(self.total_rows);
        let found = session.screen.search_grid_occurrences_in_absolute_range(
            &self.query,
            self.next_absolute_row..end,
            TerminalSearchRangeMode::StartsWithin,
        )?;
        self.occurrences.extend(found);
        self.next_absolute_row = end;
        Ok(end >= self.total_rows || self.occurrences.len() > self.key.limit)
    }
}

pub(super) fn process_next_find_search_chunk(
    jobs: &mut VecDeque<FindSearchJob>,
    sessions: &HashMap<String, TerminalFrameSession>,
) -> Option<TerminalFrameSearchEvent> {
    let mut job = jobs.pop_front()?;
    let Some(session) = sessions.get(&job.session_id) else {
        return Some(job.event(Err("search session was removed".to_string())));
    };
    if session.revision != job.revision {
        return Some(job.event(Err("search was cancelled by terminal output".to_string())));
    }
    match job.process_chunk(session) {
        Ok(true) => {
            let occurrences = std::mem::take(&mut job.occurrences);
            Some(job.event(Ok(occurrences)))
        }
        Ok(false) => {
            jobs.push_back(job);
            None
        }
        Err(error) => Some(job.event(Err(error.to_string()))),
    }
}

pub(super) fn cancel_find_search_job_for_session(
    jobs: &mut VecDeque<FindSearchJob>,
    session_id: &str,
) {
    jobs.retain(|job| job.session_id != session_id);
}

#[derive(Debug)]
pub(super) struct SelectedOccurrenceSearchJob {
    pub(super) session_id: String,
    pub(super) key: TerminalFrameSearchKey,
    pub(super) query: TerminalSearchQuery,
    pub(super) revision: u64,
    pub(super) total_rows: usize,
    pub(super) overlap_rows: usize,
    pub(super) next_absolute_row: usize,
    pub(super) matches: Vec<TerminalBufferMatch>,
    pub(super) started_at: Instant,
}

impl SelectedOccurrenceSearchJob {
    pub(super) fn new(
        session_id: String,
        key: TerminalFrameSearchKey,
        session: &TerminalFrameSession,
    ) -> Self {
        let cols = session.screen.cols().max(1);
        let query_cell_width = terminal_cell_count(&key.query);
        let overlap_rows = query_cell_width.saturating_add(cols.saturating_sub(1)) / cols + 1;
        let query = TerminalSearchQuery {
            pattern: key.query.clone(),
            regex: false,
            case_sensitive: true,
            whole_word: false,
            direction: TerminalSearchDirection::Forward,
            limit: key.limit,
        };
        Self {
            session_id,
            key,
            query,
            revision: session.revision,
            total_rows: session.screen.total_rows(),
            overlap_rows,
            next_absolute_row: 0,
            matches: Vec::new(),
            started_at: Instant::now(),
        }
    }

    pub(super) fn cancellation_event(self, message: &str) -> TerminalFrameSearchEvent {
        let result =
            TerminalFrameSearchResult::new(self.key, self.revision, Err(message.to_string()));
        TerminalFrameSearchEvent {
            session_id: self.session_id,
            purpose: TerminalFrameSearchPurpose::SelectedOccurrence,
            result,
            process_duration: self.started_at.elapsed(),
        }
    }

    pub(super) fn completion_event(mut self) -> TerminalFrameSearchEvent {
        self.matches
            .sort_unstable_by_key(|m| (m.line_index, m.start_col, m.end_col));
        self.matches
            .dedup_by_key(|m| (m.line_index, m.start_col, m.end_col));
        self.matches.truncate(self.key.limit);
        let result = TerminalFrameSearchResult::new(self.key, self.revision, Ok(self.matches));
        TerminalFrameSearchEvent {
            session_id: self.session_id,
            purpose: TerminalFrameSearchPurpose::SelectedOccurrence,
            result,
            process_duration: self.started_at.elapsed(),
        }
    }

    pub(super) fn process_chunk(
        &mut self,
        session: &TerminalFrameSession,
    ) -> Result<bool, nyaterm_terminal::TerminalSearchError> {
        if self.next_absolute_row >= self.total_rows || self.matches.len() >= self.key.limit {
            return Ok(true);
        }
        let chunk_start = self.next_absolute_row;
        let chunk_end = chunk_start
            .saturating_add(SELECTED_OCCURRENCE_SEARCH_CHUNK_ROWS)
            .min(self.total_rows);
        let search_start = chunk_start.saturating_sub(self.overlap_rows);
        let chunk_matches = session
            .screen
            .search_grid_in_absolute_range(&self.query, search_start..chunk_end)?;
        self.matches.extend(
            chunk_matches
                .into_iter()
                .map(|search_match| TerminalBufferMatch {
                    line_index: search_match.line_index,
                    start_col: search_match.start_col,
                    end_col: search_match.end_col,
                }),
        );
        self.matches
            .sort_unstable_by_key(|m| (m.line_index, m.start_col, m.end_col));
        self.matches
            .dedup_by_key(|m| (m.line_index, m.start_col, m.end_col));
        self.next_absolute_row = chunk_end;
        Ok(self.next_absolute_row >= self.total_rows || self.matches.len() >= self.key.limit)
    }
}

pub(super) fn process_next_selected_occurrence_search_chunk(
    jobs: &mut VecDeque<SelectedOccurrenceSearchJob>,
    sessions: &HashMap<String, TerminalFrameSession>,
) -> Option<TerminalFrameSearchEvent> {
    let mut job = jobs.pop_front()?;
    let Some(session) = sessions.get(&job.session_id) else {
        return Some(job.cancellation_event("selected occurrence session was removed"));
    };
    if session.revision != job.revision {
        return Some(
            job.cancellation_event("selected occurrence search was cancelled by terminal output"),
        );
    }
    match job.process_chunk(session) {
        Ok(true) => Some(job.completion_event()),
        Ok(false) => {
            jobs.push_back(job);
            None
        }
        Err(error) => Some(job.cancellation_event(&error.to_string())),
    }
}

pub(super) fn replace_selected_occurrence_search_job(
    jobs: &mut VecDeque<SelectedOccurrenceSearchJob>,
    job: SelectedOccurrenceSearchJob,
) -> Option<TerminalFrameSearchEvent> {
    let replaced = jobs
        .iter()
        .position(|pending| pending.session_id == job.session_id)
        .and_then(|index| jobs.remove(index));
    jobs.push_back(job);
    replaced.map(|stale| {
        stale.cancellation_event("selected occurrence search was replaced by a newer request")
    })
}

pub(super) fn cancel_selected_occurrence_search_job_for_session(
    jobs: &mut VecDeque<SelectedOccurrenceSearchJob>,
    session_id: &str,
    message: &str,
) -> Option<TerminalFrameSearchEvent> {
    jobs.iter()
        .position(|job| job.session_id == session_id)
        .and_then(|index| jobs.remove(index))
        .map(|job| job.cancellation_event(message))
}
