//! Protocol-preserving batch edits with one authoritative draft and atomic storage submission.
mod basis;
mod draft;
mod fields;
mod options;
mod runtime;
mod view;
mod window;
pub(in crate::features) use draft::{AssetBatchEditDraft, BatchEditMode};
pub(in crate::features) use fields::{BatchFieldId, FieldBasis};
#[cfg(test)]
pub(in crate::features) use runtime::AssetBatchSaveSummary;
pub(in crate::features) const ASSET_BATCH_SELECT_PREFIX: &str = "asset-batch.select.";
pub(in crate::features) const ASSET_BATCH_TEXT_PREFIX: &str = "asset-batch.";
#[cfg(test)]
mod tests;
