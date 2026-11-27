use super::{BatchField, ConnectionBatchApplyReport, ConnectionBatchSkipReason};
use crate::models::ConnectionType;
use crate::mosh::MoshSettings;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MoshBatchEdit {
    pub server_path: BatchField<String>,
    pub udp_host: BatchField<String>,
    pub udp_port_start: BatchField<u16>,
    pub udp_port_end: BatchField<u16>,
    pub prediction: BatchField<crate::mosh::MoshPrediction>,
}
impl MoshBatchEdit {
    pub fn is_empty(&self) -> bool {
        self.server_path.is_unchanged()
            && self.udp_host.is_unchanged()
            && self.udp_port_start.is_unchanged()
            && self.udp_port_end.is_unchanged()
            && self.prediction.is_unchanged()
    }
    pub(super) fn apply(
        &self,
        config: &mut ConnectionType,
        report: &mut ConnectionBatchApplyReport,
    ) {
        let defaults = MoshSettings::default();
        if !self.server_path.is_unchanged() {
            if let ConnectionType::Mosh { mosh, .. } = config {
                mosh.server_path = match &self.server_path {
                    BatchField::Set(value) => value.clone(),
                    BatchField::Clear => defaults.server_path.clone(),
                    BatchField::Unchanged => unreachable!(),
                };
                report.record(true, None);
            } else {
                report.record(true, Some(ConnectionBatchSkipReason::UnsupportedProtocol));
            }
        }
        if !self.udp_host.is_unchanged() {
            if let ConnectionType::Mosh { mosh, .. } = config {
                mosh.udp_host = match &self.udp_host {
                    BatchField::Set(value) => value.clone(),
                    BatchField::Clear => defaults.udp_host.clone(),
                    BatchField::Unchanged => unreachable!(),
                };
                report.record(true, None);
            } else {
                report.record(true, Some(ConnectionBatchSkipReason::UnsupportedProtocol));
            }
        }
        if !self.udp_port_start.is_unchanged() {
            if let ConnectionType::Mosh { mosh, .. } = config {
                mosh.udp_port_start = match &self.udp_port_start {
                    BatchField::Set(value) => *value,
                    BatchField::Clear => defaults.udp_port_start,
                    BatchField::Unchanged => unreachable!(),
                };
                report.record(true, None);
            } else {
                report.record(true, Some(ConnectionBatchSkipReason::UnsupportedProtocol));
            }
        }
        if !self.udp_port_end.is_unchanged() {
            if let ConnectionType::Mosh { mosh, .. } = config {
                mosh.udp_port_end = match &self.udp_port_end {
                    BatchField::Set(value) => *value,
                    BatchField::Clear => defaults.udp_port_end,
                    BatchField::Unchanged => unreachable!(),
                };
                report.record(true, None);
            } else {
                report.record(true, Some(ConnectionBatchSkipReason::UnsupportedProtocol));
            }
        }
        if !self.prediction.is_unchanged() {
            if let ConnectionType::Mosh { mosh, .. } = config {
                mosh.prediction = match &self.prediction {
                    BatchField::Set(value) => *value,
                    BatchField::Clear => defaults.prediction,
                    BatchField::Unchanged => unreachable!(),
                };
                report.record(true, None);
            } else {
                report.record(true, Some(ConnectionBatchSkipReason::UnsupportedProtocol));
            }
        }
    }
}
