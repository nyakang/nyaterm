//! V4 purpose separation without changing the portable snapshot reader/writer.
use std::io::{Read, Write};

use crate::portable_snapshot::{
    PortableSnapshotError, decrypt_snapshot_bytes, encrypt_snapshot_bytes,
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

const MAGIC: &[u8] = b"NYASYNC4";
// Local-only replicas are large and highly redundant. NYASYNC5 marks a payload
// that is deflated and base64 encoded before encryption; NYASYNC4 records stay
// readable so existing installations never need a migration step.
const COMPRESSED_MAGIC: &[u8] = b"NYASYNC5";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtectedPayload {
    purpose: String,
    binding: String,
    bytes: Vec<u8>,
}

impl Drop for ProtectedPayload {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompressedPayload {
    purpose: String,
    binding: String,
    bytes: String,
}

impl Drop for CompressedPayload {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

/// True for records written before the compressed envelope. Such records stay
/// byte-readable as-is; only a deliberate rewrite upgrades them to `NYASYNC5`.
pub fn is_legacy_envelope(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC) && !bytes.starts_with(COMPRESSED_MAGIC)
}

pub fn seal(
    password: &str,
    purpose: &str,
    binding: &str,
    bytes: &[u8],
) -> Result<Vec<u8>, PortableSnapshotError> {
    if password.trim().is_empty() {
        return Err(PortableSnapshotError::MissingMasterPassword);
    }
    let payload = ProtectedPayload {
        purpose: purpose.into(),
        binding: binding.into(),
        bytes: bytes.to_vec(),
    };
    let plaintext = Zeroizing::new(serde_json::to_vec(&payload)?);
    let domain = Zeroizing::new(format!("nyaterm-sync-v4:{}", password.trim()));
    let mut result = MAGIC.to_vec();
    result.extend(encrypt_snapshot_bytes(&domain, &plaintext)?);
    Ok(result)
}

pub fn open(
    password: &str,
    purpose: &str,
    binding: &str,
    bytes: &[u8],
) -> Result<Zeroizing<Vec<u8>>, PortableSnapshotError> {
    if bytes.starts_with(COMPRESSED_MAGIC) {
        return open_compressed(password, purpose, binding, bytes);
    }
    if password.trim().is_empty() {
        return Err(PortableSnapshotError::MissingMasterPassword);
    }
    let ciphertext = bytes
        .strip_prefix(MAGIC)
        .ok_or(PortableSnapshotError::InvalidEncryptionEnvelope)?;
    let domain = Zeroizing::new(format!("nyaterm-sync-v4:{}", password.trim()));
    let plaintext = Zeroizing::new(decrypt_snapshot_bytes(&domain, ciphertext)?);
    let mut payload: ProtectedPayload =
        serde_json::from_slice(&plaintext).map_err(|_| PortableSnapshotError::CorruptPayload)?;
    if payload.purpose != purpose || payload.binding != binding {
        return Err(PortableSnapshotError::InvalidEncryptionEnvelope);
    }
    Ok(Zeroizing::new(std::mem::take(&mut payload.bytes)))
}

/// Seals a deflated local-only payload under the NYASYNC5 envelope. Cloud
/// publications and credential envelopes keep using [`seal`] so their
/// cross-device format stays unchanged.
pub fn seal_compressed(
    password: &str,
    purpose: &str,
    binding: &str,
    bytes: &[u8],
) -> Result<Vec<u8>, PortableSnapshotError> {
    if password.trim().is_empty() {
        return Err(PortableSnapshotError::MissingMasterPassword);
    }
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes)?;
    let compressed = Zeroizing::new(encoder.finish()?);
    let payload = CompressedPayload {
        purpose: purpose.into(),
        binding: binding.into(),
        bytes: B64.encode(&*compressed),
    };
    let plaintext = Zeroizing::new(serde_json::to_vec(&payload)?);
    let domain = Zeroizing::new(format!("nyaterm-sync-v4:{}", password.trim()));
    let mut result = COMPRESSED_MAGIC.to_vec();
    result.extend(encrypt_snapshot_bytes(&domain, &plaintext)?);
    Ok(result)
}

fn open_compressed(
    password: &str,
    purpose: &str,
    binding: &str,
    bytes: &[u8],
) -> Result<Zeroizing<Vec<u8>>, PortableSnapshotError> {
    if password.trim().is_empty() {
        return Err(PortableSnapshotError::MissingMasterPassword);
    }
    let ciphertext = bytes
        .strip_prefix(COMPRESSED_MAGIC)
        .ok_or(PortableSnapshotError::InvalidEncryptionEnvelope)?;
    let domain = Zeroizing::new(format!("nyaterm-sync-v4:{}", password.trim()));
    let plaintext = Zeroizing::new(decrypt_snapshot_bytes(&domain, ciphertext)?);
    let payload: CompressedPayload =
        serde_json::from_slice(&plaintext).map_err(|_| PortableSnapshotError::CorruptPayload)?;
    if payload.purpose != purpose || payload.binding != binding {
        return Err(PortableSnapshotError::InvalidEncryptionEnvelope);
    }
    let compressed = Zeroizing::new(
        B64.decode(payload.bytes.as_bytes())
            .map_err(|_| PortableSnapshotError::CorruptPayload)?,
    );
    let mut decoder = DeflateDecoder::new(&compressed[..]);
    let mut decoded = Zeroizing::new(Vec::new());
    decoder
        .read_to_end(&mut decoded)
        .map_err(|_| PortableSnapshotError::CorruptPayload)?;
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use crate::portable_snapshot::PortableSnapshotError;
    use crate::sync_schema::encryption::{
        COMPRESSED_MAGIC, CompressedPayload, open, seal, seal_compressed,
    };
    use base64::{Engine, engine::general_purpose::STANDARD as B64};
    use zeroize::Zeroizing;

    fn compressed_envelope(purpose: &str, binding: &str, bytes: &str) -> Vec<u8> {
        let payload = CompressedPayload {
            purpose: purpose.into(),
            binding: binding.into(),
            bytes: bytes.into(),
        };
        let plaintext = Zeroizing::new(serde_json::to_vec(&payload).unwrap());
        let domain = Zeroizing::new(String::from("nyaterm-sync-v4:password"));
        let mut result = COMPRESSED_MAGIC.to_vec();
        result.extend(super::encrypt_snapshot_bytes(&domain, &plaintext).unwrap());
        result
    }

    #[test]
    fn purpose_identity_password_and_tampering_are_authenticated() {
        let bytes = seal("password", "publication", "writer/1", b"data").unwrap();
        assert_eq!(
            &*open("password", "publication", "writer/1", &bytes).unwrap(),
            b"data"
        );
        assert!(open("password", "replica", "writer/1", &bytes).is_err());
        assert!(open("password", "publication", "writer/2", &bytes).is_err());
        assert!(open("wrong", "publication", "writer/1", &bytes).is_err());
        let mut damaged = bytes;
        *damaged.last_mut().unwrap() ^= 1;
        assert!(open("password", "publication", "writer/1", &damaged).is_err());
    }

    #[test]
    fn compressed_envelope_round_trips() {
        let plaintext = br#"{"schema":4,"replica":{"document":[],"values":{}}}"#;
        let sealed =
            seal_compressed("password", "local-replica", "destination", plaintext).unwrap();
        assert!(sealed.starts_with(COMPRESSED_MAGIC));
        assert_eq!(
            &*open("password", "local-replica", "destination", &sealed).unwrap(),
            plaintext
        );
    }

    #[test]
    fn legacy_envelope_still_reads_and_decodes_identically() {
        let plaintext = vec![b'x'; 4096];
        let legacy = seal("password", "local-replica", "destination", &plaintext).unwrap();
        let compressed =
            seal_compressed("password", "local-replica", "destination", &plaintext).unwrap();
        assert!(legacy.starts_with(b"NYASYNC4"));
        assert!(compressed.starts_with(COMPRESSED_MAGIC));
        assert!(compressed.len() < legacy.len());
        let legacy_plain = open("password", "local-replica", "destination", &legacy).unwrap();
        let compressed_plain =
            open("password", "local-replica", "destination", &compressed).unwrap();
        assert_eq!(&*legacy_plain, &plaintext[..]);
        assert_eq!(&*compressed_plain, &*legacy_plain);
    }

    #[test]
    fn corrupt_compressed_payloads_report_errors() {
        let valid =
            seal_compressed("password", "local-replica", "destination", b"fixture").unwrap();

        let mut truncated = valid.clone();
        truncated.truncate(truncated.len() - 1);
        assert!(open("password", "local-replica", "destination", &truncated).is_err());

        let damaged = compressed_envelope("local-replica", "destination", "!!!not-base64");
        assert!(matches!(
            open("password", "local-replica", "destination", &damaged),
            Err(PortableSnapshotError::CorruptPayload)
        ));

        let not_deflate = B64.encode([0xFF_u8; 8]);
        let corrupted = compressed_envelope("local-replica", "destination", &not_deflate);
        assert!(matches!(
            open("password", "local-replica", "destination", &corrupted),
            Err(PortableSnapshotError::CorruptPayload)
        ));

        let wrong_binding = compressed_envelope("local-replica", "other", "Zml4dHVyZQ==");
        assert!(matches!(
            open("password", "local-replica", "destination", &wrong_binding),
            Err(PortableSnapshotError::InvalidEncryptionEnvelope)
        ));

        assert!(matches!(
            open("", "local-replica", "destination", &valid),
            Err(PortableSnapshotError::MissingMasterPassword)
        ));

        assert!(matches!(
            open(
                "password",
                "local-replica",
                "destination",
                b"NYASYNC9garbage"
            ),
            Err(PortableSnapshotError::InvalidEncryptionEnvelope)
        ));
    }
}
