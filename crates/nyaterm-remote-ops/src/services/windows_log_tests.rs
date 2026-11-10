use super::windows_logs_script;
use crate::host::{HostTarget, ensure_host_command_success};
use std::time::Duration;

#[test]
fn windows_log_script_preserves_empty_events_but_propagates_other_errors() {
    for (id, expected_success) in [
        ("NoMatchingEventsFound", true),
        ("PermissionDenied", false),
        ("NoMatchingProvidersFound", false),
    ] {
        let fixture = format!(
            r#"
function Get-WinEvent {{
  [CmdletBinding()]
  param($FilterHashtable, $MaxEvents)
  $record = [System.Management.Automation.ErrorRecord]::new(
    [System.InvalidOperationException]::new('fixture access denied'),
    '{id}', [System.Management.Automation.ErrorCategory]::PermissionDenied, $null)
  $PSCmdlet.ThrowTerminatingError($record)
}}
{}"#,
            windows_logs_script("fixture", 20)
        );
        let output = HostTarget::Local
            .run_powershell(&fixture, Duration::from_secs(10))
            .unwrap();
        let result = ensure_host_command_success(output, "service logs unavailable");
        if expected_success {
            assert!(result.unwrap().stdout.is_empty());
        } else {
            assert_eq!(result.unwrap_err().to_string(), "fixture access denied");
        }
    }
    let fixture = format!(
        r#"
function Get-WinEvent {{
  [CmdletBinding()]
  param($FilterHashtable, $MaxEvents)
  [pscustomobject]@{{ TimeCreated=[datetime]'2026-01-01'; Message='fixture log' }}
}}
{}"#,
        windows_logs_script("fixture", 20)
    );
    let output = HostTarget::Local
        .run_powershell(&fixture, Duration::from_secs(10))
        .unwrap();
    assert!(
        ensure_host_command_success(output, "service logs unavailable")
            .unwrap()
            .stdout
            .contains("fixture log")
    );
}
