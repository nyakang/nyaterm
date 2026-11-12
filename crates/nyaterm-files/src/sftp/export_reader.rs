use super::{close_sftp_session, compatibility_runtime, remote_metadata::remote_file_path_bytes};
use crate::drag_export::{ExportObserver, ExportReadEvent, ReadRequest, RemoteDragFile};
use crate::remote_file::{RemoteFileBackendKind, RemoteFileService};
use crate::sftp::SftpTransferControl;
use crate::sftp_transfer_types::SftpTransferOptions;

mod prefetch;
use prefetch::Prefetch;
use std::{
    io,
    sync::{Arc, Weak},
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinHandle};

/// Schedules on the connection's existing runtime; no new authentication policy
/// or SFTP implementation is introduced by drag export.
pub(crate) fn spawn_export_reader(
    source: Weak<RemoteFileService>,
    file: RemoteDragFile,
    options: SftpTransferOptions,
    control: Arc<SftpTransferControl>,
    requests: mpsc::Receiver<ReadRequest>,
    observer: ExportObserver,
) -> io::Result<JoinHandle<()>> {
    let service = source
        .upgrade()
        .ok_or_else(|| io::Error::other("source session is closed"))?;
    if service.selected_backend() != Some(RemoteFileBackendKind::Sftp) {
        return Err(io::ErrorKind::Unsupported.into());
    }
    let sftp = service.export_sftp_service();
    let options = sftp.effective_transfer_options(options);
    let runtime = if let Some(multiplex) = &sftp.multiplex {
        multiplex.runtime_handle()
    } else {
        compatibility_runtime()
            .map_err(|_| io::Error::other("SFTP runtime unavailable"))?
            .handle()
            .clone()
    };
    // The worker takes a service snapshot, not the authoritative Arc. The weak
    // source disappearing (disconnect, window destruction) cancels outstanding IO.
    drop(service);
    Ok(runtime.spawn(async move {
        let cache = sftp.compatibility.as_ref().map(|state| state.cache.clone());
        let sudo = sftp.sudo.clone();
        let operation = super::SFTP_COMPATIBILITY_CACHE.scope(
            cache,
            serve(
                sftp,
                file,
                options,
                control.clone(),
                requests,
                observer.clone(),
            ),
        );
        let operation = super::sudo::guard_operation(sudo, operation);
        tokio::pin!(operation);
        let result = tokio::select! {
            result = &mut operation => result,
            _ = async {
                loop {
                    if source.upgrade().is_none_or(|service| !service.authorization_is_valid()) { control.cancel(); }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            } => unreachable!(),
        };
        observer(ExportReadEvent::Finished(
            result.map_err(|error| error.to_string()),
        ));
    }))
}

async fn serve(
    service: super::SftpService,
    file: RemoteDragFile,
    options: SftpTransferOptions,
    control: Arc<SftpTransferControl>,
    mut requests: mpsc::Receiver<ReadRequest>,
    observer: ExportObserver,
) -> anyhow::Result<u64> {
    let parameters = super::TransferParameters::new(&options, &service.config.sftp);
    // Cancelled drags and metadata probes never open a connection or a file.
    let first = control.until_cancelled(requests.recv()).await?;
    let Some(first) = first else {
        anyhow::bail!(super::SFTP_TRANSFER_CANCELLED);
    };
    observer(ExportReadEvent::Opened((*control).clone()));
    let mut permit = Some(
        control
            .until_cancelled(service.export_budget.clone().acquire_owned())
            .await??,
    );
    let mut compatibility_guard = if let Some(compatibility) = &service.compatibility {
        Some(control.until_cancelled(compatibility.gate.lock()).await?)
    } else {
        None
    };
    let result = async {
        let session = control
            .until_cancelled(tokio::time::timeout(
                Duration::from_secs(60),
                service
                    .session_factory()
                    .open_with_config(parameters.client_config()),
            ))
            .await???;
        let result = async {
            let raw = remote_file_path_bytes(
                &super::SftpPathCodec::from_ssh_config(&service.config)?,
                &file.remote_path,
            )?;
            let attrs = control
                .until_cancelled(tokio::time::timeout(
                    Duration::from_secs(60),
                    session.sftp.symlink_metadata_bytes(raw.clone()),
                ))
                .await???;
            anyhow::ensure!(
                attrs.file_type() == russh_sftp::protocol::FileType::File,
                "export source is not a regular file"
            );
            let mut remote = control
                .until_cancelled(tokio::time::timeout(
                    Duration::from_secs(60),
                    session.sftp.open_bytes(raw),
                ))
                .await???;
            let result = async {
                let metadata = control
                    .until_cancelled(tokio::time::timeout(
                        Duration::from_secs(60),
                        remote.metadata(),
                    ))
                    .await???;
                anyhow::ensure!(
                    metadata.file_type() == russh_sftp::protocol::FileType::File,
                    "opened export source is not a regular file"
                );
                if let Some(expected) = file.size {
                    anyhow::ensure!(
                        metadata.size == Some(expected),
                        "remote size changed before export"
                    );
                }
                let size = metadata.size;
                let remote_ref = &remote;
                let control_ref = control.as_ref();
                let mut prefetch = Prefetch::new(
                    Box::new(move |offset, length| {
                        Box::pin(async move {
                            let mut bytes =
                                super::read_transfer_range(remote_ref, control_ref, offset, length)
                                    .await?;
                            anyhow::ensure!(bytes.len() <= length, "invalid export read length");
                            if bytes.len() < length {
                                // Do not let short packets grow a cached Vec past
                                // the configured block size through geometric growth.
                                bytes.reserve_exact(length - bytes.len());
                            }
                            // SFTP servers may return a short DATA packet without EOF.
                            while bytes.len() < length {
                                let part = super::read_transfer_range(
                                    remote_ref,
                                    control_ref,
                                    offset + bytes.len() as u64,
                                    length - bytes.len(),
                                )
                                .await?;
                                if part.is_empty() {
                                    anyhow::ensure!(
                                        size.is_none(),
                                        "remote file ended before its advertised size"
                                    );
                                    break;
                                }
                                anyhow::ensure!(
                                    part.len() <= length - bytes.len(),
                                    "invalid export read length"
                                );
                                bytes.extend_from_slice(&part);
                            }
                            Ok(bytes)
                        })
                    }),
                    control_ref,
                    size,
                    parameters.payload,
                    parameters.reads,
                );
                let mut coverage = Coverage::default();
                let mut request = Some(first);
                let mut final_progress_sent = false;
                let mut last_progress = Instant::now() - Duration::from_secs(1);
                loop {
                    if let Some(lease) = service.sudo_lease() {
                        lease.check()?;
                    }
                    let Some(next) = request.take() else {
                        break;
                    };
                    control.wait_if_paused().await?;
                    if permit.is_none() {
                        permit = Some(
                            control
                                .until_cancelled(service.export_budget.clone().acquire_owned())
                                .await??,
                        );
                        compatibility_guard = if let Some(state) = &service.compatibility {
                            Some(control.until_cancelled(state.gate.lock()).await?)
                        } else {
                            None
                        };
                    }
                    let length = size.map_or(next.length, |size| {
                        size.saturating_sub(next.offset).min(next.length as u64) as usize
                    });
                    let read = if length == 0 {
                        Ok(Vec::new())
                    } else {
                        prefetch.read(next.offset, length).await
                    };
                    match read {
                        Ok(bytes) => {
                            if bytes.is_empty() && size.is_some_and(|size| next.offset < size) {
                                let _ = next.reply.send(Err(io::Error::other(
                                    "remote file ended before its advertised size",
                                )));
                                anyhow::bail!("remote file ended before its advertised size");
                            }
                            let delivered = bytes.len() as u64;
                            if bytes.is_empty() && size.is_none() {
                                coverage.eof = Some(next.offset);
                            }
                            if next.reply.send(Ok(bytes)).is_err() {
                                anyhow::bail!(super::SFTP_TRANSFER_CANCELLED);
                            }
                            coverage.add(next.offset, delivered)?;
                            observer(ExportReadEvent::Delivered {
                                offset: next.offset,
                                length: delivered,
                                total: size,
                            });
                            let complete = coverage.complete(size);
                            if !final_progress_sent
                                && (last_progress.elapsed() >= Duration::from_millis(50)
                                    || complete)
                            {
                                observer(ExportReadEvent::Progress {
                                    bytes: coverage.prefix(),
                                    total: size,
                                });
                                final_progress_sent |= complete;
                                last_progress = Instant::now();
                            }
                        }
                        Err(error) => {
                            let _ = next
                                .reply
                                .send(Err(io::Error::other("SFTP export read failed")));
                            return Err(error);
                        }
                    }
                    let following = control
                        .until_cancelled(async {
                            loop {
                                // Idle native streams (including Clone) must not
                                // occupy every permit while the consumer reads a
                                // different stream. Drain the bounded prefetch
                                // window, then admit another SFTP operation.
                                if !prefetch.has_pending() {
                                    compatibility_guard.take();
                                    permit.take();
                                    break requests.recv().await;
                                }
                                tokio::select! {
                                    next = requests.recv() => break next,
                                    _ = prefetch.advance() => {},
                                }
                            }
                        })
                        .await;
                    match following {
                        Ok(Some(next)) => request = Some(next),
                        Ok(None) | Err(_) if coverage.complete(size) => {
                            return Ok(coverage.prefix());
                        }
                        _ => anyhow::bail!(super::SFTP_TRANSFER_CANCELLED),
                    }
                }
                anyhow::bail!(super::SFTP_TRANSFER_CANCELLED)
            }
            .await;
            use tokio::io::AsyncWriteExt as _;
            let _ = tokio::time::timeout(Duration::from_secs(5), remote.shutdown()).await;
            result
        }
        .await;
        let _ = tokio::time::timeout(Duration::from_secs(5), close_sftp_session(session)).await;
        result
    }
    .await;
    if result
        .as_ref()
        .err()
        .is_some_and(super::sftp_error_invalidates_compatibility_session)
        && let Some(state) = &service.compatibility
    {
        state.evict_failed_session().await;
    }
    result
}

/// Merge seeked ranges rather than mistaking a read near EOF for a full copy.
#[derive(Default)]
struct Coverage {
    ranges: std::collections::BTreeMap<u64, u64>,
    eof: Option<u64>,
}
impl Coverage {
    fn add(&mut self, start: u64, length: u64) -> anyhow::Result<()> {
        if length == 0 {
            return Ok(());
        }
        let mut start = start;
        let mut end = start
            .checked_add(length)
            .ok_or_else(|| anyhow::anyhow!("invalid export range"))?;
        let overlapping: Vec<_> = self
            .ranges
            .range(..=end)
            .filter(|(_, stop)| **stop >= start)
            .map(|(a, b)| (*a, *b))
            .collect();
        for (a, b) in overlapping {
            self.ranges.remove(&a);
            start = start.min(a);
            end = end.max(b);
        }
        anyhow::ensure!(self.ranges.len() < 1024, "too many disjoint export ranges");
        self.ranges.insert(start, end);
        Ok(())
    }
    fn prefix(&self) -> u64 {
        self.ranges.get(&0).copied().unwrap_or(0)
    }
    fn complete(&self, size: Option<u64>) -> bool {
        size.or(self.eof).is_some_and(|size| self.prefix() >= size)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod coverage_tests {
    use super::Coverage;
    #[test]
    fn seek_and_repeated_reads_do_not_report_holes_or_double_count() {
        let mut coverage = Coverage::default();
        coverage.add(8, 4).unwrap();
        assert!(!coverage.complete(Some(12)));
        coverage.add(0, 4).unwrap();
        coverage.add(0, 4).unwrap();
        assert_eq!(coverage.prefix(), 4);
        coverage.add(4, 4).unwrap();
        assert!(coverage.complete(Some(12)));
        assert_eq!(coverage.prefix(), 12);
    }
}
