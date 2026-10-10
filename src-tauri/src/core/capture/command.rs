pub fn build_capture_command(
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
    format!(
        " printf '\\n{MARKER_PREFIX}''START_{marker_id}__\\n'; {{ {command}; }}; _dfec=$?; printf '\\n{MARKER_PREFIX}''END_{marker_id}_'\"$_dfec\"'__\\n'; unset _dfec\n",
    )
}

fn build_powershell_capture_command(marker_id: &str, command: &str) -> String {
    let encoded_command = general_purpose::STANDARD.encode(command.as_bytes());
    format!(
        concat!(
            "$nyaiEc = 1; ",
            "$nyaiSuccess = $true; ",
            "$nyaiLastExit = 0; ",
            "$global:LASTEXITCODE = 0; ",
            "[Console]::Out.Write(\"`r`n{POWERSHELL_MARKER_PREFIX}\" + \"S_{marker_id}__`r`n\"); ",
            "try {{ ",
            "$nyaiScript = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String(\"{encoded_command}\")); ",
            "$nyaiScript = $nyaiScript + \"`r`n`$nyaiSuccess = `$?; `$nyaiLastExit = `$LASTEXITCODE\"; ",
            ". ([scriptblock]::Create($nyaiScript)) | Out-Default; ",
            "if (($nyaiLastExit -is [int]) -and $nyaiLastExit -ne 0) {{ $nyaiEc = $nyaiLastExit }} ",
            "elseif ($nyaiSuccess) {{ $nyaiEc = 0 }} else {{ $nyaiEc = 1 ",
            "}} ",
            "}} catch {{ $nyaiEc = 1; $_ | Out-String | ForEach-Object {{ [Console]::Out.Write($_) }} }} ",
            "finally {{ [Console]::Out.Write(\"`r`n{POWERSHELL_MARKER_PREFIX}\" + \"E_{marker_id}_\" + $nyaiEc + \"__`r`n\"); ",
            "Remove-Variable nyaiEc,nyaiSuccess,nyaiLastExit,nyaiScript -ErrorAction SilentlyContinue }}\r\n",
        ),
        marker_id = marker_id,
        encoded_command = encoded_command,
        POWERSHELL_MARKER_PREFIX = POWERSHELL_MARKER_PREFIX,
    )
}

fn build_cmd_capture_command(marker_id: &str, command: &str) -> String {
    let command = command
        .replace("\r\n", " & ")
        .replace('\n', " & ")
        .replace('\r', " & ");
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
