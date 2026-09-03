#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecordingMode {
    #[default]
    Transcript,
    Raw,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExistingFileBehavior {
    #[default]
    Unique,
    Append,
    Overwrite,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum RotationPolicy {
    Session,
    Daily,
    Size { max_bytes: u64 },
}

impl Default for RotationPolicy {
    fn default() -> Self {
        Self::Session
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecordingState {
    Starting,
    Recording,
    Degraded,
    Failed,
    Stopping,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptEventKind {
    Command,
    Output,
    System,
}

impl TranscriptEventKind {
    fn label(self) -> &'static str {
        match self {
            Self::Command => "COMMAND",
            Self::Output => "OUTPUT",
            Self::System => "SYSTEM",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InputOrigin {
    Keyboard,
    QuickCommand,
    StartupCommand,
    PostLogin,
    AiAgent,
    CredentialAutofill,
    OtpAutofill,
    SyncInput,
    TerminalResponse,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum InputSensitivity {
    #[default]
    Normal,
    Secret,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingStatus {
    pub session_id: String,
    pub state: RecordingState,
    pub mode: RecordingMode,
    pub file_path: String,
    pub started_at: String,
    pub written_bytes: u64,
    pub queued_bytes: u64,
    pub dropped_bytes: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RecordingContext {
    pub session_id: String,
    pub session_name: String,
    pub connection_id: Option<String>,
    pub connection_name: Option<String>,
    pub group_path: Option<String>,
    pub protocol: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub started_at: OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct RecordingProfile {
    pub mode: RecordingMode,
    pub base_path: PathBuf,
    pub path_template: String,
    pub include_timestamps: bool,
    pub include_io_labels: bool,
    pub include_session_metadata: bool,
    pub rotation: RotationPolicy,
    pub existing_file_behavior: ExistingFileBehavior,
    pub include_binary_transfer_payloads: bool,
}
