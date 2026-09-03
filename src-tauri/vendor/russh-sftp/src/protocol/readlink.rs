use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{impl_packet_for, impl_request_id, Packet, RequestId};

/// Implementation for `SSH_FXP_READLINK`
#[derive(Debug)]
pub struct ReadLink {
    pub id: u32,
    pub path: String,
    /// Raw bytes of the path, preserving its original encoding.
    pub path_bytes: Option<Vec<u8>>,
}

impl Serialize for ReadLink {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("ReadLink", 2)?;
        state.serialize_field("id", &self.id)?;
        match &self.path_bytes {
            Some(bytes) => state.serialize_field("path", bytes)?,
            None => state.serialize_field("path", &self.path)?,
        }
        state.end()
    }
}

impl<'de> Deserialize<'de> for ReadLink {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, SeqAccess, Visitor};
        use std::fmt;

        struct ReadLinkVisitor;

        impl<'de> Visitor<'de> for ReadLinkVisitor {
            type Value = ReadLink;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct ReadLink")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<ReadLink, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let id = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let path_bytes: Vec<u8> = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                Ok(ReadLink {
                    id,
                    path: String::from_utf8_lossy(&path_bytes).into_owned(),
                    path_bytes: Some(path_bytes),
                })
            }
        }

        deserializer.deserialize_struct("ReadLink", &["id", "path"], ReadLinkVisitor)
    }
}

impl_request_id!(ReadLink);
impl_packet_for!(ReadLink);

#[cfg(test)]
mod tests {
    use super::ReadLink;

    #[test]
    fn serializes_raw_path_bytes_without_lossy_conversion() {
        let raw_path = b"/remote/\x80link".to_vec();
        let bytes = crate::ser::to_bytes(&ReadLink {
            id: 7,
            path: "/remote/display-link".to_string(),
            path_bytes: Some(raw_path.clone()),
        })
        .unwrap();
        assert!(bytes.ends_with(&raw_path));
        assert!(!bytes.ends_with(b"/remote/display-link"));
    }
}
