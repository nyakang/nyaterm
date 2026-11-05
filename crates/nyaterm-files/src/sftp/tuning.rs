use crate::sftp_transfer_types::SftpTransferOptions;
use nyaterm_transport::SftpSettings;
use russh_sftp::client::Config;

/// Wire budgets are independent of the desktop's transfer queue limits.
#[derive(Clone, Copy, Debug)]
pub(super) struct TransferParameters {
    pub(super) payload: usize,
    pub(super) reads: usize,
    pub(super) writes: usize,
    packet: usize,
}

impl TransferParameters {
    pub(super) fn new(options: &SftpTransferOptions, settings: &SftpSettings) -> Self {
        let request_kib = (options.buffer_size_bytes() / 1024).clamp(64, 256);
        let manual = settings
            .pipeline_depth
            .map(|depth| depth.clamp(4, 64) as usize);
        Self {
            payload: request_kib * 1024 - 1024,
            reads: if settings.compatibility_mode {
                1
            } else {
                manual.unwrap_or(1024_usize.div_ceil(request_kib).clamp(4, 16))
            },
            writes: if settings.compatibility_mode {
                1
            } else {
                manual.unwrap_or(2048_usize.div_ceil(request_kib).clamp(8, 16))
            },
            packet: request_kib * 1024,
        }
    }

    pub(super) fn client_config(self) -> Config {
        Config {
            max_packet_len: self.packet as u32,
            max_write_packet_len: self.packet as u32,
            max_concurrent_reads: self.reads,
            max_concurrent_writes: self.writes,
            ..Config::default()
        }
    }

    pub(super) fn for_directory(self, workers: usize) -> Self {
        // Charge a full range allocation per slot, including ready, ordered replies.
        let slots = (32 * 1024 * 1024 / workers.max(1) / (2 * self.payload)).max(1);
        Self {
            reads: self.reads.min(slots),
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TransferParameters;
    use crate::sftp_transfer_types::SftpTransferOptions;
    use nyaterm_transport::SftpSettings;

    #[test]
    fn automatic_and_manual_wire_budgets_do_not_depend_on_queue_threads() {
        let settings = SftpSettings::default();
        let options = SftpTransferOptions::default().with_download_threads(1);
        let automatic = TransferParameters::new(&options, &settings);
        assert_eq!(
            (automatic.payload, automatic.reads, automatic.writes),
            (63 * 1024, 16, 16)
        );
        assert_eq!(automatic.client_config().max_write_packet_len, 64 * 1024);
        let manual = SftpSettings {
            pipeline_depth: Some(64),
            ..settings
        };
        let large = options.with_buffer_size_bytes(256 * 1024);
        let parameters = TransferParameters::new(&large, &manual);
        assert_eq!(parameters.reads, 64);
        assert!(
            2 * parameters.for_directory(16).reads * 16 * parameters.payload <= 32 * 1024 * 1024
        );
        let compatible = SftpSettings {
            compatibility_mode: true,
            ..manual
        };
        let parameters = TransferParameters::new(&large, &compatible);
        assert_eq!((parameters.reads, parameters.writes), (1, 1));
    }
}
