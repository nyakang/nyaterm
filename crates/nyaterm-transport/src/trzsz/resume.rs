//! Protocol 3/4 prefix verification; only matched prefixes may be skipped.
use serde::{Deserialize, Serialize};

use super::{TrzszUploadEntry, TrzszUploadError, TrzszUploadReader, read_upload_chunk};

#[derive(Serialize)]
pub(super) struct PrefixHash {
    pub step: i64,
    pub hash: String,
    pub over: bool,
}

#[derive(Deserialize)]
pub(super) struct PrefixAck {
    pub step: i64,
    pub r#match: bool,
}

pub(super) struct PrefixSender {
    pub limit: i64,
    pub step: i64,
    pub matched: i64,
    pub reader: TrzszUploadReader,
    pub digest: md5::Context,
}

impl PrefixSender {
    pub fn next(&mut self, entry: &TrzszUploadEntry) -> Result<PrefixHash, TrzszUploadError> {
        let end = (self.step + 10 * 1024 * 1024).min(self.limit);
        while self.step < end {
            let bytes = read_upload_chunk(entry, self.step, &mut self.reader)?;
            let count = bytes.len().min((end - self.step) as usize);
            if count == 0 {
                return Err(TrzszUploadError::InvalidSource {
                    name: entry.name.clone(),
                });
            }
            self.digest.consume(&bytes[..count]);
            self.step += count as i64;
        }
        Ok(PrefixHash {
            step: self.step,
            hash: format!("{:x}", self.digest.clone().finalize()),
            over: false,
        })
    }
}
