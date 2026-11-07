use super::{RiskAssessment, SftpRiskOperation, assess_command_risk, assess_sftp_risk};
use crate::ai::RiskLevel;
use nyaterm_mcp_protocol::{
    PathArgs, SftpChmodArgs, SftpMkdirArgs, SftpRenameArgs, SftpWriteTextArgs, TerminalExecuteArgs,
    tool,
};
use serde_json::Value;

pub fn assess_tool_risk(tool_name: &str, arguments: &Value) -> Option<RiskAssessment> {
    match tool_name {
        tool::SESSION_OPEN => Some(RiskAssessment {
            level: RiskLevel::Medium,
            reason: "opening a saved connection changes live session state".to_string(),
            auto_executable: true,
        }),
        tool::TERMINAL_START => {
            serde_json::from_value::<nyaterm_mcp_protocol::TerminalStartArgs>(arguments.clone())
                .ok()
                .map(|args| assess_command_risk(&args.command))
        }
        tool::TERMINAL_STOP => Some(RiskAssessment {
            level: RiskLevel::Medium,
            reason: "interrupting the owned terminal job".into(),
            auto_executable: true,
        }),
        tool::TERMINAL_EXECUTE => serde_json::from_value::<TerminalExecuteArgs>(arguments.clone())
            .ok()
            .map(|args| assess_command_risk(&args.command)),
        tool::SFTP_WRITE_TEXT => serde_json::from_value::<SftpWriteTextArgs>(arguments.clone())
            .ok()
            .map(|args| {
                assess_sftp_risk(
                    SftpRiskOperation::Write,
                    &args.path,
                    None,
                    args.force.unwrap_or(false),
                    None,
                )
            }),
        tool::SFTP_MKDIR => serde_json::from_value::<SftpMkdirArgs>(arguments.clone())
            .ok()
            .map(|args| {
                assess_sftp_risk(
                    SftpRiskOperation::Mkdir,
                    &args.path,
                    None,
                    false,
                    args.mode.as_deref(),
                )
            }),
        tool::SFTP_RENAME => serde_json::from_value::<SftpRenameArgs>(arguments.clone())
            .ok()
            .map(|args| {
                assess_sftp_risk(
                    SftpRiskOperation::Rename,
                    &args.old_path,
                    Some(&args.new_path),
                    false,
                    None,
                )
            }),
        tool::SFTP_DELETE => serde_json::from_value::<PathArgs>(arguments.clone())
            .ok()
            .map(|args| assess_sftp_risk(SftpRiskOperation::Delete, &args.path, None, false, None)),
        tool::SFTP_CHMOD => serde_json::from_value::<SftpChmodArgs>(arguments.clone())
            .ok()
            .map(|args| {
                assess_sftp_risk(
                    SftpRiskOperation::Chmod,
                    &args.path,
                    None,
                    false,
                    Some(&args.mode),
                )
            }),
        _ => None,
    }
}
