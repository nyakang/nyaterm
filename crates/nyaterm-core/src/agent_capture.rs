use base64::{Engine as _, engine::general_purpose};

use crate::models::connection::AiExecutionProfile;

const MARKER_PREFIX: &str = "__DF_CMD_";

pub fn build_agent_capture_command(
    profile: AiExecutionProfile,
    marker_id: &str,
    command: &str,
) -> Option<String> {
    match profile {
        AiExecutionProfile::Posix => Some(build_posix_capture_command(marker_id, command)),
        AiExecutionProfile::Powershell => {
            Some(build_powershell_capture_command(marker_id, command))
        }
        AiExecutionProfile::Cmd => Some(build_cmd_capture_command(marker_id, command)),
        AiExecutionProfile::Auto | AiExecutionProfile::SendOnly | AiExecutionProfile::Disabled => {
            None
        }
    }
}

fn build_posix_capture_command(marker_id: &str, command: &str) -> String {
    // The command's last line must remain a complete shell line: a heredoc
    // delimiter, comment or compound statement cannot share our closing brace.
    // The colon also makes empty/comment-only commands a valid command group.
    format!(
        " printf '\\n{MARKER_PREFIX}''START_{marker_id}__\\n'; {{\n:\n{command}\n}}; _dfec=$?; printf '\\n{MARKER_PREFIX}''END_{marker_id}_'\"$_dfec\"'__\\n'; unset _dfec\n",
    )
}

fn build_powershell_capture_command(marker_id: &str, command: &str) -> String {
    let encoded_command = general_purpose::STANDARD.encode(command.as_bytes());
    format!(
        concat!(
            "$nyaiEc = 0; ",
            "$nyaiSuccess = $true; ",
            "$nyaiLastExit = 0; ",
            "$global:LASTEXITCODE = 0; ",
            "Write-Output (\"`n{MARKER_PREFIX}\" + \"START_{marker_id}__\"); ",
            "try {{ ",
            "$nyaiScript = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String(\"{encoded_command}\")); ",
            "$nyaiScript = $nyaiScript + \"`r`n`$nyaiSuccess = `$?; `$nyaiLastExit = `$LASTEXITCODE\"; ",
            ". ([scriptblock]::Create($nyaiScript)); ",
            "if (($nyaiLastExit -is [int]) -and $nyaiLastExit -ne 0) {{ $nyaiEc = $nyaiLastExit }} ",
            "elseif ($nyaiSuccess) {{ $nyaiEc = 0 }} else {{ $nyaiEc = 1 ",
            "}} ",
            "}} catch {{ Write-Error $_; $nyaiEc = 1 }}; ",
            "Write-Output (\"`n{MARKER_PREFIX}\" + \"END_{marker_id}_\" + $nyaiEc + \"__\"); ",
            "Remove-Variable nyaiEc,nyaiSuccess,nyaiLastExit,nyaiScript -ErrorAction SilentlyContinue\r\n",
        ),
        MARKER_PREFIX = MARKER_PREFIX,
        marker_id = marker_id,
        encoded_command = encoded_command,
    )
}

fn build_cmd_capture_command(marker_id: &str, command: &str) -> String {
    let command = command.replace("\r\n", " & ").replace(['\n', '\r'], " & ");
    let command = command.trim();
    let command_segment = if command.is_empty() {
        String::new()
    } else {
        format!(" & {command}")
    };

    format!(
        concat!(
            "echo {MARKER_PREFIX}^START_{marker_id}__",
            "{command_segment}",
            " & call echo {MARKER_PREFIX}^END_{marker_id}_^%ERRORLEVEL^%__\r\n",
        ),
        MARKER_PREFIX = MARKER_PREFIX,
        marker_id = marker_id,
        command_segment = command_segment,
    )
}
