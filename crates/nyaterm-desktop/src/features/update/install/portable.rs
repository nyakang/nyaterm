use std::collections::BTreeSet;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;

use nyaterm_core::packaging::portable::windows_portable_layout;
use nyaterm_core::updater::UpdateArchitecture;

const MAX_ENTRIES: usize = 128;
const MAX_PAYLOAD_BYTES: u64 = 1024 * 1024 * 1024;
const BACKUP_DIRECTORY: &str = "portable-backup";

pub(super) fn recovery_pending(work: &Path) -> bool {
    // Also survives a helper crash: never automatically delete a pending backup.
    work.join(BACKUP_DIRECTORY).try_exists().unwrap_or(true)
}

pub(super) fn extract_windows_portable(
    archive_path: &Path,
    destination: &Path,
    architecture: UpdateArchitecture,
) -> Result<(), String> {
    extract_with_limits(
        archive_path,
        destination,
        architecture,
        MAX_ENTRIES,
        MAX_PAYLOAD_BYTES,
    )
}

fn extract_with_limits(
    archive_path: &Path,
    destination: &Path,
    architecture: UpdateArchitecture,
    max_entries: usize,
    max_bytes: u64,
) -> Result<(), String> {
    let layout = windows_portable_layout();
    let files = layout.files(architecture);
    let file = std::fs::File::open(archive_path).map_err(|error| error.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
    if archive.len() > max_entries {
        return Err("portable update archive contains too many entries".into());
    }
    validate_directory_entry_count(
        archive_path,
        archive.central_directory_start(),
        archive.len(),
    )?;
    let mut found = BTreeSet::new();
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|error| error.to_string())?;
        let name = entry.name().trim_end_matches('/').to_owned();
        // Validate the raw ZIP name before any host-specific path normalization.
        if name.split('/').any(|part| {
            part.is_empty() || part == "." || part == ".." || part.contains(['\\', ':'])
        }) || entry.enclosed_name().is_none()
        {
            return Err("portable update archive contains an unsafe path".into());
        }
        if entry.is_symlink() {
            return Err("portable update archive contains a symbolic link".into());
        }
        let relative = if name == layout.root && entry.is_dir() {
            ""
        } else {
            name.strip_prefix(&format!("{}/", layout.root))
                .ok_or("portable update archive has an unexpected root directory")?
        };
        if !found.insert(relative.to_ascii_lowercase()) {
            return Err(format!(
                "portable update archive contains a duplicate entry: {name}"
            ));
        }
        let is_data = relative == layout.data_directory()
            || relative.starts_with(&format!("{}/", layout.data_directory()));
        if entry.is_dir() {
            if !relative.is_empty()
                && !is_data
                && !files
                    .iter()
                    .any(|file| file.starts_with(&format!("{relative}/")))
            {
                return Err(format!(
                    "portable update archive contains an unexpected directory: {name}"
                ));
            }
            continue;
        }
        if !is_data && !files.contains(&relative) {
            return Err(format!(
                "portable update archive contains an unexpected file: {name}"
            ));
        }
        let size = entry.size();
        total = total.saturating_add(size);
        if total > max_bytes {
            return Err("portable update archive is too large".into());
        }
        if is_data {
            continue;
        }
        let output = destination.join(relative);
        std::fs::create_dir_all(output.parent().ok_or("missing payload parent")?)
            .map_err(|error| format!("failed to stage {relative}: {error}"))?;
        let mut file = std::fs::File::create_new(&output)
            .map_err(|error| format!("failed to stage {relative}: {error}"))?;
        let copied = std::io::copy(&mut entry.take(size.saturating_add(1)), &mut file)
            .map_err(|error| format!("failed to extract {relative}: {error}"))?;
        if copied != size {
            return Err(format!(
                "portable update archive entry has an invalid size: {}",
                output.display()
            ));
        }
        file.sync_all()
            .map_err(|error| format!("failed to stage {}: {error}", output.display()))?;
    }
    for name in files {
        if !found.contains(&name.to_ascii_lowercase()) || !destination.join(name).is_file() {
            return Err(format!("portable update archive is missing {name}"));
        }
    }
    Ok(())
}

fn validate_directory_entry_count(path: &Path, offset: u64, expected: usize) -> Result<(), String> {
    // ZipArchive indexes by raw filename and silently merges identical names.
    // Count central records as well, so hidden duplicates cannot bypass validation.
    // The central record's fixed header and length fields are the same in ZIP64.
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut reader = std::io::BufReader::new(file);
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|error| error.to_string())?;
    for index in 0..=expected {
        let mut signature = [0_u8; 4];
        reader
            .read_exact(&mut signature)
            .map_err(|error| error.to_string())?;
        if signature != *b"PK\x01\x02" {
            return if index == expected {
                Ok(())
            } else {
                Err("portable update archive has an inconsistent directory".into())
            };
        }
        if index == expected {
            return Err("portable update archive contains a duplicate entry".into());
        }
        let mut header = [0_u8; 42];
        reader
            .read_exact(&mut header)
            .map_err(|error| error.to_string())?;
        let variable_size: i64 = [24, 26, 28]
            .into_iter()
            .map(|start| i64::from(u16::from_le_bytes([header[start], header[start + 1]])))
            .sum();
        reader
            .seek(SeekFrom::Current(variable_size))
            .map_err(|error| error.to_string())?;
    }
    unreachable!("central directory count must terminate")
}

#[derive(Debug)]
pub(super) struct PortableUpdateError {
    pub(super) message: String,
    pub(super) recovery_required: bool,
}

impl From<String> for PortableUpdateError {
    fn from(message: String) -> Self {
        Self {
            message,
            recovery_required: false,
        }
    }
}

impl From<&str> for PortableUpdateError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

fn checked_metadata(path: &Path) -> Result<Option<std::fs::Metadata>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            let redirected = metadata.file_type().is_symlink();
            #[cfg(windows)]
            let redirected = {
                use std::os::windows::fs::MetadataExt as _;
                redirected || metadata.file_attributes() & 0x400 != 0
            };
            if redirected {
                return Err(format!(
                    "portable update path is a link or reparse point: {}",
                    path.display()
                ));
            }
            Ok(Some(metadata))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "failed to inspect portable update path {}: {error}",
            path.display()
        )),
    }
}

fn validate_payload_path(root: &Path, name: &str, required: bool) -> Result<(), String> {
    let mut path = root.to_path_buf();
    for part in name.split('/') {
        if let Some(metadata) = checked_metadata(&path)?
            && !metadata.is_dir()
        {
            return Err(format!(
                "portable update parent is not a directory: {}",
                path.display()
            ));
        }
        path.push(part);
    }
    match checked_metadata(&path)? {
        Some(metadata) if metadata.is_file() => Ok(()),
        None if !required => Ok(()),
        _ => Err(format!(
            "portable update file is missing or not regular: {name}"
        )),
    }
}

pub(super) fn commit_portable_files_with(
    staged: &Path,
    target: &Path,
    work: &Path,
    architecture: UpdateArchitecture,
    mut rename: impl FnMut(&Path, &Path) -> Result<(), String>,
) -> Result<(), PortableUpdateError> {
    let files = windows_portable_layout().files(architecture);
    // Validate the whole payload and destination before moving any installed file.
    for name in &files {
        validate_payload_path(staged, name, true)?;
        validate_payload_path(target, name, false)?;
    }
    let backup = work.join(BACKUP_DIRECTORY);
    std::fs::create_dir(&backup)
        .map_err(|error| format!("failed to create portable backup: {error}"))?;
    let mut backed_up = Vec::new();
    let mut installed = Vec::new();
    let result = (|| {
        for name in &files {
            let current = target.join(name);
            if current.exists() {
                let previous = backup.join(name);
                std::fs::create_dir_all(previous.parent().ok_or("missing backup parent")?)
                    .map_err(|error| format!("failed to prepare backup for {name}: {error}"))?;
                rename(&current, &previous)
                    .map_err(|error| format!("failed to back up {name}: {error}"))?;
                backed_up.push(*name);
            }
        }
        for name in &files {
            let output = target.join(name);
            std::fs::create_dir_all(output.parent().ok_or("missing install parent")?)
                .map_err(|error| format!("failed to prepare installation for {name}: {error}"))?;
            rename(&staged.join(name), &output)
                .map_err(|error| format!("failed to replace {name}: {error}"))?;
            installed.push(*name);
        }
        Ok::<(), String>(())
    })();
    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        for name in installed.into_iter().rev() {
            if let Err(error) = std::fs::remove_file(target.join(name)) {
                rollback_errors.push(format!("failed to remove new {name}: {error}"));
            }
        }
        for name in backed_up.into_iter().rev() {
            if let Err(error) = rename(&backup.join(name), &target.join(name)) {
                rollback_errors.push(format!("failed to restore {name}: {error}"));
            }
        }
        if rollback_errors.is_empty()
            && let Err(error) = std::fs::remove_dir_all(&backup)
        {
            rollback_errors.push(format!("failed to clean restored backup: {error}"));
        }
        if rollback_errors.is_empty() {
            return Err(error.into());
        }
        return Err(PortableUpdateError {
            message: format!(
                "{error}; rollback failed: {}; recovery files retained at {}",
                rollback_errors.join(", "),
                backup.display()
            ),
            recovery_required: true,
        });
    }
    let _ = std::fs::remove_dir_all(backup);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::path::Path;

    use nyaterm_core::packaging::portable::windows_portable_layout;
    use nyaterm_core::test_support::TestTempDir;
    use nyaterm_core::updater::{UpdateArchitecture, UpdateTarget};

    use super::{
        commit_portable_files_with, extract_windows_portable, extract_with_limits, recovery_pending,
    };
    use crate::features::update::install::cleanup_update_work_dir;

    const ARCHITECTURES: [UpdateArchitecture; 2] =
        [UpdateArchitecture::X86_64, UpdateArchitecture::Aarch64];

    fn rename(from: &Path, to: &Path) -> Result<(), String> {
        std::fs::rename(from, to).map_err(|error| error.to_string())
    }

    fn write_files(directory: &Path, architecture: UpdateArchitecture, contents: &[u8]) {
        for name in windows_portable_layout().files(architecture) {
            let path = directory.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
    }

    fn entries(architecture: UpdateArchitecture) -> Vec<(String, Vec<u8>)> {
        let layout = windows_portable_layout();
        layout
            .files(architecture)
            .iter()
            .map(|name| (format!("{}/{name}", layout.root), b"new".to_vec()))
            .collect()
    }

    fn write_zip(path: &Path, entries: &[(String, Vec<u8>)], symlink: Option<&str>) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut archive = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, contents) in entries {
            archive.start_file(name, options).unwrap();
            archive.write_all(contents).unwrap();
        }
        if let Some(name) = symlink {
            archive.add_symlink(name, "NyaTerm.exe", options).unwrap();
        }
        archive.finish().unwrap();
    }

    #[test]
    fn portable_archives_extract_both_architectures_without_user_data() {
        for architecture in ARCHITECTURES {
            let root = TestTempDir::new("nyaterm-portable-extract");
            let archive = root.join("portable.zip");
            let destination = root.join("staged");
            let mut entries = entries(architecture);
            entries.push((
                format!("{}/data/user.db", windows_portable_layout().root),
                b"ignore".to_vec(),
            ));
            write_zip(&archive, &entries, None);
            extract_windows_portable(&archive, &destination, architecture).unwrap();
            for name in windows_portable_layout().files(architecture) {
                assert_eq!(std::fs::read(destination.join(name)).unwrap(), b"new");
            }
            assert!(!destination.join("data").exists());
        }
    }

    #[test]
    fn portable_archives_reject_unsafe_unknown_duplicate_missing_and_link_entries() {
        let layout = windows_portable_layout();
        for architecture in ARCHITECTURES {
            for invalid in [
                "../outside",
                "conpty/../outside",
                "/absolute",
                "C:/drive",
                "NyaTerm.exe:stream",
                "conpty\\x64\\conpty.dll",
                "conpty/unknown.dll",
                "NYATERM.EXE",
                "data/../../outside",
                "conpty/x86/conpty.dll",
                "./NyaTerm.exe",
            ] {
                let root = TestTempDir::new("nyaterm-portable-invalid");
                let mut entries = entries(architecture);
                entries.push((format!("{}/{invalid}", layout.root), b"bad".to_vec()));
                write_zip(&root.join("archive.zip"), &entries, None);
                assert!(
                    extract_windows_portable(
                        &root.join("archive.zip"),
                        &root.join("staged"),
                        architecture
                    )
                    .is_err(),
                    "accepted {invalid}"
                );
                assert!(!root.join("outside").exists());
            }
            for missing in layout.files(architecture) {
                let root = TestTempDir::new("nyaterm-portable-missing");
                let mut entries = entries(architecture);
                entries.retain(|(name, _)| name != &format!("{}/{missing}", layout.root));
                write_zip(&root.join("archive.zip"), &entries, None);
                assert!(
                    extract_windows_portable(
                        &root.join("archive.zip"),
                        &root.join("staged"),
                        architecture
                    )
                    .is_err(),
                    "accepted missing {missing}"
                );
            }
            for name in ["conpty/link", "data/link", "conpty/link/"] {
                let root = TestTempDir::new("nyaterm-portable-link");
                write_zip(
                    &root.join("archive.zip"),
                    &entries(architecture),
                    Some(&format!("{}/{name}", layout.root)),
                );
                assert!(
                    extract_windows_portable(
                        &root.join("archive.zip"),
                        &root.join("staged"),
                        architecture
                    )
                    .unwrap_err()
                    .contains("symbolic link")
                );
            }
            let root = TestTempDir::new("nyaterm-portable-wrong-arch");
            let other = if architecture == UpdateArchitecture::X86_64 {
                UpdateArchitecture::Aarch64
            } else {
                UpdateArchitecture::X86_64
            };
            write_zip(&root.join("archive.zip"), &entries(other), None);
            assert!(
                extract_windows_portable(
                    &root.join("archive.zip"),
                    &root.join("staged"),
                    architecture
                )
                .is_err()
            );
        }
    }

    #[test]
    fn portable_archive_enforces_entry_and_payload_limits() {
        let root = TestTempDir::new("nyaterm-portable-limits");
        let archive = root.join("archive.zip");
        let entries = entries(UpdateArchitecture::X86_64);
        write_zip(&archive, &entries, None);
        assert!(
            extract_with_limits(
                &archive,
                &root.join("staged"),
                UpdateArchitecture::X86_64,
                entries.len() - 1,
                1024
            )
            .unwrap_err()
            .contains("too many entries")
        );
        assert!(
            extract_with_limits(
                &archive,
                &root.join("staged"),
                UpdateArchitecture::X86_64,
                128,
                1
            )
            .unwrap_err()
            .contains("too large")
        );
    }

    #[test]
    fn portable_archive_rejects_identical_names_hidden_by_zip_index() {
        let root = TestTempDir::new("nyaterm-portable-duplicate");
        let path = root.join("archive.zip");
        write_zip(&path, &entries(UpdateArchitecture::X86_64), None);
        let mut bytes = std::fs::read(&path).unwrap();
        let central = bytes
            .windows(4)
            .position(|value| value == b"PK\x01\x02")
            .unwrap();
        let end = bytes
            .windows(4)
            .rposition(|value| value == b"PK\x05\x06")
            .unwrap();
        let read_u16 = |offset| u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        let count = read_u16(end + 10);
        let length = 46
            + [28, 30, 32]
                .into_iter()
                .map(|offset| usize::from(read_u16(central + offset)))
                .sum::<usize>();
        let directory_size = u32::from_le_bytes(bytes[end + 12..end + 16].try_into().unwrap());
        let duplicate = bytes[central..central + length].to_vec();
        bytes.splice(end..end, duplicate);
        let end = end + length;
        bytes[end + 8..end + 10].copy_from_slice(&(count + 1).to_le_bytes());
        bytes[end + 10..end + 12].copy_from_slice(&(count + 1).to_le_bytes());
        bytes[end + 12..end + 16].copy_from_slice(&(directory_size + length as u32).to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        assert!(
            extract_windows_portable(&path, &root.join("staged"), UpdateArchitecture::X86_64)
                .unwrap_err()
                .contains("duplicate entry")
        );
    }

    #[test]
    fn portable_commit_rolls_back_every_move_and_removes_new_components() {
        for architecture in ARCHITECTURES {
            let files = windows_portable_layout().files(architecture);
            for old_complete in [true, false] {
                let old_files = files
                    .iter()
                    .filter(|name| {
                        old_complete
                            || !name.starts_with("conpty/")
                                && **name != "nyaterm-activex-helper.exe"
                    })
                    .copied()
                    .collect::<Vec<_>>();
                for fail_at in 0..old_files.len() + files.len() {
                    let root = TestTempDir::new("nyaterm-portable-rollback");
                    let (staged, target, work) =
                        (root.join("staged"), root.join("target"), root.join("work"));
                    write_files(&staged, architecture, b"new");
                    write_files(&target, architecture, b"old");
                    for name in files.iter().filter(|name| !old_files.contains(name)) {
                        std::fs::remove_file(target.join(name)).unwrap();
                    }
                    std::fs::create_dir_all(target.join("data")).unwrap();
                    std::fs::write(target.join("data/user.db"), b"user-data").unwrap();
                    std::fs::create_dir(&work).unwrap();
                    let mut call = 0;
                    let error = commit_portable_files_with(
                        &staged,
                        &target,
                        &work,
                        architecture,
                        |from, to| {
                            let current = call;
                            call += 1;
                            if current == fail_at {
                                Err("injected failure".into())
                            } else {
                                rename(from, to)
                            }
                        },
                    )
                    .unwrap_err();
                    assert!(!error.recovery_required, "{}", error.message);
                    for name in &files {
                        if old_files.contains(name) {
                            assert_eq!(
                                std::fs::read(target.join(name)).unwrap(),
                                b"old",
                                "failure {fail_at}: {name}"
                            );
                        } else {
                            assert!(!target.join(name).exists(), "new {name} survived rollback");
                        }
                    }
                    assert_eq!(
                        std::fs::read(target.join("data/user.db")).unwrap(),
                        b"user-data"
                    );
                    assert!(!recovery_pending(&work));
                }
            }
        }
    }

    #[test]
    fn portable_commit_validates_entire_payload_before_backing_up() {
        let architecture = UpdateArchitecture::X86_64;
        let root = TestTempDir::new("nyaterm-portable-preflight");
        let (staged, target, work) = (root.join("staged"), root.join("target"), root.join("work"));
        write_files(&staged, architecture, b"new");
        write_files(&target, architecture, b"old");
        std::fs::remove_file(staged.join("conpty/x64/conpty.dll")).unwrap();
        let error = commit_portable_files_with(&staged, &target, &work, architecture, |_, _| {
            panic!("must not move files")
        })
        .unwrap_err();
        assert!(!error.recovery_required);
        assert_eq!(std::fs::read(target.join("NyaTerm.exe")).unwrap(), b"old");
    }

    #[test]
    fn failed_rollback_keeps_backup_and_prevents_automatic_cleanup() {
        let architecture = UpdateArchitecture::X86_64;
        let root = TestTempDir::new("nyaterm-portable-recovery");
        let (staged, target, work) = (root.join("staged"), root.join("target"), root.join("work"));
        write_files(&staged, architecture, b"new");
        write_files(&target, architecture, b"old");
        std::fs::create_dir(&work).unwrap();
        let error =
            commit_portable_files_with(&staged, &target, &work, architecture, |from, to| {
                if from.starts_with(&staged) || from.starts_with(work.join("portable-backup")) {
                    Err("injected install/restore failure".into())
                } else {
                    rename(from, to)
                }
            })
            .unwrap_err();
        assert!(error.recovery_required);
        assert!(error.message.contains("rollback failed"));
        cleanup_update_work_dir(work.clone());
        assert_eq!(
            std::fs::read(work.join("portable-backup/NyaTerm.exe")).unwrap(),
            b"old"
        );
        assert!(recovery_pending(&work));
    }

    #[cfg(windows)]
    #[test]
    fn locked_conpty_file_rolls_back_program_files_and_can_be_retried() {
        use std::os::windows::fs::OpenOptionsExt as _;

        let architecture = UpdateArchitecture::X86_64;
        let root = TestTempDir::new("nyaterm-portable-locked-file");
        let (staged, target, work) = (root.join("staged"), root.join("target"), root.join("work"));
        write_files(&staged, architecture, b"new");
        write_files(&target, architecture, b"old");
        std::fs::create_dir(&work).unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(target.join("conpty/x64/conpty.dll"))
            .unwrap();
        let error =
            commit_portable_files_with(&staged, &target, &work, architecture, rename).unwrap_err();
        assert!(!error.recovery_required, "{}", error.message);
        assert!(
            error
                .message
                .contains("failed to back up conpty/x64/conpty.dll")
        );
        assert!(error.message.contains("os error 32"));
        for name in windows_portable_layout().files(architecture) {
            assert_eq!(std::fs::read(target.join(name)).unwrap(), b"old");
        }
        drop(lock);
        commit_portable_files_with(&staged, &target, &work, architecture, rename).unwrap();
        for name in windows_portable_layout().files(architecture) {
            assert_eq!(std::fs::read(target.join(name)).unwrap(), b"new");
        }
    }

    #[cfg(windows)]
    #[test]
    fn portable_commit_rejects_target_junction_before_moving_files() {
        use std::os::windows::process::CommandExt as _;
        use std::process::{Command, Stdio};

        let architecture = UpdateArchitecture::X86_64;
        let root = TestTempDir::new("nyaterm-portable-junction");
        let (staged, target, outside, work) = (
            root.join("staged"),
            root.join("target"),
            root.join("outside"),
            root.join("work"),
        );
        write_files(&staged, architecture, b"new");
        write_files(&target, architecture, b"old");
        std::fs::remove_dir_all(target.join("conpty")).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let status = Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(target.join("conpty"))
            .arg(&outside)
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        let error = commit_portable_files_with(&staged, &target, &work, architecture, |_, _| {
            panic!("must not move files")
        })
        .unwrap_err();
        // Remove only the junction, never its target.
        std::fs::remove_dir(target.join("conpty")).unwrap();
        assert!(error.message.contains("reparse point"));
        assert_eq!(std::fs::read(target.join("NyaTerm.exe")).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
    }

    fn verify_real_package(archive: &Path, architecture: UpdateArchitecture) {
        let root = TestTempDir::new("nyaterm-portable-real-package");
        let (staged, target, work) = (root.join("staged"), root.join("target"), root.join("work"));
        extract_windows_portable(archive, &staged, architecture).unwrap();
        write_files(&target, architecture, b"old");
        std::fs::remove_file(target.join("nyaterm-activex-helper.exe")).unwrap();
        std::fs::remove_dir_all(target.join("conpty")).unwrap();
        std::fs::create_dir_all(target.join("data/config")).unwrap();
        std::fs::write(target.join("data/config/user.db"), b"user-data").unwrap();
        std::fs::write(target.join("user-file.txt"), b"keep").unwrap();
        std::fs::create_dir(&work).unwrap();
        let expected = windows_portable_layout()
            .files(architecture)
            .iter()
            .map(|name| (*name, std::fs::read(staged.join(name)).unwrap()))
            .collect::<Vec<_>>();
        commit_portable_files_with(&staged, &target, &work, architecture, rename).unwrap();
        for (name, contents) in expected {
            assert_eq!(
                std::fs::read(target.join(name)).unwrap(),
                contents,
                "{name}"
            );
        }
        assert_eq!(
            std::fs::read(target.join("data/config/user.db")).unwrap(),
            b"user-data"
        );
        assert_eq!(
            std::fs::read(target.join("user-file.txt")).unwrap(),
            b"keep"
        );
        assert!(!target.join("data/.keep").exists());
        assert!(!recovery_pending(&work));
        cleanup_update_work_dir(work.clone());
        assert!(!work.exists());
    }

    #[test]
    fn portable_commit_installs_complete_payload_and_preserves_user_files() {
        for architecture in ARCHITECTURES {
            let root = TestTempDir::new("nyaterm-portable-install");
            write_zip(&root.join("archive.zip"), &entries(architecture), None);
            verify_real_package(&root.join("archive.zip"), architecture);
        }
    }

    /// Release CI supplies the ZIP just produced by the actual packaging script.
    #[test]
    #[ignore = "requires NYATERM_TEST_PORTABLE_ARCHIVE and NYATERM_TEST_PORTABLE_ARCH"]
    fn portable_release_archive_round_trip() {
        let archive =
            std::env::var_os("NYATERM_TEST_PORTABLE_ARCHIVE").expect("archive path required");
        let arch =
            std::env::var("NYATERM_TEST_PORTABLE_ARCH").expect("archive architecture required");
        let architecture = UpdateTarget::from_rust_target("windows", &arch)
            .unwrap()
            .architecture;
        verify_real_package(Path::new(&archive), architecture);
    }
}
