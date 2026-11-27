//! Re-export display pipeline types (prediction + framebuffer composition).
//!
//! The implementation lives in [`crate::prediction::DisplayPipeline`] so
//! predict/confirm tests stay co-located with the mosh-go port.

pub use crate::prediction::{DisplayPipeline, DisplayPreference, Predictor};
