use serde::Serialize;
use std::time::Duration;

use crate::{
    RemoteCommandOutput, SshMultiplexHandle, SshSessionConfig, ensure_remote_command_success,
};

const DOCKER_TIMEOUT: Duration = Duration::from_secs(20);
const DOCKER_LOG_TIMEOUT: Duration = Duration::from_secs(30);
const COMPOSE_PS_JSON_BEGIN: &str = "COMPOSE_PS_JSON_BEGIN";
const COMPOSE_PS_JSON_END: &str = "COMPOSE_PS_JSON_END";

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct DockerContainerStats {
    pub cpu_percent: f64,
    pub memory_percent: f64,
    pub memory_usage: String,
    pub net_io: String,
    pub block_io: String,
    pub pids: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DockerContainer {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    pub state: String,
    pub ports: String,
    pub created_at: String,
    pub size: String,
    pub stats: Option<DockerContainerStats>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DockerImage {
    pub id: String,
    pub repository: String,
    pub tag: String,
    pub size: String,
    pub created_since: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DockerVolume {
    pub driver: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DockerNetwork {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DockerComposeProject {
    pub name: String,
    pub status: String,
    pub config_files: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DockerComposeServiceContainer {
    pub id: String,
    pub name: String,
    pub state: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DockerComposeService {
    pub name: String,
    pub status: String,
    pub containers: Vec<DockerComposeServiceContainer>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DockerContainerMount {
    pub kind: String,
    pub source: String,
    pub destination: String,
    pub mode: String,
    pub rw: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DockerContainerNetwork {
    pub name: String,
    pub ip_address: String,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct DockerContainerDetails {
    pub stats: Option<DockerContainerStats>,
    pub started_at: String,
    pub finished_at: String,
    pub restart_count: u64,
    pub entrypoint: String,
    pub command: String,
    pub mounts: Vec<DockerContainerMount>,
    pub networks: Vec<DockerContainerNetwork>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct RemoteDockerOverview {
    pub available: bool,
    pub version: String,
    pub compose_available: bool,
    pub containers: Vec<DockerContainer>,
    pub images: Vec<DockerImage>,
    pub volumes: Vec<DockerVolume>,
    pub networks: Vec<DockerNetwork>,
    pub compose_projects: Vec<DockerComposeProject>,
}

#[derive(Debug, Clone)]
pub struct DockerService {
    config: SshSessionConfig,
    multiplex: Option<SshMultiplexHandle>,
    elevation: std::sync::Arc<std::sync::Mutex<Option<DockerElevation>>>,
}

#[derive(Clone, Debug)]
pub(crate) enum DockerElevation {
    Plain,
    Sudo,
    Password(nyaterm_core::SecretString),
}

fn docker_environment(command: &str) -> String {
    format!(
        "export PATH=/usr/local/bin:/usr/bin:/bin:/var/packages/ContainerManager/target/usr/bin:/var/packages/Docker/target/usr/bin:$PATH; {command}"
    )
}

pub const DOCKER_OVERVIEW_SCRIPT: &str = r#"sh -c '
if ! command -v docker >/dev/null 2>&1; then
  printf "DOCKER_AVAILABLE\t0\n"
  exit 0
fi

if ! docker info >/dev/null 2>&1; then
  printf "DOCKER_AVAILABLE\t0\n"
  docker info 2>&1 | head -n 4 >&2
  exit 0
fi

printf "DOCKER_AVAILABLE\t1\n"
version=$(docker version --format "{{.Server.Version}}" 2>/dev/null || true)
version=$(printf "%s" "$version" | tr "\t\r\n" "   ")
printf "DOCKER_VERSION\t%s\n" "$version"

docker ps -a --no-trunc --format "CONTAINER\t{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.Status}}\t{{.State}}\t{{.Ports}}\t{{.CreatedAt}}\t{{.Size}}" 2>/dev/null

if docker compose version >/dev/null 2>&1; then
  printf "COMPOSE_AVAILABLE\t1\n"
else
  printf "COMPOSE_AVAILABLE\t0\n"
fi
'"#;

pub const DOCKER_IMAGES_SCRIPT: &str = r#"docker images --no-trunc --format "IMAGE\t{{.ID}}\t{{.Repository}}\t{{.Tag}}\t{{.Size}}\t{{.CreatedSince}}" 2>/dev/null"#;
pub const DOCKER_VOLUMES_SCRIPT: &str =
    r#"docker volume ls --format "VOLUME\t{{.Driver}}\t{{.Name}}" 2>/dev/null"#;
pub const DOCKER_NETWORKS_SCRIPT: &str = r#"docker network ls --no-trunc --format "NETWORK\t{{.ID}}\t{{.Name}}\t{{.Driver}}\t{{.Scope}}" 2>/dev/null"#;
pub const DOCKER_COMPOSE_PROJECTS_SCRIPT: &str = r#"sh -c '
if ! docker compose version >/dev/null 2>&1; then
  exit 0
fi
docker compose ls --format json 2>/dev/null || true
'"#;

pub const DOCKER_CONTAINER_DETAILS_INSPECT_BEGIN: &str = "INSPECT_JSON_BEGIN";
pub const DOCKER_CONTAINER_DETAILS_INSPECT_END: &str = "INSPECT_JSON_END";
pub const DOCKER_CONTAINER_DETAILS_STATS_BEGIN: &str = "CONTAINER_STATS_BEGIN";
pub const DOCKER_CONTAINER_DETAILS_STATS_END: &str = "CONTAINER_STATS_END";

impl DockerService {
    pub fn new(config: SshSessionConfig) -> Self {
        Self {
            config,
            multiplex: None,
            elevation: Default::default(),
        }
    }

    pub fn with_multiplex(
        config: SshSessionConfig,
        multiplex: SshMultiplexHandle,
    ) -> anyhow::Result<Self> {
        multiplex.ensure_matches_config(&config)?;
        Ok(Self {
            config,
            elevation: multiplex.inner.docker_elevation.clone(),
            multiplex: Some(multiplex),
        })
    }

    pub fn overview(&self) -> anyhow::Result<RemoteDockerOverview> {
        let output = self.exec(DOCKER_OVERVIEW_SCRIPT, DOCKER_TIMEOUT)?;
        let mut overview = parse_docker_overview_output(&output.stdout);
        if overview.available {
            overview.images = self.images().unwrap_or_default();
            overview.volumes = self.volumes().unwrap_or_default();
            overview.networks = self.networks().unwrap_or_default();
            overview.compose_projects = self.compose_projects().unwrap_or_default();
        }
        Ok(overview)
    }

    pub fn images(&self) -> anyhow::Result<Vec<DockerImage>> {
        let output =
            self.exec_success(DOCKER_IMAGES_SCRIPT, DOCKER_TIMEOUT, "Docker images failed")?;
        Ok(parse_docker_images_output(&output.stdout))
    }

    pub fn volumes(&self) -> anyhow::Result<Vec<DockerVolume>> {
        let output = self.exec_success(
            DOCKER_VOLUMES_SCRIPT,
            DOCKER_TIMEOUT,
            "Docker volumes failed",
        )?;
        Ok(parse_docker_volumes_output(&output.stdout))
    }

    pub fn networks(&self) -> anyhow::Result<Vec<DockerNetwork>> {
        let output = self.exec_success(
            DOCKER_NETWORKS_SCRIPT,
            DOCKER_TIMEOUT,
            "Docker networks failed",
        )?;
        Ok(parse_docker_networks_output(&output.stdout))
    }

    pub fn compose_projects(&self) -> anyhow::Result<Vec<DockerComposeProject>> {
        let output = self.exec_success(
            DOCKER_COMPOSE_PROJECTS_SCRIPT,
            DOCKER_TIMEOUT,
            "Docker compose projects failed",
        )?;
        Ok(parse_compose_projects(&output.stdout))
    }

    pub fn container_details(&self, container_id: &str) -> anyhow::Result<DockerContainerDetails> {
        let output = self.exec_success(
            &docker_container_details_script(container_id),
            DOCKER_TIMEOUT,
            "Docker container details failed",
        )?;
        Ok(parse_docker_container_details_output(&output.stdout))
    }

    pub fn container_stats(
        &self,
        container_id: &str,
    ) -> anyhow::Result<Option<DockerContainerStats>> {
        let command = format!(
            "docker stats --no-stream --no-trunc --format \"CONTAINER_STATS\\t{{{{.ID}}}}\\t{{{{.CPUPerc}}}}\\t{{{{.MemUsage}}}}\\t{{{{.MemPerc}}}}\\t{{{{.NetIO}}}}\\t{{{{.BlockIO}}}}\\t{{{{.PIDs}}}}\" {}",
            sh_quote(container_id)
        );
        let output =
            self.exec_success(&command, DOCKER_TIMEOUT, "Docker container stats failed")?;
        Ok(parse_docker_stats_output(&output.stdout).into_iter().next())
    }

    pub fn container_action(
        &self,
        container_id: &str,
        action: &str,
    ) -> anyhow::Result<RemoteCommandOutput> {
        let action = normalize_container_action(action)?;
        let command = format!("docker {action} {}", sh_quote(container_id));
        self.exec_success(&command, DOCKER_TIMEOUT, "Docker container action failed")
    }

    pub fn image_remove(&self, image_id: &str, force: bool) -> anyhow::Result<RemoteCommandOutput> {
        let force_arg = if force { " -f" } else { "" };
        let command = format!("docker image rm{force_arg} {}", sh_quote(image_id));
        self.exec_success(&command, DOCKER_TIMEOUT, "Docker image remove failed")
    }

    pub fn volume_remove(
        &self,
        volume_name: &str,
        force: bool,
    ) -> anyhow::Result<RemoteCommandOutput> {
        let force_arg = if force { " -f" } else { "" };
        let command = format!("docker volume rm{force_arg} {}", sh_quote(volume_name));
        self.exec_success(&command, DOCKER_TIMEOUT, "Docker volume remove failed")
    }

    pub fn network_remove(&self, network_id: &str) -> anyhow::Result<RemoteCommandOutput> {
        let command = format!("docker network rm {}", sh_quote(network_id));
        self.exec_success(&command, DOCKER_TIMEOUT, "Docker network remove failed")
    }

    pub fn system_prune(&self, volumes: bool) -> anyhow::Result<RemoteCommandOutput> {
        let volumes_arg = if volumes { " --volumes" } else { "" };
        self.exec_success(
            &format!("docker system prune -f{volumes_arg}"),
            DOCKER_TIMEOUT,
            "Docker system prune failed",
        )
    }

    pub fn container_logs(
        &self,
        container_id: &str,
        tail: u32,
    ) -> anyhow::Result<RemoteCommandOutput> {
        let tail = tail.clamp(10, 2000);
        let command = format!("docker logs --tail {tail} {}", sh_quote(container_id));
        self.exec(&command, DOCKER_LOG_TIMEOUT)
    }

    pub fn compose_action(
        &self,
        project_name: &str,
        config_files: Option<&str>,
        action: &str,
    ) -> anyhow::Result<RemoteCommandOutput> {
        let action = normalize_compose_action(action)?;
        let mut command = build_compose_base_command(project_name, config_files);
        command.push(' ');
        command.push_str(action);
        if action == "up" {
            command.push_str(" -d");
        }
        self.exec_success(&command, DOCKER_TIMEOUT, "Docker compose action failed")
    }

    pub fn compose_services(
        &self,
        project_name: &str,
        config_files: Option<&str>,
    ) -> anyhow::Result<Vec<DockerComposeService>> {
        let base = build_compose_base_command(project_name, config_files);
        let command = format!(
            "services_output=$({base} config --services) || exit $?; \
             printf '%s\\n' \"$services_output\"; \
             printf '\\n{COMPOSE_PS_JSON_BEGIN}\\n'; \
             {base} ps --all --format json 2>/dev/null || true; \
             printf '\\n{COMPOSE_PS_JSON_END}\\n'"
        );
        let output =
            self.exec_success(&command, DOCKER_TIMEOUT, "Docker compose services failed")?;
        let (services_raw, ps_json_raw) = split_compose_services_output(&output.stdout);
        Ok(parse_compose_services_output(&services_raw, &ps_json_raw))
    }

    pub fn compose_service_action(
        &self,
        project_name: &str,
        config_files: Option<&str>,
        service_name: &str,
        action: &str,
    ) -> anyhow::Result<RemoteCommandOutput> {
        let action = normalize_compose_service_action(action)?;
        let mut command = build_compose_base_command(project_name, config_files);
        command.push(' ');
        command.push_str(action);
        if action == "up" {
            command.push_str(" -d");
        }
        command.push(' ');
        command.push_str(&sh_quote(service_name));
        self.exec_success(
            &command,
            DOCKER_TIMEOUT,
            "Docker compose service action failed",
        )
    }

    fn raw_exec(
        &self,
        command: String,
        input: Option<nyaterm_core::SecretString>,
        timeout: Duration,
    ) -> anyhow::Result<RemoteCommandOutput> {
        crate::remote_process::run_ssh_command_with_input(
            self.config.clone(),
            self.multiplex.clone(),
            command,
            input,
            timeout,
        )
    }

    fn detect_elevation(&self) -> anyhow::Result<DockerElevation> {
        let exists = self.raw_exec(
            docker_environment("command -v docker"),
            None,
            DOCKER_TIMEOUT,
        )?;
        if exists.exit_status != Some(0) {
            anyhow::bail!("Docker is not installed or its executable is unavailable");
        }
        let probe = self.raw_exec(docker_environment("docker info"), None, DOCKER_TIMEOUT)?;
        if probe.exit_status == Some(0) {
            return Ok(DockerElevation::Plain);
        }
        if !probe
            .stderr
            .to_ascii_lowercase()
            .contains("permission denied")
        {
            anyhow::bail!("Docker daemon is unavailable");
        }
        let privileged = format!(
            "sudo -n -- sh -c {}",
            sh_quote(&docker_environment("docker info"))
        );
        if self.raw_exec(privileged, None, DOCKER_TIMEOUT)?.exit_status == Some(0) {
            return Ok(DockerElevation::Sudo);
        }
        let probe = format!(
            "sudo -S -p '' -- sh -c {}",
            sh_quote(&docker_environment("docker info"))
        );
        if let Some(password) = self.config.password.clone()
            && self
                .raw_exec(probe.clone(), Some(password.clone()), DOCKER_TIMEOUT)?
                .exit_status
                == Some(0)
        {
            return Ok(DockerElevation::Password(password));
        }
        let provider = self
            .config
            .credential_provider
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Docker requires sudo authorization"))?;
        self.config.attempt.check().map_err(anyhow::Error::msg)?;
        let response = provider
            .request_secret(&crate::SshCredentialPrompt {
                host: self.config.host.clone(),
                port: self.config.port,
                username: self.config.username.clone(),
                connection_name: self.config.name.clone(),
                kind: crate::SshCredentialPromptKind::Password,
                reason: crate::SshCredentialPromptReason::DockerElevation,
                attempt: 1,
                prompt_text: None,
                echo: false,
            })
            .map_err(anyhow::Error::msg)?;
        let password: nyaterm_core::SecretString = response
            .ok_or_else(|| anyhow::anyhow!("Docker authorization cancelled"))?
            .into();
        if self
            .raw_exec(probe, Some(password.clone()), DOCKER_TIMEOUT)?
            .exit_status
            != Some(0)
        {
            anyhow::bail!("Docker sudo authorization failed");
        }
        Ok(DockerElevation::Password(password))
    }

    fn exec(&self, command: &str, timeout: Duration) -> anyhow::Result<RemoteCommandOutput> {
        let cached = self
            .elevation
            .lock()
            .map_err(|_| anyhow::anyhow!("Docker authorization state unavailable"))?
            .clone();
        let elevation = match cached {
            Some(cached) => cached,
            None => {
                let detected = self.detect_elevation()?;
                *self
                    .elevation
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Docker authorization state unavailable"))? =
                    Some(detected.clone());
                detected
            }
        };
        let command = docker_environment(command);
        match elevation {
            DockerElevation::Plain => self.raw_exec(command, None, timeout),
            DockerElevation::Sudo => self.raw_exec(
                format!("sudo -n -- sh -c {}", sh_quote(&command)),
                None,
                timeout,
            ),
            DockerElevation::Password(password) => self.raw_exec(
                format!("sudo -S -p '' -- sh -c {}", sh_quote(&command)),
                Some(password),
                timeout,
            ),
        }
    }

    fn exec_success(
        &self,
        command: &str,
        timeout: Duration,
        context: &str,
    ) -> anyhow::Result<RemoteCommandOutput> {
        let output = self.exec(command, timeout)?;
        ensure_remote_command_success(output, context)
    }
}

pub fn docker_container_details_script(container_id: &str) -> String {
    format!(
        "printf '{inspect_begin}\\n'; \
         docker inspect {container_id} || exit $?; \
         printf '\\n{inspect_end}\\n'; \
         printf '{stats_begin}\\n'; \
         docker stats --no-stream --no-trunc --format \"CONTAINER_STATS\\t{{{{.ID}}}}\\t{{{{.CPUPerc}}}}\\t{{{{.MemUsage}}}}\\t{{{{.MemPerc}}}}\\t{{{{.NetIO}}}}\\t{{{{.BlockIO}}}}\\t{{{{.PIDs}}}}\" {container_id} 2>/dev/null || true; \
         printf '\\n{stats_end}\\n'",
        container_id = sh_quote(container_id),
        inspect_begin = DOCKER_CONTAINER_DETAILS_INSPECT_BEGIN,
        inspect_end = DOCKER_CONTAINER_DETAILS_INSPECT_END,
        stats_begin = DOCKER_CONTAINER_DETAILS_STATS_BEGIN,
        stats_end = DOCKER_CONTAINER_DETAILS_STATS_END,
    )
}

pub fn parse_docker_overview_output(output: &str) -> RemoteDockerOverview {
    let mut overview = RemoteDockerOverview::default();

    for line in output.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        match cols.first().copied() {
            Some("DOCKER_AVAILABLE") if cols.len() >= 2 => overview.available = cols[1] == "1",
            Some("DOCKER_VERSION") if cols.len() >= 2 => overview.version = cols[1].to_string(),
            Some("COMPOSE_AVAILABLE") if cols.len() >= 2 => {
                overview.compose_available = cols[1] == "1"
            }
            Some("CONTAINER") if cols.len() >= 9 => overview.containers.push(DockerContainer {
                id: cols[1].to_string(),
                name: cols[2].to_string(),
                image: cols[3].to_string(),
                status: cols[4].to_string(),
                state: cols[5].to_string(),
                ports: cols[6].to_string(),
                created_at: cols[7].to_string(),
                size: cols[8].to_string(),
                stats: None,
            }),
            _ => {}
        }
    }

    overview
}

pub fn parse_docker_images_output(output: &str) -> Vec<DockerImage> {
    output
        .lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split('\t').collect();
            (cols.first() == Some(&"IMAGE") && cols.len() >= 6).then(|| DockerImage {
                id: cols[1].to_string(),
                repository: cols[2].to_string(),
                tag: cols[3].to_string(),
                size: cols[4].to_string(),
                created_since: cols[5].to_string(),
            })
        })
        .collect()
}

pub fn parse_docker_volumes_output(output: &str) -> Vec<DockerVolume> {
    output
        .lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split('\t').collect();
            (cols.first() == Some(&"VOLUME") && cols.len() >= 3).then(|| DockerVolume {
                driver: cols[1].to_string(),
                name: cols[2].to_string(),
            })
        })
        .collect()
}

pub fn parse_docker_networks_output(output: &str) -> Vec<DockerNetwork> {
    output
        .lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split('\t').collect();
            (cols.first() == Some(&"NETWORK") && cols.len() >= 5).then(|| DockerNetwork {
                id: cols[1].to_string(),
                name: cols[2].to_string(),
                driver: cols[3].to_string(),
                scope: cols[4].to_string(),
            })
        })
        .collect()
}

pub fn parse_compose_projects(raw: &str) -> Vec<DockerComposeProject> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }

    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| {
            let name = json_string_field(item, &["Name", "name"])?;
            let status = json_string_field(item, &["Status", "status"]).unwrap_or("");
            let config_files =
                json_string_field(item, &["ConfigFiles", "configFiles"]).unwrap_or("");
            Some(DockerComposeProject {
                name: name.to_string(),
                status: status.to_string(),
                config_files: config_files.to_string(),
            })
        })
        .collect()
}

pub fn parse_docker_container_details_output(output: &str) -> DockerContainerDetails {
    let (inspect_raw, stats_raw) = split_container_details_output(output);
    let stats = parse_docker_stats_output(&stats_raw).into_iter().next();
    let mut details = parse_container_inspect_json(&inspect_raw);
    details.stats = stats;
    details
}

pub fn parse_docker_stats_output(output: &str) -> Vec<DockerContainerStats> {
    output
        .lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split('\t').collect();
            (cols.first() == Some(&"CONTAINER_STATS") && cols.len() >= 8).then(|| {
                DockerContainerStats {
                    cpu_percent: parse_percent(cols[2]),
                    memory_usage: cols[3].to_string(),
                    memory_percent: parse_percent(cols[4]),
                    net_io: cols[5].to_string(),
                    block_io: cols[6].to_string(),
                    pids: cols[7].to_string(),
                }
            })
        })
        .collect()
}

pub fn parse_compose_services_output(
    services_raw: &str,
    ps_json_raw: &str,
) -> Vec<DockerComposeService> {
    let mut services: Vec<DockerComposeService> = services_raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|name| DockerComposeService {
            name: name.to_string(),
            status: String::new(),
            containers: Vec::new(),
        })
        .collect();

    for item in parse_compose_ps_json_values(ps_json_raw) {
        let Some(service_name) = json_string_field(&item, &["Service", "service"]) else {
            continue;
        };

        if !services.iter().any(|service| service.name == service_name) {
            services.push(DockerComposeService {
                name: service_name.to_string(),
                status: String::new(),
                containers: Vec::new(),
            });
        }

        let container = DockerComposeServiceContainer {
            id: json_string_field(&item, &["ID", "Id", "id"])
                .unwrap_or("")
                .to_string(),
            name: json_string_field(&item, &["Name", "name"])
                .unwrap_or("")
                .to_string(),
            state: json_string_field(&item, &["State", "state"])
                .unwrap_or("")
                .to_string(),
            status: json_string_field(&item, &["Status", "status"])
                .unwrap_or("")
                .to_string(),
        };

        if let Some(service) = services
            .iter_mut()
            .find(|service| service.name == service_name)
        {
            service.containers.push(container);
        }
    }

    for service in &mut services {
        service
            .containers
            .sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        service.status = compose_service_status(&service.containers);
    }

    services
}

pub fn normalize_container_action(action: &str) -> anyhow::Result<&'static str> {
    match action.trim().to_ascii_lowercase().as_str() {
        "start" => Ok("start"),
        "stop" => Ok("stop"),
        "restart" => Ok("restart"),
        "kill" => Ok("kill"),
        "remove" | "rm" => Ok("rm"),
        _ => anyhow::bail!("Unsupported Docker container action"),
    }
}

pub fn normalize_compose_action(action: &str) -> anyhow::Result<&'static str> {
    match action.trim().to_ascii_lowercase().as_str() {
        "up" => Ok("up"),
        "down" => Ok("down"),
        "restart" => Ok("restart"),
        _ => anyhow::bail!("Unsupported Docker compose action"),
    }
}

pub fn normalize_compose_service_action(action: &str) -> anyhow::Result<&'static str> {
    match action.trim().to_ascii_lowercase().as_str() {
        "up" => Ok("up"),
        "stop" => Ok("stop"),
        "restart" => Ok("restart"),
        _ => anyhow::bail!("Unsupported Docker compose service action"),
    }
}

fn parse_percent(value: &str) -> f64 {
    value.trim().trim_end_matches('%').parse().unwrap_or(0.0)
}

fn split_container_details_output(output: &str) -> (String, String) {
    enum Section {
        None,
        Inspect,
        Stats,
    }

    let mut inspect = String::new();
    let mut stats = String::new();
    let mut section = Section::None;

    for line in output.lines() {
        match line {
            DOCKER_CONTAINER_DETAILS_INSPECT_BEGIN => {
                section = Section::Inspect;
                continue;
            }
            DOCKER_CONTAINER_DETAILS_INSPECT_END => {
                section = Section::None;
                continue;
            }
            DOCKER_CONTAINER_DETAILS_STATS_BEGIN => {
                section = Section::Stats;
                continue;
            }
            DOCKER_CONTAINER_DETAILS_STATS_END => {
                section = Section::None;
                continue;
            }
            _ => {}
        }

        match section {
            Section::Inspect => {
                inspect.push_str(line);
                inspect.push('\n');
            }
            Section::Stats => {
                stats.push_str(line);
                stats.push('\n');
            }
            Section::None => {}
        }
    }

    (inspect, stats)
}

fn parse_container_inspect_json(raw: &str) -> DockerContainerDetails {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw.trim()) else {
        return DockerContainerDetails::default();
    };
    let Some(item) = value.as_array().and_then(|items| items.first()) else {
        return DockerContainerDetails::default();
    };

    let state = item.get("State").and_then(serde_json::Value::as_object);
    let config = item.get("Config").and_then(serde_json::Value::as_object);

    DockerContainerDetails {
        stats: None,
        started_at: state
            .and_then(|state| json_object_string_field(state, "StartedAt"))
            .unwrap_or("")
            .to_string(),
        finished_at: state
            .and_then(|state| json_object_string_field(state, "FinishedAt"))
            .unwrap_or("")
            .to_string(),
        restart_count: item
            .get("RestartCount")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        entrypoint: config
            .and_then(|config| json_object_command_field(config, "Entrypoint"))
            .unwrap_or_default(),
        command: config
            .and_then(|config| json_object_command_field(config, "Cmd"))
            .unwrap_or_default(),
        mounts: parse_container_mounts(item.get("Mounts")),
        networks: parse_container_networks(item.get("NetworkSettings")),
    }
}

fn parse_container_mounts(value: Option<&serde_json::Value>) -> Vec<DockerContainerMount> {
    value
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| DockerContainerMount {
                    kind: json_string_field(item, &["Type", "type"])
                        .unwrap_or("")
                        .to_string(),
                    source: json_string_field(item, &["Source", "source"])
                        .unwrap_or("")
                        .to_string(),
                    destination: json_string_field(item, &["Destination", "destination"])
                        .unwrap_or("")
                        .to_string(),
                    mode: json_string_field(item, &["Mode", "mode"])
                        .unwrap_or("")
                        .to_string(),
                    rw: item
                        .get("RW")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_container_networks(value: Option<&serde_json::Value>) -> Vec<DockerContainerNetwork> {
    let Some(networks) = value
        .and_then(|value| value.get("Networks"))
        .and_then(serde_json::Value::as_object)
    else {
        return Vec::new();
    };

    let mut items: Vec<DockerContainerNetwork> = networks
        .iter()
        .map(|(name, value)| DockerContainerNetwork {
            name: name.to_string(),
            ip_address: json_string_field(value, &["IPAddress", "GlobalIPv6Address"])
                .unwrap_or("")
                .to_string(),
        })
        .collect();
    items.sort_by(|left, right| left.name.cmp(&right.name));
    items
}

fn parse_compose_ps_json_values(raw: &str) -> Vec<serde_json::Value> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(items) = value.as_array() {
            return items.clone();
        }
        if value.is_object() {
            return vec![value];
        }
    }

    raw.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
        .filter(|value| value.is_object())
        .collect()
}

fn compose_service_status(containers: &[DockerComposeServiceContainer]) -> String {
    if containers.is_empty() {
        return String::new();
    }

    if containers
        .iter()
        .any(|container| container.state.eq_ignore_ascii_case("running"))
    {
        return "running".to_string();
    }

    let mut states: Vec<&str> = Vec::new();
    for container in containers {
        let state = container.state.trim();
        if !state.is_empty()
            && !states
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(state))
        {
            states.push(state);
        }
    }

    if states.is_empty() {
        containers
            .iter()
            .find_map(|container| {
                let status = container.status.trim();
                (!status.is_empty()).then_some(status)
            })
            .unwrap_or("")
            .to_string()
    } else {
        states.join(", ")
    }
}

fn build_compose_base_command(project_name: &str, config_files: Option<&str>) -> String {
    let mut command = String::from("docker compose");
    if let Some(config_files) = config_files.filter(|value| !value.trim().is_empty()) {
        for file in config_files
            .split(',')
            .map(str::trim)
            .filter(|file| !file.is_empty())
        {
            command.push_str(" -f ");
            command.push_str(&sh_quote(file));
        }
    }
    command.push_str(" -p ");
    command.push_str(&sh_quote(project_name));
    command
}

fn split_compose_services_output(output: &str) -> (String, String) {
    let mut services = String::new();
    let mut ps_json = String::new();
    let mut in_ps_json = false;

    for line in output.lines() {
        if line == COMPOSE_PS_JSON_BEGIN {
            in_ps_json = true;
            continue;
        }
        if line == COMPOSE_PS_JSON_END {
            in_ps_json = false;
            continue;
        }
        if in_ps_json {
            ps_json.push_str(line);
            ps_json.push('\n');
        } else {
            services.push_str(line);
            services.push('\n');
        }
    }

    (services, ps_json)
}

fn json_object_string_field<'a>(
    item: &'a serde_json::Map<String, serde_json::Value>,
    name: &str,
) -> Option<&'a str> {
    item.get(name).and_then(serde_json::Value::as_str)
}

fn json_object_command_field(
    item: &serde_json::Map<String, serde_json::Value>,
    name: &str,
) -> Option<String> {
    let value = item.get(name)?;
    if value.is_null() {
        return Some(String::new());
    }
    if let Some(value) = value.as_str() {
        return Some(value.to_string());
    }
    value.as_array().map(|items| {
        items
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
            .join(" ")
    })
}

fn json_string_field<'a>(item: &'a serde_json::Value, names: &[&str]) -> Option<&'a str> {
    names
        .iter()
        .find_map(|name| item.get(*name).and_then(serde_json::Value::as_str))
}

fn sh_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_compose_action, normalize_compose_service_action, normalize_container_action,
        parse_compose_projects, parse_compose_services_output,
        parse_docker_container_details_output, parse_docker_images_output,
        parse_docker_networks_output, parse_docker_overview_output, parse_docker_volumes_output,
    };

    #[test]
    fn parses_docker_overview_rows() {
        let raw = concat!(
            "DOCKER_AVAILABLE\t1\n",
            "DOCKER_VERSION\t26.1.0\n",
            "CONTAINER\tabc123\tweb\tnginx:latest\tUp 2 minutes\trunning\t0.0.0.0:80->80/tcp\t2026-01-01 00:00:00 +0000 UTC\t0B\n",
            "COMPOSE_AVAILABLE\t1\n",
        );

        let overview = parse_docker_overview_output(raw);

        assert!(overview.available);
        assert_eq!(overview.version, "26.1.0");
        assert!(overview.compose_available);
        assert_eq!(overview.containers[0].name, "web");
        assert!(overview.containers[0].stats.is_none());
    }

    #[test]
    fn parses_missing_docker_state() {
        let overview = parse_docker_overview_output("DOCKER_AVAILABLE\t0\n");
        assert!(!overview.available);
        assert!(overview.containers.is_empty());
    }

    #[test]
    fn parses_docker_resource_rows() {
        let images =
            parse_docker_images_output("IMAGE\tsha256:fff\tnginx\tlatest\t70MB\t2 days ago\n");
        let volumes = parse_docker_volumes_output("VOLUME\tlocal\tdata\n");
        let networks = parse_docker_networks_output("NETWORK\tdef456\tbridge\tbridge\tlocal\n");
        let compose_projects = parse_compose_projects(
            r#"[{"Name":"demo","Status":"running(1)","ConfigFiles":"/srv/demo/compose.yaml"}]"#,
        );

        assert_eq!(images[0].repository, "nginx");
        assert_eq!(volumes[0].name, "data");
        assert_eq!(networks[0].name, "bridge");
        assert_eq!(compose_projects[0].name, "demo");
    }

    #[test]
    fn parses_container_details_with_stats() {
        let raw = concat!(
            "INSPECT_JSON_BEGIN\n",
            r#"[{"State":{"StartedAt":"2026-01-01T00:00:00Z","FinishedAt":"0001-01-01T00:00:00Z"},"RestartCount":2,"Config":{"Entrypoint":["/entry"],"Cmd":["run","server"]},"Mounts":[{"Type":"bind","Source":"/host","Destination":"/app","Mode":"ro","RW":false}],"NetworkSettings":{"Networks":{"bridge":{"IPAddress":"172.17.0.2"}}}}]"#,
            "\nINSPECT_JSON_END\n",
            "CONTAINER_STATS_BEGIN\n",
            "CONTAINER_STATS\tabc123\t1.25%\t10MiB / 1GiB\t0.98%\t1kB / 2kB\t0B / 0B\t3\n",
            "CONTAINER_STATS_END\n",
        );

        let details = parse_docker_container_details_output(raw);

        assert_eq!(details.restart_count, 2);
        assert_eq!(details.entrypoint, "/entry");
        assert_eq!(details.command, "run server");
        assert_eq!(details.mounts[0].destination, "/app");
        assert_eq!(details.networks[0].name, "bridge");
        assert_eq!(details.stats.unwrap().cpu_percent, 1.25);
    }

    #[test]
    fn parses_compose_service_json_array_output() {
        let services = parse_compose_services_output(
            "web\nworker\n",
            r#"[{"Service":"web","Name":"demo-web-1","ID":"abc","State":"running","Status":"Up 2 minutes"}]"#,
        );

        assert_eq!(services.len(), 2);
        assert_eq!(services[0].name, "web");
        assert_eq!(services[0].status, "running");
        assert_eq!(services[0].containers[0].name, "demo-web-1");
        assert_eq!(services[1].name, "worker");
        assert!(services[1].containers.is_empty());
    }

    #[test]
    fn validates_docker_actions() {
        assert_eq!(normalize_container_action("remove").unwrap(), "rm");
        assert!(normalize_container_action("exec").is_err());
        assert_eq!(normalize_compose_action("up").unwrap(), "up");
        assert!(normalize_compose_action("pull").is_err());
        assert_eq!(
            normalize_compose_service_action("restart").unwrap(),
            "restart"
        );
        assert!(normalize_compose_service_action("down").is_err());
    }
}
