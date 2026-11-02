use std::time::Duration;

pub use nyaterm_core::ascend_npu::{
    RemoteNpu, RemoteNpuOverview, RemoteNpuProcess, parse_npu_overview_output,
};

use crate::{
    RemoteCommandOutput, SshMultiplexHandle, SshSessionConfig, ensure_remote_command_success,
    run_ssh_command,
};

const ASCEND_NPU_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct RemoteNpuService {
    config: SshSessionConfig,
    multiplex: Option<SshMultiplexHandle>,
}

pub const ASCEND_NPU_OVERVIEW_SCRIPT: &str = r#"sh -s <<'NYATERM_ASCEND_NPU_SCRIPT'
LC_ALL=C
export LC_ALL

find_npu_smi() {
  if command -v npu-smi >/dev/null 2>&1; then
    command -v npu-smi
    return 0
  fi

  for candidate in \
    /usr/local/bin/npu-smi \
    /usr/local/Ascend/driver/tools/npu-smi \
    /usr/local/Ascend/driver/tools/*/npu-smi
  do
    if [ -x "$candidate" ]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done

  return 1
}

read_install_version() {
  file=$1
  awk -F= '
    {
      key=$1
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", key)
      if (tolower(key) == "version") {
        value=$0
        sub(/^[^=]*=/, "", value)
        gsub(/^[[:space:]"]+|[[:space:]"]+$/, "", value)
        print value
        exit
      }
    }
  ' "$file" 2>/dev/null
}

find_cann_version() {
  for root in "${ASCEND_HOME_PATH:-}" /usr/local/Ascend "${HOME:-}/Ascend"; do
    [ -n "$root" ] || continue
    for info_file in \
      "$root"/ascend-toolkit/latest/*-linux/ascend_toolkit_install.info \
      "$root"/nnae/latest/ascend_nnae_install.info \
      "$root"/nnrt/latest/*-linux/ascend_nnrt_install.info
    do
      [ -r "$info_file" ] || continue
      version=$(read_install_version "$info_file")
      if [ -n "$version" ]; then
        printf '%s\n' "$version"
        return 0
      fi
    done
  done
  return 1
}

npu_smi=$(find_npu_smi 2>/dev/null || true)
if [ -z "$npu_smi" ]; then
  printf 'NPU_AVAILABLE\t0\n'
  exit 0
fi

npu_output=$("$npu_smi" info 2>&1)
status=$?
if [ "$status" -ne 0 ] || [ -z "$npu_output" ]; then
  printf 'NPU_AVAILABLE\t0\n'
  printf 'NPU_ERROR\t%s\n' "$(printf '%s' "$npu_output" | tr '\n\t' '  ' | cut -c1-500)"
  exit 0
fi

printf 'NPU_AVAILABLE\t1\n'
cann_version=$(find_cann_version 2>/dev/null || true)
printf 'NPU_CANN_VERSION\t%s\n' "$cann_version"
printf 'NPU_SMI_BEGIN\n'
printf '%s\n' "$npu_output"
printf 'NPU_SMI_END\n'
NYATERM_ASCEND_NPU_SCRIPT
"#;

impl RemoteNpuService {
    pub fn new(config: SshSessionConfig) -> Self {
        Self {
            config,
            multiplex: None,
        }
    }

    pub fn with_multiplex(
        config: SshSessionConfig,
        multiplex: SshMultiplexHandle,
    ) -> anyhow::Result<Self> {
        multiplex.ensure_matches_config(&config)?;
        Ok(Self {
            config,
            multiplex: Some(multiplex),
        })
    }

    pub fn overview(&self) -> anyhow::Result<RemoteNpuOverview> {
        let output = self.exec_success(
            ASCEND_NPU_OVERVIEW_SCRIPT,
            "Failed to fetch Ascend NPU overview",
        )?;
        Ok(parse_npu_overview_output(&output.stdout))
    }

    fn exec_success(&self, command: &str, context: &str) -> anyhow::Result<RemoteCommandOutput> {
        let output = run_ssh_command(
            self.config.clone(),
            self.multiplex.clone(),
            command.as_bytes().to_vec(),
            ASCEND_NPU_TIMEOUT,
        )?;
        ensure_remote_command_success(output, context)
    }
}
