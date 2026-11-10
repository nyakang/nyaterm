//! Device acceptance in a unique directory; prints no device filenames or contents.
use nyaterm_android::adb::client::AdbClient;
use nyaterm_core::models::android::AdbDeviceSelector;
use nyaterm_files::adb::AdbFileService;
use nyaterm_files::sftp::{RemoteFilePath, SftpTransferControl};
fn main() -> anyhow::Result<()> {
    let serial = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("host:port required"))?;
    let (host, port) = serial
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("host:port required"))?;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug");
    let client = AdbClient::local(Some(&root.join("adb/adb")))?;
    let files = AdbFileService::new(client.bind(
        &AdbDeviceSelector::Tcp {
            host: host.into(),
            port: port.parse()?,
        },
        1,
    )?);
    let directory = RemoteFilePath::new(format!("/sdcard/nyaterm-test-{}", uuid::Uuid::new_v4()));
    files.create_dir_remote_path(&directory, None)?;
    let result = (|| {
        let local = tempfile::tempdir()?;
        let input = local.path().join("input");
        let data: Vec<u8> = (0..1024 * 1024).map(|n| (n * 31 + 7) as u8).collect();
        std::fs::write(&input, &data)?;
        let names = [
            "space name",
            "中文",
            "quote'\"",
            "line\nbreak",
            "$(touch unexpected)",
            "a\\b",
        ];
        for (number, name) in names.into_iter().enumerate() {
            let path = files.child_path(&directory, name)?;
            files.upload(&input, &path, false, &SftpTransferControl::new(), |_| {})?;
            let output = local.path().join(format!("output-{number}"));
            files.download(&path, &output, false, &SftpTransferControl::new(), |_| {})?;
            anyhow::ensure!(
                std::fs::read(&output)? == data,
                "device round trip mismatch"
            );
            anyhow::ensure!(
                files
                    .upload(&input, &path, false, &SftpTransferControl::new(), |_| {})
                    .is_err(),
                "exclusive commit replaced target"
            );
            let renamed = files.child_path(&directory, &format!("renamed-{number}"))?;
            files.rename_remote_paths_no_replace(&path, &renamed)?;
            files.delete_remote_path(&renamed)?;
        }
        let target = files.child_path(&directory, "cancel-target")?;
        files.upload(&input, &target, false, &SftpTransferControl::new(), |_| {})?;
        let control = SftpTransferControl::new();
        let cancel = control.clone();
        anyhow::ensure!(
            files
                .upload(&input, &target, true, &control, move |_| cancel.cancel())
                .is_err(),
            "canceled upload committed"
        );
        let output = local.path().join("cancel-output");
        files.download(&target, &output, false, &SftpTransferControl::new(), |_| {})?;
        anyhow::ensure!(
            std::fs::read(&output)? == data,
            "canceled upload damaged destination"
        );
        anyhow::ensure!(
            files.list_dir_path(&directory)?.len() == 1,
            "temporary files leaked"
        );
        files.delete_remote_path(&target)?;
        anyhow::ensure!(
            files.list_dir_path(&directory)?.is_empty(),
            "device directory not empty"
        );
        println!(
            "special names, verified upload/download, exclusive commit and cancellation passed"
        );
        Ok(())
    })();
    let cleanup = files.delete_remote_path(&directory);
    result.and(cleanup)
}
