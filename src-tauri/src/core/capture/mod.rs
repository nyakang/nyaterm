//! Marker-based output capture for AI Agent PTY command execution.
//!
//! Instead of opening a separate exec channel (which is unaware of nested
//! shells, containers, or SSH hops), we inject the command directly into the
//! interactive PTY wrapped with unique boundary markers, then intercept the
//! markers in the output stream to extract the command's output and exit code.
//!
//! Key design decisions:
//!
//! 1. The shell **echoes** everything written to the PTY. The command text
//!    itself appears in the output stream before the command runs. We handle
//!    this with a `WaitingForStart` phase that suppresses all output until
//!    the real START marker appears in the *execution* output.
//!
//! 2. The wrapper breaks or escapes marker patterns so the echo text never
//!    contains a matchable `__DF_CMD_START_` or `__DF_CMD_END_` sequence.
//!    Only the execution output does.
//!
//! 3. Variable names avoid `__` to prevent the end-marker parser from
//!    finding false `__` suffixes inside echoed variable references.
//!
//! 4. END is a stream boundary, never a transport-chunk boundary. Output
//!    after END is preserved, including the shell prompt.
//!
//! 5. PowerShell uses compact printable markers and a short session dispatcher.
//!    ConPTY rewrites long lines at narrow widths; OSC markers bypass its paint
//!    queue and can arrive ahead of the command output, so they are unsuitable
//!    as capture boundaries.

use base64::{Engine as _, engine::general_purpose};
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::oneshot;

use crate::config::AiExecutionProfile;

include!("types.rs");
include!("command.rs");
include!("powershell.rs");
include!("processor.rs");
include!("tests.rs");
include!("conpty_tests.rs");
