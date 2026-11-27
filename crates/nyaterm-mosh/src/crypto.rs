//! AES-128-OCB3 authenticated encryption (RFC 7253) for Mosh wire packets.
//!
//! Datagram layout:
//! ```text
//! [ dir_seq: u64 BE ][ ciphertext || tag:16 ]
//! ```
//! Nonce is 12 bytes: `00 00 00 00 || dir_seq`.

use aes::Aes128;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::error::{Error, Result};

const BLOCK: usize = 16;
const TAG: usize = 16;

/// Direction bit (MSB of the 64-bit sequence word).
pub const DIR_TO_CLIENT: u64 = 1 << 63;
pub const DIR_TO_SERVER: u64 = 0;
pub const SEQ_MASK: u64 = !(1u64 << 63);

/// Minimum wire datagram: 8 (nonce header) + 16 (tag).
pub const MIN_DATAGRAM: usize = 24;

/// AES-128-OCB3 cipher used by Mosh.
#[derive(Clone)]
pub struct Ocb {
    enc: Aes128,
    dec: Aes128,
    l_star: [u8; BLOCK],
    l_dollar: [u8; BLOCK],
    l: [[u8; BLOCK]; 32],
}

impl Drop for Ocb {
    fn drop(&mut self) {
        // `aes/zeroize` clears both expanded AES key schedules. Clear the
        // derived OCB masks owned by this wrapper as well.
        self.l_star.zeroize();
        self.l_dollar.zeroize();
        self.l.zeroize();
    }
}

impl ZeroizeOnDrop for Ocb {}

impl Ocb {
    pub fn new(key: &[u8]) -> Result<Self> {
        if key.len() != 16 {
            return Err(Error::InvalidKey(format!(
                "key must be 16 bytes, got {}",
                key.len()
            )));
        }
        let enc =
            Aes128::new_from_slice(key).map_err(|e| Error::Crypto(format!("aes init: {e}")))?;
        let dec =
            Aes128::new_from_slice(key).map_err(|e| Error::Crypto(format!("aes init: {e}")))?;

        let mut l_star = [0u8; BLOCK];
        enc.encrypt_block((&mut l_star).into());

        let l_dollar = gf_double(l_star);
        let mut l = [[0u8; BLOCK]; 32];
        l[0] = gf_double(l_dollar);
        for i in 1..32 {
            l[i] = gf_double(l[i - 1]);
        }

        Ok(Self {
            enc,
            dec,
            l_star,
            l_dollar,
            l,
        })
    }

    pub fn from_base64(key_b64: &str) -> Result<Self> {
        if key_b64.len() != 22 {
            return Err(Error::InvalidKey(format!(
                "mosh key must be exactly 22 base64 characters, got {}",
                key_b64.len()
            )));
        }
        let s = Zeroizing::new(format!("{key_b64}=="));
        let raw = Zeroizing::new(
            B64.decode(s.as_bytes())
                .map_err(|e| Error::InvalidKey(format!("base64: {e}")))?,
        );
        Self::new(&raw)
    }

    /// Encrypt `plaintext` under `nonce` (≤15 bytes).
    /// Returns `ciphertext || tag` (mosh wire order).
    pub fn encrypt(&self, nonce: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let mut offset = self.init_offset(nonce);
        let mut checksum = [0u8; BLOCK];
        let full_blocks = plaintext.len() / BLOCK;
        let remaining = plaintext.len() % BLOCK;
        let mut ciphertext = vec![0u8; plaintext.len()];

        for i in 0..full_blocks {
            xor_into(&mut offset, &self.l[ntz(i + 1)]);

            let mut p_block = [0u8; BLOCK];
            p_block.copy_from_slice(&plaintext[i * BLOCK..(i + 1) * BLOCK]);
            xor_into(&mut checksum, &p_block);

            let mut tmp = p_block;
            xor_into(&mut tmp, &offset);
            self.enc.encrypt_block((&mut tmp).into());
            xor_into(&mut tmp, &offset);
            ciphertext[i * BLOCK..(i + 1) * BLOCK].copy_from_slice(&tmp);
        }

        if remaining > 0 {
            xor_into(&mut offset, &self.l_star);
            let mut pad = offset;
            self.enc.encrypt_block((&mut pad).into());

            let start = full_blocks * BLOCK;
            let mut p_star = [0u8; BLOCK];
            p_star[..remaining].copy_from_slice(&plaintext[start..]);
            p_star[remaining] = 0x80;

            for i in 0..remaining {
                ciphertext[start + i] = p_star[i] ^ pad[i];
            }
            xor_into(&mut checksum, &p_star);
        }

        xor_into(&mut checksum, &offset);
        xor_into(&mut checksum, &self.l_dollar);
        let mut tag = checksum;
        self.enc.encrypt_block((&mut tag).into());

        let mut out = ciphertext;
        out.extend_from_slice(&tag);
        out
    }

    pub fn decrypt(&self, nonce: &[u8], ciphertext_and_tag: &[u8]) -> Option<Vec<u8>> {
        if ciphertext_and_tag.len() < TAG {
            return None;
        }
        let ciphertext = &ciphertext_and_tag[..ciphertext_and_tag.len() - TAG];
        let tag = &ciphertext_and_tag[ciphertext_and_tag.len() - TAG..];

        let mut offset = self.init_offset(nonce);
        let mut checksum = [0u8; BLOCK];
        let full_blocks = ciphertext.len() / BLOCK;
        let remaining = ciphertext.len() % BLOCK;
        let mut plaintext = vec![0u8; ciphertext.len()];

        for i in 0..full_blocks {
            xor_into(&mut offset, &self.l[ntz(i + 1)]);

            let mut c_block = [0u8; BLOCK];
            c_block.copy_from_slice(&ciphertext[i * BLOCK..(i + 1) * BLOCK]);

            let mut tmp = c_block;
            xor_into(&mut tmp, &offset);
            self.dec.decrypt_block((&mut tmp).into());
            xor_into(&mut tmp, &offset);
            plaintext[i * BLOCK..(i + 1) * BLOCK].copy_from_slice(&tmp);
            xor_into(&mut checksum, &tmp);
        }

        if remaining > 0 {
            xor_into(&mut offset, &self.l_star);
            let mut pad = offset;
            self.enc.encrypt_block((&mut pad).into());

            let start = full_blocks * BLOCK;
            let c_star = &ciphertext[start..];
            let mut p_star = [0u8; BLOCK];
            for i in 0..remaining {
                p_star[i] = c_star[i] ^ pad[i];
                plaintext[start + i] = p_star[i];
            }
            p_star[remaining] = 0x80;
            xor_into(&mut checksum, &p_star);
        }

        xor_into(&mut checksum, &offset);
        xor_into(&mut checksum, &self.l_dollar);
        let mut expected = checksum;
        self.enc.encrypt_block((&mut expected).into());

        if !constant_time_eq(tag, &expected) {
            return None;
        }
        Some(plaintext)
    }

    pub fn nonce_for(dir_seq: u64) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        nonce[4..12].copy_from_slice(&dir_seq.to_be_bytes());
        nonce
    }

    pub fn seal_datagram(&self, dir_seq: u64, plaintext: &[u8]) -> Vec<u8> {
        let nonce = Self::nonce_for(dir_seq);
        let body = self.encrypt(&nonce, plaintext);
        let mut out = Vec::with_capacity(8 + body.len());
        out.extend_from_slice(&dir_seq.to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    pub fn open_datagram(&self, packet: &[u8]) -> Option<(u64, Vec<u8>)> {
        if packet.len() < MIN_DATAGRAM {
            return None;
        }
        let mut seq_bytes = [0u8; 8];
        seq_bytes.copy_from_slice(&packet[..8]);
        let dir_seq = u64::from_be_bytes(seq_bytes);
        let nonce = Self::nonce_for(dir_seq);
        let plaintext = self.decrypt(&nonce, &packet[8..])?;
        Some((dir_seq, plaintext))
    }

    fn init_offset(&self, nonce: &[u8]) -> [u8; BLOCK] {
        let nonce_len = nonce.len().min(15);
        let mut nn = [0u8; BLOCK];
        nn[0] = (((TAG * 8) % 128) as u8) & 0x7f;
        nn[BLOCK - 1 - nonce_len] |= 0x01;
        nn[BLOCK - nonce_len..].copy_from_slice(&nonce[..nonce_len]);

        let bottom = (nn[15] & 0x3f) as usize;
        nn[15] &= 0xc0;

        let mut ktop = nn;
        self.enc.encrypt_block((&mut ktop).into());

        let mut stretch = [0u8; 24];
        stretch[..16].copy_from_slice(&ktop);
        for i in 0..8 {
            stretch[16 + i] = ktop[i] ^ ktop[i + 1];
        }

        let mut offset = [0u8; BLOCK];
        let byte_shift = bottom >> 3;
        let bit_shift = bottom & 7;
        for (i, byte) in offset.iter_mut().enumerate() {
            let idx = byte_shift + i;
            if idx < 24 {
                if bit_shift == 0 {
                    *byte = stretch[idx];
                } else {
                    *byte = stretch[idx].wrapping_shl(bit_shift as u32);
                    if idx + 1 < 24 {
                        *byte |= stretch[idx + 1] >> (8 - bit_shift);
                    }
                }
            }
        }
        offset
    }
}

pub fn pack_timestamps(ts: u16, ts_reply: u16) -> [u8; 4] {
    let mut out = [0u8; 4];
    out[0..2].copy_from_slice(&ts.to_be_bytes());
    out[2..4].copy_from_slice(&ts_reply.to_be_bytes());
    out
}

pub fn unpack_timestamps(buf: &[u8]) -> Option<(u16, u16, &[u8])> {
    if buf.len() < 4 {
        return None;
    }
    let ts = u16::from_be_bytes([buf[0], buf[1]]);
    let ts_reply = u16::from_be_bytes([buf[2], buf[3]]);
    Some((ts, ts_reply, &buf[4..]))
}

fn gf_double(block: [u8; BLOCK]) -> [u8; BLOCK] {
    let mut out = [0u8; BLOCK];
    let mut carry = 0u8;
    for i in (0..BLOCK).rev() {
        let b = block[i];
        out[i] = (b << 1) | carry;
        carry = b >> 7;
    }
    if carry != 0 {
        out[BLOCK - 1] ^= 0x87;
    }
    out
}

fn xor_into(dst: &mut [u8; BLOCK], src: &[u8; BLOCK]) {
    for i in 0..BLOCK {
        dst[i] ^= src[i];
    }
}

fn ntz(n: usize) -> usize {
    if n == 0 {
        return 32;
    }
    n.trailing_zeros() as usize
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::{
        DIR_TO_CLIENT, DIR_TO_SERVER, Ocb, SEQ_MASK, ntz, pack_timestamps, unpack_timestamps,
    };
    use zeroize::ZeroizeOnDrop;

    #[test]
    fn ocb_key_schedule_and_derived_masks_are_zeroized_on_drop() {
        fn assert_zeroize_on_drop<T: ZeroizeOnDrop>() {}
        assert_zeroize_on_drop::<Ocb>();
    }
    use rand::RngCore;

    #[test]
    fn ntz_values() {
        assert_eq!(ntz(1), 0);
        assert_eq!(ntz(2), 1);
        assert_eq!(ntz(4), 2);
        assert_eq!(ntz(0), 32);
    }

    #[test]
    fn bad_key_length() {
        assert!(Ocb::new(&[1, 2, 3]).is_err());
    }

    #[test]
    fn base64_key_with_missing_padding() {
        let ocb = Ocb::from_base64("AAAAAAAAAAAAAAAAAAAAAA").unwrap();
        let nonce = [0u8; 12];
        let ct = ocb.encrypt(&nonce, b"hi");
        assert_eq!(ocb.decrypt(&nonce, &ct).unwrap(), b"hi");
    }

    #[test]
    fn mosh_key_requires_the_official_22_character_form() {
        assert!(Ocb::from_base64("AAAAAAAAAAAAAAAAAAAAAA==").is_err());
        assert!(Ocb::from_base64(" AAAAAAAAAAAAAAAAAAAAAA ").is_err());
        assert!(Ocb::from_base64("AAAAAAAAAAAAAAAAAAAAA!").is_err());
    }

    #[test]
    fn roundtrip_various_sizes() {
        let mut key = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut key);
        let ocb = Ocb::new(&key).unwrap();
        let mut nonce = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce);

        for size in [0usize, 1, 15, 16, 17, 31, 32, 100, 1024, 1400] {
            let mut plaintext = vec![0u8; size];
            rand::thread_rng().fill_bytes(&mut plaintext);
            let ct = ocb.encrypt(&nonce, &plaintext);
            assert_eq!(ocb.decrypt(&nonce, &ct).unwrap(), plaintext);

            let mut tampered = ct.clone();
            *tampered.last_mut().unwrap() ^= 0x01;
            assert!(ocb.decrypt(&nonce, &tampered).is_none());
        }
    }

    #[test]
    fn mosh_wire_format_server_to_client() {
        let mut key = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut key);
        let ocb = Ocb::new(&key).unwrap();

        let dir_seq = DIR_TO_CLIENT | 1;
        let mut plaintext = Vec::new();
        plaintext.extend_from_slice(&pack_timestamps(12345, 0));
        plaintext.extend_from_slice(b"hello from mosh server");

        let wire = ocb.seal_datagram(dir_seq, &plaintext);
        let (rx_dir, rx_pt) = ocb.open_datagram(&wire).unwrap();
        assert_eq!(rx_dir & DIR_TO_CLIENT, DIR_TO_CLIENT);
        let (ts, _, body) = unpack_timestamps(&rx_pt).unwrap();
        assert_eq!(ts, 12345);
        assert_eq!(body, b"hello from mosh server");
    }

    #[test]
    fn interop_with_mosh_go_vector() {
        // Vectors from github.com/unixshells/mosh-go v0.5.2
        let key = hex::decode("000102030405060708090a0b0c0d0e0f").unwrap();
        let ocb = Ocb::new(&key).unwrap();
        let nonce = hex::decode("000000000000000000000001").unwrap();
        let pt = b"hello mosh ocb interop";
        let ct = ocb.encrypt(&nonce, pt);
        let expected = hex::decode(
            "242c7c518d75725a2fc1be1697a51782f834ee179acf4b447e04206da5b9e4999063645da59b",
        )
        .unwrap();
        assert_eq!(ct, expected);
        assert_eq!(ocb.decrypt(&nonce, &ct).unwrap(), pt);

        let nonce2 = hex::decode("000000008000000000000001").unwrap();
        let ct2 = ocb.encrypt(&nonce2, pt);
        let expected2 = hex::decode(
            "71edb32bfd34129388458b1c8bed99ae229532e6c78b9ac94f5b76c0935f96557e399b6f1899",
        )
        .unwrap();
        assert_eq!(ct2, expected2);
    }

    #[test]
    fn short_ciphertext_rejected() {
        let ocb = Ocb::new(&[0u8; 16]).unwrap();
        assert!(ocb.decrypt(&[0u8; 12], &[0u8; 15]).is_none());
        assert!(ocb.decrypt(&[0u8; 12], &[]).is_none());
    }

    #[test]
    fn different_nonces_different_ciphertext() {
        // Adapted from mosh-go TestOCBDifferentNonces
        let ocb = Ocb::new(&[7u8; 16]).unwrap();
        let pt = b"same plaintext";
        let mut n1 = [0u8; 12];
        n1[11] = 1;
        let mut n2 = [0u8; 12];
        n2[11] = 2;
        let c1 = ocb.encrypt(&n1, pt);
        let c2 = ocb.encrypt(&n2, pt);
        assert_ne!(c1, c2);
        assert_eq!(ocb.decrypt(&n1, &c1).unwrap(), pt);
        assert_eq!(ocb.decrypt(&n2, &c2).unwrap(), pt);
        assert!(ocb.decrypt(&n2, &c1).is_none());
    }

    #[test]
    fn empty_plaintext_tag_size() {
        // Adapted from mosh-go TestOCBEmptyPlaintextTagSize
        let ocb = Ocb::new(&[0u8; 16]).unwrap();
        let ct = ocb.encrypt(&[0u8; 12], b"");
        assert_eq!(ct.len(), 16, "empty plaintext yields tag only");
        assert_eq!(ocb.decrypt(&[0u8; 12], &ct).unwrap(), b"");
    }

    #[test]
    fn open_datagram_too_short() {
        let ocb = Ocb::new(&[1u8; 16]).unwrap();
        assert!(ocb.open_datagram(&[0u8; 20]).is_none());
        assert!(ocb.open_datagram(&[]).is_none());
    }

    #[test]
    fn client_to_server_direction_bit() {
        let ocb = Ocb::new(&[9u8; 16]).unwrap();
        let dir_seq = DIR_TO_SERVER | 42;
        let wire = ocb.seal_datagram(dir_seq, b"keys");
        let (rx, pt) = ocb.open_datagram(&wire).unwrap();
        assert_eq!(rx & DIR_TO_CLIENT, 0);
        assert_eq!(rx & SEQ_MASK, 42);
        assert_eq!(pt, b"keys");
    }

    /// RFC 7253 Appendix A vectors (TAGLEN=128, empty AAD only).
    /// Ported from mosh-go `TestOCBRFC7253Vectors` / RFC 7253.
    #[test]
    fn rfc7253_appendix_a_empty_aad() {
        let key = hex::decode("000102030405060708090A0B0C0D0E0F").unwrap();
        let ocb = Ocb::new(&key).unwrap();
        let vectors: &[(&str, &str, &str, &str)] = &[
            // name, nonce, plain, ct||tag
            (
                "Vector1_empty_empty",
                "BBAA99887766554433221100",
                "",
                "785407BFFFC8AD9EDCC5520AC9111EE6",
            ),
            (
                "Vector3_empty_8B",
                "BBAA99887766554433221102",
                "0001020304050607",
                "6DD42C17CBF9C7835DFD6E630E8F98EB3D2A49B0DC0F314E",
            ),
            (
                "Vector5_empty_16B",
                "BBAA99887766554433221104",
                "000102030405060708090A0B0C0D0E0F",
                "571D535B60B277188BE5147170A9A22C5E77B6AF964090C0F8F567B7B2763E1C",
            ),
            (
                "Vector7_empty_24B",
                "BBAA99887766554433221106",
                "000102030405060708090A0B0C0D0E0F1011121314151617",
                "5CE88EC2E0692706A915C00AEB8B23968467B2CFBB580496A361F6B4F1C479B222D7011EAA7B3144",
            ),
            (
                "Vector9_empty_32B",
                "BBAA99887766554433221108",
                "000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F",
                "FED5B2062E331BD1D243DCE4030BF42B1F0391097939C462293DAC9FABC97010CFD6EF3E7FF48413E807CE43F63E7977",
            ),
        ];
        for (name, nonce_h, plain_h, ct_h) in vectors {
            let nonce = hex::decode(nonce_h).unwrap();
            let plaintext = if plain_h.is_empty() {
                vec![]
            } else {
                hex::decode(plain_h).unwrap()
            };
            let expected = hex::decode(ct_h).unwrap();
            let got = ocb.encrypt(&nonce, &plaintext);
            assert_eq!(
                hex::encode_upper(&got),
                hex::encode_upper(&expected),
                "encrypt {name}"
            );
            let pt = ocb.decrypt(&nonce, &expected).expect("decrypt");
            assert_eq!(pt, plaintext, "decrypt {name}");
        }
    }

    #[test]
    fn ntz_table_from_mosh_go() {
        // mosh-go TestNTZ
        let cases = [
            (1, 0),
            (2, 1),
            (3, 0),
            (4, 2),
            (5, 0),
            (6, 1),
            (7, 0),
            (8, 3),
            (16, 4),
            (32, 5),
            (0, 32),
        ];
        for (n, want) in cases {
            assert_eq!(ntz(n), want, "ntz({n})");
        }
    }

    #[test]
    fn pack_unpack_timestamps() {
        let packed = pack_timestamps(0x1234, 0xabcd);
        let (ts, reply, rest) = unpack_timestamps(&packed).unwrap();
        assert_eq!(ts, 0x1234);
        assert_eq!(reply, 0xabcd);
        assert!(rest.is_empty());
        assert!(unpack_timestamps(&[1, 2, 3]).is_none());
    }
}
