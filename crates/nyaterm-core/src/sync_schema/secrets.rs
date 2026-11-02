//! Credential values have their own encryption domain. Causal registers carry
//! references only, so retaining conflicts never embeds plaintext credentials.
use std::fmt;

use argon2::{Algorithm, Argon2, Params, Version};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::portable_snapshot::PortableSnapshotError;
use crate::sync_schema::{SECRET_KINDS, SyncResourceId, SyncValue, encryption};

pub const MAX_SECRET_BYTES: usize = 8 * 1024 * 1024;

pub fn payload_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "key" => &["key", "cert", "passphrase"],
        "password" => &["username", "password"],
        "credential" => &[
            "username",
            "password",
            "username_prompt_regex",
            "password_prompt_regex",
            "enabled",
        ],
        "otp" => &[
            "otp_type",
            "issuer",
            "username",
            "secret",
            "algorithm",
            "digits",
            "period",
            "counter",
        ],
        "inline_secret" => &["binding", "password"],
        "ai_secret" => &["binding", "api_key", "extra_headers"],
        _ => &[],
    }
}

pub fn secret_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "key" => &["key", "cert", "passphrase"],
        "password" | "credential" => &["password"],
        "otp" => &["secret"],
        "inline_secret" => &["password"],
        "ai_secret" => &["api_key"],
        _ => &[],
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretReference {
    pub resource: SyncResourceId,
    pub version: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncryptedSecretValue {
    pub reference: SecretReference,
    pub ciphertext: Vec<u8>,
    /// A rotated object retains its causal identity. The original reference
    /// key is itself encrypted under the new password and exact reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrapped_reference_key: Option<Vec<u8>>,
}

impl fmt::Debug for EncryptedSecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EncryptedSecretValue(<redacted>)")
    }
}

/// Kept only for one blocking operation. The keyed version makes repeated
/// observations stable without exposing a fast plaintext/password hash in a
/// causal register. Random salts/nonces remain in the value's encrypted envelope.
pub struct SecretValueCodec<'a> {
    password: &'a str,
    reference_key: Zeroizing<[u8; 32]>,
}

impl<'a> SecretValueCodec<'a> {
    pub fn new(password: &'a str) -> Result<Self, PortableSnapshotError> {
        let password = password.trim();
        if password.is_empty() {
            return Err(PortableSnapshotError::MissingMasterPassword);
        }
        let mut reference_key = Zeroizing::new([0; 32]);
        let parameters = Params::new(19 * 1024, 2, 1, Some(32)).expect("fixed KDF parameters");
        Argon2::new(Algorithm::Argon2id, Version::V0x13, parameters)
            .hash_password_into(
                password.as_bytes(),
                b"nyaterm-sync-v4-secret-reference",
                reference_key.as_mut(),
            )
            .map_err(|_| PortableSnapshotError::InvalidEncryptionEnvelope)?;
        Ok(Self {
            password,
            reference_key,
        })
    }

    pub fn reference(
        &self,
        resource: &SyncResourceId,
        value: &SyncValue,
    ) -> Result<SecretReference, PortableSnapshotError> {
        if !SECRET_KINDS.contains(&resource.kind.as_str())
            || resource.id.is_empty()
            || value.bytes().len() > MAX_SECRET_BYTES
        {
            return Err(PortableSnapshotError::CorruptPayload);
        }
        let binding = serde_json::to_vec(resource)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(self.reference_key.as_ref())
            .expect("HMAC accepts 256-bit keys");
        mac.update(&(binding.len() as u64).to_le_bytes());
        mac.update(&binding);
        mac.update(value.bytes());
        Ok(SecretReference {
            resource: resource.clone(),
            version: hex::encode(mac.finalize().into_bytes()),
        })
    }

    pub fn seal(
        &self,
        resource: &SyncResourceId,
        value: &SyncValue,
    ) -> Result<EncryptedSecretValue, PortableSnapshotError> {
        let reference = self.reference(resource, value)?;
        let binding = serde_json::to_string(&reference)?;
        let ciphertext =
            encryption::seal(self.password, "credential-value", &binding, value.bytes())?;
        Ok(EncryptedSecretValue {
            reference,
            ciphertext,
            wrapped_reference_key: None,
        })
    }

    pub fn rewrap(
        &self,
        object: &EncryptedSecretValue,
        next_password: &str,
    ) -> Result<EncryptedSecretValue, PortableSnapshotError> {
        let value = self.open(&object.reference, object)?;
        let binding = serde_json::to_string(&object.reference)?;
        let key = self.object_reference_key(object)?;
        Ok(EncryptedSecretValue {
            reference: object.reference.clone(),
            ciphertext: encryption::seal(
                next_password,
                "credential-value",
                &binding,
                value.bytes(),
            )?,
            wrapped_reference_key: Some(encryption::seal(
                next_password,
                "credential-reference-key",
                &binding,
                key.as_ref(),
            )?),
        })
    }

    fn object_reference_key(
        &self,
        object: &EncryptedSecretValue,
    ) -> Result<Zeroizing<[u8; 32]>, PortableSnapshotError> {
        let Some(bytes) = &object.wrapped_reference_key else {
            return Ok(self.reference_key.clone());
        };
        if bytes.len() > 4096 {
            return Err(PortableSnapshotError::CorruptPayload);
        }
        let binding = serde_json::to_string(&object.reference)?;
        let plaintext =
            encryption::open(self.password, "credential-reference-key", &binding, bytes)?;
        let key: [u8; 32] = plaintext
            .as_slice()
            .try_into()
            .map_err(|_| PortableSnapshotError::CorruptPayload)?;
        Ok(Zeroizing::new(key))
    }

    pub fn open(
        &self,
        expected: &SecretReference,
        object: &EncryptedSecretValue,
    ) -> Result<SyncValue, PortableSnapshotError> {
        if &object.reference != expected || object.ciphertext.len() > MAX_SECRET_BYTES + 4096 {
            return Err(PortableSnapshotError::CorruptPayload);
        }
        let binding = serde_json::to_string(expected)?;
        let plaintext = encryption::open(
            self.password,
            "credential-value",
            &binding,
            &object.ciphertext,
        )?;
        let value = SyncValue::new(
            &serde_json::from_slice(&plaintext)
                .map_err(|_| PortableSnapshotError::CorruptPayload)?,
        );
        let verifier = Self {
            password: self.password,
            reference_key: self.object_reference_key(object)?,
        };
        if verifier.reference(&expected.resource, &value)? != *expected {
            return Err(PortableSnapshotError::CorruptPayload);
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use crate::sync_schema::secrets::SecretValueCodec;
    use crate::sync_schema::{SyncResourceId, SyncValue};
    use serde_json::json;

    #[test]
    fn stable_references_random_ciphertext_and_authenticated_resource_binding() {
        let codec = SecretValueCodec::new("fixture password").unwrap();
        let resource = SyncResourceId::new("password", "account");
        let value = SyncValue::new(&json!({"password":"fixture private value"}));
        let first = codec.seal(&resource, &value).unwrap();
        let second = codec.seal(&resource, &value).unwrap();
        assert_eq!(first.reference, second.reference);
        assert_ne!(first.ciphertext, second.ciphertext);
        assert_eq!(codec.open(&first.reference, &first).unwrap(), value);
        assert!(!format!("{first:?}").contains("fixture private value"));
        let wrong = SecretValueCodec::new("wrong password").unwrap();
        assert!(wrong.open(&first.reference, &first).is_err());
        let mut redirected = first.reference.clone();
        redirected.resource.id = "other account".into();
        assert!(codec.open(&redirected, &first).is_err());
        let mut corrupted = first.clone();
        *corrupted.ciphertext.last_mut().unwrap() ^= 1;
        assert!(codec.open(&first.reference, &corrupted).is_err());
        assert_ne!(
            codec.reference(&resource, &value).unwrap(),
            wrong.reference(&resource, &value).unwrap()
        );
    }

    #[test]
    fn rotation_preserves_causal_references_and_authenticates_the_original_reference_key() {
        let first = SecretValueCodec::new("first password").unwrap();
        let second = SecretValueCodec::new("second password").unwrap();
        let third = SecretValueCodec::new("third password").unwrap();
        let resource = SyncResourceId::new("password", "fixture");
        let value = SyncValue::new(&json!({"password":"fixture value"}));
        let original = first.seal(&resource, &value).unwrap();
        let rotated = first.rewrap(&original, "second password").unwrap();
        assert_eq!(rotated.reference, original.reference);
        assert_eq!(second.open(&original.reference, &rotated).unwrap(), value);
        assert!(first.open(&original.reference, &rotated).is_err());
        let twice = second.rewrap(&rotated, "third password").unwrap();
        assert_eq!(third.open(&original.reference, &twice).unwrap(), value);
        let mut corrupted = twice.clone();
        *corrupted
            .wrapped_reference_key
            .as_mut()
            .unwrap()
            .last_mut()
            .unwrap() ^= 1;
        assert!(third.open(&original.reference, &corrupted).is_err());
        let legacy_json = serde_json::to_vec(&original).unwrap();
        let decoded = serde_json::from_slice(&legacy_json).unwrap();
        assert_eq!(first.open(&original.reference, &decoded).unwrap(), value);
    }
}
