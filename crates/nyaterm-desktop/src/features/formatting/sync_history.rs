use nyaterm_core::CloudSyncHistoryEntry;
use rust_i18n::t;
use time::{OffsetDateTime, UtcOffset};

pub(in crate::features) fn cloud_sync_history_trigger(trigger: &str) -> String {
    t!(match trigger {
        "manual_push" | "manual_provider_push" => "settings.syncHistory.push",
        "manual_pull" | "manual_provider_pull" => "settings.syncHistory.pull",
        "manual_force_push" | "manual_provider_force_push" => "settings.syncHistory.forcePush",
        "manual_force_pull" | "manual_provider_force_pull" => "settings.syncHistory.forcePull",
        "recover_current_remote" => "settings.syncHistory.recover",
        "auto_startup" => "settings.syncHistory.startup",
        "auto_periodic" => "settings.syncHistory.periodic",
        "auto_focus" => "settings.syncHistory.focus",
        "auto_settings" => "settings.syncHistory.settings",
        "auto_change" => "settings.syncHistory.change",
        "auto_deferred_pull" => "settings.syncHistory.deferredPull",
        "auto_retry" => "settings.syncHistory.retry",
        _ => "settings.syncHistory.other",
    })
    .to_string()
}

pub(in crate::features) fn cloud_sync_history_summary(entry: &CloudSyncHistoryEntry) -> String {
    // Stored messages can be technical, long, or in a previous UI language.
    // Preserve them verbatim in details and use stable semantic summaries here.
    if entry.kind == "sync" && entry.status == "success" {
        // A successful push may be UpToDate. The trigger alone cannot prove
        // that bytes were uploaded or downloaded. Recognize only exact outcome
        // messages emitted by supported UI languages, otherwise stay neutral.
        for key in [
            "settings.syncUpToDate",
            "settings.syncPushSuccess",
            "settings.syncPullSuccess",
            "settings.syncRecoverCurrentSuccess",
        ] {
            if ["en", "zh-CN", "zh-TW", "ja", "ko", "fr", "pt-BR"]
                .iter()
                .any(|locale| entry.message.trim() == t!(key, locale = *locale).as_ref())
            {
                return t!(key).to_string();
            }
        }
    }
    t!(match entry.status.as_str() {
        "conflict" => "settings.syncHistory.conflict",
        "failed" => "settings.syncHistory.failed",
        "cancelled" | "canceled" => "settings.syncHistory.cancelled",
        "running" => "settings.syncState.running",
        "success" if entry.kind == "backup" => "settings.syncHistory.backup",
        "success" => "settings.syncHistory.completed",
        _ => "settings.syncHistory.recorded",
    })
    .to_string()
}

pub(in crate::features) struct SyncHistoryTimeFormatter {
    now: OffsetDateTime,
}

impl SyncHistoryTimeFormatter {
    pub(in crate::features) fn new() -> Self {
        let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
        Self {
            now: OffsetDateTime::now_utc().to_offset(offset),
        }
    }

    fn datetime(&self, timestamp_ms: u64) -> Option<OffsetDateTime> {
        if timestamp_ms == 0 {
            return None;
        }
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(timestamp_ms) * 1_000_000)
            .ok()
            .map(|value| value.to_offset(self.now.offset()))
    }

    pub(in crate::features) fn day(&self, timestamp_ms: u64) -> String {
        let Some(value) = self.datetime(timestamp_ms) else {
            return t!("settings.syncHistory.unknownDate").to_string();
        };
        if value.date() == self.now.date() {
            t!("settings.syncHistory.today").to_string()
        } else if Some(value.date()) == self.now.date().previous_day() {
            t!("settings.syncHistory.yesterday").to_string()
        } else {
            value.date().to_string()
        }
    }

    pub(in crate::features) fn clock(&self, timestamp_ms: u64) -> String {
        self.datetime(timestamp_ms)
            .map(|value| format!("{:02}:{:02}", value.hour(), value.minute()))
            .unwrap_or_else(|| "—".to_string())
    }

    pub(in crate::features) fn full(&self, timestamp_ms: u64) -> String {
        self.datetime(timestamp_ms)
            .map(|value| {
                format!(
                    "{} {:02}:{:02}:{:02}",
                    value.date(),
                    value.hour(),
                    value.minute(),
                    value.second()
                )
            })
            .unwrap_or_else(|| t!("settings.syncHistory.unknownDate").to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{SyncHistoryTimeFormatter, cloud_sync_history_summary};
    use nyaterm_core::CloudSyncHistoryEntry;
    use time::{Date, Month, UtcOffset};

    #[test]
    fn successful_push_does_not_imply_an_upload_and_old_language_outcomes_are_localized() {
        let mut entry = CloudSyncHistoryEntry::sync(
            "success",
            "manual_provider_push",
            None,
            None,
            rust_i18n::t!("settings.syncUpToDate", locale = "en").to_string(),
        );
        assert_eq!(
            cloud_sync_history_summary(&entry),
            rust_i18n::t!("settings.syncUpToDate")
        );
        entry.message = "unknown technical result".to_string();
        assert_eq!(
            cloud_sync_history_summary(&entry),
            rust_i18n::t!("settings.syncHistory.completed")
        );
        entry.status = "conflict".to_string();
        assert_eq!(
            cloud_sync_history_summary(&entry),
            rust_i18n::t!("settings.syncHistory.conflict")
        );
    }

    #[test]
    fn history_groups_use_local_calendar_days_across_year_boundaries() {
        let offset = UtcOffset::from_hms(8, 0, 0).unwrap();
        let now = Date::from_calendar_date(2026, Month::January, 1)
            .unwrap()
            .with_hms(0, 10, 0)
            .unwrap()
            .assume_offset(offset);
        let formatter = SyncHistoryTimeFormatter { now };
        let previous = now - time::Duration::minutes(20);
        let timestamp = (previous.unix_timestamp_nanos() / 1_000_000) as u64;
        assert_eq!(formatter.clock(timestamp), "23:50");
        assert_eq!(
            formatter.day(timestamp),
            rust_i18n::t!("settings.syncHistory.yesterday")
        );
        assert_eq!(formatter.full(timestamp), "2025-12-31 23:50:00");
        let earlier = (now - time::Duration::days(2)).unix_timestamp_nanos() / 1_000_000;
        assert_eq!(formatter.day(earlier as u64), "2025-12-30");
        assert_eq!(formatter.clock(u64::MAX), "—");
        assert_eq!(formatter.clock(0), "—");
    }
}
