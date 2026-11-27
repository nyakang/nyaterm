//! Datagram fragmentation (upstream mosh `network.cc` / mosh-go layout).
//!
//! ```text
//! [instruction_id : 8 bytes BE]
//! [final:1 bit | fragment_num:15 bits : 2 bytes BE]
//! [payload...]
//! ```

use crate::error::{Error, Result};

pub const FRAGMENT_HEADER_SIZE: usize = 10;
pub const FRAGMENT_FINAL_BIT: u16 = 0x8000;
/// Max payload per fragment.
/// Upstream: get_MTU() - ADDED_BYTES(12) - OCB(16) - frag_header(10) ≈ 1214 on
/// IPv4 path MTU 1280. Prefer that over mosh-go's 1300 to avoid IP fragmentation.
pub const MAX_FRAGMENT_PAYLOAD: usize = 1214;
/// Official Mosh's compressor and decompressor each use a 2048×2048-byte
/// buffer. Accept the same complete-instruction boundary while still
/// rejecting unbounded fragment accumulation.
pub const MAX_INSTRUCTION_BYTES: usize = 2048 * 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    pub id: u64,
    pub fragment_num: u16,
    pub is_final: bool,
    pub payload: Vec<u8>,
}

impl Fragment {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(FRAGMENT_HEADER_SIZE + self.payload.len());
        out.extend_from_slice(&self.id.to_be_bytes());
        let mut num_and_final = self.fragment_num & 0x7fff;
        if self.is_final {
            num_and_final |= FRAGMENT_FINAL_BIT;
        }
        out.extend_from_slice(&num_and_final.to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        if data.len() < FRAGMENT_HEADER_SIZE {
            return Err(Error::Protocol("fragment too short".into()));
        }
        let id = u64::from_be_bytes(data[0..8].try_into().unwrap());
        let num_and_final = u16::from_be_bytes(data[8..10].try_into().unwrap());
        Ok(Self {
            id,
            fragment_num: num_and_final & 0x7fff,
            is_final: num_and_final & FRAGMENT_FINAL_BIT != 0,
            payload: data[FRAGMENT_HEADER_SIZE..].to_vec(),
        })
    }
}

/// Split a compressed instruction into fragments.
pub fn fragmentize(id: u64, data: &[u8]) -> Vec<Fragment> {
    fragmentize_with_payload(id, data, MAX_FRAGMENT_PAYLOAD)
}

pub fn fragmentize_with_payload(id: u64, data: &[u8], max_payload: usize) -> Vec<Fragment> {
    let max_payload = max_payload.max(1);
    let fragment_count = data.len().max(1).div_ceil(max_payload);
    if fragment_count > 0x8000 {
        return Vec::new();
    }
    if data.is_empty() {
        return vec![Fragment {
            id,
            fragment_num: 0,
            is_final: true,
            payload: vec![],
        }];
    }
    let n = data.len().div_ceil(max_payload);
    let mut frags = Vec::with_capacity(n);
    for i in 0..n {
        let start = i * max_payload;
        let end = (start + max_payload).min(data.len());
        frags.push(Fragment {
            id,
            fragment_num: i as u16,
            is_final: i + 1 == n,
            payload: data[start..end].to_vec(),
        });
    }
    frags
}

#[derive(Debug, Default)]
pub struct Assembler {
    current_id: u64,
    fragments: Vec<Option<Vec<u8>>>,
    total_num: Option<usize>,
    total_size: usize,
    has_id: bool,
}

impl Assembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a fragment; returns complete message when all pieces arrived.
    pub fn add(&mut self, f: Fragment) -> Option<Vec<u8>> {
        if !self.has_id || f.id != self.current_id {
            self.current_id = f.id;
            self.has_id = true;
            self.fragments.clear();
            self.total_num = None;
            self.total_size = 0;
        }

        let idx = f.fragment_num as usize;
        if self.fragments.len() <= idx {
            self.fragments.resize_with(idx + 1, || None);
        }
        // Only count newly filled slots (retransmits overwrite without double-counting).
        let is_new_slot = self.fragments[idx].is_none();
        if is_new_slot {
            self.total_size = self.total_size.saturating_add(f.payload.len());
            if self.total_size > MAX_INSTRUCTION_BYTES {
                self.fragments.clear();
                self.total_num = None;
                self.total_size = 0;
                return None;
            }
        }
        self.fragments[idx] = Some(f.payload);

        if f.is_final {
            self.total_num = Some(idx + 1);
        }

        let total = self.total_num?;
        if self.fragments.len() < total {
            return None;
        }
        for fragment in self.fragments.iter().take(total) {
            fragment.as_ref()?;
        }

        let mut msg = Vec::new();
        for fragment in self.fragments.iter().take(total) {
            msg.extend_from_slice(fragment.as_ref().expect("fragments checked above"));
        }
        self.fragments.clear();
        self.total_num = None;
        self.total_size = 0;
        Some(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Assembler, FRAGMENT_FINAL_BIT, Fragment, MAX_FRAGMENT_PAYLOAD, MAX_INSTRUCTION_BYTES,
        fragmentize, fragmentize_with_payload,
    };

    // Cases adapted from unixshells/mosh-go fragment_test.go

    #[test]
    fn fragment_codec_final_bit() {
        let f = Fragment {
            id: 9,
            fragment_num: 2,
            is_final: true,
            payload: b"xyz".to_vec(),
        };
        let enc = f.encode();
        let dec = Fragment::decode(&enc).unwrap();
        assert_eq!(dec, f);
        let num = u16::from_be_bytes([enc[8], enc[9]]);
        assert_eq!(num & FRAGMENT_FINAL_BIT, FRAGMENT_FINAL_BIT);
        assert_eq!(num & 0x7fff, 2);
    }

    #[test]
    fn fragment_not_final() {
        let f = Fragment {
            id: 1,
            fragment_num: 0,
            is_final: false,
            payload: b"x".to_vec(),
        };
        let got = Fragment::decode(&f.encode()).unwrap();
        assert!(!got.is_final);
        assert_eq!(got.payload, b"x");
    }

    #[test]
    fn fragment_too_short() {
        assert!(Fragment::decode(&[1, 2, 3]).is_err());
        assert!(Fragment::decode(&[]).is_err());
        assert!(Fragment::decode(&[0u8; 9]).is_err());
    }

    #[test]
    fn fragmentize_small() {
        let data = b"small message";
        let frags = fragmentize(1, data);
        assert_eq!(frags.len(), 1);
        assert!(frags[0].is_final);
        assert_eq!(frags[0].payload, data);
        assert_eq!(frags[0].id, 1);
    }

    #[test]
    fn fragmentize_empty() {
        let frags = fragmentize(1, &[]);
        assert_eq!(frags.len(), 1);
        assert!(frags[0].is_final);
        assert!(frags[0].payload.is_empty());
    }

    #[test]
    fn fragmentize_large_exact_layout() {
        // mosh-go: max*3+500 → 4 fragments, last payload 500
        let data = vec![0xCDu8; MAX_FRAGMENT_PAYLOAD * 3 + 500];
        let frags = fragmentize(7, &data);
        assert_eq!(frags.len(), 4);
        for (i, f) in frags.iter().enumerate() {
            assert_eq!(f.id, 7);
            assert_eq!(f.fragment_num as usize, i);
            assert_eq!(f.is_final, i + 1 == frags.len());
        }
        for f in &frags[..3] {
            assert_eq!(f.payload.len(), MAX_FRAGMENT_PAYLOAD);
        }
        assert_eq!(frags[3].payload.len(), 500);
    }

    #[test]
    fn assembler_in_order() {
        let data = vec![0x11u8; MAX_FRAGMENT_PAYLOAD * 2 + 100];
        let frags = fragmentize(1, &data);
        let mut a = Assembler::new();
        for (i, f) in frags.into_iter().enumerate() {
            let result = a.add(f);
            if i + 1 < 3 {
                assert!(result.is_none(), "premature at {i}");
            } else {
                assert_eq!(result.unwrap(), data);
            }
        }
    }

    #[test]
    fn assembler_out_of_order() {
        let data = vec![0x22u8; MAX_FRAGMENT_PAYLOAD * 3 + 1];
        let frags = fragmentize(1, &data);
        let order = [2, 0, 3, 1];
        let mut a = Assembler::new();
        let mut result = None;
        for idx in order {
            if let Some(r) = a.add(frags[idx].clone()) {
                result = Some(r);
            }
        }
        assert_eq!(result.unwrap(), data);
    }

    #[test]
    fn assembler_new_id_discards_incomplete() {
        let mut a = Assembler::new();
        assert!(
            a.add(Fragment {
                id: 1,
                fragment_num: 0,
                is_final: false,
                payload: b"old".to_vec(),
            })
            .is_none()
        );
        let data = b"new message";
        let frags = fragmentize(2, data);
        assert_eq!(a.add(frags[0].clone()).unwrap(), data);
    }

    #[test]
    fn assembler_accepts_a_complete_older_instruction_like_stock() {
        let mut a = Assembler::new();
        assert!(
            a.add(Fragment {
                id: 5,
                fragment_num: 0,
                is_final: true,
                payload: b"five".to_vec(),
            })
            .is_some()
        );
        assert_eq!(
            a.add(Fragment {
                id: 3,
                fragment_num: 0,
                is_final: true,
                payload: b"three".to_vec(),
            })
            .unwrap(),
            b"three"
        );
    }

    #[test]
    fn assembler_missing_fragment() {
        let mut a = Assembler::new();
        assert!(
            a.add(Fragment {
                id: 1,
                fragment_num: 0,
                is_final: false,
                payload: b"aaa".to_vec(),
            })
            .is_none()
        );
        // skip fragment 1, send final fragment 2
        assert!(
            a.add(Fragment {
                id: 1,
                fragment_num: 2,
                is_final: true,
                payload: b"ccc".to_vec(),
            })
            .is_none()
        );
    }

    #[test]
    fn assembler_duplicate_fragment() {
        let mut a = Assembler::new();
        let frag0 = Fragment {
            id: 1,
            fragment_num: 0,
            is_final: false,
            payload: b"hello".to_vec(),
        };
        let frag1 = Fragment {
            id: 1,
            fragment_num: 1,
            is_final: true,
            payload: b" world".to_vec(),
        };
        assert!(a.add(frag0.clone()).is_none());
        assert!(a.add(frag0).is_none());
        assert_eq!(a.add(frag1).unwrap(), b"hello world");
    }

    #[test]
    fn wire_round_trip_fragmentize_marshal_reassemble() {
        let data = vec![0x33u8; MAX_FRAGMENT_PAYLOAD * 2 + 42];
        let frags = fragmentize(99, &data);
        let mut a = Assembler::new();
        let mut result = None;
        for f in frags {
            let got = Fragment::decode(&f.encode()).unwrap();
            if let Some(r) = a.add(got) {
                result = Some(r);
            }
        }
        assert_eq!(result.unwrap(), data);
    }

    #[test]
    fn split_and_assemble() {
        let data: Vec<u8> = (0..2500u16).map(|n| (n % 256) as u8).collect();
        let frags = fragmentize(42, &data);
        assert!(frags.len() > 1);
        assert!(frags.last().unwrap().is_final);

        let mut asm = Assembler::new();
        let mut result = None;
        for f in frags {
            if let Some(m) = asm.add(f) {
                result = Some(m);
            }
        }
        assert_eq!(result.unwrap(), data);
    }

    #[test]
    fn large_payload_multiple_fragments() {
        let payload = vec![0xABu8; MAX_FRAGMENT_PAYLOAD * 3 + 42];
        let frags = fragmentize(7, &payload);
        assert!(frags.len() >= 4);
        let mut asm = Assembler::new();
        let mut out = None;
        for f in frags {
            if let Some(m) = asm.add(f) {
                out = Some(m);
            }
        }
        assert_eq!(out.unwrap(), payload);
    }

    #[test]
    fn assembler_accepts_official_compressed_states_larger_than_one_mibibyte() {
        let payload = vec![0x5a; (1 << 20) + MAX_FRAGMENT_PAYLOAD];
        let mut assembler = Assembler::new();
        let mut assembled = None;

        for fragment in fragmentize(77, &payload) {
            if let Some(message) = assembler.add(fragment) {
                assembled = Some(message);
            }
        }

        let assembled = assembled.expect("official-size state should reassemble");
        assert_eq!(assembled.len(), payload.len());
        assert!(
            assembled == payload,
            "reassembled payload must remain exact"
        );
    }

    #[test]
    fn assembler_rejects_states_larger_than_the_official_boundary() {
        let payload = vec![0x5b; MAX_INSTRUCTION_BYTES + 1];
        let mut assembler = Assembler::new();
        let mut assembled = None;

        for fragment in fragmentize(78, &payload) {
            if let Some(message) = assembler.add(fragment) {
                assembled = Some(message);
            }
        }

        assert!(assembled.is_none());
    }

    #[test]
    fn fragment_numbers_never_wrap_past_the_wire_field() {
        let payload = vec![0x5c; 0x8001];
        assert!(fragmentize_with_payload(79, &payload, 1).is_empty());
    }
}
