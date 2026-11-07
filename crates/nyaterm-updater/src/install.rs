#[cfg(windows)]
use std::ffi::OsString;
#[cfg(any(windows, target_os = "macos", test))]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[cfg(any(windows, test))]
mod portable;
#[cfg(windows)]
use nyaterm_core::packaging::portable::windows_portable_layout;
#[cfg(windows)]
use nyaterm_core::updater::UpdateTarget;
#[cfg(windows)]
use portable::{PortableUpdateError, commit_portable_files_with, extract_windows_portable};

/// Installation stages reported to the caller's native progress adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateStage {
    Waiting,
    BackingUp,
    Installing,
    Replacing,
    Starting,
}

/// Helper work executed by the caller, which owns progress UI and panic handling.
pub type HelperOperation =
    Box<dyn FnOnce(&mut dyn FnMut(UpdateStage)) -> Result<(), String> + Send + 'static>;

const PORTABLE_HELPER_FLAG: &str = "--nyaterm-portable-update-helper";
const INSTALLED_HELPER_FLAG: &str = "--nyaterm-installed-update-helper";
const UPDATE_CLEANUP_ENV: &str = "NYATERM_UPDATE_CLEANUP";
#[derive(Clone, Debug)]
pub enum PreparedUpdate {
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

#[cfg(windows)]
fn is_valid_installed_executable(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("NyaTerm.exe"))
}

pub fn prepare_artifact(
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
        if !is_valid_installed_executable(&executable) {
            return Err(format!(
                "unexpected installed executable name: {}",
                executable.display()
            ));
        }

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
    if !target_dir.join(&windows_portable_layout().marker).is_file() {
        return Err("portable installation marker is missing".into());
    }
    let work_dir = artifact
        .parent()
        .ok_or("update directory unavailable")?
        .to_path_buf();
    let staged_dir = work_dir.join("portable-payload");
    std::fs::create_dir(&staged_dir).map_err(|error| error.to_string())?;
    let architecture = UpdateTarget::from_rust_target("windows", std::env::consts::ARCH)
        .map_err(|error| error.to_string())?
        .architecture;
    let result = extract_windows_portable(&artifact, &staged_dir, architecture);
    if let Err(error) = result {
        let _ = std::fs::remove_dir_all(&staged_dir);
        return Err(error);
    }
    let helper = work_dir.join("NyaTerm-update-helper.exe");
    std::fs::copy(&executable, &helper)
        .map_err(|error| format!("failed to prepare portable update helper: {error}"))?;
    Ok(PreparedUpdate::WindowsPortable {
        helper,
        staged_dir,
        target_dir,
        work_dir,
    })
}

/// Start a detached installer only after the existing application close guards pass.
/// The helper waits for this process before touching the installed application.
pub fn launch_installer(prepared: &PreparedUpdate, language: &str) -> Result<(), String> {
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
                language.into(),
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
                language.into(),
            ],
        );
    }
    #[cfg(unix)]
    {
        let _ = language;
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
        .map_err(|error| format!("failed to launch update helper: {error}"))
}

pub fn run_update_helper_if_requested(
    run_progress: impl FnOnce(&str, HelperOperation) -> Result<(), String>,
) -> bool {
    #[cfg(not(windows))]
    let _ = run_progress;
    let args = std::env::args_os().collect::<Vec<_>>();
    let Some(flag) = args.get(1).and_then(|value| value.to_str()) else {
        return false;
    };
    if flag != PORTABLE_HELPER_FLAG && flag != INSTALLED_HELPER_FLAG {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::System::Threading::WaitForInputIdle;

        let portable = flag == PORTABLE_HELPER_FLAG;
        let target = args.get(4).map(PathBuf::from).map(|path| {
            if portable {
                path.join("NyaTerm.exe")
            } else {
                path
            }
        });
        let work_dir = args.get(5).map(PathBuf::from);
        let recovery_work_dir = work_dir.clone();
        let language = args
            .get(6)
            .and_then(|value| value.to_str())
            .unwrap_or("en")
            .to_string();
        let launch_target = target.clone();
        let recovery_required = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_recovery = recovery_required.clone();
        let result = run_progress(
            &language,
            Box::new(move |report| {
                if portable {
                    run_windows_portable_helper(&args, report).map_err(|error| {
                        worker_recovery.store(
                            error.recovery_required,
                            std::sync::atomic::Ordering::Release,
                        );
                        error.message
                    })?;
                } else {
                    run_windows_installed_helper(&args, report)?;
                }
                report(UpdateStage::Starting);
                let target = launch_target.ok_or("updated application path is unavailable")?;
                let mut command = Command::new(target);
                if let Some(work_dir) = work_dir {
                    command.env(UPDATE_CLEANUP_ENV, work_dir);
                }
                let child = command
                    .spawn()
                    .map_err(|error| format!("failed to restart NyaTerm: {error}"))?;
                // Keep the helper's progress window visible while the new GUI initializes.
                unsafe { WaitForInputIdle(child.as_raw_handle(), 30_000) };
                Ok(())
            }),
        );
        if let Err(error) = &result
            && let Some(target) = target
        {
            if let Some(target_dir) = target.parent() {
                write_update_error(target_dir, error);
            }
            if target.is_file()
                && !recovery_required.load(std::sync::atomic::Ordering::Acquire)
                && (!portable
                    || !recovery_work_dir
                        .as_deref()
                        .is_some_and(portable::recovery_pending))
            {
                let mut command = Command::new(target);
                // Portable failures retain diagnostics; incomplete rollback must never clean backups.
                if let Some(work_dir) = recovery_work_dir.filter(|_| !portable) {
                    command.env(UPDATE_CLEANUP_ENV, work_dir);
                }
                let _ = command.spawn();
            }
        }
    }
    true
}

pub fn take_update_cleanup_path() -> Option<PathBuf> {
    let path = std::env::var_os(UPDATE_CLEANUP_ENV).map(PathBuf::from)?;
    unsafe { std::env::remove_var(UPDATE_CLEANUP_ENV) };
    Some(path)
}

pub fn cleanup_update_work_dir(path: PathBuf) {
    #[cfg(any(windows, test))]
    if portable::recovery_pending(&path) {
        return;
    }
    let _ = std::fs::remove_dir_all(path);
}

#[cfg(windows)]
fn run_windows_portable_helper(
    args: &[OsString],
    report: &mut dyn FnMut(UpdateStage),
) -> Result<(), PortableUpdateError> {
    if args.len() != 7 {
        return Err("invalid portable update helper arguments".into());
    }
    let parent = args[2]
        .to_str()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or("invalid updater parent process ID")?;
    let staged_dir = PathBuf::from(&args[3]);
    let target_dir = PathBuf::from(&args[4]);
    let work_dir = PathBuf::from(&args[5]);
    if !target_dir.join(&windows_portable_layout().marker).is_file()
        || staged_dir.parent() != Some(work_dir.as_path())
    {
        return Err("portable update paths failed validation".into());
    }
    report(UpdateStage::Waiting);
    wait_for_process_exit(parent)?;
    report(UpdateStage::Replacing);
    let architecture = UpdateTarget::from_rust_target("windows", std::env::consts::ARCH)
        .map_err(|error| error.to_string())?
        .architecture;
    commit_portable_files_with(
        &staged_dir,
        &target_dir,
        &work_dir,
        architecture,
        rename_with_retry,
    )
}

#[cfg(windows)]
fn run_windows_installed_helper(
    args: &[OsString],
    report: &mut dyn FnMut(UpdateStage),
) -> Result<(), String> {
    use std::os::windows::process::CommandExt as _;

    if args.len() != 7 {
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
    if !is_valid_installed_executable(&target) || !target_dir.join("Uninstall.exe").is_file() {
        return Err("installed update target failed validation".into());
    }
    report(UpdateStage::Waiting);
    wait_for_process_exit(parent)?;

    report(UpdateStage::BackingUp);
    let backup = target_dir.with_extension(format!("previous-{}", nyaterm_core::uuid()));
    copy_directory(target_dir, &backup)?;
    report(UpdateStage::Installing);
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
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
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
    let wait_error = (result == WAIT_FAILED).then(std::io::Error::last_os_error);
    unsafe { CloseHandle(process) };
    match result {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT => Err("timed out waiting for NyaTerm to exit".into()),
        _ => Err(format!(
            "failed to wait for NyaTerm to exit ({result}): {wait_error:?}"
        )),
    }
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
    #[test]
    fn windows_helper_launch_reports_spawn_failure_with_system_error() {
        use super::launch_windows_helper;

        let root =
            nyaterm_core::test_support::TestTempDir::new("nyaterm-update-helper-spawn-error");
        std::fs::create_dir_all(&root).unwrap();
        let error = launch_windows_helper(&root.join("missing.exe"), "/C", []).unwrap_err();
        assert!(error.contains("failed to launch update helper"));
        assert!(error.contains("os error 2"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_helper_launch_adapter_starts_process_without_console() {
        use super::launch_windows_helper;
        use std::ffi::OsString;

        let cmd = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/cmd.exe");
        launch_windows_helper(&cmd, "/C", [OsString::from("exit"), OsString::from("0")]).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_updater_waits_for_parent_process_exit() {
        use super::wait_for_process_exit;
        use std::os::windows::process::CommandExt as _;
        use std::process::{Command, Stdio};

        let mut child = Command::new("cmd.exe")
            .args(["/D", "/C", "exit", "0"])
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait_for_process_exit(child.id()).unwrap();
        assert!(child.wait().unwrap().success());
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
    fn installed_executable_name_is_case_insensitive() {
        use super::is_valid_installed_executable;
        use std::path::Path;

        for name in ["NyaTerm.exe", "nyaterm.exe", "NYATERM.EXE", "NyAtErM.ExE"] {
            assert!(is_valid_installed_executable(Path::new(name)));
        }

        for name in [
            "NyaTerm-old.exe",
            "NyaTerm.exe.bak",
            "Uninstall.exe",
            "other.exe",
        ] {
            assert!(!is_valid_installed_executable(Path::new(name)));
        }
    }
}
