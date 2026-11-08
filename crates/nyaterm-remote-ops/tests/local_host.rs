#![cfg(unix)]
//! The same backends that serve SSH hosts must work against the local machine.

use nyaterm_remote_ops::host::HostTarget;
use nyaterm_remote_ops::remote_process::SshProcessService;
use nyaterm_remote_ops::stats::{RemoteStatsSampler, RemoteStatsService};

#[test]
fn local_process_list_contains_this_test_process() {
    let service = SshProcessService::with_target(HostTarget::Local);
    let processes = service.list_processes().expect("local ps");
    let me = std::process::id();
    let found = processes
        .iter()
        .find(|p| p.pid == me)
        .expect("own pid listed");
    assert!(!found.command.is_empty());
    assert!(processes.len() > 5);
}

#[test]
fn local_stats_report_cpu_and_memory() {
    let service = RemoteStatsService::with_target(HostTarget::Local);
    let sampler = RemoteStatsSampler::default();
    let stats = service.snapshot(&sampler, "local").expect("local stats");
    println!("{stats:#?}");
}
