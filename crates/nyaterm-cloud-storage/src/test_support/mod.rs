//! Local HTTP fixtures and endpoint overrides for adapter integration tests.
pub mod drive;
pub mod http;

pub use crate::metrics::with_test_drive_http;
pub use drive::DriveFixture;
pub use http::Server as CloudStorageFixture;
