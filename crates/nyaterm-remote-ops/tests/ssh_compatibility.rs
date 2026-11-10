use nyaterm_remote_ops::ascend_npu::RemoteNpuService;
use nyaterm_remote_ops::docker::DockerService;
use nyaterm_remote_ops::gpu::RemoteGpuService;
use nyaterm_remote_ops::remote_process::SshProcessService;
use nyaterm_remote_ops::stats::{RemoteStatsSampler, RemoteStatsService};
use nyaterm_transport::{
    SshHostKey, SshHostKeyDecision, SshHostKeyVerifier, SshMultiplexHandle,
    SshResourceProbesDisabled, SshSessionConfig, open_ssh_multiplex_handle,
};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::{ChannelId, server};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

fn blocked<T: std::fmt::Debug>(result: anyhow::Result<T>) {
    assert!(result.unwrap_err().is::<SshResourceProbesDisabled>());
}

fn check_blocked_services(config: SshSessionConfig, multiplex: Option<SshMultiplexHandle>) {
    let target = nyaterm_remote_ops::host::HostTarget::Ssh {
        config: config.clone(),
        multiplex: multiplex.clone(),
    };
    let os = target.os_hint();
    let entry = nyaterm_remote_ops::services::ServiceEntry {
        name: "fixture.service".into(),
        description: String::new(),
        state: nyaterm_remote_ops::services::ServiceState::Running,
        detail: String::new(),
        startup: nyaterm_remote_ops::services::ServiceStartup::Enabled,
        pid: Some(42),
        scope: "system".into(),
    };
    blocked(nyaterm_remote_ops::ports::list_ports(
        &target,
        os,
        Default::default(),
    ));
    blocked(nyaterm_remote_ops::ports::process_details(
        &target,
        os,
        42,
        Default::default(),
    ));
    blocked(nyaterm_remote_ops::services::list_services(
        &target,
        os,
        Default::default(),
    ));
    blocked(nyaterm_remote_ops::services::run_action(
        &target,
        os,
        nyaterm_remote_ops::services::ServiceBackend::Systemd,
        &entry,
        nyaterm_remote_ops::services::ServiceAction::Stop,
        Default::default(),
    ));
    blocked(nyaterm_remote_ops::services::fetch_logs(
        &target,
        os,
        nyaterm_remote_ops::services::ServiceBackend::Systemd,
        &entry,
        10,
        Default::default(),
    ));
    blocked(nyaterm_remote_ops::host::signal_pid(
        &target,
        os,
        42,
        true,
        Default::default(),
    ));
    let (stats, processes, gpu, npu, docker) = if let Some(multiplex) = multiplex {
        (
            RemoteStatsService::with_multiplex(config.clone(), multiplex.clone()).unwrap(),
            SshProcessService::with_multiplex(config.clone(), multiplex.clone()).unwrap(),
            RemoteGpuService::with_multiplex(config.clone(), multiplex.clone()).unwrap(),
            RemoteNpuService::with_multiplex(config.clone(), multiplex.clone()).unwrap(),
            DockerService::with_multiplex(config, multiplex, Default::default()).unwrap(),
        )
    } else {
        (
            RemoteStatsService::new(config.clone()),
            SshProcessService::new(config.clone()),
            RemoteGpuService::new(config.clone()),
            RemoteNpuService::new(config.clone()),
            DockerService::new(config),
        )
    };
    blocked(stats.snapshot(&RemoteStatsSampler::default(), "compatible"));
    blocked(processes.list_processes());
    blocked(processes.signal_process(42, "TERM"));
    blocked(processes.renice_process(42, 0));
    blocked(gpu.overview());
    blocked(npu.overview());
    blocked(docker.overview());
    blocked(docker.images());
    blocked(docker.networks());
    blocked(docker.volumes());
    blocked(docker.compose_projects());
    blocked(docker.container_details("container"));
    blocked(docker.container_stats("container"));
    blocked(docker.container_action("container", "stop"));
    blocked(docker.container_logs("container", 10));
    blocked(docker.compose_services("project", None));
}

#[test]
fn ssh_compatibility_returns_before_the_generic_ssh_executor_checks_shell_availability() {
    // Reaching run_ssh_command would return ShellUnavailable instead of this error.
    let config = SshSessionConfig {
        ssh_compatibility_mode: true,
        runtime_mode: nyaterm_core::models::ssh_runtime::SshRuntimeMode::Sftp,
        ..Default::default()
    };
    check_blocked_services(config, None);
}

struct Peer {
    execs: Arc<AtomicUsize>,
    channels: Vec<russh::Channel<server::Msg>>,
}

impl server::Handler for Peer {
    type Error = russh::Error;

    async fn auth_none(&mut self, _: &str) -> Result<server::Auth, Self::Error> {
        Ok(server::Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.channels.push(channel);
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        id: ChannelId,
        _: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.execs.fetch_add(1, Ordering::SeqCst);
        session.channel_success(id)?;
        session.exit_status_request(id, 0)?;
        session.eof(id)?;
        session.close(id)?;
        Ok(())
    }
}

struct Verify(Arc<AtomicUsize>);

impl SshHostKeyVerifier for Verify {
    fn verify(&self, _: &SshHostKey) -> Result<SshHostKeyDecision, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(SshHostKeyDecision::Accept)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_compatibility_shared_connection_blocks_all_resource_execs_but_allows_explicit_commands()
 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let execs = Arc::new(AtomicUsize::new(0));
    let verifications = Arc::new(AtomicUsize::new(0));
    let peer_execs = execs.clone();
    let server = tokio::spawn(async move {
        let key = russh::keys::PrivateKey::new(
            KeypairData::Ed25519(Ed25519Keypair::from_seed(&[17; 32])),
            "compatibility-test",
        )
        .unwrap();
        let config = Arc::new(server::Config {
            keys: vec![key],
            auth_rejection_time: Duration::ZERO,
            ..Default::default()
        });
        let (socket, _) = listener.accept().await.unwrap();
        let _ = server::run_stream(
            config,
            socket,
            Peer {
                execs: peer_execs,
                channels: Vec::new(),
            },
        )
        .await
        .unwrap()
        .await;
    });
    tokio::time::timeout(
        Duration::from_secs(20),
        tokio::task::spawn_blocking(move || {
            let mut config = SshSessionConfig {
                host: "127.0.0.1".into(),
                port,
                username: "test".into(),
                allow_none_auth: true,
                host_key_verifier: Some(Arc::new(Verify(verifications.clone()))),
                ssh_compatibility_mode: true,
                ..Default::default()
            };
            let multiplex = open_ssh_multiplex_handle(config.clone()).unwrap();
            assert_eq!(verifications.load(Ordering::SeqCst), 1);
            check_blocked_services(config.clone(), None);
            check_blocked_services(config.clone(), Some(multiplex.clone()));
            assert_eq!(execs.load(Ordering::SeqCst), 0);
            SshProcessService::with_multiplex(config.clone(), multiplex.clone())
                .unwrap()
                .run_command("true", Duration::from_secs(3))
                .unwrap();
            assert_eq!(execs.load(Ordering::SeqCst), 1);
            config.ssh_compatibility_mode = false;
            let before = execs.load(Ordering::SeqCst);
            RemoteStatsService::with_multiplex(config.clone(), multiplex.clone())
                .unwrap()
                .snapshot(&RemoteStatsSampler::default(), "standard")
                .unwrap();
            SshProcessService::with_multiplex(config.clone(), multiplex.clone())
                .unwrap()
                .list_processes()
                .unwrap();
            RemoteGpuService::with_multiplex(config.clone(), multiplex.clone())
                .unwrap()
                .overview()
                .unwrap();
            RemoteNpuService::with_multiplex(config.clone(), multiplex.clone())
                .unwrap()
                .overview()
                .unwrap();
            DockerService::with_multiplex(config, multiplex.clone(), Default::default())
                .unwrap()
                .overview()
                .unwrap();
            assert_eq!(execs.load(Ordering::SeqCst), before + 7);
            multiplex.disconnect().unwrap();
        }),
    )
    .await
    .unwrap()
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}
