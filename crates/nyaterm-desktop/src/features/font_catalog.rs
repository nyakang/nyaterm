use std::collections::{HashMap, HashSet, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::features::terminal::TerminalFontMeasurementFailure;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum FontCatalogLoadState {
    Unloaded,
    Loading,
    Loaded,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum FontCatalogKind {
    Ui,
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum FontResolutionSource {
    Configured,
    UserFallback(usize),
    PlatformDefault,
    EmergencyMetricsFallback,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) struct FontResolutionStatus {
    pub(in crate::features) configured_family: String,
    pub(in crate::features) effective_family: String,
    pub(in crate::features) source: FontResolutionSource,
    pub(in crate::features) reason: Option<FontAvailabilityReason>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum FontAvailabilityReason {
    NotInstalled,
    FontNotResolved,
    ResolvedFamilyMismatch,
    MissingGlyphAdvance,
    NotMonospaced,
    ShapingMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) enum FontAvailability {
    Available { resolved_family: Arc<str> },
    Internal,
    Unavailable { reason: FontAvailabilityReason },
    Checking,
}

impl FontAvailability {
    pub(in crate::features) fn resolved_family(&self) -> Option<&str> {
        match self {
            Self::Available { resolved_family } => Some(resolved_family),
            Self::Internal | Self::Unavailable { .. } | Self::Checking => None,
        }
    }

    pub(in crate::features) fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. } | Self::Internal)
    }
}

impl From<TerminalFontMeasurementFailure> for FontAvailabilityReason {
    fn from(value: TerminalFontMeasurementFailure) -> Self {
        match value {
            TerminalFontMeasurementFailure::FontNotResolved => Self::FontNotResolved,
            TerminalFontMeasurementFailure::ResolvedFamilyMismatch => Self::ResolvedFamilyMismatch,
            TerminalFontMeasurementFailure::MissingGlyphAdvance => Self::MissingGlyphAdvance,
            TerminalFontMeasurementFailure::NotMonospaced => Self::NotMonospaced,
            TerminalFontMeasurementFailure::ShapingMismatch => Self::ShapingMismatch,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) struct FontCatalogEntry {
    pub(in crate::features) family: String,
    pub(in crate::features) ui: FontAvailability,
    pub(in crate::features) terminal: FontAvailability,
}

impl FontCatalogEntry {
    pub(in crate::features) fn new(
        family: String,
        ui: FontAvailability,
        terminal: FontAvailability,
    ) -> Self {
        Self {
            family,
            ui,
            terminal,
        }
    }

    fn availability(&self, kind: FontCatalogKind) -> FontAvailability {
        match kind {
            FontCatalogKind::Ui => self.ui.clone(),
            FontCatalogKind::Terminal => self.terminal.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(in crate::features) struct FontCatalogSnapshot {
    generation: u64,
    system_font_fingerprint: u64,
    ui_options: Vec<String>,
    terminal_options: Vec<String>,
    probes: HashMap<String, FontCatalogEntry>,
}

impl FontCatalogSnapshot {
    pub(in crate::features) fn from_entries(
        generation: u64,
        entries: impl IntoIterator<Item = FontCatalogEntry>,
    ) -> Self {
        let mut probes: HashMap<String, FontCatalogEntry> = HashMap::new();
        for entry in entries {
            let key = normalize_font_family(&entry.family);
            if let Some(existing) = probes.get_mut(&key) {
                existing.ui = merge_availability(&existing.ui, &entry.ui);
                existing.terminal = merge_availability(&existing.terminal, &entry.terminal);
            } else {
                probes.insert(key, entry);
            }
        }

        let mut ui_options = probes
            .values()
            .filter(|entry| {
                is_user_selectable_font_family(&entry.family, FontCatalogKind::Ui)
                    && entry.ui.is_available()
            })
            .map(|entry| entry.family.clone())
            .collect::<Vec<_>>();
        let mut terminal_options = probes
            .values()
            .filter(|entry| {
                is_user_selectable_font_family(&entry.family, FontCatalogKind::Terminal)
                    && entry.terminal.is_available()
            })
            .map(|entry| entry.family.clone())
            .collect::<Vec<_>>();
        sort_font_options(&mut ui_options);
        sort_font_options(&mut terminal_options);

        Self {
            generation,
            system_font_fingerprint: font_names_fingerprint(probes.keys()),
            ui_options,
            terminal_options,
            probes,
        }
    }

    pub(in crate::features) fn generation(&self) -> u64 {
        self.generation
    }

    pub(in crate::features) fn system_font_fingerprint(&self) -> u64 {
        self.system_font_fingerprint
    }

    pub(in crate::features) fn ui_options(&self) -> &[String] {
        &self.ui_options
    }

    pub(in crate::features) fn terminal_options(&self) -> &[String] {
        &self.terminal_options
    }

    pub(in crate::features) fn availability(
        &self,
        family: &str,
        kind: FontCatalogKind,
    ) -> FontAvailability {
        if is_internal_font_family(family, kind) {
            return FontAvailability::Internal;
        }
        self.probes
            .get(&normalize_font_family(family))
            .map(|entry| entry.availability(kind))
            .unwrap_or(FontAvailability::Unavailable {
                reason: FontAvailabilityReason::NotInstalled,
            })
    }

    pub(in crate::features) fn resolve_stack(
        &self,
        families: &[String],
        kind: FontCatalogKind,
        platform_default: &str,
    ) -> FontResolutionStatus {
        let configured_family = families.first().cloned().unwrap_or_default();
        let mut first_reason = None;
        for (index, family) in families.iter().enumerate() {
            match self.availability(family, kind) {
                FontAvailability::Available { resolved_family } => {
                    return FontResolutionStatus {
                        configured_family,
                        effective_family: resolved_family.to_string(),
                        source: if index == 0 {
                            FontResolutionSource::Configured
                        } else {
                            FontResolutionSource::UserFallback(index)
                        },
                        reason: first_reason,
                    };
                }
                FontAvailability::Internal => {
                    return FontResolutionStatus {
                        configured_family,
                        effective_family: family.clone(),
                        source: if index == 0 {
                            FontResolutionSource::Configured
                        } else {
                            FontResolutionSource::UserFallback(index)
                        },
                        reason: first_reason,
                    };
                }
                FontAvailability::Unavailable { reason } => {
                    first_reason.get_or_insert(reason);
                }
                FontAvailability::Checking => continue,
            }
        }

        let effective_family = self
            .availability(platform_default, kind)
            .resolved_family()
            .unwrap_or(platform_default)
            .to_string();
        FontResolutionStatus {
            configured_family,
            effective_family,
            source: FontResolutionSource::PlatformDefault,
            reason: first_reason,
        }
    }
}

#[derive(Clone, Debug)]
pub(in crate::features) struct FontCatalogPresentation {
    state: FontCatalogLoadState,
    generation: u64,
    snapshot: Arc<FontCatalogSnapshot>,
}

impl PartialEq for FontCatalogPresentation {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state && self.generation == other.generation
    }
}

impl Eq for FontCatalogPresentation {}

impl FontCatalogPresentation {
    pub(in crate::features) fn empty() -> Self {
        Self::new(
            FontCatalogLoadState::Unloaded,
            0,
            Arc::new(FontCatalogSnapshot::default()),
        )
    }

    pub(in crate::features) fn new(
        state: FontCatalogLoadState,
        generation: u64,
        snapshot: Arc<FontCatalogSnapshot>,
    ) -> Self {
        Self {
            state,
            generation,
            snapshot,
        }
    }

    pub(in crate::features) fn state(&self) -> FontCatalogLoadState {
        self.state
    }

    pub(in crate::features) fn generation(&self) -> u64 {
        self.generation
    }

    pub(in crate::features) fn ui_options(&self) -> &[String] {
        self.snapshot.ui_options()
    }

    pub(in crate::features) fn terminal_options(&self) -> &[String] {
        self.snapshot.terminal_options()
    }

    pub(in crate::features) fn availability(
        &self,
        family: &str,
        kind: FontCatalogKind,
    ) -> FontAvailability {
        if self.state != FontCatalogLoadState::Loaded {
            return FontAvailability::Checking;
        }
        self.snapshot.availability(family, kind)
    }

    pub(in crate::features) fn resolve_stack(
        &self,
        families: &[String],
        kind: FontCatalogKind,
        platform_default: &str,
    ) -> Option<FontResolutionStatus> {
        if self.state != FontCatalogLoadState::Loaded {
            return None;
        }
        Some(
            self.snapshot
                .resolve_stack(families, kind, platform_default),
        )
    }
}

pub(in crate::features) struct FontCatalogState {
    snapshot: Arc<FontCatalogSnapshot>,
    state: FontCatalogLoadState,
    generation: u64,
    system_font_fingerprint: Option<u64>,
    fingerprint_check_in_flight: bool,
}

impl FontCatalogState {
    pub(in crate::features) fn new(ui_options: Vec<String>, terminal_options: Vec<String>) -> Self {
        let generation = 0;
        let state = if ui_options.is_empty() && terminal_options.is_empty() {
            FontCatalogLoadState::Unloaded
        } else {
            FontCatalogLoadState::Loaded
        };
        let ui_families = ui_options
            .iter()
            .map(|family| normalize_font_family(family))
            .collect::<HashSet<_>>();
        let terminal_families = terminal_options
            .iter()
            .map(|family| normalize_font_family(family))
            .collect::<HashSet<_>>();
        let mut families = ui_options;
        families.extend(terminal_options);
        let entries = families.into_iter().map(|family| {
            let key = normalize_font_family(&family);
            let ui = if ui_families.contains(&key) {
                FontAvailability::Available {
                    resolved_family: family.clone().into(),
                }
            } else {
                FontAvailability::Unavailable {
                    reason: FontAvailabilityReason::NotInstalled,
                }
            };
            let terminal = if terminal_families.contains(&key) {
                FontAvailability::Available {
                    resolved_family: family.clone().into(),
                }
            } else {
                FontAvailability::Unavailable {
                    reason: FontAvailabilityReason::NotInstalled,
                }
            };
            FontCatalogEntry::new(family, ui, terminal)
        });
        Self {
            snapshot: Arc::new(FontCatalogSnapshot::from_entries(generation, entries)),
            state,
            generation,
            // The initial options are only a compatibility seed and do not represent the
            // complete system font list. The first lightweight check must therefore be able
            // to trigger one full refresh.
            system_font_fingerprint: None,
            fingerprint_check_in_flight: false,
        }
    }

    pub(in crate::features) fn snapshot(&self) -> &FontCatalogSnapshot {
        &self.snapshot
    }

    pub(in crate::features) fn snapshot_arc(&self) -> Arc<FontCatalogSnapshot> {
        Arc::clone(&self.snapshot)
    }

    pub(in crate::features) fn state(&self) -> FontCatalogLoadState {
        self.state
    }

    pub(in crate::features) fn generation(&self) -> u64 {
        self.generation
    }

    pub(in crate::features) fn begin_load(&mut self) -> Option<u64> {
        if self.state == FontCatalogLoadState::Loading {
            return None;
        }
        if self.state == FontCatalogLoadState::Loaded {
            return None;
        }
        self.generation = self.generation.saturating_add(1);
        self.fingerprint_check_in_flight = false;
        self.state = FontCatalogLoadState::Loading;
        Some(self.generation)
    }

    pub(in crate::features) fn begin_refresh(&mut self) -> Option<u64> {
        if self.state == FontCatalogLoadState::Loading {
            return None;
        }
        self.generation = self.generation.saturating_add(1);
        self.fingerprint_check_in_flight = false;
        self.state = FontCatalogLoadState::Loading;
        Some(self.generation)
    }

    pub(in crate::features) fn commit(
        &mut self,
        generation: u64,
        snapshot: FontCatalogSnapshot,
    ) -> bool {
        if self.generation != generation || snapshot.generation() != generation {
            return false;
        }
        self.snapshot = Arc::new(snapshot);
        self.system_font_fingerprint = Some(self.snapshot.system_font_fingerprint());
        self.fingerprint_check_in_flight = false;
        self.state = FontCatalogLoadState::Loaded;
        true
    }

    pub(in crate::features) fn begin_font_names_fingerprint_check(&mut self) -> bool {
        if self.state != FontCatalogLoadState::Loaded || self.fingerprint_check_in_flight {
            return false;
        }
        self.fingerprint_check_in_flight = true;
        true
    }

    pub(in crate::features) fn finish_font_names_fingerprint_check(
        &mut self,
        fingerprint: u64,
    ) -> bool {
        let should_refresh = self.fingerprint_check_in_flight
            && self.state == FontCatalogLoadState::Loaded
            && self.system_font_fingerprint != Some(fingerprint);
        self.fingerprint_check_in_flight = false;
        should_refresh
    }

    pub(in crate::features) fn cancel_font_names_fingerprint_check(&mut self) {
        self.fingerprint_check_in_flight = false;
    }

    pub(in crate::features) fn fail(&mut self, generation: u64) -> bool {
        if self.generation != generation {
            return false;
        }
        self.fingerprint_check_in_flight = false;
        self.state = FontCatalogLoadState::Failed;
        true
    }

    pub(in crate::features) fn resolve_stack(
        &self,
        families: &[String],
        kind: FontCatalogKind,
        platform_default: &str,
    ) -> Option<FontResolutionStatus> {
        if self.state != FontCatalogLoadState::Loaded {
            return None;
        }
        Some(
            self.snapshot
                .resolve_stack(families, kind, platform_default),
        )
    }
}

pub(in crate::features) fn normalize_font_family(value: &str) -> String {
    value
        .trim()
        .trim_matches(|ch| ch == '"' || ch == '\'')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

pub(in crate::features) fn font_names_fingerprint<'a, I>(font_names: I) -> u64
where
    I: IntoIterator<Item = &'a String>,
{
    let mut normalized_names = font_names
        .into_iter()
        .map(|family| normalize_font_family(family))
        .filter(|family| !family.is_empty())
        .collect::<Vec<_>>();
    normalized_names.sort_unstable();
    normalized_names.dedup();

    let mut hasher = DefaultHasher::new();
    normalized_names.hash(&mut hasher);
    hasher.finish()
}

fn sort_font_options(options: &mut [String]) {
    options.sort_by_key(|family| normalize_font_family(family));
}

fn merge_availability(current: &FontAvailability, incoming: &FontAvailability) -> FontAvailability {
    if current.is_available() {
        current.clone()
    } else {
        incoming.clone()
    }
}

fn is_internal_font_family(family: &str, kind: FontCatalogKind) -> bool {
    let family = normalize_font_family(family);
    match kind {
        FontCatalogKind::Terminal => matches!(
            family.as_str(),
            "monospace" | "system-monospace" | "ui-monospace"
        ),
        FontCatalogKind::Ui => matches!(
            family.as_str(),
            "system-ui" | "ui-sans-serif" | "sans-serif" | "serif"
        ),
    }
}

fn is_user_selectable_font_family(family: &str, kind: FontCatalogKind) -> bool {
    let family = normalize_font_family(family);
    !family.starts_with('.')
        && !matches!(
            family.as_str(),
            "monospace"
                | "system-monospace"
                | "ui-monospace"
                | "system-ui"
                | "ui-sans-serif"
                | "sans-serif"
                | "serif"
        )
        && !is_internal_font_family(&family, kind)
        && !is_symbol_icon_font_family(&family)
}

/// Symbol faces cannot serve as the primary font for ordinary UI or terminal text.
/// This only filters user options; the font catalog retains them for internal use.
fn is_symbol_icon_font_family(family: &str) -> bool {
    matches!(
        family,
        "wingdings"
            | "wingdings 2"
            | "wingdings 3"
            | "webdings"
            | "marlett"
            | "symbol"
            | "segoe fluent icons"
            | "segoe mdl2 assets"
            | "segoe ui symbol"
            | "hololens mdl2 assets"
            | "segmdl2"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        FontAvailability, FontAvailabilityReason, FontCatalogEntry, FontCatalogKind,
        FontCatalogLoadState, FontCatalogSnapshot, FontCatalogState, FontResolutionSource,
        font_names_fingerprint,
    };

    #[test]
    fn catalog_filters_unavailable_and_sorts_options() {
        let snapshot = FontCatalogSnapshot::from_entries(
            0,
            [
                super::FontCatalogEntry::new(
                    "Zed Mono".to_string(),
                    FontAvailability::Available {
                        resolved_family: "Zed Mono".into(),
                    },
                    FontAvailability::Available {
                        resolved_family: "Zed Mono".into(),
                    },
                ),
                super::FontCatalogEntry::new(
                    "Missing Mono".to_string(),
                    FontAvailability::Unavailable {
                        reason: FontAvailabilityReason::NotInstalled,
                    },
                    FontAvailability::Unavailable {
                        reason: FontAvailabilityReason::NotInstalled,
                    },
                ),
                super::FontCatalogEntry::new(
                    "Inter".to_string(),
                    FontAvailability::Available {
                        resolved_family: "Inter".into(),
                    },
                    FontAvailability::Unavailable {
                        reason: FontAvailabilityReason::NotMonospaced,
                    },
                ),
            ],
        );

        assert_eq!(snapshot.ui_options(), ["Inter", "Zed Mono"]);
        assert_eq!(snapshot.terminal_options(), ["Zed Mono"]);
        assert!(matches!(
            snapshot.availability("Missing Mono", FontCatalogKind::Terminal),
            FontAvailability::Unavailable {
                reason: FontAvailabilityReason::NotInstalled
            }
        ));
    }

    #[test]
    fn generic_families_are_internal_and_not_user_options() {
        let snapshot = FontCatalogSnapshot::from_entries(
            0,
            [
                super::FontCatalogEntry::new(
                    "monospace".to_string(),
                    FontAvailability::Available {
                        resolved_family: "monospace".into(),
                    },
                    FontAvailability::Available {
                        resolved_family: "monospace".into(),
                    },
                ),
                super::FontCatalogEntry::new(
                    ".SystemUIFont".to_string(),
                    FontAvailability::Available {
                        resolved_family: ".SystemUIFont".into(),
                    },
                    FontAvailability::Unavailable {
                        reason: FontAvailabilityReason::NotInstalled,
                    },
                ),
            ],
        );
        assert!(matches!(
            snapshot.availability("monospace", FontCatalogKind::Terminal),
            FontAvailability::Internal
        ));
        assert!(matches!(
            snapshot.availability("system-ui", FontCatalogKind::Ui),
            FontAvailability::Internal
        ));
        assert!(snapshot.terminal_options().is_empty());
        assert!(snapshot.ui_options().is_empty());
    }

    #[test]
    fn icon_faces_are_kept_out_of_both_user_font_lists() {
        let available = |family: &str| {
            super::FontCatalogEntry::new(
                family.to_string(),
                FontAvailability::Available {
                    resolved_family: family.into(),
                },
                FontAvailability::Available {
                    resolved_family: family.into(),
                },
            )
        };
        let snapshot = FontCatalogSnapshot::from_entries(
            0,
            [
                available("Segoe Fluent Icons"),
                available("Segoe MDL2 Assets"),
                available("HoloLens MDL2 Assets"),
                available("Wingdings"),
                available("Wingdings 2"),
                available("Wingdings 3"),
                available("Cascadia Mono"),
            ],
        );

        assert_eq!(snapshot.terminal_options(), ["Cascadia Mono"]);
        assert_eq!(snapshot.ui_options(), ["Cascadia Mono"]);
        assert!(
            snapshot
                .availability("Segoe MDL2 Assets", FontCatalogKind::Ui)
                .is_available()
        );
    }

    #[test]
    fn localized_and_non_latin_text_fonts_remain_user_options() {
        for family in ["宋体", "微软雅黑", "Microsoft Himalaya", "Himalaya"] {
            assert!(super::is_user_selectable_font_family(
                family,
                FontCatalogKind::Ui
            ));
            assert!(super::is_user_selectable_font_family(
                family,
                FontCatalogKind::Terminal
            ));
        }
    }

    #[test]
    fn family_normalization_handles_quotes_and_case() {
        let snapshot = FontCatalogSnapshot::from_entries(
            0,
            [super::FontCatalogEntry::new(
                "JetBrains Mono".to_string(),
                FontAvailability::Available {
                    resolved_family: "JetBrains Mono".into(),
                },
                FontAvailability::Available {
                    resolved_family: "JetBrains Mono".into(),
                },
            )],
        );
        assert!(
            snapshot
                .availability("  \"jetbrains   mono\"  ", FontCatalogKind::Terminal)
                .is_available()
        );
    }

    #[test]
    fn failed_catalog_load_can_retry_without_accepting_stale_results() {
        let mut state = FontCatalogState::new(Vec::new(), Vec::new());
        let first_generation = state.begin_load().expect("first catalog load");
        assert!(state.fail(first_generation));
        assert_eq!(state.state(), FontCatalogLoadState::Failed);

        let second_generation = state.begin_load().expect("retry catalog load");
        assert!(second_generation > first_generation);
        assert!(!state.commit(
            second_generation,
            FontCatalogSnapshot::from_entries(first_generation, []),
        ));
        assert!(!state.commit(
            first_generation,
            FontCatalogSnapshot::from_entries(first_generation, []),
        ));
        assert_eq!(state.state(), FontCatalogLoadState::Loading);
        assert!(state.commit(
            second_generation,
            FontCatalogSnapshot::from_entries(second_generation, []),
        ));
        assert_eq!(state.state(), FontCatalogLoadState::Loaded);
    }

    #[test]
    fn resolution_reports_first_available_fallback_and_reason() {
        let mut state = FontCatalogState::new(Vec::new(), Vec::new());
        let generation = state.begin_load().expect("catalog load");
        assert!(state.commit(
            generation,
            FontCatalogSnapshot::from_entries(
                generation,
                [FontCatalogEntry::new(
                    "JetBrains Mono".to_string(),
                    FontAvailability::Unavailable {
                        reason: FontAvailabilityReason::NotInstalled,
                    },
                    FontAvailability::Available {
                        resolved_family: "JetBrains Mono".into(),
                    },
                )],
            ),
        ));

        let status = state
            .resolve_stack(
                &["Missing Mono".to_string(), "JetBrains Mono".to_string()],
                FontCatalogKind::Terminal,
                "monospace",
            )
            .expect("resolved font");
        assert_eq!(status.effective_family, "JetBrains Mono");
        assert_eq!(status.source, FontResolutionSource::UserFallback(1));
        assert_eq!(status.reason, Some(FontAvailabilityReason::NotInstalled));

        let platform_status = state
            .resolve_stack(&[], FontCatalogKind::Terminal, "monospace")
            .expect("platform fallback");
        assert_eq!(
            platform_status.source,
            FontResolutionSource::PlatformDefault
        );
        assert_eq!(platform_status.effective_family, "monospace");
    }

    #[test]
    fn loaded_catalog_can_be_refreshed_explicitly() {
        let mut state = FontCatalogState::new(vec!["Inter".to_string()], Vec::new());
        assert_eq!(state.state(), FontCatalogLoadState::Loaded);
        let generation = state.begin_refresh().expect("explicit catalog refresh");
        assert_eq!(state.state(), FontCatalogLoadState::Loading);
        assert!(state.commit(
            generation,
            FontCatalogSnapshot::from_entries(generation, []),
        ));
        assert!(state.snapshot().ui_options().is_empty());
    }

    #[test]
    fn font_name_fingerprint_is_order_insensitive_and_normalized() {
        let first = vec!["Inter".to_string(), "JetBrains  Mono".to_string()];
        let reordered = vec![" jetbrains mono ".to_string(), "inter".to_string()];
        let changed = vec!["Inter".to_string(), "Menlo".to_string()];

        assert_eq!(
            font_names_fingerprint(&first),
            font_names_fingerprint(&reordered)
        );
        assert_ne!(
            font_names_fingerprint(&first),
            font_names_fingerprint(&changed)
        );
    }

    #[test]
    fn loaded_catalog_skips_unchanged_fingerprint_and_refreshes_changed_one() {
        let mut state = FontCatalogState::new(Vec::new(), Vec::new());
        let generation = state.begin_load().expect("catalog load");
        let snapshot = FontCatalogSnapshot::from_entries(
            generation,
            [FontCatalogEntry::new(
                "Inter".to_string(),
                FontAvailability::Available {
                    resolved_family: "Inter".into(),
                },
                FontAvailability::Unavailable {
                    reason: FontAvailabilityReason::NotMonospaced,
                },
            )],
        );
        let fingerprint = snapshot.system_font_fingerprint();
        assert!(state.commit(generation, snapshot));

        assert!(state.begin_font_names_fingerprint_check());
        assert!(!state.begin_font_names_fingerprint_check());
        assert!(!state.finish_font_names_fingerprint_check(fingerprint));

        assert!(state.begin_font_names_fingerprint_check());
        assert!(state.finish_font_names_fingerprint_check(fingerprint.wrapping_add(1)));
        assert_eq!(state.state(), FontCatalogLoadState::Loaded);
        assert!(state.begin_refresh().is_some());
    }
}
