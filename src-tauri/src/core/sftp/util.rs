//! Shared helpers for remote file system backends: path quoting, permission
//! formatting, and common type definitions.

use crate::error::{AppError, AppResult};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(crate) const SFTP_FILE_TYPE_MASK: u32 = 0o170000;
pub(crate) const POSIX_MODE_MASK: u32 = 0o7777;

/// Parsed entry from a remote directory listing for the file explorer.
#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub permissions: String,
    pub owner: String,
    pub group: String,
    pub mtime: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_path_token: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryChild {
    pub name: String,
    pub path: String,
    pub is_symlink: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_path_token: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct RemotePathRef {
    display_path: String,
    raw_path: Option<Vec<u8>>,
}

impl RemotePathRef {
    pub(crate) fn new(display_path: &str, raw_path_token: Option<&str>) -> AppResult<Self> {
        let raw_path = raw_path_token
            .filter(|token| !token.trim().is_empty())
            .map(decode_raw_path_token)
            .transpose()?;
        Ok(Self {
            display_path: display_path.to_string(),
            raw_path,
        })
    }

    pub(crate) fn display_path(&self) -> &str {
        &self.display_path
    }

    pub(crate) fn raw_path(&self) -> Option<&[u8]> {
        self.raw_path.as_deref()
    }

    pub(crate) fn sibling(&self, file_name: &str) -> Self {
        let display_path = sibling_path(self.display_path().as_bytes(), file_name.as_bytes())
            .map_or_else(
                || file_name.to_string(),
                |bytes| String::from_utf8_lossy(&bytes).into_owned(),
            );
        let raw_path = self
            .raw_path()
            .and_then(|path| sibling_path(path, file_name.as_bytes()));
        Self {
            display_path,
            raw_path,
        }
    }
}

fn sibling_path(path: &[u8], file_name: &[u8]) -> Option<Vec<u8>> {
    if file_name.is_empty() {
        return None;
    }
    let parent = path
        .iter()
        .rposition(|byte| *byte == b'/')
        .map(|index| &path[..index])
        .unwrap_or_default();
    let mut sibling = Vec::with_capacity(parent.len() + file_name.len() + 1);
    if parent.is_empty() {
        if path.starts_with(b"/") {
            sibling.push(b'/');
        }
    } else {
        sibling.extend_from_slice(parent);
        sibling.push(b'/');
    }
    sibling.extend_from_slice(file_name);
    Some(sibling)
}

pub(crate) fn raw_path_token(raw_path: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(raw_path)
}

pub(crate) fn decode_raw_path_token(token: &str) -> AppResult<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|error| AppError::Config(format!("Invalid remote path token: {error}")))
}

#[derive(Debug, Clone, Serialize)]
pub struct FileProperties {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub symlink_target: Option<String>,
    pub size: u64,
    pub permissions: String,
    pub owner: String,
    pub group: String,
    pub uid: String,
    pub gid: String,
    pub mtime: u64,
    pub atime: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RemoteFileAttributeUpdate {
    pub mode: Option<String>,
    pub owner: Option<String>,
    pub group: Option<String>,
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTextFile {
    pub path: String,
    pub content: String,
    pub size: u64,
    pub mtime: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime_nanos: Option<String>,
    pub content_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextFileUnsupportedReason {
    Binary,
    UnsupportedEncoding,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TextFileOpenResult {
    Text { file: RemoteTextFile },
    Unsupported { reason: TextFileUnsupportedReason },
}

pub fn classify_text_file(file: RemoteBinaryFile) -> TextFileOpenResult {
    if file.content_bytes.contains(&0) {
        return TextFileOpenResult::Unsupported {
            reason: TextFileUnsupportedReason::Binary,
        };
    }
    match String::from_utf8(file.content_bytes) {
        Ok(content) => TextFileOpenResult::Text {
            file: RemoteTextFile {
                path: file.path,
                size: file.size,
                mtime: file.mtime,
                mtime_nanos: file.mtime_nanos,
                content_hash: content_hash(content.as_bytes()),
                content,
            },
        },
        Err(_) => TextFileOpenResult::Unsupported {
            reason: TextFileUnsupportedReason::UnsupportedEncoding,
        },
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteBinaryFile {
    pub path: String,
    pub content_bytes: Vec<u8>,
    pub size: u64,
    pub mtime: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime_nanos: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteRemoteTextResult {
    pub status: String,
    pub mtime: Option<u64>,
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime_nanos: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

impl WriteRemoteTextResult {
    pub fn saved(mtime: u64, size: u64, mtime_nanos: Option<String>, content_hash: String) -> Self {
        Self {
            status: "saved".to_string(),
            mtime: Some(mtime),
            size: Some(size),
            mtime_nanos,
            content_hash: Some(content_hash),
        }
    }

    pub fn conflict(mtime: u64, size: u64, mtime_nanos: Option<String>) -> Self {
        Self {
            status: "conflict".to_string(),
            mtime: Some(mtime),
            size: Some(size),
            mtime_nanos,
            content_hash: None,
        }
    }
}

pub(crate) fn content_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub(crate) fn ensure_text_bytes(bytes: &[u8], max_bytes: u64) -> AppResult<()> {
    if bytes.len() as u64 > max_bytes {
        return Err(AppError::Config(format!(
            "File is too large to open as text ({} bytes > {} bytes)",
            bytes.len(),
            max_bytes
        )));
    }
    if bytes.contains(&0) {
        return Err(AppError::Config(
            "Binary files are not supported by the built-in editor".to_string(),
        ));
    }
    Ok(())
}

/// POSIX shell-safe quoting: wraps `input` in single quotes and escapes any
/// embedded single-quote characters.  An empty string returns `''`.
pub(crate) fn sh_quote(input: &str) -> String {
    if input.is_empty() {
        return "''".to_string();
    }
    let escaped = input.replace('\'', "'\\''");
    format!("'{}'", escaped)
}

pub(crate) fn remote_dir_listing_path(path: &str) -> String {
    if path == "/" || path.ends_with('/') {
        path.to_string()
    } else {
        format!("{path}/")
    }
}

/// Convert a POSIX permission bitmask to the classic `ls -l` string like `-rwxr-xr-x`.
pub(crate) fn permissions_to_string(mode: u32, type_char: char) -> String {
    let mut s = String::with_capacity(10);

    s.push(type_char);

    s.push(if mode & 0o400 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o200 != 0 { 'w' } else { '-' });
    s.push(match (mode & 0o100 != 0, mode & 0o4000 != 0) {
        (true, true) => 's',
        (false, true) => 'S',
        (true, false) => 'x',
        (false, false) => '-',
    });

    s.push(if mode & 0o040 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o020 != 0 { 'w' } else { '-' });
    s.push(match (mode & 0o010 != 0, mode & 0o2000 != 0) {
        (true, true) => 's',
        (false, true) => 'S',
        (true, false) => 'x',
        (false, false) => '-',
    });

    s.push(if mode & 0o004 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o002 != 0 { 'w' } else { '-' });
    s.push(match (mode & 0o001 != 0, mode & 0o1000 != 0) {
        (true, true) => 't',
        (false, true) => 'T',
        (true, false) => 'x',
        (false, false) => '-',
    });

    s
}

pub(crate) fn type_char_from_mode(mode: u32) -> char {
    match mode & SFTP_FILE_TYPE_MASK {
        0o040000 => 'd',
        0o120000 => 'l',
        _ => '-',
    }
}

pub(crate) fn describe_permissions(mode: Option<u32>) -> String {
    match mode {
        Some(mode) => format!(
            "{mode:#06o} ({})",
            permissions_to_string(mode, type_char_from_mode(mode))
        ),
        None => "none".to_string(),
    }
}

pub(crate) fn permissions_string_to_octal_mode(permissions: &str) -> Option<String> {
    let chars: Vec<char> = permissions.chars().collect();
    if chars.len() < 10 {
        return None;
    }

    let mut mode = 0u32;
    if chars[1] == 'r' {
        mode |= 0o400;
    }
    if chars[2] == 'w' {
        mode |= 0o200;
    }
    match chars[3] {
        'x' => mode |= 0o100,
        's' => mode |= 0o4100,
        'S' => mode |= 0o4000,
        _ => {}
    }
    if chars[4] == 'r' {
        mode |= 0o040;
    }
    if chars[5] == 'w' {
        mode |= 0o020;
    }
    match chars[6] {
        'x' => mode |= 0o010,
        's' => mode |= 0o2010,
        'S' => mode |= 0o2000,
        _ => {}
    }
    if chars[7] == 'r' {
        mode |= 0o004;
    }
    if chars[8] == 'w' {
        mode |= 0o002;
    }
    match chars[9] {
        'x' => mode |= 0o001,
        't' => mode |= 0o1001,
        'T' => mode |= 0o1000,
        _ => {}
    }

    Some(format!("{mode:04o}"))
}

pub(crate) fn scp_finalize_replace_command(
    tmp_path: &str,
    target_path: &str,
    original: Option<&FileProperties>,
) -> String {
    let mut commands = Vec::new();
    if let Some(props) = original {
        if let Some(owner_group) = scp_owner_group_spec(props) {
            commands.push(format!(
                "chown {} -- {}",
                sh_quote(&owner_group),
                sh_quote(tmp_path)
            ));
        }
        if let Some(mode) = permissions_string_to_octal_mode(&props.permissions) {
            commands.push(format!(
                "chmod {} -- {}",
                sh_quote(&mode),
                sh_quote(tmp_path)
            ));
        }
    }
    commands.push(format!(
        "mv -f -- {} {}",
        sh_quote(tmp_path),
        sh_quote(target_path)
    ));
    commands.join(" && ")
}

fn scp_owner_group_spec(props: &FileProperties) -> Option<String> {
    let uid = props.uid.trim();
    let gid = props.gid.trim();
    if is_numeric_id(uid) && is_numeric_id(gid) {
        return Some(format!("{uid}:{gid}"));
    }

    let owner = props.owner.trim();
    let group = props.group.trim();
    if !owner.is_empty() && !group.is_empty() {
        return Some(format!("{owner}:{group}"));
    }

    None
}

fn is_numeric_id(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub(crate) fn owner_or_id(owner: &Option<String>, uid: Option<u32>) -> String {
    owner
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| uid.map(|value| value.to_string()))
        .unwrap_or_default()
}

pub(crate) fn group_or_id(group: &Option<String>, gid: Option<u32>) -> String {
    group
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| gid.map(|value| value.to_string()))
        .unwrap_or_default()
}

pub(crate) fn parse_octal_mode(mode: &str) -> crate::error::AppResult<u32> {
    u32::from_str_radix(mode, 8)
        .map_err(|_| crate::error::AppError::Channel(format!("Invalid octal mode: {}", mode)))
}

pub(crate) fn sanitize_download_file_name(name: &str) -> String {
    sanitize_download_file_name_for_platform(name, cfg!(windows))
}

pub(crate) fn append_safe_local_child_path(parent: &str, child_name: &str) -> String {
    std::path::Path::new(parent)
        .join(sanitize_download_file_name(child_name))
        .to_string_lossy()
        .to_string()
}

fn sanitize_download_file_name_for_platform(name: &str, windows: bool) -> String {
    let base = name;
    if base.is_empty() {
        return "download".to_string();
    }

    let mut result = String::new();
    let mut chars = base.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        let is_last = chars.peek().is_none();
        if should_percent_encode_download_char(ch, is_last, windows) {
            percent_encode_char(ch, &mut result);
        } else {
            result.push_str(&base[index..index + ch.len_utf8()]);
        }
    }

    if result.is_empty() {
        result.push_str("download");
    }

    if windows && is_windows_reserved_device_name(&result) {
        let mut chars = result.chars();
        if let Some(first) = chars.next() {
            let mut escaped = String::new();
            percent_encode_char(first, &mut escaped);
            escaped.push_str(chars.as_str());
            result = escaped;
        }
    }

    result
}

fn should_percent_encode_download_char(ch: char, is_last: bool, windows: bool) -> bool {
    ch == '%'
        || ch == '/'
        || ch == '\0'
        || ch.is_control()
        || (windows
            && (matches!(ch, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*')
                || (is_last && matches!(ch, ' ' | '.'))))
}

fn percent_encode_char(ch: char, output: &mut String) {
    let mut bytes = [0u8; 4];
    for byte in ch.encode_utf8(&mut bytes).as_bytes() {
        output.push('%');
        output.push_str(&format!("{byte:02X}"));
    }
}

fn is_windows_reserved_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.as_bytes()[3].is_ascii_digit()
            && upper.as_bytes()[3] != b'0')
}

#[cfg(test)]
mod tests {
    use super::{
        FileProperties, RemoteBinaryFile, RemotePathRef, TextFileOpenResult,
        TextFileUnsupportedReason, classify_text_file, content_hash, decode_raw_path_token,
        permissions_string_to_octal_mode, raw_path_token, sanitize_download_file_name_for_platform,
        scp_finalize_replace_command,
    };

    #[test]
    fn text_open_classifies_binary_and_invalid_utf8() {
        let file = |content_bytes: Vec<u8>| RemoteBinaryFile {
            path: "/tmp/file".to_string(),
            size: content_bytes.len() as u64,
            mtime: 1,
            mtime_nanos: None,
            content_bytes,
        };
        assert!(matches!(
            classify_text_file(file(b"hello\0world".to_vec())),
            TextFileOpenResult::Unsupported {
                reason: TextFileUnsupportedReason::Binary
            }
        ));
        assert!(matches!(
            classify_text_file(file(vec![0xff, 0xfe])),
            TextFileOpenResult::Unsupported {
                reason: TextFileUnsupportedReason::UnsupportedEncoding
            }
        ));
        let opened = classify_text_file(file(b"hello".to_vec()));
        assert!(matches!(
            opened,
            TextFileOpenResult::Text { ref file }
                if file.content == "hello" && file.content_hash == content_hash(b"hello")
        ));
    }

    #[test]
    fn raw_path_token_round_trips_non_utf8_bytes() {
        let raw_path = b"/home/user/\xce\xc4\xbc\xfe.txt";
        let token = raw_path_token(raw_path);
        assert_eq!(decode_raw_path_token(&token).unwrap(), raw_path);
    }

    #[test]
    fn remote_path_ref_prefers_token_bytes() {
        let raw_path = b"/remote/\x80name";
        let token = raw_path_token(raw_path);
        let path_ref = RemotePathRef::new("/remote/display-name", Some(&token)).unwrap();

        assert_eq!(path_ref.display_path(), "/remote/display-name");
        assert_eq!(path_ref.raw_path().unwrap(), raw_path);
    }

    #[test]
    fn remote_path_sibling_preserves_raw_parent_bytes() {
        let raw_path = b"/remote/\x80dir/\x81link";
        let token = raw_path_token(raw_path);
        let path_ref =
            RemotePathRef::new("/remote/display-dir/display-link", Some(&token)).unwrap();
        let sibling = path_ref.sibling(".nyaterm-link-test");

        assert_eq!(
            sibling.raw_path().unwrap(),
            b"/remote/\x80dir/.nyaterm-link-test"
        );
        assert_eq!(
            sibling.display_path(),
            "/remote/display-dir/.nyaterm-link-test"
        );
    }

    #[test]
    fn percent_encodes_windows_invalid_characters() {
        assert_eq!(
            sanitize_download_file_name_for_platform("a<b>:c\"d|e?f*.txt", true),
            "a%3Cb%3E%3Ac%22d%7Ce%3Ff%2A.txt"
        );
        assert_eq!(
            sanitize_download_file_name_for_platform("foo\\bar.txt", true),
            "foo%5Cbar.txt"
        );
    }

    #[test]
    fn encodes_windows_reserved_device_names() {
        assert_eq!(
            sanitize_download_file_name_for_platform("CON.txt", true),
            "%43ON.txt"
        );
        assert_eq!(
            sanitize_download_file_name_for_platform("nul", true),
            "%6Eul"
        );
        assert_eq!(
            sanitize_download_file_name_for_platform("LPT1.log", true),
            "%4CPT1.log"
        );
    }

    #[test]
    fn encodes_windows_trailing_dot_and_space() {
        assert_eq!(
            sanitize_download_file_name_for_platform("file. ", true),
            "file.%20"
        );
        assert_eq!(
            sanitize_download_file_name_for_platform("file.", true),
            "file%2E"
        );
    }

    #[test]
    fn preserves_readable_safe_characters() {
        assert_eq!(
            sanitize_download_file_name_for_platform("中文 name-_.txt", true),
            "中文 name-_.txt"
        );
    }

    #[test]
    fn encodes_percent_to_keep_generated_sequences_unambiguous() {
        assert_eq!(
            sanitize_download_file_name_for_platform("100%.txt", true),
            "100%25.txt"
        );
    }

    #[test]
    fn unix_rules_are_minimal() {
        assert_eq!(
            sanitize_download_file_name_for_platform("a:b?c*.txt", false),
            "a:b?c*.txt"
        );
        assert_eq!(
            sanitize_download_file_name_for_platform("100%.txt", false),
            "100%25.txt"
        );
    }

    #[test]
    fn parses_ls_permissions_to_octal_mode() {
        assert_eq!(
            permissions_string_to_octal_mode("-rw-r--r--").as_deref(),
            Some("0644")
        );
        assert_eq!(
            permissions_string_to_octal_mode("-rwxr-xr-x").as_deref(),
            Some("0755")
        );
        assert_eq!(
            permissions_string_to_octal_mode("-rwsr-sr-t").as_deref(),
            Some("7755")
        );
        assert_eq!(
            permissions_string_to_octal_mode("-rwSr-Sr-T").as_deref(),
            Some("7644")
        );
        assert_eq!(permissions_string_to_octal_mode("bad"), None);
    }

    #[test]
    fn scp_finalize_replace_prefers_numeric_uid_gid() {
        let props = file_properties("-rwsr-sr-t", "kang", "kang", "1000", "1001");
        let command = scp_finalize_replace_command("/tmp/new", "/etc/app.conf", Some(&props));

        assert!(command.contains("chown '1000:1001' -- '/tmp/new'"));
        assert!(command.contains("chmod '7755' -- '/tmp/new'"));
        assert!(command.ends_with("mv -f -- '/tmp/new' '/etc/app.conf'"));
    }

    #[test]
    fn scp_finalize_replace_falls_back_to_owner_group_names() {
        let props = file_properties("-rw-r--r--", "kang", "staff", "kang", "staff");
        let command = scp_finalize_replace_command("/tmp/new", "/home/kang/file", Some(&props));

        assert!(command.contains("chown 'kang:staff' -- '/tmp/new'"));
        assert!(command.contains("chmod '0644' -- '/tmp/new'"));
    }

    #[test]
    fn scp_finalize_replace_orders_chown_before_chmod_before_mv() {
        let props = file_properties("-rwSr-Sr-T", "kang", "staff", "1000", "1001");
        let command = scp_finalize_replace_command("/tmp/new", "/home/kang/file", Some(&props));

        let chown = command.find("chown").unwrap();
        let chmod = command.find("chmod").unwrap();
        let mv = command.find("mv -f").unwrap();
        assert!(chown < chmod);
        assert!(chmod < mv);
    }

    #[test]
    fn scp_finalize_replace_skips_unparseable_mode() {
        let props = file_properties("bad", "kang", "staff", "1000", "1001");
        let command = scp_finalize_replace_command("/tmp/new", "/home/kang/file", Some(&props));

        assert!(command.contains("chown '1000:1001' -- '/tmp/new'"));
        assert!(!command.contains("chmod"));
        assert!(command.ends_with("mv -f -- '/tmp/new' '/home/kang/file'"));
    }

    fn file_properties(
        permissions: &str,
        owner: &str,
        group: &str,
        uid: &str,
        gid: &str,
    ) -> FileProperties {
        FileProperties {
            name: "file".to_string(),
            is_dir: false,
            is_symlink: false,
            symlink_target: None,
            size: 0,
            permissions: permissions.to_string(),
            owner: owner.to_string(),
            group: group.to_string(),
            uid: uid.to_string(),
            gid: gid.to_string(),
            mtime: 0,
            atime: 0,
        }
    }
}
