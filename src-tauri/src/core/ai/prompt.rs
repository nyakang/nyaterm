use crate::config::AiSettings;

use super::types::{AiAction, AiChatRequest, CommandObservation};

pub(super) const SYSTEM_PROMPT: &str = r#"你是一个专业、谨慎、安全优先的 Linux / DevOps / 云原生终端助手。
你的任务是帮助用户解释终端输出、生成 Shell 命令、分析错误、提供排查步骤。

必须遵守：
1. 不要建议不可逆高危操作，除非明确说明风险和安全替代方案。
2. 默认生成只读诊断命令。
3. 对任何删除、格式化、重启、停服务、改权限、批量变更命令标记风险。
4. 命令必须适配用户当前系统、架构、shell 和权限上下文。
5. 输出必须结构化，包含命令、说明、风险等级、影响范围和回滚建议。
6. 不要编造当前系统不存在的信息；不确定时给出验证命令。
7. 不要要求用户粘贴密码、私钥、token。

只返回一个 JSON 对象，不要使用 Markdown 代码块。格式：
{
  "text": "给用户看的说明",
  "commandCards": [
    {
      "id": "cmd-uuid",
      "title": "标题",
      "command": "shell command",
      "explanation": "命令说明",
      "riskLevel": "low|medium|high|critical",
      "riskReason": "风险原因",
      "expectedEffect": "预计影响",
      "rollback": "回滚方式或无需回滚",
      "category": "Linux 性能"
    }
  ]
}"#;

pub(super) const AGENT_SYSTEM_PROMPT: &str = r#"你是一个终端自动化 Agent，通过"思考—执行—观察"循环完成用户的任务。

每一轮你只能做一件事：执行一条命令或给出最终回答。

规则：
1. 每轮只返回一个 JSON 对象，不要使用 Markdown。
2. 如果需要执行命令，返回 action 为 "execute_command"。
3. 任务完成或无需执行命令时，返回 action 为 "final_answer"。
4. 优先使用只读命令收集信息，再做修改操作。
5. 不要执行不可逆高危命令（如 rm -rf /、mkfs、停止 SSH 等），改为在 thought 中说明风险并给出 final_answer。
6. 不要编造信息；不确定时先用验证命令确认。
7. 不要要求用户提供密码、私钥、token。
8. 命令必须适配用户当前的系统和 shell 环境。
9. riskLevel 规则：只读命令 → low，普通写操作 → medium，删除/重启/权限修改 → high，不可逆破坏 → critical。

执行命令的 JSON 格式：
{
  "thought": "分析当前状态和下一步计划",
  "action": "execute_command",
  "command": "要执行的单条 shell 命令",
  "riskLevel": "low"
}

给出最终回答的 JSON 格式：
{
  "thought": "任务完成的原因",
  "action": "final_answer",
  "answer": "向用户展示的总结"
}"#;

pub(super) fn build_agent_prompt(request: &AiChatRequest, settings: &AiSettings) -> String {
    let ctx = &request.context;
    format!(
        r#"用户任务：
{user_input}

当前连接上下文：
- 连接名：{connection_name}
- 主机：{host}
- 用户：{username}
- 当前目录：{cwd}
- 操作系统：{os}
- 架构：{arch}

最近终端输出（最多 {line_limit} 行）：
{recent_output}

请开始执行任务。每轮只返回一个 JSON 对象。"#,
        user_input = request.user_input,
        connection_name = ctx.connection_name.as_deref().unwrap_or("-"),
        host = ctx.host.as_deref().unwrap_or("-"),
        username = ctx.username.as_deref().unwrap_or("-"),
        cwd = ctx.cwd.as_deref().unwrap_or("-"),
        os = ctx.os.as_deref().unwrap_or("-"),
        arch = ctx.arch.as_deref().unwrap_or(std::env::consts::ARCH),
        line_limit = settings.context_line_limit,
        recent_output = ctx.recent_output,
    )
}

pub(super) fn build_observation_message(obs: &CommandObservation, command: &str) -> String {
    let status = obs
        .exit_code
        .map(|c| format!("exit code {c}"))
        .unwrap_or_else(|| "unknown exit code".to_string());
    let output = if obs.output.len() > 8000 {
        let truncated = &obs.output[obs.output.len() - 8000..];
        format!("...(truncated)\n{truncated}")
    } else {
        obs.output.clone()
    };
    format!(
        "命令 `{command}` 执行完成（{status}，耗时 {duration}ms）。\n\n输出：\n{output}\n\n请根据观察结果决定下一步。只返回 JSON 对象。",
        duration = obs.duration_ms,
    )
}

pub(super) fn build_prompt(request: &AiChatRequest, settings: &AiSettings) -> String {
    let action = match request.action {
        AiAction::GenerateCommand => "根据自然语言需求生成 1 到 2 条 Shell 命令",
        AiAction::ExplainOutput => "解释最近终端输出并给出下一步建议",
        AiAction::ExplainSelected => "解释用户选中的终端文本并给出下一步建议",
        AiAction::AnalyzeError => "分析终端错误输出并给出排查步骤",
        AiAction::RepairFromSelection => "根据选中内容生成修复或排查命令",
        AiAction::CustomTerminalAction => "根据用户配置的终端 AI 功能处理选中内容",
        AiAction::CustomFileAction => "根据用户配置的文件 AI 功能处理文件内容",
    };
    let ctx = &request.context;
    format!(
        r#"任务：{action}
用户需求：
{user_input}

当前连接上下文：
- 连接名：{connection_name}
- 主机：{host}
- 端口：{port}
- 用户：{username}
- 当前目录：{cwd}
- 操作系统：{os}
- 架构：{arch}
- 当前输入：{input_buffer}

选中文本：
{selected_text}

最近终端输出（最多 {line_limit} 行）：
{recent_output}

要求：
- 语言：{language}
- 安全模式：{safety_mode}
- 最多生成 {max_commands} 条命令
- 优先生成只读诊断命令
- 如果信息不足，请给出验证命令
- 必须返回 JSON 对象，不要返回 Markdown"#,
        user_input = request.user_input,
        connection_name = ctx.connection_name.as_deref().unwrap_or("-"),
        host = ctx.host.as_deref().unwrap_or("-"),
        port = ctx
            .port
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        username = ctx.username.as_deref().unwrap_or("-"),
        cwd = ctx.cwd.as_deref().unwrap_or("-"),
        os = ctx.os.as_deref().unwrap_or("-"),
        arch = ctx.arch.as_deref().unwrap_or(std::env::consts::ARCH),
        input_buffer = ctx.input_buffer,
        selected_text = ctx.selected_text,
        line_limit = settings.context_line_limit,
        recent_output = ctx.recent_output,
        language = request.options.language,
        safety_mode = request.options.safety_mode,
        max_commands = request.options.max_output_commands,
    )
}
