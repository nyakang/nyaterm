//! tmux control mode (`tmux -CC`) integration.
//!
//! [`detect`] sniffs control-mode notifications in a normal terminal stream;
//! [`session`] drives the control channel once detected, mapping tmux panes
//! onto virtual sessions so each gets its own terminal (native scrollback,
//! selection and input) in the frontend.

pub(crate) mod control;
pub(crate) mod detect;
pub(crate) mod session;

pub(crate) use detect::{TmuxControlDetector, TmuxFeed};
pub(crate) use session::{ControlExit, run_control_session};
