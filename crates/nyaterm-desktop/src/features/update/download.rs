use crate::features::NyaTermApp;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use gpui::{Context, Window};
use nyaterm_core::updater::{
    UpdateManifest, UpdatePackageKind, UpdateTarget, parse_current_version,
};
use nyaterm_transport::connection_attempt::ConnectionAttempt;
use std::io::{Read as _, Write as _};
use std::path::Path;

const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDgyQUYxQTA2NTYyQTNEOTkKUldTWlBTcFdCaHF2Z29pS0pEdE13U3ZUMVZVTlpGVmQ0YlU2cWlORkdNWU1BY005MU01YjFiU2IK";

#[derive(Clone, Debug)]
pub(crate) enum DownloadState {
    Idle,
    Downloading { received: u64, total: Option<u64> },
    Ready(super::install::PreparedUpdate),
    Failed(String),
}

pub(in crate::features) fn supports_native_install(portable: bool) -> bool {
    if cfg!(debug_assertions) {
        return false;
    }
    let Ok(executable) = std::env::current_exe() else {
        return false;
    };
    #[cfg(windows)]
    {
        let Some(directory) = executable.parent() else {
            return false;
        };
        return if portable {
            directory.join("nyaterm-portable").is_file()
        } else {
            directory.join("Uninstall.exe").is_file()
        };
    }
    #[cfg(target_os = "macos")]
    {
        if portable {
            return false;
        }
        let Some(bundle) = executable.ancestors().nth(3) else {
            return false;
        };
        return bundle.extension().and_then(|value| value.to_str()) == Some("app")
            && !bundle
                .components()
                .any(|part| part.as_os_str() == "Caskroom" || part.as_os_str() == "Cellar");
    }
    #[cfg(target_os = "linux")]
    {
        let _ = executable;
        return !portable && std::env::var_os("APPIMAGE").is_some();
    }
    #[allow(unreachable_code)]
    false
}

fn decode_update_signature(value: &str) -> Result<minisign_verify::Signature, String> {
    let signature = STANDARD
        .decode(value)
        .map_err(|_| "invalid update signature encoding")?;
    minisign_verify::Signature::decode(
        std::str::from_utf8(&signature).map_err(|_| "invalid update signature")?,
    )
    .map_err(|_| "invalid update signature".to_string())
}

fn download_signed_update(
    version: &str,
    directory: &Path,
    portable: bool,
    cancel: &ConnectionAttempt,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<super::install::PreparedUpdate, String> {
    let version = parse_current_version(version).map_err(|error| error.to_string())?;
    if !nyaterm_core::app_identity::AppFlavor::current().accepts_update(&version) {
        return Err("update belongs to a different application flavor".into());
    }
    let target = UpdateTarget::from_rust_target(std::env::consts::OS, std::env::consts::ARCH)
        .map_err(|error| error.to_string())?;
    let package = if portable {
        UpdatePackageKind::WindowsPortable
    } else {
        UpdatePackageKind::Installed
    };
    let client = zed_reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|error| error.to_string())?;
    let manifest_url = format!("https://downloads.nyaterm.app/releases/v{version}/latest.json");
    let mut manifest_body = String::new();
    client
        .get(&manifest_url)
        .send()
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .take(1024 * 1024 + 1)
        .read_to_string(&mut manifest_body)
        .map_err(|error| error.to_string())?;
    if manifest_body.len() > 1024 * 1024 {
        return Err("update manifest too large".into());
    }
    let manifest = UpdateManifest::parse_for_version(&manifest_body, &version)
        .map_err(|error| error.to_string())?;
    let selected = manifest
        .select_artifact(&version, target, package)
        .map_err(|error| error.to_string())?;
    let signature = decode_update_signature(&selected.signature)?;
    let public_key = STANDARD
        .decode(PUBLIC_KEY)
        .map_err(|_| "invalid public key")?;
    let public_key = minisign_verify::PublicKey::decode(
        std::str::from_utf8(&public_key).map_err(|_| "invalid public key")?,
    )
    .map_err(|_| "invalid public key")?;
    let mut verifier = public_key
        .verify_stream(&signature)
        .map_err(|_| "unsupported update signature")?;
    cancel.check()?;
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let name = selected.filename;
    let partial = directory.join(format!("{name}.partial"));
    let artifact = directory.join(&name);
    let result = (|| {
        let mut response = None;
        let mut last_error = None;
        for url in std::iter::once(selected.url.as_str()).chain(selected.fallback_url.as_deref()) {
            match client
                .get(url)
                .send()
                .and_then(|response| response.error_for_status())
            {
                Ok(candidate) => {
                    response = Some(candidate);
                    break;
                }
                Err(error) => last_error = Some(error.to_string()),
            }
        }
        let mut response = response.ok_or_else(|| {
            last_error.unwrap_or_else(|| "no update download URL is available".to_string())
        })?;
        let total = response.content_length();
        if total.is_some_and(|total| total > 1024 * 1024 * 1024) {
            return Err("update too large".into());
        }
        let mut output = std::fs::File::create(&partial).map_err(|error| error.to_string())?;
        let mut buffer = vec![0; 256 * 1024];
        let mut received = 0;
        loop {
            cancel.check()?;
            let count = response
                .read(&mut buffer)
                .map_err(|error| error.to_string())?;
            if count == 0 {
                break;
            }
            received += count as u64;
            if received > 1024 * 1024 * 1024 {
                return Err("update too large".into());
            }
            verifier.update(&buffer[..count]);
            output
                .write_all(&buffer[..count])
                .map_err(|error| error.to_string())?;
            progress(received, total);
        }
        if total.is_some_and(|total| received != total) {
            return Err("update download truncated".into());
        }
        verifier
            .finalize()
            .map_err(|_| "update signature verification failed")?;
        output.sync_all().map_err(|error| error.to_string())?;
        drop(output);
        cancel.check()?;
        std::fs::rename(&partial, &artifact).map_err(|error| error.to_string())?;
        let target = std::env::current_exe().map_err(|error| error.to_string())?;
        super::install::prepare_artifact(artifact, target, portable)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(partial);
    }
    result
}

impl NyaTermApp {
    pub(in crate::features) fn start_native_update_download(&mut self, cx: &mut Context<Self>) {
        if !supports_native_install(self.runtime.mode() == nyaterm_core::RuntimeMode::Portable) {
            return;
        }
        let cancel = ConnectionAttempt::default();
        let Some((info, generation, event_tx)) = self.update.update(cx, |update, cx| {
            let request = update.begin_download(cancel.clone());
            if request.is_some() {
                cx.notify();
            }
            request
        }) else {
            return;
        };
        let directory = self
            .runtime
            .cache_dir()
            .join("updates")
            .join(nyaterm_core::uuid());
        let portable = self.runtime.mode() == nyaterm_core::RuntimeMode::Portable;
        let progress_tx = event_tx.clone();
        let result_tx = event_tx.clone();
        let blocking_jobs = self.update.read(cx).blocking_jobs();
        let scheduled = blocking_jobs.submit_detached("update-download", move |_| {
            let result = download_signed_update(
                &info.latest_version,
                &directory,
                portable,
                &cancel,
                |received, total| {
                    let _ = progress_tx.unbounded_send(super::UpdateEvent::Download {
                        generation,
                        state: DownloadState::Downloading { received, total },
                    });
                },
            );
            let state = match result {
                Ok(prepared) => DownloadState::Ready(prepared),
                Err(error) => DownloadState::Failed(error),
            };
            let _ = result_tx.unbounded_send(super::UpdateEvent::Download { generation, state });
        });
        if let Err(error) = scheduled {
            let _ = event_tx.unbounded_send(super::UpdateEvent::Download {
                generation,
                state: DownloadState::Failed(format!("could not start update download: {error}")),
            });
        }
        cx.notify();
    }

    pub(in crate::features) fn cancel_native_update_download(&mut self, cx: &mut Context<Self>) {
        self.update.update(cx, |update, cx| {
            update.cancel_download();
            cx.notify();
        });
        cx.notify();
    }

    pub(in crate::features) fn prepare_native_update_exit(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let update = self.update.read(cx);
        if !update.install_requested {
            return true;
        }
        let DownloadState::Ready(_) = update.download else {
            return false;
        };
        true
    }

    pub(crate) fn launch_pending_update_after_shutdown(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let prepared = self.update.update(cx, |update, _| {
            if !update.install_requested {
                return Ok(None);
            }
            let DownloadState::Ready(prepared) = update.download.clone() else {
                return Err("the downloaded update is no longer available".to_string());
            };
            update.clear_install_request();
            Ok(Some(prepared))
        })?;
        let Some(prepared) = prepared else {
            return Ok(());
        };
        super::install::launch_installer(&prepared)?;
        self.update.update(cx, |update, cx| {
            update.mark_applying();
            cx.notify();
        });
        Ok(())
    }

    pub(in crate::features) fn request_native_update_install(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.update.read(cx).download, DownloadState::Ready(_)) {
            return;
        }
        if self.notes.has_open_editor_windows()
            || self
                .transfer
                .editor_workspace()
                .is_some_and(|workspace| workspace.tabs.iter().any(|tab| tab.dirty || tab.saving))
        {
            self.notify_operation(
                "update-unsaved",
                nyaterm_ui::notification::NyaNotificationKind::Warning,
                rust_i18n::t!("updater.saveEditorsFirst").to_string(),
                cx,
            );
            return;
        }
        self.update.update(cx, |update, cx| {
            update.install_requested = true;
            cx.notify();
        });
        self.close_update_dialog(window, cx);
        if let Some(controller) = self.desktop_controller.clone() {
            let _ = controller.update(cx, |controller, cx| controller.request_quit(cx));
        } else {
            self.handle_window_close_request(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PUBLIC_KEY, decode_update_signature, supports_native_install};
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    #[test]
    fn published_signing_key_loads_and_debug_builds_do_not_self_update() {
        let key = STANDARD.decode(PUBLIC_KEY).unwrap();
        assert!(minisign_verify::PublicKey::decode(std::str::from_utf8(&key).unwrap()).is_ok());
        if cfg!(debug_assertions) {
            assert!(!supports_native_install(true));
            assert!(!supports_native_install(false));
        }
    }

    #[test]
    fn malformed_update_signatures_are_rejected_before_download() {
        assert!(decode_update_signature("not-base64").is_err());
        assert!(decode_update_signature(&STANDARD.encode("not a minisign signature")).is_err());
    }
}
