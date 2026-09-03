pub struct CapturedOutput {
    pub output: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub source_truncated: bool,
}

const MARKER_PREFIX: &str = "__DF_CMD_";
const MAX_CAPTURE_BYTES: usize = 4 * 1024 * 1024;
