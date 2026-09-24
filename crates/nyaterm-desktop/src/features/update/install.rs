#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use std::io::Read as _;
#[cfg(windows)]
use std::path::Component;
#[cfg(any(windows, target_os = "macos", test))]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const PORTABLE_HELPER_FLAG: &str = "--nyaterm-portable-update-helper";
const INSTALLED_HELPER_FLAG: &str = "--nyaterm-installed-update-helper";
const UPDATE_CLEANUP_ENV: &str = "NYATERM_UPDATE_CLEANUP";
#[cfg(windows)]
const PORTABLE_ROOT: &str = "NyaTerm-portable";
#[cfg(windows)]
const PORTABLE_MARKER: &str = "nyaterm-portable";
#[cfg(windows)]
const PORTABLE_FILES: [&str; 7] = [
    "NyaTerm.exe",
    "nyaterm-rdp-helper.exe",
    "nyaterm-vnc-helper.exe",
    "nyaterm-mcp.exe",
    PORTABLE_MARKER,
    "LICENSE",
    "VERSION",
];
#[cfg(windows)]
const MAX_PORTABLE_ENTRIES: usize = 128;
#[cfg(windows)]
const MAX_PORTABLE_PAYLOAD_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) enum PreparedUpdate {
    #[cfg(not(windows))]
    Installed { artifact: PathBuf, target: PathBuf },
    #[cfg(windows)]
    WindowsInstalled {
        helper: PathBuf,
        installer: PathBuf,
        target: PathBuf,
        work_dir: PathBuf,
    },
    #[cfg(windows)]
    WindowsPortable {
        helper: PathBuf,
        staged_dir: PathBuf,
        target_dir: PathBuf,
        work_dir: PathBuf,
    },
}

pub(super) fn prepare_artifact(
    artifact: PathBuf,
    executable: PathBuf,
    portable: bool,
) -> Result<PreparedUpdate, String> {
    if portable {
        #[cfg(windows)]
        return prepare_windows_portable(artifact, executable);
        #[cfg(not(windows))]
        return Err("portable updates are only supported on Windows".into());
    }
    #[cfg(windows)]
    {
        let directory = executable
            .parent()
            .ok_or("installed executable has no parent directory")?;
        if !directory.join("Uninstall.exe").is_file() {
            return Err("this installation must be updated manually".into());
        }
        let work_dir = artifact
            .parent()
            .ok_or("update directory unavailable")?
            .to_path_buf();
        let helper = work_dir.join("NyaTerm-update-helper.exe");
        std::fs::copy(&executable, &helper).map_err(|error| error.to_string())?;
        Ok(PreparedUpdate::WindowsInstalled {
            helper,
            installer: artifact,
            target: executable,
            work_dir,
        })
    }
    #[cfg(target_os = "macos")]
    {
        let target = executable
            .ancestors()
            .nth(3)
            .ok_or("application bundle unavailable")?
            .to_path_buf();
        if target
            .components()
            .any(|part| part.as_os_str() == "Caskroom" || part.as_os_str() == "Cellar")
        {
            return Err("use Homebrew to update this installation".into());
        }
        if target.extension().and_then(|value| value.to_str()) != Some("app") {
            return Err("not an application bundle".into());
        }
        if target.file_name().and_then(|name| name.to_str())
            != Some(nyaterm_core::app_identity::AppFlavor::current().macos_bundle_name())
        {
            return Err("application bundle belongs to a different identity".into());
        }
        let stage = target
            .parent()
            .ok_or("application parent unavailable")?
            .join(format!(".nyaterm-update-{}", nyaterm_core::uuid()));
        std::fs::create_dir(&stage).map_err(|error| error.to_string())?;
        let file = std::fs::File::open(&artifact).map_err(|error| error.to_string())?;
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
        for entry in archive.entries().map_err(|error| error.to_string())? {
            let mut entry = entry.map_err(|error| error.to_string())?;
            if !entry.header().entry_type().is_file() && !entry.header().entry_type().is_dir() {
                return Err("unsupported bundle archive entry".into());
            }
            if !entry.unpack_in(&stage).map_err(|error| error.to_string())? {
                return Err("invalid bundle archive path".into());
            }
        }
        let bundle = staged_macos_bundle(&stage, nyaterm_core::app_identity::AppFlavor::current())?;
        Ok(PreparedUpdate::Installed {
            artifact: bundle,
            target,
        })
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = executable;
        let target = PathBuf::from(
            std::env::var_os("APPIMAGE")
                .ok_or("use the package manager to update this installation")?,
        );
        let target = target.canonicalize().map_err(|error| error.to_string())?;
        let stage = target
            .parent()
            .ok_or("AppImage parent unavailable")?
            .join(format!(".nyaterm-update-{}.AppImage", nyaterm_core::uuid()));
        std::fs::copy(&artifact, &stage).map_err(|error| error.to_string())?;
        std::fs::set_permissions(&stage, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| error.to_string())?;
        Ok(PreparedUpdate::Installed {
            artifact: stage,
            target,
        })
    }
}

#[cfg(windows)]
fn prepare_windows_portable(
    artifact: PathBuf,
    executable: PathBuf,
) -> Result<PreparedUpdate, String> {
    let target_dir = executable
        .parent()
        .ok_or("portable executable has no parent directory")?
        .to_path_buf();
    if !target_dir.join(PORTABLE_MARKER).is_file() {
        return Err("portable installation marker is missing".into());
    }
    let work_dir = artifact
        .parent()
        .ok_or("update directory unavailable")?
        .to_path_buf();
    let staged_dir = work_dir.join("portable-payload");
    std::fs::create_dir(&staged_dir).map_err(|error| error.to_string())?;
    let result = extract_windows_portable(&artifact, &staged_dir);
    if let Err(error) = result {
        let _ = std::fs::remove_dir_all(&staged_dir);
        return Err(error);
    }
    let helper = work_dir.join("NyaTerm-update-helper.exe");
    std::fs::copy(&executable, &helper).map_err(|error| error.to_string())?;
    Ok(PreparedUpdate::WindowsPortable {
        helper,
        staged_dir,
        target_dir,
        work_dir,
    })
}

#[cfg(windows)]
fn extract_windows_portable(archive_path: &Path, destination: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive_path).map_err(|error| error.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
    if archive.len() > MAX_PORTABLE_ENTRIES {
        return Err("portable update archive contains too many entries".into());
    }
    let mut found = std::collections::BTreeSet::new();
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|error| error.to_string())?;
        let enclosed = entry
            .enclosed_name()
            .ok_or("portable update archive contains an unsafe path")?;
        let mut components = enclosed.components();
        if components.next() != Some(Component::Normal(PORTABLE_ROOT.as_ref())) {
            return Err("portable update archive has an unexpected root directory".into());
        }
        let mut relative = PathBuf::new();
        for component in components {
            let Component::Normal(name) = component else {
                return Err("portable update archive contains an unsafe path".into());
            };
            relative.push(name);
        }
        if relative.as_os_str().is_empty() || entry.is_dir() {
            continue;
        }
        if entry.is_symlink() {
            return Err("portable update archive contains a symbolic link".into());
        }
        if relative.starts_with("data") {
            continue;
        }
        let Some(name) = relative.to_str() else {
            return Err("portable update archive contains a non-Unicode filename".into());
        };
        let expected = PORTABLE_FILES
            .iter()
            .find(|expected| expected.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                format!("portable update archive contains an unexpected file: {name}")
            })?;
        if !found.insert(expected.to_ascii_lowercase()) {
            return Err(format!(
                "portable update archive contains a duplicate file: {name}"
            ));
        }
        if name != *expected {
            return Err(format!(
                "portable update archive contains an unexpected file: {name}"
            ));
        }
        total = total.saturating_add(entry.size());
        if total > MAX_PORTABLE_PAYLOAD_BYTES {
            return Err("portable update archive is too large".into());
        }
        let output = destination.join(name);
        let mut file = std::fs::File::create(output).map_err(|error| error.to_string())?;
        let copied = std::io::copy(&mut entry.take(MAX_PORTABLE_PAYLOAD_BYTES + 1), &mut file)
            .map_err(|error| error.to_string())?;
        if copied > MAX_PORTABLE_PAYLOAD_BYTES {
            return Err("portable update archive entry is too large".into());
        }
        file.sync_all().map_err(|error| error.to_string())?;
    }
    for name in PORTABLE_FILES {
        if !found.contains(&name.to_ascii_lowercase()) {
            return Err(format!("portable update archive is missing {name}"));
        }
    }
    Ok(())
}

/// Start a detached installer only after the existing application close guards pass.
/// The helper waits for this process before touching the installed application.
pub(super) fn launch_installer(prepared: &PreparedUpdate) -> Result<(), String> {
    #[cfg(windows)]
    if let PreparedUpdate::WindowsInstalled {
        helper,
        installer,
        target,
        work_dir,
    } = prepared
    {
        return launch_windows_helper(
            helper,
            INSTALLED_HELPER_FLAG,
            [
                std::process::id().to_string().into(),
                installer.as_os_str().to_owned(),
                target.as_os_str().to_owned(),
                work_dir.as_os_str().to_owned(),
            ],
        );
    }
    #[cfg(windows)]
    if let PreparedUpdate::WindowsPortable {
        helper,
        staged_dir,
        target_dir,
        work_dir,
    } = prepared
    {
        return launch_windows_helper(
            helper,
            PORTABLE_HELPER_FLAG,
            [
                std::process::id().to_string().into(),
                staged_dir.as_os_str().to_owned(),
                target_dir.as_os_str().to_owned(),
                work_dir.as_os_str().to_owned(),
            ],
        );
    }
    #[cfg(unix)]
    {
        let PreparedUpdate::Installed { artifact, target } = prepared;
        let script = r#"parent=$1; staged=$2; target=$3; mode=$4
while kill -0 "$parent" 2>/dev/null; do sleep 0.2; done
backup="${target}.previous.$$"
if test -e "$backup"; then exit 1; fi
mv -- "$target" "$backup" || exit 1
if mv -- "$staged" "$target"; then
  rm -rf -- "$backup"
  if test "$mode" = macos; then open "$target"; else "$target" >/dev/null 2>&1 & fi
else
  mv -- "$backup" "$target"
fi
"#;
        Command::new("sh")
            .args(["-c", script, "nyaterm-update"])
            .arg(std::process::id().to_string())
            .arg(artifact)
            .arg(target)
            .arg(std::env::consts::OS)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(windows)]
fn launch_windows_helper(
    helper: &Path,
    flag: &str,
    args: impl IntoIterator<Item = OsString>,
) -> Result<(), String> {
    use std::os::windows::process::CommandExt as _;

    Command::new(helper)
        .arg(flag)
        .args(args)
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub fn run_update_helper_if_requested() -> bool {
    let args = std::env::args_os().collect::<Vec<_>>();
    let Some(flag) = args.get(1).and_then(|value| value.to_str()) else {
        return false;
    };
    if flag != PORTABLE_HELPER_FLAG && flag != INSTALLED_HELPER_FLAG {
        return false;
    }
    #[cfg(windows)]
    {
        let result = match flag {
            PORTABLE_HELPER_FLAG => run_windows_portable_helper(&args),
            INSTALLED_HELPER_FLAG => run_windows_installed_helper(&args),
            _ => unreachable!(),
        };
        let target = args.get(4).map(PathBuf::from).map(|path| {
            if flag == PORTABLE_HELPER_FLAG {
                path.join("NyaTerm.exe")
            } else {
                path
            }
        });
        let work_dir = args.get(5).map(PathBuf::from);
        if let Some(target) = target {
            if let Err(error) = &result
                && let Some(target_dir) = target.parent()
            {
                write_update_error(target_dir, error);
            }
            if target.is_file() {
                let mut command = Command::new(target);
                if let Some(work_dir) = work_dir {
                    command.env(UPDATE_CLEANUP_ENV, work_dir);
                }
                let _ = command.spawn();
            }
        }
    }
    true
}

pub(crate) fn take_update_cleanup_path() -> Option<PathBuf> {
    let path = std::env::var_os(UPDATE_CLEANUP_ENV).map(PathBuf::from)?;
    unsafe { std::env::remove_var(UPDATE_CLEANUP_ENV) };
    Some(path)
}

pub(crate) fn cleanup_update_work_dir(path: PathBuf) {
    let _ = std::fs::remove_dir_all(path);
}

#[cfg(windows)]
fn run_windows_portable_helper(args: &[OsString]) -> Result<(), String> {
    if args.len() != 6 {
        return Err("invalid portable update helper arguments".into());
    }
    let parent = args[2]
        .to_str()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or("invalid updater parent process ID")?;
    let staged_dir = PathBuf::from(&args[3]);
    let target_dir = PathBuf::from(&args[4]);
    let work_dir = PathBuf::from(&args[5]);
    if !target_dir.join(PORTABLE_MARKER).is_file()
        || staged_dir.parent() != Some(work_dir.as_path())
    {
        return Err("portable update paths failed validation".into());
    }
    wait_for_process_exit(parent)?;
    commit_portable_files(&staged_dir, &target_dir, &work_dir)
}

#[cfg(windows)]
fn run_windows_installed_helper(args: &[OsString]) -> Result<(), String> {
    use std::os::windows::process::CommandExt as _;

    if args.len() != 6 {
        return Err("invalid installed update helper arguments".into());
    }
    let parent = parse_helper_parent(args)?;
    let installer = PathBuf::from(&args[3]);
    let target = PathBuf::from(&args[4]);
    let work_dir = PathBuf::from(&args[5]);
    if installer.parent() != Some(work_dir.as_path()) || !installer.is_file() {
        return Err("installed update paths failed validation".into());
    }
    let target_dir = target
        .parent()
        .ok_or("installed application directory is unavailable")?;
    if target.file_name().and_then(|name| name.to_str()) != Some("NyaTerm.exe")
        || !target_dir.join("Uninstall.exe").is_file()
    {
        return Err("installed update target failed validation".into());
    }
    wait_for_process_exit(parent)?;

    let backup = target_dir.with_extension(format!("previous-{}", nyaterm_core::uuid()));
    copy_directory(target_dir, &backup)?;
    let status = Command::new(&installer)
        .arg("/S")
        .arg(format!("/D={}", target_dir.display()))
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("failed to start update installer: {error}"));
    let installed = status
        .as_ref()
        .is_ok_and(|status| status.success() && target.is_file());
    if installed {
        std::fs::remove_dir_all(&backup)
            .map_err(|error| format!("failed to remove installed update backup: {error}"))?;
        return Ok(());
    }

    let install_error = status
        .map(|status| format!("update installer exited with {status}"))
        .unwrap_or_else(|error| error);
    rollback_installed_directory(target_dir, &backup)
        .map_err(|rollback| format!("{install_error}; rollback failed: {rollback}"))?;
    Err(install_error)
}

#[cfg(windows)]
fn parse_helper_parent(args: &[OsString]) -> Result<u32, String> {
    args.get(2)
        .and_then(|value| value.to_str())
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| "invalid updater parent process ID".to_string())
}

#[cfg(windows)]
fn copy_directory(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Err("installed update backup already exists".into());
    }
    std::fs::create_dir(destination).map_err(|error| error.to_string())?;
    let result = copy_directory_contents(source, destination);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(destination);
    }
    result
}

#[cfg(windows)]
fn copy_directory_contents(source: &Path, destination: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        let output = destination.join(entry.file_name());
        if file_type.is_dir() {
            std::fs::create_dir(&output).map_err(|error| error.to_string())?;
            copy_directory_contents(&entry.path(), &output)?;
        } else if file_type.is_file() {
            std::fs::copy(entry.path(), output).map_err(|error| error.to_string())?;
        } else {
            return Err("installed application contains an unsupported filesystem entry".into());
        }
    }
    Ok(())
}

#[cfg(windows)]
fn rollback_installed_directory(target: &Path, backup: &Path) -> Result<(), String> {
    let failed = target.with_extension(format!("failed-{}", nyaterm_core::uuid()));
    if target.exists() {
        rename_with_retry(target, &failed)?;
    }
    if let Err(error) = rename_with_retry(backup, target) {
        if failed.exists() {
            let _ = rename_with_retry(&failed, target);
        }
        return Err(error);
    }
    let _ = std::fs::remove_dir_all(failed);
    Ok(())
}

#[cfg(windows)]
fn wait_for_process_exit(process_id: u32) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, process_id) };
    if process.is_null() {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
            return Ok(());
        }
        return Err(format!("failed to wait for NyaTerm to exit: {error}"));
    }
    let result = unsafe { WaitForSingleObject(process, 120_000) };
    unsafe { CloseHandle(process) };
    if result != WAIT_OBJECT_0 {
        return Err("timed out waiting for NyaTerm to exit".into());
    }
    Ok(())
}

#[cfg(windows)]
fn commit_portable_files(staged: &Path, target: &Path, work: &Path) -> Result<(), String> {
    commit_portable_files_with(staged, target, work, rename_with_retry)
}

#[cfg(windows)]
fn commit_portable_files_with(
    staged: &Path,
    target: &Path,
    work: &Path,
    mut rename: impl FnMut(&Path, &Path) -> Result<(), String>,
) -> Result<(), String> {
    let backup = work.join("portable-backup");
    std::fs::create_dir(&backup).map_err(|error| error.to_string())?;
    let mut backed_up = Vec::new();
    let mut installed = Vec::new();
    let result = (|| {
        for name in PORTABLE_FILES {
            if !staged.join(name).is_file() {
                return Err(format!("staged portable update is missing {name}"));
            }
            let current = target.join(name);
            if current.exists() {
                rename(&current, &backup.join(name))?;
                backed_up.push(name);
            }
        }
        for name in PORTABLE_FILES {
            rename(&staged.join(name), &target.join(name))?;
            installed.push(name);
        }
        Ok(())
    })();
    if let Err(error) = result {
        for name in installed.into_iter().rev() {
            let _ = std::fs::remove_file(target.join(name));
        }
        let mut rollback_errors = Vec::new();
        for name in backed_up.into_iter().rev() {
            if let Err(rollback) = rename(&backup.join(name), &target.join(name)) {
                rollback_errors.push(format!("{name}: {rollback}"));
            }
        }
        if rollback_errors.is_empty() {
            return Err(error);
        }
        return Err(format!(
            "{error}; rollback failed: {}",
            rollback_errors.join(", ")
        ));
    }
    let _ = std::fs::remove_dir_all(backup);
    Ok(())
}

#[cfg(windows)]
fn rename_with_retry(from: &Path, to: &Path) -> Result<(), String> {
    let mut last_error = None;
    for _ in 0..100 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
    Err(last_error
        .map(|error| error.to_string())
        .unwrap_or_else(|| "rename failed".to_string()))
}

#[cfg(windows)]
fn write_update_error(target_dir: &Path, message: &str) {
    let log_dir = target_dir.join("data").join("logs");
    if std::fs::create_dir_all(&log_dir).is_ok() {
        let _ = std::fs::write(log_dir.join("update-error.log"), message);
    }
}

#[cfg(any(target_os = "macos", test))]
fn staged_macos_bundle(
    stage: &Path,
    flavor: nyaterm_core::app_identity::AppFlavor,
) -> Result<PathBuf, String> {
    let bundle = stage.join(flavor.macos_bundle_name());
    for binary in [
        "NyaTerm",
        "nyaterm-rdp-helper",
        "nyaterm-vnc-helper",
        "nyaterm-mcp",
    ] {
        if !bundle.join("Contents/MacOS").join(binary).is_file() {
            return Err("update bundle is missing an application helper".into());
        }
    }
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::staged_macos_bundle;
    use nyaterm_core::app_identity::AppFlavor;

    #[cfg(windows)]
    use super::{
        PORTABLE_FILES, PORTABLE_ROOT, commit_portable_files_with, extract_windows_portable,
    };
    #[cfg(windows)]
    use std::io::Write as _;
    #[cfg(windows)]
    use std::path::Path;

    #[cfg(windows)]
    fn write_portable_zip(path: &Path, entries: &[(&str, &[u8], Option<u32>)]) {
        let file = std::fs::File::create(path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        for (name, contents, permissions) in entries {
            let mut options = zip::write::SimpleFileOptions::default();
            if let Some(permissions) = permissions {
                options = options.unix_permissions(*permissions);
            }
            archive.start_file(*name, options).unwrap();
            archive.write_all(contents).unwrap();
        }
        archive.finish().unwrap();
    }

    #[cfg(windows)]
    fn portable_entries() -> Vec<(String, Vec<u8>, Option<u32>)> {
        PORTABLE_FILES
            .iter()
            .map(|name| (format!("{PORTABLE_ROOT}/{name}"), b"new".to_vec(), None))
            .collect()
    }

    #[cfg(windows)]
    fn write_portable_files(directory: &Path, contents: &[u8]) {
        std::fs::create_dir_all(directory).unwrap();
        for name in PORTABLE_FILES {
            std::fs::write(directory.join(name), contents).unwrap();
        }
    }

    #[test]
    fn staged_updates_require_the_current_flavor_bundle_and_all_helpers() {
        let stage = nyaterm_core::test_support::TestTempDir::new("nyaterm-update-test");
        for flavor in [AppFlavor::Stable, AppFlavor::Preview] {
            assert!(staged_macos_bundle(&stage, flavor).is_err());
            let bundle = stage.join(flavor.macos_bundle_name());
            let binaries = bundle.join("Contents/MacOS");
            std::fs::create_dir_all(&binaries).unwrap();
            for name in ["NyaTerm", "nyaterm-rdp-helper", "nyaterm-vnc-helper"] {
                std::fs::write(binaries.join(name), b"test").unwrap();
            }
            assert!(staged_macos_bundle(&stage, flavor).is_err());
            std::fs::write(binaries.join("nyaterm-mcp"), b"test").unwrap();
            assert_eq!(staged_macos_bundle(&stage, flavor).unwrap(), bundle);
        }
    }

    #[cfg(windows)]
    #[test]
    fn portable_archive_extracts_only_the_fixed_payload_and_ignores_data() {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-portable-update-extract");
        let archive_path = root.join("portable.zip");
        let destination = root.join("staged");
        std::fs::create_dir_all(&destination).unwrap();
        let mut entries = portable_entries();
        entries.push((
            format!("{PORTABLE_ROOT}/data/settings.json"),
            b"must-not-extract".to_vec(),
            None,
        ));
        let borrowed = entries
            .iter()
            .map(|(name, contents, permissions)| (name.as_str(), contents.as_slice(), *permissions))
            .collect::<Vec<_>>();
        write_portable_zip(&archive_path, &borrowed);

        extract_windows_portable(&archive_path, &destination).unwrap();
        for name in PORTABLE_FILES {
            assert_eq!(std::fs::read(destination.join(name)).unwrap(), b"new");
        }
        assert!(!destination.join("data").exists());
    }

    #[cfg(windows)]
    #[test]
    fn portable_archive_rejects_unsafe_duplicate_missing_and_symlink_entries() {
        for (label, mutate) in [
            ("unsafe", 0_u8),
            ("duplicate", 1_u8),
            ("missing", 2_u8),
            ("symlink", 3_u8),
        ] {
            let root = nyaterm_core::test_support::TestTempDir::new(&format!(
                "nyaterm-portable-update-{label}"
            ));
            let archive_path = root.join("portable.zip");
            let destination = root.join("staged");
            std::fs::create_dir_all(&destination).unwrap();
            let mut entries = portable_entries();
            match mutate {
                0 => entries.push((format!("{PORTABLE_ROOT}/../outside"), b"bad".to_vec(), None)),
                1 => entries.push((
                    format!("{PORTABLE_ROOT}/NYATERM.EXE"),
                    b"duplicate".to_vec(),
                    None,
                )),
                2 => {
                    entries.retain(|(name, _, _)| !name.ends_with("VERSION"));
                }
                3 => entries.push((
                    format!("{PORTABLE_ROOT}/link"),
                    b"NyaTerm.exe".to_vec(),
                    Some(0o120777),
                )),
                _ => unreachable!(),
            }
            let borrowed = entries
                .iter()
                .map(|(name, contents, permissions)| {
                    (name.as_str(), contents.as_slice(), *permissions)
                })
                .collect::<Vec<_>>();
            write_portable_zip(&archive_path, &borrowed);
            assert!(
                extract_windows_portable(&archive_path, &destination).is_err(),
                "{label} archive must be rejected"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn portable_commit_rolls_back_every_backup_and_install_failure() {
        for fail_at in 0..(PORTABLE_FILES.len() * 2) {
            let root = nyaterm_core::test_support::TestTempDir::new(&format!(
                "nyaterm-portable-update-rollback-{fail_at}"
            ));
            let staged = root.join("staged");
            let target = root.join("target");
            let work = root.join("work");
            write_portable_files(&staged, b"new");
            write_portable_files(&target, b"old");
            std::fs::create_dir_all(target.join("data")).unwrap();
            std::fs::write(target.join("data/user.db"), b"user-data").unwrap();
            std::fs::create_dir(&work).unwrap();
            let mut call = 0_usize;
            let result = commit_portable_files_with(&staged, &target, &work, |from, to| {
                let current = call;
                call += 1;
                if current == fail_at {
                    return Err(format!("injected failure at {fail_at}"));
                }
                std::fs::rename(from, to).map_err(|error| error.to_string())
            });
            assert!(result.is_err(), "failure {fail_at} must be reported");
            for name in PORTABLE_FILES {
                assert_eq!(
                    std::fs::read(target.join(name)).unwrap(),
                    b"old",
                    "failure {fail_at} did not restore {name}"
                );
            }
            assert_eq!(
                std::fs::read(target.join("data/user.db")).unwrap(),
                b"user-data"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn portable_commit_replaces_all_program_files_and_preserves_data() {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-portable-update-commit");
        let staged = root.join("staged");
        let target = root.join("target");
        let work = root.join("work");
        write_portable_files(&staged, b"new");
        write_portable_files(&target, b"old");
        std::fs::create_dir_all(target.join("data")).unwrap();
        std::fs::write(target.join("data/user.db"), b"user-data").unwrap();
        std::fs::create_dir(&work).unwrap();

        commit_portable_files_with(&staged, &target, &work, |from, to| {
            std::fs::rename(from, to).map_err(|error| error.to_string())
        })
        .unwrap();

        for name in PORTABLE_FILES {
            assert_eq!(std::fs::read(target.join(name)).unwrap(), b"new");
        }
        assert_eq!(
            std::fs::read(target.join("data/user.db")).unwrap(),
            b"user-data"
        );
        assert!(!work.join("portable-backup").exists());
    }
}
