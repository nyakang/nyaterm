use nyaterm_remote_ops::ascend_npu::RemoteNpuService;
use nyaterm_remote_ops::docker::DockerService;
use nyaterm_remote_ops::gpu::RemoteGpuService;
use nyaterm_remote_ops::remote_process::{
    is_process_list_unsupported, normalize_process_signal, parse_process_output,
};
use nyaterm_remote_ops::stats::RemoteStatsService;
use nyaterm_transport::SshSessionConfig;

#[test]
fn remote_services_keep_dedicated_fallback_constructors() {
    let config = SshSessionConfig {
        name: "fallback".to_string(),
        host: "example.test".to_string(),
        username: "tester".to_string(),
        ..Default::default()
    };

    let stats = format!("{:?}", RemoteStatsService::new(config.clone()));
    let gpu = format!("{:?}", RemoteGpuService::new(config.clone()));
    let npu = format!("{:?}", RemoteNpuService::new(config.clone()));
    let docker = format!("{:?}", DockerService::new(config));

    for debug in [stats, gpu, npu, docker] {
        assert!(debug.contains("multiplex: None"));
    }
}

#[test]
fn process_parser_reads_legacy_rows() {
    let rows = "PROCESS\t42\t1\troot\tSs\t0.4\t1.2\t1234\t5678\t01:02\tsshd\t/usr/sbin/sshd -D\n";

    let processes = parse_process_output(rows);

    assert_eq!(processes.len(), 1);
    assert_eq!(processes[0].pid, 42);
    assert_eq!(processes[0].ppid, 1);
    assert_eq!(processes[0].user, "root");
    assert_eq!(processes[0].cpu_percent, 0.4);
    assert_eq!(processes[0].command_line, "/usr/sbin/sshd -D");
}

#[test]
fn process_parser_preserves_command_lines_containing_tabs() {
    let rows = "PROCESS\t9\t1\troot\tS\t0\t0\t1\t2\t-\tawk\tawk\twith\ttabs\n";

    let processes = parse_process_output(rows);

    assert_eq!(processes.len(), 1);
    assert_eq!(processes[0].command_line, "awk\twith\ttabs");
}

#[test]
fn process_parser_detects_unsupported_marker() {
    assert!(is_process_list_unsupported(
        "warning\nNYATERM_PROCESS_UNSUPPORTED\n"
    ));
    assert!(!is_process_list_unsupported(
        "PROCESS\t1\t0\troot\tS\t0\t0\t0\t0\t-\tsh\tsh\n"
    ));
}

#[test]
fn process_signal_normalization_matches_legacy_allowlist() {
    assert_eq!(normalize_process_signal("sigterm").unwrap(), "TERM");
    assert_eq!(normalize_process_signal("9").unwrap(), "KILL");
    assert_eq!(normalize_process_signal("cont").unwrap(), "CONT");
    assert!(normalize_process_signal("USR1").is_err());
}
