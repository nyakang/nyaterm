use nyaterm_mcp_host::discovery::{DiscoveryPermissions, DiscoveryStore};
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct NativePermissions;
impl DiscoveryPermissions for NativePermissions {
    fn set_private(&self, path: &Path, directory: bool) -> anyhow::Result<()> {
        if directory {
            set_private_directory_permissions(path)
        } else {
            set_private_file_permissions(path)
        }
    }
}

pub(super) fn discovery_store(config_dir: &Path) -> DiscoveryStore {
    DiscoveryStore::new(config_dir, Arc::new(NativePermissions))
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(windows)]
fn set_private_directory_permissions(path: &Path) -> anyhow::Result<()> {
    set_windows_current_user_acl(path, true)
}

#[cfg(windows)]
fn set_private_file_permissions(path: &Path) -> anyhow::Result<()> {
    set_windows_current_user_acl(path, false)
}

#[cfg(windows)]
fn set_windows_current_user_acl(path: &Path, directory: bool) -> anyhow::Result<()> {
    crate::platform::windows_acl::set_current_user_only(path, directory)
}

#[cfg(not(any(unix, windows)))]
fn set_private_directory_permissions(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn set_private_file_permissions(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

pub(super) fn default_config_dir() -> anyhow::Result<PathBuf> {
    let executable = std::env::current_exe()?;
    default_config_dir_from(&executable, dirs::home_dir().as_deref())
}

fn default_config_dir_from(executable: &Path, home: Option<&Path>) -> anyhow::Result<PathBuf> {
    let directory = executable
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot resolve NyaTerm executable directory"))?;
    if ["nyaterm-portable", "portable.flag"]
        .into_iter()
        .any(|marker| directory.join(marker).is_file())
    {
        return Ok(directory.join("data").join("config"));
    }
    Ok(nyaterm_core::runtime::AppRuntime::installed_config_dir(
        home.ok_or_else(|| anyhow::anyhow!("cannot resolve user home directory"))?,
        nyaterm_core::app_identity::AppFlavor::current(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{default_config_dir_from, discovery_store};
    #[cfg(windows)]
    use nyaterm_mcp_protocol::DiscoveryDocument;

    #[test]
    fn installed_discovery_uses_the_current_application_flavor() {
        let home = std::path::Path::new("test-home");
        let executable = std::path::Path::new("test-bin/nyaterm");
        assert_eq!(
            default_config_dir_from(executable, Some(home)).unwrap(),
            nyaterm_core::runtime::AppRuntime::installed_config_dir(
                home,
                nyaterm_core::app_identity::AppFlavor::current()
            )
        );
    }

    #[test]
    fn stale_discovery_can_be_removed_repeatedly() {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-mcp-test");
        let store = discovery_store(&root);
        store.remove().unwrap();
        store.remove().unwrap();
    }

    #[test]
    fn both_portable_markers_resolve_discovery_under_executable_data() {
        for marker in ["nyaterm-portable", "portable.flag"] {
            let root = nyaterm_core::test_support::TestTempDir::new(&format!(
                "nyaterm-mcp-portable-{}",
                marker.replace('.', "-")
            ));
            std::fs::create_dir_all(root.path()).unwrap();
            std::fs::write(root.join(marker), b"").unwrap();
            let executable = root.join(if cfg!(windows) {
                "nyaterm.exe"
            } else {
                "nyaterm"
            });
            assert_eq!(
                default_config_dir_from(&executable, None).unwrap(),
                root.join("data").join("config")
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn discovery_replacement_preserves_private_acl_and_removes_temporary_files() {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-mcp-discovery-test");
        let store = discovery_store(&root);
        let first = discovery_document("first-token", "first-generation");
        let second = discovery_document("second-token", "second-generation");

        store.write(&first).unwrap();
        store.write(&second).unwrap();

        let actual: DiscoveryDocument =
            serde_json::from_slice(&std::fs::read(root.join("mcp/discovery.json")).unwrap())
                .unwrap();
        assert_eq!(actual.token, second.token);
        assert_eq!(actual.generation, second.generation);
        crate::platform::windows_acl::assert_current_user_only(&root.join("mcp"), true);
        crate::platform::windows_acl::assert_current_user_only(
            &root.join("mcp/discovery.json"),
            false,
        );

        let temporary_files = std::fs::read_dir(root.join("mcp"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with(".discovery-") && name.ends_with(".tmp")
            })
            .count();
        assert_eq!(temporary_files, 0);
    }

    #[cfg(windows)]
    fn discovery_document(token: &str, generation: &str) -> DiscoveryDocument {
        DiscoveryDocument {
            version: 1,
            pid: std::process::id(),
            host: "127.0.0.1".into(),
            port: 47_123,
            token: token.into(),
            generation: generation.into(),
            permission_mode: "read-only".into(),
        }
    }
}
