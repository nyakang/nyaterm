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
const MAX_LOCAL_REPLICA_BYTES: u64 = 256 * 1024 * 1024;
const LOCAL_REPLICA_PURPOSE: &str = "local-replica";
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
    if purpose != LOCAL_REPLICA_PURPOSE {
        return Err(PortableSnapshotError::InvalidEncryptionEnvelope);
    }
    validate_local_replica_size(bytes.len() as u64, MAX_LOCAL_REPLICA_BYTES)?;
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
    // Reject local-only compression before allocating or decrypting a remote
    // publication. Its ciphertext size limit cannot bound deflated plaintext.
    if purpose != LOCAL_REPLICA_PURPOSE {
        return Err(PortableSnapshotError::InvalidEncryptionEnvelope);
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
    decode_local_replica(&compressed, MAX_LOCAL_REPLICA_BYTES)
}

fn validate_local_replica_size(size: u64, limit: u64) -> Result<(), PortableSnapshotError> {
    if size > limit {
        return Err(PortableSnapshotError::LocalReplicaPayloadTooLarge { limit_bytes: limit });
    }
    Ok(())
}

fn decode_local_replica(
    compressed: &[u8],
    limit: u64,
) -> Result<Zeroizing<Vec<u8>>, PortableSnapshotError> {
    let mut decoder = DeflateDecoder::new(compressed);
    let mut decoded = Zeroizing::new(Vec::new());
    let mut chunk = Zeroizing::new([0_u8; 16 * 1024]);
    loop {
        let count = decoder
            .read(&mut *chunk)
            .map_err(|_| PortableSnapshotError::CorruptPayload)?;
        if count == 0 {
            break;
        }
        validate_local_replica_size(decoded.len() as u64 + count as u64, limit)?;
        decoded.extend_from_slice(&chunk[..count]);
    }
    if decoder.total_in() != compressed.len() as u64 {
        return Err(PortableSnapshotError::CorruptPayload);
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use crate::portable_snapshot::PortableSnapshotError;
    use crate::sync_schema::encryption::{
        COMPRESSED_MAGIC, CompressedPayload, MAX_LOCAL_REPLICA_BYTES, decode_local_replica, open,
        seal, seal_compressed, validate_local_replica_size,
    };
    use base64::{Engine, engine::general_purpose::STANDARD as B64};
    use flate2::{Compression, write::DeflateEncoder};
    use std::io::Write;
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

    fn deflate(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn compressed_envelopes_are_local_only_even_when_authenticated() {
        for purpose in [
            "publication",
            "password-rotation",
            "credential",
            "rotation-proof",
        ] {
            assert!(matches!(
                seal_compressed("password", purpose, "destination", b"fixture"),
                Err(PortableSnapshotError::InvalidEncryptionEnvelope)
            ));
            let sealed =
                compressed_envelope(purpose, "destination", &B64.encode(deflate(b"fixture")));
            assert!(matches!(
                open("password", purpose, "destination", &sealed),
                Err(PortableSnapshotError::InvalidEncryptionEnvelope)
            ));
        }
    }

    #[test]
    fn compressed_payload_authentication_rejects_wrong_identity_password_and_tampering() {
        let sealed =
            seal_compressed("password", "local-replica", "destination", b"fixture").unwrap();
        assert!(open("password", "local-replica", "other", &sealed).is_err());
        assert!(open("wrong", "local-replica", "destination", &sealed).is_err());
        let mut damaged = sealed;
        *damaged.last_mut().unwrap() ^= 1;
        assert!(open("password", "local-replica", "destination", &damaged).is_err());
    }

    #[test]
    fn decoded_size_limit_accepts_the_boundary_and_rejects_expansion_beyond_it() {
        let limit = 32 * 1024;
        for size in [limit - 1, limit, limit + 1, limit * 4] {
            let compressed = deflate(&vec![b'x'; size]);
            let decoded = decode_local_replica(&compressed, limit as u64);
            if size <= limit {
                assert_eq!(decoded.unwrap().len(), size);
            } else {
                assert!(matches!(
                    decoded,
                    Err(PortableSnapshotError::LocalReplicaPayloadTooLarge { .. })
                ));
            }
        }
        validate_local_replica_size(MAX_LOCAL_REPLICA_BYTES, MAX_LOCAL_REPLICA_BYTES).unwrap();
        assert!(matches!(
            validate_local_replica_size(MAX_LOCAL_REPLICA_BYTES + 1, MAX_LOCAL_REPLICA_BYTES),
            Err(PortableSnapshotError::LocalReplicaPayloadTooLarge { .. })
        ));
    }

    #[test]
    fn authenticated_truncated_and_trailing_deflate_data_are_rejected() {
        let compressed = deflate(b"fixture");
        let mut trailing = compressed.clone();
        trailing.extend_from_slice(b"trailing");
        for invalid in [&compressed[..compressed.len() - 1], &trailing[..]] {
            let sealed = compressed_envelope("local-replica", "destination", &B64.encode(invalid));
            assert!(matches!(
                open("password", "local-replica", "destination", &sealed),
                Err(PortableSnapshotError::CorruptPayload)
            ));
        }
        assert!(decode_local_replica(&[], 0).is_err());
        assert!(decode_local_replica(&deflate(b""), 0).unwrap().is_empty());
    }

    #[test]
    #[ignore = "full 256 MiB boundary regression; run explicitly on a machine with sufficient memory"]
    fn full_local_replica_limit_rejects_both_sealing_and_authenticated_expansion() {
        let source = vec![b'x'; MAX_LOCAL_REPLICA_BYTES as usize + 1];
        assert!(matches!(
            seal_compressed("password", "local-replica", "destination", &source),
            Err(PortableSnapshotError::LocalReplicaPayloadTooLarge { .. })
        ));
        let compressed = deflate(&source);
        drop(source);
        let sealed = compressed_envelope("local-replica", "destination", &B64.encode(compressed));
        assert!(matches!(
            open("password", "local-replica", "destination", &sealed),
            Err(PortableSnapshotError::LocalReplicaPayloadTooLarge { .. })
        ));
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
