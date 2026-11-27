use nyaterm_core::mosh::{MoshPrediction, MoshSettings};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MoshEditorDraft {
    pub server_path: String,
    pub udp_host: String,
    pub port_start: String,
    pub port_end: String,
    pub prediction: MoshPrediction,
}

impl Default for MoshEditorDraft {
    fn default() -> Self {
        Self::from(MoshSettings::default())
    }
}

impl From<MoshSettings> for MoshEditorDraft {
    fn from(settings: MoshSettings) -> Self {
        Self {
            server_path: settings.server_path,
            udp_host: settings.udp_host,
            port_start: settings.udp_port_start.to_string(),
            port_end: settings.udp_port_end.to_string(),
            prediction: settings.prediction,
        }
    }
}

impl MoshEditorDraft {
    pub fn settings(&self) -> Result<MoshSettings, &'static str> {
        let settings = MoshSettings {
            server_path: self.server_path.trim().into(),
            udp_host: self.udp_host.trim().into(),
            udp_port_start: self
                .port_start
                .trim()
                .parse()
                .map_err(|_| "Invalid Mosh UDP port range")?,
            udp_port_end: self
                .port_end
                .trim()
                .parse()
                .map_err(|_| "Invalid Mosh UDP port range")?,
            prediction: self.prediction,
        };
        settings.validate()?;
        Ok(settings)
    }
}
