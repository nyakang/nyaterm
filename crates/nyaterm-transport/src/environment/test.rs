//! Unit tests for the shell environment module.

#[test]
fn resolved_local_shell_uses_real_program_to_match_environment_snapshot() {
    let program = std::path::PathBuf::from("resolved-profile-shell");
    let shell = crate::local_shell::ResolvedLocalShell {
        program: program.to_string_lossy().into_owned(),
        args: Vec::new(),
    };
    let mut snapshot = super::EnvironmentSnapshot {
        values: std::sync::Arc::new(std::collections::HashMap::new()),
        exact: true,
        source_shell: Some(program),
    };
    assert!(shell.matches_environment(&snapshot));
    snapshot.source_shell = Some(std::path::PathBuf::from("wt.exe"));
    assert!(!shell.matches_environment(&snapshot));
    snapshot.source_shell = None;
    assert!(!shell.matches_environment(&snapshot));
}

#[cfg(unix)]
use std::collections::HashMap;

#[cfg(windows)]
use super::parser::parse_cmd_shell_output;
use super::parser::{parse_base64_shell_output, parse_complete_base64_shell_output};
#[cfg(unix)]
use super::shell::build_complete_fish_shell_script;
#[cfg(windows)]
use super::shell::{
    build_complete_windows_shell_script, build_shell_command, build_windows_shell_script,
    fallback_environment_shell_path_from, load_from_windows_shell_candidates,
    windows_environment_shell_candidates,
};
use super::{
    AutoRefreshClaim, EnvironmentValue, MAX_ENVIRONMENT_VARIABLE_NAME_LENGTH,
    ShellEnvironmentCache, ShellEnvironmentError, normalize_environment_variable_name,
};
#[cfg(windows)]
use std::io::ErrorKind;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(windows)]
use std::path::{Path, PathBuf};
#[cfg(any(unix, windows))]
use std::sync::Arc;
use std::time::{Duration, Instant};
#[cfg(unix)]
use std::{fs, path::PathBuf};

#[cfg(unix)]
async fn run_script_fixture<T, F, Fut>(mut operation: F) -> Result<T, ShellEnvironmentError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, ShellEnvironmentError>>,
{
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match operation().await {
            // A parallel fork can briefly retain a just-written script's writer
            // until exec closes it. ETXTBSY means the fixture never started, so
            // retrying admission cannot duplicate its observable shell effects.
            Err(ShellEnvironmentError::Spawn(error))
                if error.raw_os_error() == Some(libc::ETXTBSY) && Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            result => return result,
        }
    }
}

#[cfg(any(unix, windows))]
impl super::ShellEnvironmentCache {
    pub(crate) fn with_values_for_test(
        program: PathBuf,
        values: std::collections::HashMap<String, EnvironmentValue>,
    ) -> Arc<Self> {
        let cache = Self::new();
        cache
            .store_complete_snapshot(values, false, false, Some(program))
            .unwrap();
        cache
    }

    pub(crate) fn with_shell_path_for_test(path: std::path::PathBuf) -> Arc<Self> {
        Self::with_shell_path_and_timeout_for_test(path, super::DEFAULT_TIMEOUT)
    }

    pub(crate) fn with_shell_path_and_timeout_for_test(
        path: std::path::PathBuf,
        timeout: Duration,
    ) -> Arc<Self> {
        let mut cache = Self::new();
        let cache_inner =
            Arc::get_mut(&mut cache).expect("new shell environment cache is not shared");
        cache_inner.shell_path = Some(path);
        cache_inner.timeout = timeout;
        cache
    }
}

#[cfg(unix)]
impl super::ShellEnvironmentCache {
    fn store_value_for_test(
        &self,
        variable: String,
        value: super::EnvironmentValue,
    ) -> Result<(), super::ShellEnvironmentError> {
        self.store_values([(variable, value)])
    }
}

#[cfg(unix)]
impl super::EnvironmentSnapshot {
    pub(crate) fn from_test(values: HashMap<String, super::EnvironmentValue>, exact: bool) -> Self {
        Self {
            values: Arc::new(values),
            exact,
            source_shell: None,
        }
    }
}

#[test]
fn variable_name_validation_rejects_shell_code() {
    let cache = ShellEnvironmentCache::new();
    let error = cache.cached("VALUE; touch /tmp/pwned").unwrap_err();
    assert!(matches!(error, ShellEnvironmentError::InvalidVariableName));
}

#[test]
fn parser_ignores_shell_noise_and_reads_marked_values() {
    let output = b"welcome\n__NYATERM_ENV_test__:START\n__NYATERM_ENV_test__:VALUE:PATH:L3RtcA==\nhook-noise\nhook-after-value\n__NYATERM_ENV_test__:VALUE_END:PATH\n__NYATERM_ENV_test__:END\n";
    let variables = vec!["PATH".to_string()];
    let values = parse_base64_shell_output("__NYATERM_ENV_test__", output, &variables).unwrap();
    assert_eq!(values.get("PATH").map(|value| value.as_str()), Some("/tmp"));
}

#[test]
fn parser_rejects_invalid_utf8_without_accepting_corrupted_value() {
    let output = b"__NYATERM_ENV_test__:START\n__NYATERM_ENV_test__:VALUE:SECRET:_w==\n__NYATERM_ENV_test__:VALUE_END:SECRET\n__NYATERM_ENV_test__:END\n";
    let variables = vec!["SECRET".to_string()];

    let error = parse_base64_shell_output("__NYATERM_ENV_test__", output, &variables).unwrap_err();

    assert!(matches!(error, ShellEnvironmentError::ValueEncoding));
}

#[test]
fn targeted_parser_distinguishes_empty_from_missing_values() {
    let marker = "__NYATERM_ENV_test__";
    let output = concat!(
        "__NYATERM_ENV_test__:START\n",
        "__NYATERM_ENV_test__:VALUE:EMPTY:\n",
        "__NYATERM_ENV_test__:VALUE_END:EMPTY\n",
        "__NYATERM_ENV_test__:VALUE_END:MISSING\n",
        "__NYATERM_ENV_test__:END\n",
    );
    let variables = vec!["EMPTY".to_string(), "MISSING".to_string()];

    let values = parse_base64_shell_output(marker, output.as_bytes(), &variables).unwrap();

    assert_eq!(values.get("EMPTY").map(EnvironmentValue::as_str), Some(""));
    assert!(!values.contains_key("MISSING"));
}

#[test]
fn complete_parser_keeps_empty_values_and_ignores_noise() {
    let output = concat!(
        "startup noise\n",
        "__NYATERM_ENV_test__:START\n",
        "__NYATERM_ENV_test__:VALUE:PATH:L3RtcA==\n",
        "__NYATERM_ENV_test__:VALUE_END:PATH\n",
        "__NYATERM_ENV_test__:VALUE:EMPTY:\n",
        "__NYATERM_ENV_test__:VALUE_END:EMPTY\n",
        "__NYATERM_ENV_test__:VALUE:NOT-A-VARIABLE:not-base64!!!\n",
        "__NYATERM_ENV_test__:VALUE_END:NOT-A-VARIABLE\n",
        "__NYATERM_ENV_test__:END\n",
        "trailing noise\n",
    );

    let values =
        parse_complete_base64_shell_output("__NYATERM_ENV_test__", output.as_bytes()).unwrap();

    assert_eq!(
        values.get("PATH").map(EnvironmentValue::as_str),
        Some("/tmp")
    );
    assert_eq!(values.get("EMPTY").map(EnvironmentValue::as_str), Some(""));
    assert!(!values.contains_key("NOT-A-VARIABLE"));
}

#[test]
fn complete_parser_accepts_unicode_identifier_names() {
    let marker = "__NYATERM_ENV_test__";
    let output = concat!(
        "__NYATERM_ENV_test__:START\n",
        "__NYATERM_ENV_test__:VALUE:变量:5Lit5paH\n",
        "__NYATERM_ENV_test__:VALUE_END:变量\n",
        "__NYATERM_ENV_test__:END\n",
    );

    let values = parse_complete_base64_shell_output(marker, output.as_bytes()).unwrap();

    assert_eq!(
        values.get("变量").map(EnvironmentValue::as_str),
        Some("中文")
    );
}

#[test]
fn complete_blob_parser_preserves_empty_and_multiline_values() {
    let marker = "__NYATERM_ENV_test__";
    let blob = b"PATH\0/tmp\0EMPTY\0\0ProgramFiles(x86)\0C:\\Program Files (x86)\0MULTILINE\0first\nsecond\0";
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, blob);
    let output = format!("noise\n{marker}:START\n{marker}:BLOB:{encoded}\n{marker}:END\n");

    let values = parse_complete_base64_shell_output(marker, output.as_bytes()).unwrap();

    assert_eq!(
        values.get("PATH").map(EnvironmentValue::as_str),
        Some("/tmp")
    );
    assert_eq!(values.get("EMPTY").map(EnvironmentValue::as_str), Some(""));
    assert_eq!(
        values
            .get("ProgramFiles(x86)")
            .map(EnvironmentValue::as_str),
        Some("C:\\Program Files (x86)")
    );
    assert_eq!(
        values.get("MULTILINE").map(EnvironmentValue::as_str),
        Some("first\nsecond")
    );
}

#[test]
fn complete_export_blob_parser_preserves_fish_path_serialization() {
    let marker = "__NYATERM_ENV_test__";
    let sentinel = super::COMPLETE_SNAPSHOT_SENTINEL_VARIABLE;
    let blob = format!(
        "PATH=/custom/bin:/usr/bin\0EMPTY=\0MULTILINE=first\nsecond\0EQUAL=a=b\0{sentinel}={marker}\0"
    );
    let encoded =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, blob.as_bytes());
    let output = format!("{marker}:START\n{marker}:EXPORT_BLOB:{encoded}\n{marker}:END\n");

    let values = parse_complete_base64_shell_output(marker, output.as_bytes()).unwrap();

    assert_eq!(
        values.get("PATH").map(EnvironmentValue::as_str),
        Some("/custom/bin:/usr/bin")
    );
    assert_eq!(values.get("EMPTY").map(EnvironmentValue::as_str), Some(""));
    assert_eq!(
        values.get("MULTILINE").map(EnvironmentValue::as_str),
        Some("first\nsecond")
    );
    assert_eq!(
        values.get("EQUAL").map(EnvironmentValue::as_str),
        Some("a=b")
    );
    assert!(!values.contains_key(sentinel));
}

#[test]
fn complete_export_blob_parser_rejects_an_incomplete_stream() {
    let marker = "__NYATERM_ENV_test__";
    let blob = b"PATH=/custom/bin:/usr/bin\0";
    let encoded =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, blob.as_slice());
    let output = format!("{marker}:START\n{marker}:EXPORT_BLOB:{encoded}\n{marker}:END\n");

    let error = parse_complete_base64_shell_output(marker, output.as_bytes()).unwrap_err();

    assert!(matches!(error, ShellEnvironmentError::OutputEncoding));
}

#[cfg(unix)]
#[test]
fn complete_fish_script_uses_the_exported_nul_stream() {
    let marker = "__NYATERM_ENV_test__";
    let script = build_complete_fish_shell_script(marker);

    assert!(script.contains("command env -0"));
    assert!(script.contains("test $exported_environment_status -eq 0"));
    assert!(script.contains("for pipeline_status_code in $pipeline_status"));
    assert!(!script.contains("for status in $pipeline_status"));
    assert!(script.contains("EXPORT_BLOB"));
    assert!(!script.contains("$$variable"));
}

#[test]
fn variable_name_validation_rejects_names_longer_than_runtime_limit() {
    let variable = "A".repeat(MAX_ENVIRONMENT_VARIABLE_NAME_LENGTH + 1);

    let error = normalize_environment_variable_name(&variable).unwrap_err();

    assert!(matches!(error, ShellEnvironmentError::InvalidVariableName));
}

#[tokio::test]
async fn batch_lookup_rejects_an_unbounded_request() {
    let variables = (0..=256).map(|index| format!("NYATERM_BATCH_{index}"));
    let variables: Vec<_> = variables.collect();
    let cache = ShellEnvironmentCache::new();

    let error = cache.warm(&variables).await.unwrap_err();

    assert!(matches!(error, ShellEnvironmentError::RequestTooLarge));
}

#[cfg(unix)]
#[test]
fn cache_rejects_an_oversized_environment_value() {
    let values = HashMap::from([(
        "OVERSIZED".to_string(),
        EnvironmentValue::new("x".repeat(8 * 1024 * 1024 + 1)),
    )]);

    let error = super::ensure_environment_map_fit(&values).unwrap_err();

    assert!(matches!(error, ShellEnvironmentError::CacheLimitExceeded));
}

#[cfg(unix)]
#[test]
fn snapshot_lookup_supports_platform_environment_names() {
    let snapshot = super::EnvironmentSnapshot::from_test(
        HashMap::from([
            (
                "ProgramFiles(x86)".to_string(),
                EnvironmentValue::new("C:\\Program Files (x86)".to_string()),
            ),
            (
                "$LITERAL".to_string(),
                EnvironmentValue::new("literal-dollar-name".to_string()),
            ),
        ]),
        true,
    );

    assert_eq!(
        snapshot
            .get("ProgramFiles(x86)")
            .as_ref()
            .map(EnvironmentValue::as_str),
        Some("C:\\Program Files (x86)")
    );
    assert_eq!(
        snapshot
            .get("$LITERAL")
            .as_ref()
            .map(EnvironmentValue::as_str),
        Some("literal-dollar-name")
    );
}

#[tokio::test]
async fn resolve_caches_a_requested_value() {
    let cache = ShellEnvironmentCache::new();
    let value = cache.refresh("PATH").await.unwrap();
    assert!(value.is_some());
    assert!(cache.cached("PATH").unwrap().is_some());
}

#[cfg(unix)]
#[tokio::test]
async fn initialize_caches_a_complete_shell_snapshot() {
    let cache = ShellEnvironmentCache::new();

    assert!(cache.snapshot().unwrap().is_none());
    cache.initialize().await.unwrap();
    let snapshot = cache.snapshot().unwrap().expect("complete shell snapshot");

    assert!(!snapshot.is_empty());
    assert!(snapshot.get("PATH").is_some());
    assert!(cache.is_initialized().unwrap());
    assert!(cache.has_exact_snapshot().unwrap());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn initialize_preserves_an_inherited_fallback_when_shell_fails() {
    let missing_shell = PathBuf::from(format!(
        "nyaterm-missing-shell-{}.exe",
        uuid::Uuid::new_v4().simple()
    ));
    let cache = ShellEnvironmentCache::with_shell_path_for_test(missing_shell);

    assert!(cache.initialize().await.is_err());

    let snapshot = cache
        .snapshot()
        .unwrap()
        .expect("inherited fallback snapshot");
    assert!(cache.is_initialized().unwrap());
    assert!(!cache.has_exact_snapshot().unwrap());
    assert!(!snapshot.replaces_inherited_environment());
}

#[cfg(unix)]
#[tokio::test]
async fn initialize_retries_after_an_inherited_fallback_was_installed() {
    let mut cache =
        ShellEnvironmentCache::with_shell_path_for_test(PathBuf::from("/path/that/does/not/exist"));
    assert!(cache.initialize().await.is_err());

    Arc::get_mut(&mut cache)
        .expect("cache has no outstanding snapshots")
        .shell_path = Some(PathBuf::from("/bin/sh"));
    cache.initialize().await.expect("retry complete shell load");

    let snapshot = cache.snapshot().unwrap().expect("complete shell snapshot");
    assert!(snapshot.replaces_inherited_environment());
}

#[tokio::test]
async fn missing_variable_is_cached_until_refresh() {
    let cache = ShellEnvironmentCache::new();
    let variable = format!("NYATERM_TEST_MISSING_{}", uuid::Uuid::new_v4().simple());
    assert!(cache.resolve(&variable).await.unwrap().is_none());
    assert!(cache.is_missing_cached(&variable).unwrap());
    assert!(cache.resolve(&variable).await.unwrap().is_none());
}

#[test]
fn auto_refresh_marks_all_waiting_variables_missing() {
    let cache = ShellEnvironmentCache::new();
    cache
        .store_complete_snapshot(std::collections::HashMap::new(), true, true, None)
        .expect("initialize empty snapshot");

    assert!(matches!(
        cache.claim_auto_refresh("FIRST"),
        Ok(AutoRefreshClaim::Leader)
    ));
    assert!(matches!(
        cache.claim_auto_refresh("SECOND"),
        Ok(AutoRefreshClaim::Wait)
    ));
    cache.finish_auto_refresh().expect("finish refresh");

    assert!(cache.is_missing_cached("FIRST").unwrap());
    assert!(cache.is_missing_cached("SECOND").unwrap());
}

#[cfg(windows)]
#[test]
fn powershell_shell_loader_uses_powershell_protocol() {
    let marker = "__NYATERM_ENV_TEST__";
    let variables = vec!["SSH_AUTH_SOCK".to_string()];
    let script = build_windows_shell_script(Path::new("powershell.exe"), marker, &variables);

    assert!(script.contains("Write-Output '__NYATERM_ENV_TEST__:START'"));
    assert!(script.contains("[Environment]::GetEnvironmentVariable('SSH_AUTH_SOCK', 'Process')"));
    assert!(!script.contains("@echo off"));
    assert!(!script.contains("set SSH_AUTH_SOCK"));

    let command = build_shell_command(Path::new("powershell.exe"), &script);
    let arguments: Vec<_> = command
        .as_std()
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        arguments,
        vec!["-NoLogo".to_string(), "-Command".to_string(), script]
    );
}

#[cfg(windows)]
#[test]
fn cmd_shell_loader_uses_cmd_protocol() {
    let marker = "__NYATERM_ENV_TEST__";
    let variables = vec!["SSH_AUTH_SOCK".to_string()];
    let script = build_windows_shell_script(Path::new("cmd.exe"), marker, &variables);

    assert!(script.starts_with("@echo off&chcp 65001 >nul&"));
    assert!(script.contains("set SSH_AUTH_SOCK 2>nul"));
    assert!(!script.contains("Write-Output"));
    assert!(!script.contains("[Environment]::GetEnvironmentVariable"));

    let command = build_shell_command(Path::new("cmd.exe"), &script);
    let arguments: Vec<_> = command
        .as_std()
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        arguments,
        vec!["/d".to_string(), "/s".to_string(), "/c".to_string(), script,]
    );
}

#[cfg(windows)]
#[test]
fn complete_powershell_loader_uses_environment_enumerator() {
    let marker = "__NYATERM_ENV_TEST__";
    let script = build_complete_windows_shell_script(Path::new("powershell.exe"), marker);

    assert!(script.contains("GetEnvironmentVariables('Process')"));
    assert!(script.contains("ToBase64String"));
    assert!(!script.contains("set&"));
}

#[cfg(windows)]
#[test]
fn complete_cmd_loader_lists_all_environment_variables() {
    let marker = "__NYATERM_ENV_TEST__";
    let script = build_complete_windows_shell_script(Path::new("cmd.exe"), marker);

    assert_eq!(
        script,
        "@echo off&chcp 65001 >nul&echo __NYATERM_ENV_TEST__:START&set&echo __NYATERM_ENV_TEST__:END"
    );
}

#[cfg(windows)]
#[test]
fn windows_environment_shell_candidates_prioritize_cmd_before_powershell() {
    let fallback = PathBuf::from("C:\\Windows\\System32\\cmd.exe");

    assert_eq!(
        windows_environment_shell_candidates(fallback.clone()),
        [
            fallback,
            PathBuf::from("powershell.exe"),
            PathBuf::from("pwsh.exe"),
        ]
    );
}

#[cfg(windows)]
#[test]
fn missing_or_blank_comspec_falls_back_to_cmd_exe() {
    assert_eq!(
        fallback_environment_shell_path_from(None),
        PathBuf::from("cmd.exe")
    );
    assert_eq!(
        fallback_environment_shell_path_from(Some(PathBuf::new())),
        PathBuf::from("cmd.exe")
    );
    assert_eq!(
        fallback_environment_shell_path_from(Some(PathBuf::from("   "))),
        PathBuf::from("cmd.exe")
    );
}

#[cfg(windows)]
#[tokio::test]
async fn windows_shell_loader_falls_back_to_cmd_after_spawn_failure() {
    let missing_shell = PathBuf::from(format!(
        "nyaterm-missing-shell-{}.exe",
        uuid::Uuid::new_v4().simple()
    ));
    let cmd = PathBuf::from("cmd.exe");
    let variables = vec!["PATH".to_string()];

    let (values, detected_shell) = load_from_windows_shell_candidates(
        [missing_shell, cmd.clone()],
        Duration::from_secs(5),
        &variables,
    )
    .await
    .expect("cmd fallback loads PATH");

    assert_eq!(detected_shell, cmd);
    assert!(values.contains_key("PATH"));
}

#[cfg(windows)]
#[tokio::test]
async fn powershell_shell_loader_reads_requested_value_when_available() {
    let cache = ShellEnvironmentCache::with_shell_path_and_timeout_for_test(
        PathBuf::from("powershell.exe"),
        Duration::from_secs(30),
    );

    match cache.refresh("PATH").await {
        Ok(value) => assert!(value.is_some()),
        Err(ShellEnvironmentError::Spawn(error)) if error.kind() == ErrorKind::NotFound => {
            eprintln!("Windows PowerShell is unavailable; cmd fallback is covered separately");
        }
        Err(error) => panic!("PowerShell environment lookup failed: {error}"),
    }
}

#[cfg(windows)]
#[test]
fn cmd_shell_output_parser_reads_only_requested_values() {
    let marker = "__NYATERM_ENV_TEST__";
    let output = concat!(
        "__NYATERM_ENV_TEST__:START \r\n",
        "__NYATERM_ENV_TEST__:VARIABLE_START:PATH \r\n",
        "PATH=C:\\Windows\\System32;C:\\Tools=stable\r\n",
        "PATH_EXTRA=must-not-leak\r\n",
        "__NYATERM_ENV_TEST__:VARIABLE_END:PATH \r\n",
        "__NYATERM_ENV_TEST__:VARIABLE_START:MISSING \r\n",
        "Environment variable MISSING is not defined.\r\n",
        "__NYATERM_ENV_TEST__:VARIABLE_END:MISSING \r\n",
        "__NYATERM_ENV_TEST__:END \r\n",
    );
    let variables = vec!["PATH".to_string(), "MISSING".to_string()];
    let values = parse_cmd_shell_output(marker, output.as_bytes(), &variables).unwrap();

    assert_eq!(
        values.get("PATH").map(EnvironmentValue::as_str),
        Some("C:\\Windows\\System32;C:\\Tools=stable")
    );
    assert!(!values.contains_key("MISSING"));
}

#[cfg(windows)]
#[test]
fn cmd_shell_output_parser_rejects_invalid_utf8_in_requested_value() {
    let marker = "__NYATERM_ENV_TEST__";
    let output = [
        marker.as_bytes(),
        b":START\r\n",
        marker.as_bytes(),
        b":VARIABLE_START:PATH\r\nPATH=",
        &[0xff],
        b"\r\n",
        marker.as_bytes(),
        b":VARIABLE_END:PATH\r\n",
        marker.as_bytes(),
        b":END\r\n",
    ]
    .concat();
    let variables = vec!["PATH".to_string()];

    let error = parse_cmd_shell_output(marker, &output, &variables).unwrap_err();

    assert!(matches!(error, ShellEnvironmentError::ValueEncoding));
}

#[cfg(windows)]
#[test]
fn complete_cmd_shell_output_parser_reads_all_values() {
    let marker = "__NYATERM_ENV_TEST__";
    let output = concat!(
        "noise\r\n",
        "__NYATERM_ENV_TEST__:START\r\n",
        "PATH=C:\\Windows\\System32  \r\n",
        "EMPTY=\r\n",
        "ProgramFiles(x86)=C:\\Program Files (x86)\r\n",
        "=C:=C:\\work\r\n",
        "__NYATERM_ENV_TEST__:END\r\n",
    );

    let values = super::parser::parse_complete_cmd_shell_output(marker, output.as_bytes()).unwrap();

    assert_eq!(
        values.get("PATH").map(EnvironmentValue::as_str),
        Some("C:\\Windows\\System32  ")
    );
    assert_eq!(values.get("EMPTY").map(EnvironmentValue::as_str), Some(""));
    assert_eq!(
        values
            .get("ProgramFiles(x86)")
            .map(EnvironmentValue::as_str),
        Some("C:\\Program Files (x86)")
    );
    assert!(!values.contains_key("=C:"));
}

#[cfg(windows)]
#[tokio::test]
async fn cmd_shell_loader_reads_a_requested_environment_value() {
    let cache = ShellEnvironmentCache::with_shell_path_for_test(PathBuf::from("cmd.exe"));

    let value = cache.refresh("PATH").await.unwrap();

    assert!(value.is_some());
}

#[cfg(windows)]
#[tokio::test]
async fn cmd_shell_loader_caches_a_missing_variable() {
    let cache = ShellEnvironmentCache::with_shell_path_for_test(PathBuf::from("cmd.exe"));
    let variable = format!("NYATERM_CMD_MISSING_{}", uuid::Uuid::new_v4().simple());

    assert!(cache.refresh(&variable).await.unwrap().is_none());
    assert!(cache.is_missing_cached(&variable).unwrap());
}

#[cfg(unix)]
#[tokio::test]
async fn refresh_drops_a_stale_value_when_shell_cannot_start() {
    let cache =
        ShellEnvironmentCache::with_shell_path_for_test(PathBuf::from("/path/that/does/not/exist"));
    cache
        .store_value_for_test(
            "SSH_AUTH_SOCK".to_string(),
            EnvironmentValue::new("/stale/agent.sock".to_string()),
        )
        .unwrap();

    assert!(cache.refresh("SSH_AUTH_SOCK").await.is_err());
    assert!(cache.cached("SSH_AUTH_SOCK").unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn shell_loader_works_without_a_tokio_runtime() {
    let cache = ShellEnvironmentCache::new();
    let value = futures::executor::block_on(cache.resolve("PATH")).unwrap();
    assert!(value.is_some());
}

#[cfg(any(unix, windows))]
#[test]
fn shell_loader_without_a_tokio_runtime_honors_lock_deadline() {
    let cache = ShellEnvironmentCache::new();
    let holder_cache = cache.clone();
    let (ready_sender, ready_receiver) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build lock holder runtime");
        runtime.block_on(async move {
            let _guard = holder_cache.load_lock.lock().await;
            ready_sender.send(()).expect("signal held load lock");
            std::thread::sleep(Duration::from_millis(100));
        });
    });
    ready_receiver
        .recv()
        .expect("receive held load lock signal");

    let result = futures::executor::block_on(
        cache.resolve_until("PATH", Instant::now() + Duration::from_millis(10)),
    );

    assert!(matches!(result, Err(ShellEnvironmentError::Timeout)));
    holder.join().expect("join lock holder thread");
}

#[cfg(unix)]
#[tokio::test]
async fn shell_loader_reads_a_requested_exported_value() {
    let cache = ShellEnvironmentCache::new();
    let variables = vec!["PATH".to_string()];
    let values = cache
        .load_from_shell_until(&variables, Instant::now() + Duration::from_secs(10))
        .await
        .unwrap();
    assert!(values.contains_key("PATH"));
}

#[cfg(unix)]
#[tokio::test]
async fn shell_loader_reads_a_custom_exported_value() {
    let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-shell-environment");
    fs::create_dir(&root).expect("create shell test directory");
    let shell = root.join("shell");
    fs::write(
            &shell,
            "#!/bin/sh\nexport NYATERM_TEST_CUSTOM_AGENT=/tmp/nyaterm-agent.sock\nexec /bin/sh \"$@\"\n",
        )
        .expect("write shell test wrapper");
    let mut permissions = fs::metadata(&shell)
        .expect("read shell test wrapper permissions")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&shell, permissions).expect("make shell test wrapper executable");

    let cache = ShellEnvironmentCache::with_shell_path_for_test(PathBuf::from(&shell));
    let value = run_script_fixture(|| cache.refresh("NYATERM_TEST_CUSTOM_AGENT"))
        .await
        .expect("load custom exported value")
        .expect("custom exported value is present");

    assert_eq!(value.as_str(), "/tmp/nyaterm-agent.sock");
}

#[cfg(unix)]
#[tokio::test]
async fn targeted_shell_lookup_preserves_empty_and_unset_values() {
    let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-targeted-shell-values");
    fs::create_dir(&root).expect("create targeted shell test directory");
    let shell = root.join("shell");
    fs::write(
        &shell,
        "#!/bin/sh\nexport NYATERM_TEST_TARGETED_EMPTY=\nunset HOME\nexec /bin/sh \"$@\"\n",
    )
    .expect("write targeted shell test wrapper");
    let mut permissions = fs::metadata(&shell)
        .expect("read targeted shell test wrapper permissions")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&shell, permissions).expect("make targeted shell test wrapper executable");

    let cache = ShellEnvironmentCache::with_shell_path_for_test(shell);
    let variables = [
        "NYATERM_TEST_TARGETED_EMPTY".to_string(),
        "HOME".to_string(),
    ];
    run_script_fixture(|| cache.refresh_many(&variables))
        .await
        .expect("load targeted shell values");

    assert_eq!(
        cache
            .cached("NYATERM_TEST_TARGETED_EMPTY")
            .unwrap()
            .as_ref()
            .map(EnvironmentValue::as_str),
        Some("")
    );
    assert!(cache.cached("HOME").unwrap().is_none());
    assert!(cache.is_missing_cached("HOME").unwrap());
}

#[cfg(unix)]
#[tokio::test]
async fn initialize_reads_the_complete_custom_shell_environment() {
    let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-complete-shell-environment");
    fs::create_dir(&root).expect("create complete shell test directory");
    let shell = root.join("shell");
    fs::write(
            &shell,
            "#!/bin/sh\nexport NYATERM_TEST_COMPLETE_ALPHA=alpha\nexport NYATERM_TEST_COMPLETE_EMPTY=\nexport NYATERM_TEST_COMPLETE_MULTILINE='first\nNYATERM_TEST_COMPLETE_FAKE=not-a-variable'\nexec /bin/sh \"$@\"\n",
        )
        .expect("write complete shell test wrapper");
    let mut permissions = fs::metadata(&shell)
        .expect("read complete shell test wrapper permissions")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&shell, permissions).expect("make complete shell test wrapper executable");

    let cache = ShellEnvironmentCache::with_shell_path_for_test(shell);
    run_script_fixture(|| cache.initialize())
        .await
        .expect("load complete shell environment");
    let snapshot = cache.snapshot().unwrap().expect("complete shell snapshot");

    assert_eq!(
        snapshot
            .get("NYATERM_TEST_COMPLETE_ALPHA")
            .as_ref()
            .map(EnvironmentValue::as_str),
        Some("alpha")
    );
    assert_eq!(
        snapshot
            .get("NYATERM_TEST_COMPLETE_EMPTY")
            .as_ref()
            .map(EnvironmentValue::as_str),
        Some("")
    );
    assert_eq!(
        snapshot
            .get("NYATERM_TEST_COMPLETE_MULTILINE")
            .as_ref()
            .map(EnvironmentValue::as_str),
        Some("first\nNYATERM_TEST_COMPLETE_FAKE=not-a-variable")
    );
    assert!(snapshot.get("NYATERM_TEST_COMPLETE_FAKE").is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn resolve_refreshes_the_complete_snapshot_once_for_a_new_variable() {
    let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-refresh-shell-environment");
    fs::create_dir(&root).expect("create refresh shell test directory");
    let state_file = root.join("loaded");
    let shell = root.join("shell");
    let state_path = state_file.display();
    fs::write(
            &shell,
            format!(
                "#!/bin/sh\nif [ -e '{state_path}' ]; then export NYATERM_TEST_LATE_VALUE=ready; else : > '{state_path}'; fi\nexec /bin/sh \"$@\"\n"
            ),
        )
        .expect("write refresh shell test wrapper");
    let mut permissions = fs::metadata(&shell)
        .expect("read refresh shell test wrapper permissions")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&shell, permissions).expect("make refresh shell test wrapper executable");

    let cache = ShellEnvironmentCache::with_shell_path_for_test(shell);
    run_script_fixture(|| cache.initialize())
        .await
        .expect("load initial shell snapshot");
    assert!(cache.cached("NYATERM_TEST_LATE_VALUE").unwrap().is_none());

    let value = cache
        .resolve("NYATERM_TEST_LATE_VALUE")
        .await
        .expect("refresh missing shell variable")
        .expect("late shell variable is present after refresh");
    assert_eq!(value.as_str(), "ready");
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_auto_refresh_does_not_leave_the_cache_stuck() {
    let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-cancelled-refresh");
    fs::create_dir(&root).expect("create cancelled refresh test directory");
    let state_file = root.join("loaded");
    let slow_file = root.join("slow");
    let variable = format!(
        "NYATERM_TEST_CANCELLED_REFRESH_{}",
        uuid::Uuid::new_v4().simple()
    );
    let shell = root.join("shell");
    fs::write(
            &shell,
            format!(
                "#!/bin/sh\nif [ -e '{slow}' ]; then export {variable}=ready; elif [ -e '{state}' ]; then : > '{slow}'; rm -f '{state}'; sleep 1; else : > '{state}'; fi\nexec /bin/sh \"$@\"\n",
                state = state_file.display(),
                slow = slow_file.display(),
            ),
        )
        .expect("write cancelled refresh shell wrapper");
    let mut permissions = fs::metadata(&shell)
        .expect("read cancelled refresh shell wrapper permissions")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&shell, permissions)
        .expect("make cancelled refresh shell wrapper executable");

    let cache = ShellEnvironmentCache::with_shell_path_for_test(shell);
    run_script_fixture(|| cache.initialize())
        .await
        .expect("load initial shell snapshot");
    assert!(cache.cached(&variable).unwrap().is_none());

    let cancelled =
        tokio::time::timeout(Duration::from_millis(100), cache.resolve(&variable)).await;
    assert!(
        cancelled.is_err(),
        "the deliberately slow refresh must time out"
    );

    let value = tokio::time::timeout(Duration::from_secs(5), cache.resolve(&variable))
        .await
        .expect("cache must not remain stuck after cancellation")
        .expect("retry the cancelled refresh")
        .expect("variable becomes available on retry");
    assert_eq!(value.as_str(), "ready");
}

#[cfg(unix)]
#[tokio::test]
async fn refresh_all_keeps_the_previous_snapshot_when_loading_fails() {
    let mut cache = ShellEnvironmentCache::with_shell_path_for_test(PathBuf::from("/bin/sh"));
    cache
        .initialize()
        .await
        .expect("load initial shell snapshot");
    let before = cache.snapshot().unwrap().expect("initial shell snapshot");
    let path_before = before.get("PATH").map(|value| value.as_str().to_string());
    drop(before);

    Arc::get_mut(&mut cache)
        .expect("cache has no outstanding snapshot handles")
        .shell_path = Some(PathBuf::from("/path/that/does/not/exist"));

    assert!(cache.refresh_all().await.is_err());
    let after = cache
        .snapshot()
        .unwrap()
        .expect("previous snapshot remains available");
    assert_eq!(
        after.get("PATH").map(|value| value.as_str().to_string()),
        path_before
    );
}

#[test]
fn environment_value_debug_output_is_redacted() {
    let value = EnvironmentValue::new("/private/agent.sock".to_string());
    assert_eq!(format!("{value:?}"), "<redacted>");
}

#[test]
fn shell_reader_marker_is_not_retained_in_environment_snapshots() {
    let values = super::canonicalize_environment_values(std::collections::HashMap::from([
        (
            "NYATERM_SHELL_ENV_READER".to_string(),
            EnvironmentValue::new("1".to_string()),
        ),
        (
            "NYATERM_TEST_VISIBLE".to_string(),
            EnvironmentValue::new("ok".to_string()),
        ),
    ]));

    assert!(!values.contains_key("NYATERM_SHELL_ENV_READER"));
    assert_eq!(
        values
            .get("NYATERM_TEST_VISIBLE")
            .map(EnvironmentValue::as_str),
        Some("ok")
    );
}
