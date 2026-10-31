//! tmux control mode (`tmux -CC`) integration for SSH sessions.
//!
//! Detection stays passive until the user (or a configured post-login
//! command) submits a tmux control-mode command line; only then may output
//! take over the channel, and only after the nonce-bearing handshake proves
//! the stream is really a control client. Once confirmed, [`session`] drives
//! the control channel and projects each remote pane as a virtual terminal
//! session so the desktop workspace can treat panes like ordinary sessions.

pub(crate) mod control;
pub(crate) mod detect;
pub(crate) mod handshake;
pub(crate) mod input;
pub(crate) mod session;

pub(crate) use detect::{TmuxControlDetector, TmuxFeed};
pub(crate) use session::{ControlExit, run_control_session};

pub use control::TmuxUiNode;
pub use session::{TmuxStatePayload, TmuxWindowInfo};
