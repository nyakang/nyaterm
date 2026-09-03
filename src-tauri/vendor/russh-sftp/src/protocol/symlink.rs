use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{impl_packet_for, impl_request_id, Packet, RequestId};

/// Implementation for `SSH_FXP_SYMLINK`
#[derive(Debug)]
pub struct Symlink {
    pub id: u32,
    pub linkpath: String,
    pub targetpath: String,
    /// Raw bytes of `linkpath`, preserving its original encoding.
    pub linkpath_bytes: Option<Vec<u8>>,
    /// Raw bytes of `targetpath`, preserving its original encoding.
    pub targetpath_bytes: Option<Vec<u8>>,
}

impl Serialize for Symlink {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Symlink", 3)?;
        state.serialize_field("id", &self.id)?;
        match &self.linkpath_bytes {
            Some(bytes) => state.serialize_field("linkpath", bytes)?,
            None => state.serialize_field("linkpath", &self.linkpath)?,
        }
        match &self.targetpath_bytes {
            Some(bytes) => state.serialize_field("targetpath", bytes)?,
            None => state.serialize_field("targetpath", &self.targetpath)?,
        }
        state.end()
    }
}

impl<'de> Deserialize<'de> for Symlink {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, SeqAccess, Visitor};
        use std::fmt;

        struct SymlinkVisitor;

        impl<'de> Visitor<'de> for SymlinkVisitor {
            type Value = Symlink;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct Symlink")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Symlink, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let id = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let linkpath_bytes: Vec<u8> = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                let targetpath_bytes: Vec<u8> = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(2, &self))?;
                Ok(Symlink {
                    id,
                    linkpath: String::from_utf8_lossy(&linkpath_bytes).into_owned(),
                    targetpath: String::from_utf8_lossy(&targetpath_bytes).into_owned(),
                    linkpath_bytes: Some(linkpath_bytes),
                    targetpath_bytes: Some(targetpath_bytes),
                })
            }
        }

        deserializer.deserialize_struct(
            "Symlink",
            &["id", "linkpath", "targetpath"],
            SymlinkVisitor,
        )
    }
}

impl_request_id!(Symlink);
impl_packet_for!(Symlink);

#[cfg(test)]
mod tests {
    use super::Symlink;

    #[test]
    fn serializes_raw_target_and_link_bytes() {
        let raw_target = b"../release/\x81".to_vec();
        let raw_link = b"/remote/\x80current".to_vec();
        let bytes = crate::ser::to_bytes(&Symlink {
            id: 9,
            linkpath: "display-target".to_string(),
            targetpath: "display-link".to_string(),
            linkpath_bytes: Some(raw_target.clone()),
            targetpath_bytes: Some(raw_link.clone()),
        })
        .unwrap();
        let target_pos = bytes
            .windows(raw_target.len())
            .position(|window| window == raw_target)
            .unwrap();
        let link_pos = bytes
            .windows(raw_link.len())
            .position(|window| window == raw_link)
            .unwrap();
        assert!(target_pos < link_pos);
    }
}
