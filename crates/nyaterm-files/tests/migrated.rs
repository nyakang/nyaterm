use nyaterm_files::sftp::SftpService;
use nyaterm_transport::{SftpSettings, SshSessionConfig, SshSessionProfile};

#[test]
fn sftp_service_rejects_operations_when_disabled() {
    let service = SftpService::new(SshSessionConfig {
        sftp: SftpSettings {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    });

    let error = service.list_dir("/").expect_err("SFTP disabled");

    assert!(error.to_string().contains("SFTP is disabled"));
}

#[test]
fn sftp_service_rejects_network_device_profile_even_when_saved_enabled() {
    let service = SftpService::new(SshSessionConfig {
        profile: SshSessionProfile::NetworkDevice,
        sftp: SftpSettings {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    });

    let error = service
        .list_dir("/")
        .expect_err("network device SFTP must be disabled before connecting");

    assert!(error.to_string().contains("SFTP is disabled"));
}
