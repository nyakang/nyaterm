//! Today's log tail for one network tool.

use std::path::{Path, PathBuf};

use nyaterm_transport::networktools::NetworkToolKind;

/// Read today's tail from this service's own JSONL files. The application
/// routes each service target into `nyaterm-<service>.<date>.jsonl`, so no
/// target filtering is needed here; other sinks and other services are never in
/// these files. Only the current local day is read, including that day's
/// `_NNN` shards; older days stay in the log directory for manual inspection.
pub(in crate::features) fn read_today(
    dir: &Path,
    kind: NetworkToolKind,
    limit: usize,
) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let today = nyaterm_core::diagnostics::local_now().date();
    let prefix = format!(
        "nyaterm-{}.{:04}-{:02}-{:02}",
        kind.log_name(),
        today.year(),
        u8::from(today.month()),
        today.day()
    );
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(&prefix) && name.ends_with(nyaterm_core::LOG_FILE_SUFFIX)
                })
        })
        .collect();
    files.sort();

    let mut lines = Vec::new();
    for path in files.iter().rev() {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let mut matched: Vec<String> = text.lines().map(str::to_string).collect();
        matched.reverse();
        lines.extend(matched);
        if lines.len() >= limit {
            break;
        }
    }
    lines.truncate(limit);
    lines.reverse();
    lines
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use nyaterm_transport::networktools::NetworkToolKind;
    use tracing_subscriber::{EnvFilter, layer::SubscriberExt};

    use super::read_today;

    /// Copied from `nyaterm-app`'s default filter; keep them in sync. The bare
    /// `networktools` target used to fall through to the trailing `warn`, which
    /// dropped every lifecycle `info` event before it reached the JSONL sink.
    const DEFAULT_FILTER: &str =
        "nyaterm=info,nyaterm_core=info,nyaterm_transport=info,nyaterm_windows_rdp=info,warn";

    /// The panel reads the local day it is opened on, so fixtures follow the
    /// same clock rather than a hard-coded date.
    fn today_date() -> String {
        let today = nyaterm_core::diagnostics::local_now().date();
        format!(
            "{:04}-{:02}-{:02}",
            today.year(),
            u8::from(today.month()),
            today.day()
        )
    }

    fn today_stem() -> String {
        format!("nyaterm-tftp.{}", today_date())
    }

    fn service_file() -> String {
        format!("{}.jsonl", today_stem())
    }

    /// Emit events through the production filter into the service's real JSONL
    /// file, then read them back through the configuration window's reader.
    fn capture(dir: &Path, emit: impl FnOnce()) -> Vec<String> {
        let file = fs::File::create(dir.join(service_file())).expect("create log file");
        {
            let subscriber = tracing_subscriber::registry()
                .with(EnvFilter::new(DEFAULT_FILTER))
                .with(tracing_subscriber::fmt::layer().json().with_writer(file));
            tracing::subscriber::with_default(subscriber, emit);
        }
        read_today(dir, NetworkToolKind::Tftp, 200)
    }

    #[test]
    fn bare_networktools_target_is_dropped_by_the_default_global_level() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let lines = capture(dir.path(), || {
            tracing::info!(target: "networktools", service = "tftp", "transfer started");
        });
        assert!(
            lines.is_empty(),
            "a bare target inherits the global warn fallback and must not reach the file: {lines:#?}"
        );
    }

    #[test]
    fn lifecycle_events_under_the_service_target_reach_its_own_file() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = NetworkToolKind::Tftp.log_target();
        let lines = capture(dir.path(), || {
            tracing::info!(target: "nyaterm::networktools::tftp", service = "tftp", "transfer started");
            tracing::info!(target: "nyaterm::networktools::tftp", service = "tftp", "transfer finished");
            tracing::error!(target: "nyaterm::networktools::tftp", service = "tftp", "transfer failed");
        });
        assert_eq!(lines.len(), 3, "{lines:#?}");
        assert!(
            lines
                .iter()
                .all(|line| line.contains("\"target\":\"nyaterm::networktools::tftp\""))
        );
        assert_eq!(target, "nyaterm::networktools::tftp");
        assert!(lines[0].contains("transfer started"));
        assert!(lines[1].contains("transfer finished"));
        assert!(lines[2].contains("transfer failed"));
    }

    #[test]
    fn the_log_view_reads_only_the_service_file() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        fs::write(
            dir.path()
                .join(format!("nyaterm-diagnostics.{}.jsonl", today_date())),
            "{\"target\":\"nyaterm\",\"fields\":{\"message\":\"diagnostics\"}}\n",
        )
        .expect("write diagnostics fixture");
        fs::write(
            dir.path().join(format!("nyaterm-ftp.{}.jsonl", today_date())),
            "{\"target\":\"nyaterm::networktools::ftp\",\"fields\":{\"message\":\"other service\"}}\n",
        )
        .expect("write other service fixture");
        fs::write(
            dir.path().join(service_file()),
            "{\"target\":\"nyaterm::networktools::tftp\",\"fields\":{\"message\":\"ours\"}}\n",
        )
        .expect("write service fixture");

        let lines = read_today(dir.path(), NetworkToolKind::Tftp, 200);
        assert_eq!(lines.len(), 1, "{lines:#?}");
        assert!(lines[0].contains("ours"));
    }

    #[test]
    fn only_today_and_its_shards_are_read() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let today = nyaterm_core::diagnostics::local_now().date();
        let yesterday = today.previous_day().expect("yesterday exists");
        let yesterday_file = format!(
            "nyaterm-tftp.{:04}-{:02}-{:02}.jsonl",
            yesterday.year(),
            u8::from(yesterday.month()),
            yesterday.day()
        );
        fs::write(
            dir.path().join(yesterday_file),
            "{\"fields\":{\"message\":\"yesterday\"}}\n",
        )
        .expect("write yesterday fixture");
        fs::write(
            dir.path().join(service_file()),
            "{\"fields\":{\"message\":\"today base\"}}\n",
        )
        .expect("write base fixture");
        fs::write(
            dir.path().join(format!("{}_001.jsonl", today_stem())),
            "{\"fields\":{\"message\":\"today shard\"}}\n",
        )
        .expect("write shard fixture");

        let lines = read_today(dir.path(), NetworkToolKind::Tftp, 200);
        assert_eq!(lines.len(), 2, "{lines:#?}");
        assert!(lines.iter().all(|line| !line.contains("yesterday")));
        assert!(lines[0].contains("today base"));
        assert!(lines[1].contains("today shard"));
    }
}
