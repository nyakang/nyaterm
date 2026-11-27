//! UI-independent FTP file service.
//!
//! The file browser talks to this module through the desktop feature layer. All
//! operations are blocking and must run on a worker thread, never in a GPUI render
//! path.

pub mod config;
mod connection;
pub(crate) mod copy;
pub mod encoding;
pub mod error;
mod io;
pub mod list;
pub mod pool;
mod replace;
pub(crate) mod resume;
pub mod service;
mod text;

pub use config::{
    FtpConfig, FtpFilenameEncodingMode, FtpLogin, FtpPassiveMode, FtpSecurityMode, FtpTlsVersion,
};
pub use encoding::{FtpEncoding, system_filename_encoding};
pub use error::FtpError;
pub use list::{FtpEntry, FtpEntryKind, entries_from_lines, join_remote_path, parent_and_name};
pub use service::FtpFileService;

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod tls_tests;
