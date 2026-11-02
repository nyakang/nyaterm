//! V4 purpose separation without changing the portable snapshot reader/writer.
use crate::portable_snapshot::{
    PortableSnapshotError, decrypt_snapshot_bytes, encrypt_snapshot_bytes,
};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

const MAGIC: &[u8] = b"NYASYNC4";
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

#[cfg(test)]
mod tests {
    use crate::sync_schema::encryption::{open, seal};
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
}
