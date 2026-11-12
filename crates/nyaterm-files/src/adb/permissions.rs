//! Conservative mount capabilities. Cache lifetime is one immutable device binding.
use crate::adb::{AdbFileService, raw_path};
use crate::sftp::{RemoteFilePath, SftpTransferControl};
use nyaterm_android::adb::shell::{execute_with_cancel, quote};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PermissionCapability {
    Posix,
    MountAssigned,
    Unknown,
}

#[derive(Default)]
pub(super) struct PermissionCache(Mutex<HashMap<Vec<u8>, PermissionCapability>>);
impl std::fmt::Debug for PermissionCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PermissionCache { .. }")
    }
}

fn unescape(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            let digits = bytes.get(i + 1..i + 4)?;
            if !digits.iter().all(|b| (b'0'..=b'7').contains(b)) {
                return None;
            }
            let value = (digits[0] - b'0') as u16 * 64
                + (digits[1] - b'0') as u16 * 8
                + (digits[2] - b'0') as u16;
            output.push(u8::try_from(value).ok()?);
            i += 4;
        } else {
            output.push(bytes[i]);
            i += 1;
        }
    }
    Some(output)
}
fn below(path: &[u8], root: &[u8]) -> bool {
    root == b"/" || path == root || (path.starts_with(root) && path.get(root.len()) == Some(&b'/'))
}

fn classify(probe: &[u8]) -> Option<(Vec<u8>, PermissionCapability)> {
    let mut sections = probe.splitn(3, |b| *b == 0);
    let canonical = sections.next()?.strip_suffix(b"\n")?;
    if !canonical.starts_with(b"/") {
        return None;
    }
    let stat_fs = sections.next()?.strip_suffix(b"\n")?;
    let mounts = sections.next()?;
    let mut selected = None;
    let mut longest = 0;
    for line in mounts.split(|b| *b == b'\n') {
        let fields: Vec<_> = line.split(|b| *b == b' ').collect();
        let Some(separator) = fields.iter().position(|f| *f == b"-") else {
            continue;
        };
        if separator < 6 || fields.len() < separator + 4 {
            continue;
        }
        let Some(mount) = unescape(fields[4]) else {
            continue;
        };
        if !below(canonical, &mount) || mount.len() < longest {
            continue;
        }
        longest = mount.len();
        let fs = fields[separator + 1];
        let source = fields[separator + 2];
        use PermissionCapability::{MountAssigned, Posix, Unknown};
        let capability = match fs {
            b"sdcardfs" if stat_fs == b"sdcardfs" => MountAssigned,
            b"vfat" | b"msdos" if matches!(stat_fs, b"msdos" | b"vfat") => MountAssigned,
            b"exfat" if stat_fs == b"exfat" => MountAssigned,
            b"fuse"
                if stat_fs == b"fuse"
                    && source == b"/dev/fuse"
                    && (below(&mount, b"/storage/emulated")
                        || [
                            b"/mnt/runtime/default/emulated".as_slice(),
                            b"/mnt/runtime/read/emulated",
                            b"/mnt/runtime/write/emulated",
                            b"/mnt/runtime/full/emulated",
                        ]
                        .iter()
                        .any(|root| below(&mount, root))) =>
            {
                MountAssigned
            }
            b"ext4" | b"f2fs" | b"tmpfs" | b"xfs" | b"btrfs" if fs == stat_fs => Posix,
            _ => Unknown,
        };
        // Full mount identity includes mount ID, device, root, mountpoint and filesystem.
        let mut identity = line.to_vec();
        identity.push(0);
        identity.extend_from_slice(stat_fs);
        selected = Some((identity, capability));
    }
    selected
}

impl AdbFileService {
    pub(super) fn permission_capability(
        &self,
        parent: &RemoteFilePath,
        control: &SftpTransferControl,
    ) -> anyhow::Result<PermissionCapability> {
        let quoted = quote(&raw_path(parent)?)?;
        let mut command = b"realpath -- ".to_vec();
        command.extend_from_slice(&quoted);
        command.extend_from_slice(b" && printf '\\000' && stat -f -c '%T' -- ");
        command.extend_from_slice(&quoted);
        command.extend_from_slice(b" && printf '\\000' && cat /proc/self/mountinfo");
        let probe = execute_with_cancel(&self.binding, &command, 1024 * 1024, || {
            self.check_transfer(control)
        });
        self.check_transfer(control)?;
        let Some((identity, capability)) = probe.ok().and_then(|bytes| classify(&bytes)) else {
            return Ok(PermissionCapability::Unknown);
        };
        let mut cache = self.permissions.0.lock().unwrap_or_else(|p| p.into_inner());
        if cache.len() >= 64 {
            cache.clear();
        }
        Ok(*cache.entry(identity).or_insert(capability))
    }
}

#[cfg(test)]
mod tests {
    use crate::adb::permissions::{PermissionCapability, classify};
    #[test]
    fn only_confirmed_android_emulated_fuse_is_mount_assigned() {
        for (mount, fs, stat, source, expected) in [
            (
                "/storage/emulated",
                "fuse",
                "fuse",
                "/dev/fuse",
                PermissionCapability::MountAssigned,
            ),
            (
                "/storage/other",
                "fuse",
                "fuse",
                "/dev/fuse",
                PermissionCapability::Unknown,
            ),
            (
                "/storage/emulated",
                "fuse",
                "fuse",
                "other",
                PermissionCapability::Unknown,
            ),
            (
                "/storage/emulated",
                "ext4",
                "ext4",
                "/dev/block",
                PermissionCapability::Posix,
            ),
            (
                "/storage/emulated",
                "vfat",
                "msdos",
                "/dev/block",
                PermissionCapability::MountAssigned,
            ),
            (
                "/storage/emulated",
                "ext4",
                "fuse",
                "/dev/block",
                PermissionCapability::Unknown,
            ),
        ] {
            let probe =
                format!("{mount}/0\n\0{stat}\n\x001 0 0:1 / {mount} rw - {fs} {source} rw\n");
            assert_eq!(classify(probe.as_bytes()).unwrap().1, expected);
        }
        assert!(classify(b"invalid").is_none());
    }
    #[test]
    fn longest_mount_and_component_boundaries_prevent_permission_relaxation() {
        let probe = b"/storage/emulatedX/0\n\0fuse\n\x001 0 0:1 / / rw - ext4 /dev/block rw\n2 1 0:2 / /storage/emulated rw - fuse /dev/fuse rw\n";
        assert_eq!(classify(probe).unwrap().1, PermissionCapability::Unknown);
        let probe = b"/storage/emulated/0/private/file\n\0ext4\n\x001 0 0:1 / /storage/emulated rw - fuse /dev/fuse rw\n2 1 0:2 / /storage/emulated/0/private rw - ext4 /dev/block rw\n";
        assert_eq!(classify(probe).unwrap().1, PermissionCapability::Posix);
    }
}
