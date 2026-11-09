//! MobaXterm credential formats. CFB8 has no authentication: plaintext checks
//! reject many wrong keys, but cannot prove that a supplied password is correct.

use aes::Aes256;
use aes::cipher::{Block, BlockEncrypt, KeyInit};
use base64::Engine as _;
use sha2::{Digest, Sha512};
use zeroize::{Zeroize as _, Zeroizing};

use crate::SecretString;

#[derive(Default)]
pub(super) struct SecretContext<'a> {
    pub master_password: Option<&'a str>,
    pub session_p: Option<&'a str>,
    pub system_username: Option<&'a str>,
    pub system_hostname: Option<&'a str>,
    pub username: Option<&'a str>,
    pub hostname: Option<&'a str>,
}

pub(super) fn decrypt(ciphertext: &str, context: &SecretContext<'_>) -> Option<SecretString> {
    if let Some(master) = context.master_password.filter(|value| !value.is_empty()) {
        return decode_plaintext(&decrypt_master(ciphertext, master)?);
    }
    if let (Some(system_user), Some(system_host), Some(user), Some(host)) = (
        context.system_username,
        context.system_hostname,
        context.username,
        context.hostname,
    ) {
        let system = format!("{system_user}{system_host}");
        let connection = format!("{user}{host}");
        let key = weak_key(&system, &connection);
        if let Some(secret) =
            decrypt_weak(ciphertext, &key).and_then(|bytes| decode_plaintext(&bytes))
        {
            return Some(secret);
        }
    }
    let session_p = context.session_p.filter(|value| !value.is_empty())?;
    decrypt_weak(ciphertext, &weak_key(session_p, session_p))
        .and_then(|bytes| decode_plaintext(&bytes))
}

fn decode_base64(value: &str, alternate: bool) -> Option<Vec<u8>> {
    let mut normalized: String = value
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect();
    if alternate {
        normalized = normalized.replace('@', "+").replace('_', "/");
    }
    while !normalized.len().is_multiple_of(4) {
        normalized.push('=');
    }
    base64::engine::general_purpose::STANDARD
        .decode(normalized)
        .ok()
}

fn decrypt_master(ciphertext: &str, master: &str) -> Option<Zeroizing<Vec<u8>>> {
    let digest = Zeroizing::new(<[u8; 64]>::from(Sha512::digest(master.as_bytes())));
    let cipher = Aes256::new_from_slice(&digest[..32]).ok()?;
    let mut feedback = Zeroizing::new([0_u8; 16]);
    let body = if ciphertext.starts_with("_@") {
        // v25's 20 base64 characters decode to a 15-byte header. Its IV is
        // the literal 16 ASCII characters following the two-character marker.
        if ciphertext.len() <= 20 || !ciphertext.is_ascii() {
            return None;
        }
        feedback.copy_from_slice(&ciphertext.as_bytes()[2..18]);
        let decoded = decode_base64(ciphertext, true)?;
        decoded.get(15..)?.to_vec()
    } else {
        let mut block = Block::<Aes256>::default();
        cipher.encrypt_block(&mut block);
        feedback.copy_from_slice(&block);
        block.zeroize();
        decode_base64(ciphertext, false)?
    };
    if body.is_empty() {
        return None;
    }
    let mut plaintext = Zeroizing::new(Vec::with_capacity(body.len()));
    for byte in body {
        let mut block = Block::<Aes256>::default();
        block.copy_from_slice(feedback.as_ref());
        cipher.encrypt_block(&mut block);
        plaintext.push(byte ^ block[0]);
        block.zeroize();
        feedback.copy_within(1.., 0);
        feedback[15] = byte;
    }
    Some(plaintext)
}

fn repeated(value: &str) -> String {
    let mut result = value.to_owned();
    if !result.is_empty() {
        while result.encode_utf16().count() < 20 {
            result.push_str(&result.clone());
        }
    }
    result
}

fn weak_key(system: &str, connection: &str) -> Zeroizing<[u8; 16]> {
    let system = repeated(system);
    let connection = repeated(connection);
    let sources: [Vec<u16>; 4] = [
        system.to_uppercase().encode_utf16().collect(),
        connection.to_uppercase().encode_utf16().collect(),
        system.to_lowercase().encode_utf16().collect(),
        connection.to_lowercase().encode_utf16().collect(),
    ];
    let mut key = Zeroizing::new(*b"0d5e9n1348/U2+67");
    for index in 0..16 {
        if let Some(value) = sources[(index + 1) % 4].get(index)
            && let Ok(byte) = u8::try_from(*value)
            && (byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/')
            && !key.contains(&byte)
        {
            key[index] = byte;
        }
    }
    key
}

fn decrypt_weak(ciphertext: &str, key: &[u8; 16]) -> Option<Zeroizing<Vec<u8>>> {
    let filtered: Vec<u8> = ciphertext
        .bytes()
        .filter(|byte| key.contains(byte))
        .collect();
    if filtered.is_empty() || !filtered.len().is_multiple_of(2) {
        return None;
    }
    let mut rotating = Zeroizing::new(*key);
    let mut plaintext = Zeroizing::new(Vec::with_capacity(filtered.len() / 2));
    for pair in filtered.as_chunks::<2>().0 {
        let low = rotating.iter().position(|byte| *byte == pair[0])?;
        rotating.rotate_right(1);
        let high = rotating.iter().position(|byte| *byte == pair[1])?;
        rotating.rotate_right(1);
        plaintext.push(((high << 4) | low) as u8);
    }
    Some(plaintext)
}

fn decode_plaintext(bytes: &[u8]) -> Option<SecretString> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    if end == 0 || bytes[end..].iter().any(|byte| *byte != 0) {
        return None;
    }
    let bytes = &bytes[..end];
    if bytes.iter().any(|byte| *byte < 0x20 && *byte != b'\t') {
        return None;
    }
    let plaintext = std::str::from_utf8(bytes).ok()?;
    if plaintext.chars().any(|ch| ch.is_control() && ch != '\t') {
        return None;
    }
    Some(SecretString::from(plaintext))
}

#[cfg(test)]
mod tests;
