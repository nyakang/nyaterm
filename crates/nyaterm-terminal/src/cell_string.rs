//! Text of a single terminal cell.
//!
//! Almost every cell holds one Unicode scalar value, so it is stored inline
//! (at most four UTF-8 bytes). Only the rare cell that carries combining marks
//! beyond four bytes pays for a shared heap allocation. Building and dropping
//! a row of ordinary cells therefore allocates nothing and touches no atomic
//! reference counts, which dominated snapshot construction when every cell held
//! an `Arc<str>`.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::sync::Arc;

/// One scalar value in UTF-8 never exceeds four bytes.
const INLINE_CAPACITY: usize = 4;

#[derive(Clone)]
pub struct CellText(Repr);

#[derive(Clone)]
enum Repr {
    /// `bytes[..len]` is always valid UTF-8; every constructor copies from a `&str`.
    Inline {
        len: u8,
        bytes: [u8; INLINE_CAPACITY],
    },
    Shared(Arc<str>),
}

impl CellText {
    pub const EMPTY: Self = Self(Repr::Inline {
        len: 0,
        bytes: [0; INLINE_CAPACITY],
    });

    pub fn from_char(ch: char) -> Self {
        let mut bytes = [0; INLINE_CAPACITY];
        let len = ch.encode_utf8(&mut bytes).len();
        Self(Repr::Inline {
            len: len as u8,
            bytes,
        })
    }

    pub fn as_str(&self) -> &str {
        match &self.0 {
            Repr::Inline { len, bytes } => {
                // SAFETY: `Repr::Inline` is only built in `from_char` and
                // `From<&str>`, which copy the complete UTF-8 encoding of a
                // `char` or `&str` into `bytes[..len]`. The field is private, so
                // nothing else can change it. Skipping validation matters: this
                // runs for every cell in every snapshot row, and checked
                // `from_utf8` cost as much as the allocations it replaced.
                unsafe { std::str::from_utf8_unchecked(&bytes[..usize::from(*len)]) }
            }
            Repr::Shared(text) => text,
        }
    }
}

impl Default for CellText {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl Deref for CellText {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl From<&str> for CellText {
    fn from(text: &str) -> Self {
        if text.len() <= INLINE_CAPACITY {
            let mut bytes = [0; INLINE_CAPACITY];
            bytes[..text.len()].copy_from_slice(text.as_bytes());
            Self(Repr::Inline {
                len: text.len() as u8,
                bytes,
            })
        } else {
            Self(Repr::Shared(Arc::from(text)))
        }
    }
}

impl From<String> for CellText {
    fn from(text: String) -> Self {
        Self::from(text.as_str())
    }
}

impl From<Arc<str>> for CellText {
    fn from(text: Arc<str>) -> Self {
        if text.len() <= INLINE_CAPACITY {
            Self::from(&*text)
        } else {
            Self(Repr::Shared(text))
        }
    }
}

impl PartialEq for CellText {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for CellText {}

impl PartialEq<str> for CellText {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for CellText {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl Hash for CellText {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl fmt::Debug for CellText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), formatter)
    }
}

impl fmt::Display for CellText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::CellText;

    #[test]
    fn single_scalar_values_are_stored_inline_for_every_utf8_width() {
        for ch in ['a', 'é', '日', '😀'] {
            let text = CellText::from_char(ch);
            assert_eq!(text.as_str(), ch.to_string());
            assert!(matches!(text.0, super::Repr::Inline { .. }), "{ch}");
        }
    }

    #[test]
    fn long_text_is_shared_and_equal_to_the_same_text_built_another_way() {
        let combined = "e\u{301}\u{302}";
        let from_str = CellText::from(combined);
        let from_string = CellText::from(combined.to_string());
        let from_arc = CellText::from(std::sync::Arc::<str>::from(combined));

        assert!(matches!(from_str.0, super::Repr::Shared(_)));
        assert_eq!(from_str, from_string);
        assert_eq!(from_str, from_arc);
        assert_eq!(from_str.chars().count(), 3);
    }

    #[test]
    fn short_text_converts_to_inline_regardless_of_source() {
        let from_arc = CellText::from(std::sync::Arc::<str>::from("ab"));

        assert!(matches!(from_arc.0, super::Repr::Inline { .. }));
        assert_eq!(from_arc, CellText::from("ab"));
    }

    #[test]
    fn empty_text_is_the_default_and_equals_an_empty_str() {
        assert!(CellText::default().is_empty());
        assert_eq!(CellText::EMPTY, "");
        assert_eq!(CellText::from(""), CellText::EMPTY);
    }

    #[test]
    fn an_ordinary_cell_text_is_not_larger_than_the_arc_it_replaces() {
        assert!(std::mem::size_of::<CellText>() <= std::mem::size_of::<std::sync::Arc<str>>() + 8);
    }
}
