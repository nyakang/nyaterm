//! Encoding and decoding of raw SFTP path tokens.

use encoding_rs::{Encoding, UTF_8};
use nyaterm_core::character_encoding::CharacterEncoding;
use nyaterm_transport::SshSessionConfig;

#[derive(Clone, Copy, Debug)]
pub struct SftpPathCodec {
    encoding_name: &'static str,
    encoding: &'static Encoding,
}

impl SftpPathCodec {
    pub(crate) fn same_encoding(&self, other: &Self) -> bool {
        self.encoding == other.encoding
    }

    pub(super) fn is_utf8(&self) -> bool {
        self.encoding == UTF_8
    }

    pub fn from_ssh_config(config: &SshSessionConfig) -> anyhow::Result<Self> {
        let charset =
            CharacterEncoding::resolve_filename(&config.sftp.filename_encoding, &config.encoding)?;
        Self::from_encoding_name(charset.label())
    }

    pub fn from_encoding_name(encoding: &str) -> anyhow::Result<Self> {
        let charset = CharacterEncoding::parse(encoding)?;
        Ok(Self {
            encoding_name: charset.label(),
            encoding: charset.codec(),
        })
    }

    #[cfg(test)]
    pub fn encoding_name(&self) -> &'static str {
        self.encoding_name
    }

    pub fn encode_path(&self, path: &str) -> anyhow::Result<Vec<u8>> {
        Ok(CharacterEncoding::parse(self.encoding_name)?.encode(path)?)
    }

    #[cfg(test)]
    pub fn decode_path(&self, path: &[u8]) -> anyhow::Result<String> {
        let (decoded, had_errors) = self.encoding.decode_without_bom_handling(path);
        if had_errors {
            anyhow::bail!("SFTP path cannot be decoded as {}", self.encoding_name);
        }
        Ok(decoded.into_owned())
    }

    pub fn decode_path_lossy(&self, path: &[u8]) -> String {
        let (decoded, had_errors) = self.encoding.decode_without_bom_handling(path);
        if had_errors {
            String::from_utf8_lossy(path).into_owned()
        } else {
            decoded.into_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SftpPathCodec;
    use nyaterm_core::character_encoding::CharacterEncoding;
    use nyaterm_transport::{SftpSettings, SshSessionConfig};

    #[test]
    fn all_filename_codecs_round_trip_and_keep_bom_characters() {
        for (label, name) in [
            ("UTF-8", "测试"),
            ("GBK", "测试"),
            ("GB18030", "测试😀"),
            ("Big5", "測試"),
            ("Shift_JIS", "日本語"),
            ("EUC-KR", "한국어"),
        ] {
            let codec = SftpPathCodec::from_encoding_name(label).unwrap();
            let path = format!("/tmp/{name}.txt");
            assert_eq!(
                codec
                    .decode_path(&codec.encode_path(&path).unwrap())
                    .unwrap(),
                path
            );
        }
        let codec = SftpPathCodec::from_encoding_name("UTF-8").unwrap();
        assert_eq!(
            codec.decode_path_lossy("\u{feff}name".as_bytes()),
            "\u{feff}name"
        );
        assert!(
            SftpPathCodec::from_encoding_name("GBK")
                .unwrap()
                .encode_path("secret😀")
                .is_err()
        );
    }

    #[test]
    fn filename_follow_tokens_and_aliases_share_terminal_catalog() {
        for label in ["", "global", "terminal", " GLOBAL "] {
            let config = SshSessionConfig {
                encoding: "cp936".to_string(),
                sftp: SftpSettings {
                    filename_encoding: label.to_string(),
                    ..Default::default()
                },
                ..Default::default()
            };
            assert_eq!(
                SftpPathCodec::from_ssh_config(&config)
                    .unwrap()
                    .encoding_name(),
                CharacterEncoding::Gbk.label()
            );
        }
    }
}
