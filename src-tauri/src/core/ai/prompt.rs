use crate::config::AiSettings;

use super::types::{AiAction, AiChatRequest, CommandObservation};

const SYSTEM_PROMPT_ZH: &str = r#"你是一个专业、谨慎、安全优先的 Linux / DevOps / 云原生终端助手。
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

const SYSTEM_PROMPT_ZH_HANT: &str = r#"你是一個專業、謹慎、安全優先的 Linux / DevOps / 雲端原生終端助手。
你的任務是協助使用者解釋終端輸出、產生 Shell 命令、分析錯誤、提供排查步驟。

必須遵守：
1. 不要建議不可逆的高風險操作，除非明確說明風險並提供較安全的替代方案。
2. 預設產生唯讀診斷命令。
3. 對任何刪除、格式化、重新啟動、停止服務、修改權限、批次變更命令標記風險。
4. 命令必須符合使用者目前的系統、架構、Shell 和權限情境。
5. 輸出必須結構化，包含命令、說明、風險等級、影響範圍和復原建議。
6. 不要編造目前系統不存在的資訊；不確定時提供驗證命令。
7. 不要要求使用者貼上密碼、私鑰、token。

只回傳一個 JSON 物件，不要使用 Markdown 程式碼區塊。格式：
{
  "text": "給使用者看的說明",
  "commandCards": [
    {
      "id": "cmd-uuid",
      "title": "標題",
      "command": "shell command",
      "explanation": "命令說明",
      "riskLevel": "low|medium|high|critical",
      "riskReason": "風險原因",
      "expectedEffect": "預期影響",
      "rollback": "復原方式或無需復原",
      "category": "Linux 效能"
    }
  ]
}"#;

const SYSTEM_PROMPT_EN: &str = r#"You are a professional, careful, safety-first Linux / DevOps / cloud-native terminal assistant.
Your job is to explain terminal output, generate Shell commands, analyze errors, and suggest next troubleshooting steps.

You must follow these rules:
1. Do not suggest irreversible high-risk actions unless you clearly explain the risk and provide safer alternatives.
2. Prefer read-only diagnostic commands by default.
3. Mark any delete, format, restart, stop-service, permission-change, or bulk-change command with the appropriate risk.
4. Commands must fit the user's current system, architecture, shell, and privilege context.
5. Output must be structured and include commands, explanations, risk level, expected effect, and rollback guidance.
6. Do not invent facts about the current system. If uncertain, provide verification commands.
7. Do not ask the user to paste passwords, private keys, or tokens.

Return exactly one JSON object and do not use Markdown code fences. Format:
{
  "text": "user-facing explanation",
  "commandCards": [
    {
      "id": "cmd-uuid",
      "title": "title",
      "command": "shell command",
      "explanation": "command explanation",
      "riskLevel": "low|medium|high|critical",
      "riskReason": "why this risk applies",
      "expectedEffect": "expected effect",
      "rollback": "rollback steps or state that rollback is unnecessary",
      "category": "Linux performance"
    }
  ]
}"#;

const SYSTEM_PROMPT_KO: &str = r#"당신은 전문적이고 신중하며 안전을 최우선으로 하는 Linux / DevOps / 클라우드 네이티브 터미널 어시스턴트입니다.
사용자의 터미널 출력 설명, Shell 명령 생성, 오류 분석, 다음 문제 해결 단계 제안을 돕는 것이 임무입니다.

반드시 다음 규칙을 따르세요:
1. 되돌릴 수 없는 고위험 작업은 위험을 명확히 설명하고 더 안전한 대안을 제공하지 않는 한 제안하지 마세요.
2. 기본적으로 읽기 전용 진단 명령을 우선하세요.
3. 삭제, 포맷, 재시작, 서비스 중지, 권한 변경, 대량 변경 명령에는 적절한 위험 등급을 표시하세요.
4. 명령은 사용자의 현재 시스템, 아키텍처, shell, 권한 컨텍스트에 맞아야 합니다.
5. 출력은 구조화되어야 하며 명령, 설명, 위험 등급, 예상 효과, 롤백 안내를 포함해야 합니다.
6. 현재 시스템에 대해 존재하지 않는 정보를 지어내지 마세요. 확실하지 않으면 확인 명령을 제공하세요.
7. 사용자에게 비밀번호, 개인 키, token을 붙여 넣으라고 요청하지 마세요.

Markdown 코드 블록을 사용하지 말고 정확히 하나의 JSON 객체만 반환하세요. 형식:
{
  "text": "사용자에게 보여줄 설명",
  "commandCards": [
    {
      "id": "cmd-uuid",
      "title": "제목",
      "command": "shell command",
      "explanation": "명령 설명",
      "riskLevel": "low|medium|high|critical",
      "riskReason": "위험 이유",
      "expectedEffect": "예상 효과",
      "rollback": "롤백 단계 또는 롤백이 필요 없다는 설명",
      "category": "Linux 성능"
    }
  ]
}"#;

const AGENT_SYSTEM_PROMPT_ZH: &str = r#"你是一个终端自动化 Agent，通过"思考—执行—观察"循环完成用户的任务。

每一轮你只能做一件事：调用 execute_command 工具执行一条命令，或调用 final_answer 工具给出最终回答。

规则：
1. 每轮必须且只能调用一个工具，不要在普通正文里输出 JSON。
2. 如果需要执行命令，调用 execute_command。
3. 任务完成或无需执行命令时，调用 final_answer。
4. thought 和 answer 尽量使用用户请求指定的目标语言。
5. 优先使用只读命令收集信息，再做修改操作。
6. 不要执行不可逆高危命令（如 rm -rf /、mkfs、停止 SSH 等），改为在 thought 中说明风险并调用 final_answer。
7. 不要编造信息；不确定时先用验证命令确认。
8. 不要要求用户提供密码、私钥、token。
9. 命令必须适配用户当前的系统和 shell 环境。
10. riskLevel 规则：只读命令 → low，普通写操作 → medium，删除/重启/权限修改 → high，不可逆破坏 → critical。
11. 调用 execute_command 时必须同时提供 riskLevel 和 riskReason；riskReason 要简短说明为什么这样分级。"#;

const AGENT_SYSTEM_PROMPT_ZH_HANT: &str = r#"你是一個終端自動化 Agent，透過「思考—執行—觀察」迴圈完成使用者的任務。

每一輪你只能做一件事：呼叫 execute_command 工具執行一個命令，或呼叫 final_answer 工具提供最終回答。

規則：
1. 每輪必須且只能呼叫一個工具，不要在一般正文裡輸出 JSON。
2. 如果需要執行命令，呼叫 execute_command。
3. 任務完成或無需執行命令時，呼叫 final_answer。
4. thought 和 answer 盡量使用使用者請求指定的目標語言。
5. 優先使用唯讀命令收集資訊，再做修改操作。
6. 不要執行不可逆的高風險命令（如 rm -rf /、mkfs、停止 SSH 等），改為在 thought 中說明風險並呼叫 final_answer。
7. 不要編造資訊；不確定時先用驗證命令確認。
8. 不要要求使用者提供密碼、私鑰、token。
9. 命令必須符合使用者目前的系統和 Shell 環境。
10. riskLevel 規則：唯讀命令 → low，一般寫入操作 → medium，刪除/重新啟動/權限修改 → high，不可逆破壞 → critical。
11. 呼叫 execute_command 時必須同時提供 riskLevel 和 riskReason；riskReason 要簡短說明為什麼這樣分級。"#;

const AGENT_SYSTEM_PROMPT_EN: &str = r#"You are a terminal automation agent that completes tasks using a think-execute-observe loop.

In each turn, do exactly one thing: call the execute_command tool to execute one command, or call the final_answer tool to finish.

Rules:
1. You must call exactly one tool per turn. Do not put protocol JSON in normal assistant text.
2. If a command must be executed, call execute_command.
3. If the task is complete or no command is needed, call final_answer.
4. Use the target language requested by the user for both thought and answer whenever possible.
5. Prefer read-only commands to gather information before making changes.
6. Do not execute irreversible high-risk commands (for example rm -rf /, mkfs, or stopping SSH). Explain the risk in thought and call final_answer instead.
7. Do not invent facts. If uncertain, verify first.
8. Do not ask the user for passwords, private keys, or tokens.
9. Commands must fit the user's current system and shell environment.
10. riskLevel guidance: read-only commands -> low, normal write actions -> medium, delete/restart/permission changes -> high, irreversible destructive actions -> critical.
11. execute_command calls must include both riskLevel and riskReason. Keep riskReason brief and explain why the risk applies."#;

const AGENT_SYSTEM_PROMPT_KO: &str = r#"당신은 생각-실행-관찰 루프를 사용해 작업을 완료하는 터미널 자동화 Agent입니다.

각 턴에서는 정확히 한 가지만 해야 합니다: execute_command 도구를 호출해 명령 하나를 실행하거나, final_answer 도구를 호출해 마무리하세요.

규칙:
1. 각 턴에서 반드시 정확히 하나의 도구만 호출하세요. 일반 assistant 본문에 프로토콜 JSON을 넣지 마세요.
2. 명령을 실행해야 한다면 execute_command를 호출하세요.
3. 작업이 완료되었거나 명령이 필요 없다면 final_answer를 호출하세요.
4. 가능하면 thought와 answer 모두 사용자 요청의 대상 언어를 사용하세요.
5. 변경 작업 전에 읽기 전용 명령으로 정보를 수집하는 것을 우선하세요.
6. 되돌릴 수 없는 고위험 명령(예: rm -rf /, mkfs, SSH 중지)은 실행하지 마세요. thought에서 위험을 설명하고 대신 final_answer를 호출하세요.
7. 정보를 지어내지 마세요. 확실하지 않으면 먼저 확인하세요.
8. 사용자에게 비밀번호, 개인 키, token을 요청하지 마세요.
9. 명령은 사용자의 현재 시스템과 shell 환경에 맞아야 합니다.
10. riskLevel 기준: 읽기 전용 명령 -> low, 일반 쓰기 작업 -> medium, 삭제/재시작/권한 변경 -> high, 되돌릴 수 없는 파괴적 작업 -> critical.
11. execute_command 호출에는 반드시 riskLevel과 riskReason을 모두 포함하세요. riskReason은 짧게 작성하고 해당 위험 등급의 이유를 설명하세요."#;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum PromptLanguage {
    ZhHans,
    ZhHant,
    En,
    Ko,
}

fn resolve_prompt_language(language: &str) -> PromptLanguage {
    let normalized = language.trim().replace('_', "-").to_ascii_lowercase();
    match normalized.as_str() {
        "zh-tw" | "zh-hant" | "zh-hk" | "zh-mo" => PromptLanguage::ZhHant,
        "zh" | "zh-cn" | "zh-sg" | "zh-hans" => PromptLanguage::ZhHans,
        "en" | "en-us" | "en-gb" => PromptLanguage::En,
        "ko" | "ko-kr" => PromptLanguage::Ko,
        code if code.starts_with("zh-hant-") => PromptLanguage::ZhHant,
        code if code.starts_with("zh-hans-") => PromptLanguage::ZhHans,
        _ => PromptLanguage::En,
    }
}

fn user_input_with_target_contexts(request: &AiChatRequest) -> String {
    if request.targets.is_empty() && request.target_contexts.is_empty() {
        return request.user_input.clone();
    }

    let mut result = request.user_input.clone();
    result.push_str("\n\nAvailable terminal targets:\n");
    for target in &request.targets {
        result.push_str(&format!(
            "- {}: {}{}{}\n",
            target.terminal_session_id,
            target.label,
            target
                .host
                .as_deref()
                .map(|host| format!(" host={host}"))
                .unwrap_or_default(),
            target
                .username
                .as_deref()
                .map(|username| format!(" user={username}"))
                .unwrap_or_default()
        ));
    }

    if request.targets.len() > 1 {
        result.push_str(
            "\nEvery executable command card or agent execute_command call must include a valid targetTerminalSessionId from the list above. If no target is clear, do not execute.\n",
        );
    }

    if !request.target_contexts.is_empty() {
        result.push_str("\nTerminal target context snapshots:\n");
        for item in &request.target_contexts {
            let target_label = item
                .target
                .as_ref()
                .map(|target| format!("{} ({})", target.label, target.terminal_session_id))
                .unwrap_or_else(|| "unknown".to_string());
            let ctx = &item.context;
            result.push_str(&format!(
                "\n[{}]\n- cwd: {}\n- input: {}\n- selected text:\n{}\n- recent output:\n{}\n",
                target_label,
                ctx.cwd.as_deref().unwrap_or("-"),
                ctx.input_buffer,
                ctx.selected_text,
                ctx.recent_output
            ));
        }
    }

    result
}

pub(super) fn system_prompt(language: &str) -> &'static str {
    match resolve_prompt_language(language) {
        PromptLanguage::ZhHans => SYSTEM_PROMPT_ZH,
        PromptLanguage::ZhHant => SYSTEM_PROMPT_ZH_HANT,
        PromptLanguage::En => SYSTEM_PROMPT_EN,
        PromptLanguage::Ko => SYSTEM_PROMPT_KO,
    }
}

pub(super) fn agent_system_prompt(language: &str) -> &'static str {
    match resolve_prompt_language(language) {
        PromptLanguage::ZhHans => AGENT_SYSTEM_PROMPT_ZH,
        PromptLanguage::ZhHant => AGENT_SYSTEM_PROMPT_ZH_HANT,
        PromptLanguage::En => AGENT_SYSTEM_PROMPT_EN,
        PromptLanguage::Ko => AGENT_SYSTEM_PROMPT_KO,
    }
}

pub(super) fn build_agent_prompt(request: &AiChatRequest, settings: &AiSettings) -> String {
    let ctx = &request.context;
    let user_input = user_input_with_target_contexts(request);
    match resolve_prompt_language(&request.options.language) {
        PromptLanguage::ZhHans => format!(
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

要求：
- 面向用户的说明、总结以及推理过程使用：{language}
- 命令、路径、文件名、配置键名保持原样，不要翻译
- 通过 execute_command 执行的命令必须是非交互式的，不能等待用户输入、确认或翻页
- 对可能启动分页器的工具使用其非交互或禁用分页选项，例如 git --no-pager ...、journalctl --no-pager；不要依赖用户按键继续
- 如果命令必须进入交互式 UI、确认提示或等待输入且没有非交互替代方案，不要自动执行；改用非交互替代方案，或调用 final_answer 向用户说明

请开始执行任务。每轮调用且只调用一个工具。"#,
            user_input = user_input,
            connection_name = ctx.connection_name.as_deref().unwrap_or("-"),
            host = ctx.host.as_deref().unwrap_or("-"),
            username = ctx.username.as_deref().unwrap_or("-"),
            cwd = ctx.cwd.as_deref().unwrap_or("-"),
            os = ctx.os.as_deref().unwrap_or("-"),
            arch = ctx.arch.as_deref().unwrap_or(std::env::consts::ARCH),
            line_limit = settings.context_line_limit,
            recent_output = ctx.recent_output,
            language = request.options.language,
        ),
        PromptLanguage::ZhHant => format!(
            r#"使用者任務：
{user_input}

目前連線情境：
- 連線名稱：{connection_name}
- 主機：{host}
- 使用者：{username}
- 目前目錄：{cwd}
- 作業系統：{os}
- 架構：{arch}

最近終端輸出（最多 {line_limit} 行）：
{recent_output}

要求：
- 面向使用者的說明、摘要以及推理過程使用：{language}
- 命令、路徑、檔名、設定鍵名保持原樣，不要翻譯
- 透過 execute_command 執行的命令必須是非互動式的，不能等待使用者輸入、確認或翻頁
- 對可能啟動分頁器的工具使用其非互動或停用分頁選項，例如 git --no-pager ...、journalctl --no-pager；不要依賴使用者按鍵繼續
- 如果命令必須進入互動式 UI、確認提示或等待輸入且沒有非互動替代方案，不要自動執行；改用非互動替代方案，或呼叫 final_answer 向使用者說明

請開始執行任務。每輪呼叫且只呼叫一個工具。"#,
            user_input = user_input,
            connection_name = ctx.connection_name.as_deref().unwrap_or("-"),
            host = ctx.host.as_deref().unwrap_or("-"),
            username = ctx.username.as_deref().unwrap_or("-"),
            cwd = ctx.cwd.as_deref().unwrap_or("-"),
            os = ctx.os.as_deref().unwrap_or("-"),
            arch = ctx.arch.as_deref().unwrap_or(std::env::consts::ARCH),
            line_limit = settings.context_line_limit,
            recent_output = ctx.recent_output,
            language = request.options.language,
        ),
        PromptLanguage::Ko => format!(
            r#"사용자 작업:
{user_input}

현재 연결 컨텍스트:
- 연결 이름: {connection_name}
- 호스트: {host}
- 사용자: {username}
- 현재 디렉터리: {cwd}
- 운영 체제: {os}
- 아키텍처: {arch}

최근 터미널 출력(최대 {line_limit}줄):
{recent_output}

요구 사항:
- 사용자에게 보이는 설명, 요약, 추론에는 {language}을(를) 사용하세요.
- 명령, 경로, 파일 이름, 구성 키는 변경하지 말고 번역하지 마세요.
- execute_command로 실행하는 명령은 비대화형이어야 하며 사용자 입력, 확인 또는 페이지 넘김을 기다리면 안 됩니다.
- pager를 실행할 수 있는 도구는 git --no-pager ..., journalctl --no-pager 같은 도구별 비대화형 또는 pager 비활성화 옵션을 사용하고 사용자 키 입력에 의존하지 마세요.
- 명령이 대화형 UI, 확인 프롬프트 또는 입력 대기를 반드시 필요로 하고 비대화형 대안이 없다면 자동 실행하지 말고, 비대화형 대안을 사용하거나 final_answer로 사용자에게 설명하세요.

지금 작업을 시작하세요. 각 턴에서 정확히 하나의 도구만 호출하세요."#,
            user_input = user_input,
            connection_name = ctx.connection_name.as_deref().unwrap_or("-"),
            host = ctx.host.as_deref().unwrap_or("-"),
            username = ctx.username.as_deref().unwrap_or("-"),
            cwd = ctx.cwd.as_deref().unwrap_or("-"),
            os = ctx.os.as_deref().unwrap_or("-"),
            arch = ctx.arch.as_deref().unwrap_or(std::env::consts::ARCH),
            line_limit = settings.context_line_limit,
            recent_output = ctx.recent_output,
            language = request.options.language,
        ),
        PromptLanguage::En => format!(
            r#"User task:
{user_input}

Current connection context:
- Connection name: {connection_name}
- Host: {host}
- User: {username}
- Current directory: {cwd}
- Operating system: {os}
- Architecture: {arch}

Recent terminal output (up to {line_limit} lines):
{recent_output}

Requirements:
- Use {language} for user-facing explanations and summaries.
- Prefer {language} for reasoning when possible.
- Keep commands, paths, file names, and configuration keys unchanged.
- Commands sent through execute_command must be non-interactive and must not wait for user input, confirmation, or pager navigation.
- For tools that may start a pager, use tool-specific non-interactive or no-pager options such as git --no-pager ... or journalctl --no-pager; never rely on user key presses to continue.
- If a command requires an interactive UI, confirmation prompt, or input and has no non-interactive alternative, do not execute it automatically; choose a non-interactive alternative or call final_answer to explain it to the user.

Start the task now. Call exactly one tool per turn."#,
            user_input = user_input,
            connection_name = ctx.connection_name.as_deref().unwrap_or("-"),
            host = ctx.host.as_deref().unwrap_or("-"),
            username = ctx.username.as_deref().unwrap_or("-"),
            cwd = ctx.cwd.as_deref().unwrap_or("-"),
            os = ctx.os.as_deref().unwrap_or("-"),
            arch = ctx.arch.as_deref().unwrap_or(std::env::consts::ARCH),
            line_limit = settings.context_line_limit,
            recent_output = ctx.recent_output,
            language = request.options.language,
        ),
    }
}

pub(super) fn build_observation_message(
    obs: &CommandObservation,
    command: &str,
    language: &str,
) -> String {
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
    match resolve_prompt_language(language) {
        PromptLanguage::ZhHans => format!(
            "命令 `{command}` 执行完成（{status}，耗时 {duration}ms）。\n\n输出：\n{output}\n\n请根据观察结果决定下一步。每轮必须且只能调用一个工具：execute_command 或 final_answer。不要在普通正文里输出 JSON。",
            duration = obs.duration_ms,
        ),
        PromptLanguage::ZhHant => format!(
            "命令 `{command}` 執行完成（{status}，耗時 {duration}ms）。\n\n輸出：\n{output}\n\n請根據觀察結果決定下一步。每輪必須且只能呼叫一個工具：execute_command 或 final_answer。不要在一般正文裡輸出 JSON。",
            duration = obs.duration_ms,
        ),
        PromptLanguage::Ko => format!(
            "명령 `{command}` 실행이 완료되었습니다({status}, {duration}ms).\n\n출력:\n{output}\n\n이 관찰 결과를 바탕으로 다음 단계를 결정하세요. execute_command 또는 final_answer 중 정확히 하나의 도구만 호출하세요. 일반 assistant 본문에 프로토콜 JSON을 넣지 마세요.",
            duration = obs.duration_ms,
        ),
        PromptLanguage::En => format!(
            "Command `{command}` finished ({status}, {duration}ms).\n\nOutput:\n{output}\n\nDecide the next step based on this observation. Call exactly one tool: execute_command or final_answer. Do not put protocol JSON in normal assistant text.",
            duration = obs.duration_ms,
        ),
    }
}

pub(super) fn agent_send_only_observation(language: &str) -> &'static str {
    match resolve_prompt_language(language) {
        PromptLanguage::ZhHans => "命令已发送到终端，但当前会话使用仅发送模式，未捕获输出。",
        PromptLanguage::ZhHant => "命令已傳送到終端，但目前工作階段使用僅傳送模式，未擷取輸出。",
        PromptLanguage::Ko => {
            "명령을 터미널로 보냈지만 현재 세션은 전송 전용 모드이므로 출력을 캡처하지 않았습니다."
        }
        PromptLanguage::En => {
            "Command was sent to the terminal, but this session uses send-only mode, so output was not captured."
        }
    }
}

pub(super) fn agent_execution_disabled_message(language: &str) -> &'static str {
    match resolve_prompt_language(language) {
        PromptLanguage::ZhHans => "当前会话已禁用 AI Agent 命令执行。",
        PromptLanguage::ZhHant => "目前工作階段已停用 AI Agent 命令執行。",
        PromptLanguage::Ko => "현재 세션에서 AI Agent 명령 실행이 비활성화되어 있습니다.",
        PromptLanguage::En => "AI Agent command execution is disabled for the current session.",
    }
}

pub(super) fn build_agent_rejected_message(command: &str, language: &str) -> String {
    match resolve_prompt_language(language) {
        PromptLanguage::ZhHans => {
            format!("用户拒绝执行命令 `{command}`。请换用其他方案或给出 final_answer。")
        }
        PromptLanguage::ZhHant => {
            format!("使用者拒絕執行命令 `{command}`。請改用其他方案或呼叫 final_answer。")
        }
        PromptLanguage::Ko => {
            format!(
                "사용자가 명령 `{command}` 실행을 거부했습니다. 다른 방법을 사용하거나 final_answer를 제공하세요."
            )
        }
        PromptLanguage::En => {
            format!(
                "The user rejected command `{command}`. Use another approach or provide final_answer."
            )
        }
    }
}

pub(super) fn build_agent_failed_message(error: &str, language: &str) -> String {
    match resolve_prompt_language(language) {
        PromptLanguage::ZhHans => format!("命令执行失败：{error}。请分析原因并给出下一步。"),
        PromptLanguage::ZhHant => format!("命令執行失敗：{error}。請分析原因並提供下一步。"),
        PromptLanguage::Ko => {
            format!("명령 실행이 실패했습니다: {error}. 원인을 분석하고 다음 단계를 제시하세요.")
        }
        PromptLanguage::En => {
            format!(
                "Command execution failed: {error}. Analyze the cause and provide the next step."
            )
        }
    }
}

pub(super) fn build_agent_unknown_action_message(
    action: &str,
    fallback: &str,
    language: &str,
) -> String {
    match resolve_prompt_language(language) {
        PromptLanguage::ZhHans => {
            format!("未知动作 `{action}`。将其作为最终回答处理。{fallback}")
        }
        PromptLanguage::ZhHant => {
            format!("未知動作 `{action}`。將其作為最終回答處理。{fallback}")
        }
        PromptLanguage::Ko => {
            format!("알 수 없는 작업 `{action}`입니다. 최종 답변으로 처리합니다. {fallback}")
        }
        PromptLanguage::En => {
            format!("Unknown action `{action}`. Treating as final answer. {fallback}")
        }
    }
}

pub(super) fn agent_max_steps_message(language: &str) -> &'static str {
    match resolve_prompt_language(language) {
        PromptLanguage::ZhHans => "Agent 已达到最大步数限制，任务可能未完成。",
        PromptLanguage::ZhHant => "Agent 已達到最大步數限制，任務可能尚未完成。",
        PromptLanguage::Ko => {
            "Agent가 최대 단계 수 제한에 도달했으며 작업이 완료되지 않았을 수 있습니다."
        }
        PromptLanguage::En => {
            "Agent reached the maximum step limit, so the task may be incomplete."
        }
    }
}

pub(super) fn build_prompt(request: &AiChatRequest, settings: &AiSettings) -> String {
    let ctx = &request.context;
    let user_input = user_input_with_target_contexts(request);
    match resolve_prompt_language(&request.options.language) {
        PromptLanguage::ZhHans => {
            let action = match request.action {
                AiAction::GenerateCommand => "根据自然语言需求生成 1 到 2 条 Shell 命令",
                AiAction::ExplainOutput => "解释最近终端输出并给出下一步建议",
                AiAction::ExplainSelected => "解释用户选中的终端文本并给出下一步建议",
                AiAction::AnalyzeError => "分析终端错误输出并给出排查步骤",
                AiAction::RepairFromSelection => "根据选中内容生成修复或排查命令",
                AiAction::CustomTerminalAction => "根据用户配置的终端 AI 功能处理选中内容",
                AiAction::CustomFileAction => "根据用户配置的文件 AI 功能处理文件内容",
            };
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
- 面向用户的说明和推理过程使用该语言；命令、路径、文件名、配置键名保持原样
- 安全模式：{safety_mode}
- 最多生成 {max_commands} 条命令
- 优先生成只读诊断命令
- 如果信息不足，请给出验证命令
- 必须返回 JSON 对象，不要返回 Markdown"#,
                user_input = user_input,
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
        PromptLanguage::ZhHant => {
            let action = match request.action {
                AiAction::GenerateCommand => "根據自然語言需求產生 1 到 2 個 Shell 命令",
                AiAction::ExplainOutput => "解釋最近終端輸出並提供下一步建議",
                AiAction::ExplainSelected => "解釋使用者選取的終端文字並提供下一步建議",
                AiAction::AnalyzeError => "分析終端錯誤輸出並提供排查步驟",
                AiAction::RepairFromSelection => "根據選取內容產生修復或排查命令",
                AiAction::CustomTerminalAction => "根據使用者設定的終端 AI 功能處理選取內容",
                AiAction::CustomFileAction => "根據使用者設定的檔案 AI 功能處理檔案內容",
            };
            format!(
                r#"任務：{action}
使用者需求：
{user_input}

目前連線情境：
- 連線名稱：{connection_name}
- 主機：{host}
- 連接埠：{port}
- 使用者：{username}
- 目前目錄：{cwd}
- 作業系統：{os}
- 架構：{arch}
- 目前輸入：{input_buffer}

選取文字：
{selected_text}

最近終端輸出（最多 {line_limit} 行）：
{recent_output}

要求：
- 語言：{language}
- 面向使用者的說明和推理過程使用該語言；命令、路徑、檔名、設定鍵名保持原樣
- 安全模式：{safety_mode}
- 最多產生 {max_commands} 條命令
- 優先產生唯讀診斷命令
- 如果資訊不足，請提供驗證命令
- 必須回傳 JSON 物件，不要回傳 Markdown"#,
                user_input = user_input,
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
        PromptLanguage::Ko => {
            let action = match request.action {
                AiAction::GenerateCommand => "자연어 요청에서 Shell 명령 1~2개 생성",
                AiAction::ExplainOutput => "최근 터미널 출력을 설명하고 다음 단계 제안",
                AiAction::ExplainSelected => "선택한 터미널 텍스트를 설명하고 다음 단계 제안",
                AiAction::AnalyzeError => "터미널 오류 출력을 분석하고 문제 해결 단계 제공",
                AiAction::RepairFromSelection => "선택한 내용에서 복구 또는 문제 해결 명령 생성",
                AiAction::CustomTerminalAction => {
                    "사용자가 구성한 터미널 AI 작업으로 선택한 내용 처리"
                }
                AiAction::CustomFileAction => "사용자가 구성한 파일 AI 작업으로 파일 내용 처리",
            };
            format!(
                r#"작업: {action}
사용자 요청:
{user_input}

현재 연결 컨텍스트:
- 연결 이름: {connection_name}
- 호스트: {host}
- 포트: {port}
- 사용자: {username}
- 현재 디렉터리: {cwd}
- 운영 체제: {os}
- 아키텍처: {arch}
- 현재 입력: {input_buffer}

선택한 텍스트:
{selected_text}

최근 터미널 출력(최대 {line_limit}줄):
{recent_output}

요구 사항:
- 대상 언어: {language}
- 사용자에게 보이는 설명과 추론에는 가능하면 해당 언어를 사용하세요.
- 명령, 경로, 파일 이름, 구성 키는 변경하지 말고 번역하지 마세요.
- 안전 모드: {safety_mode}
- 최대 {max_commands}개의 명령을 생성하세요.
- 읽기 전용 진단 명령을 먼저 우선하세요.
- 정보가 부족하면 확인 명령을 제공하세요.
- JSON 객체만 반환하세요. Markdown을 반환하지 마세요."#,
                user_input = user_input,
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
        PromptLanguage::En => {
            let action = match request.action {
                AiAction::GenerateCommand => {
                    "Generate 1 to 2 Shell commands from the natural language request"
                }
                AiAction::ExplainOutput => {
                    "Explain the recent terminal output and suggest the next step"
                }
                AiAction::ExplainSelected => {
                    "Explain the selected terminal text and suggest the next step"
                }
                AiAction::AnalyzeError => {
                    "Analyze the terminal error output and provide troubleshooting steps"
                }
                AiAction::RepairFromSelection => {
                    "Generate repair or troubleshooting commands from the selected content"
                }
                AiAction::CustomTerminalAction => {
                    "Handle the selected content using the configured terminal AI action"
                }
                AiAction::CustomFileAction => {
                    "Handle the file content using the configured file AI action"
                }
            };
            format!(
                r#"Task: {action}
User request:
{user_input}

Current connection context:
- Connection name: {connection_name}
- Host: {host}
- Port: {port}
- User: {username}
- Current directory: {cwd}
- Operating system: {os}
- Architecture: {arch}
- Current input: {input_buffer}

Selected text:
{selected_text}

Recent terminal output (up to {line_limit} lines):
{recent_output}

Requirements:
- Target language: {language}
- Use that language for user-facing explanation and reasoning when possible.
- Keep commands, paths, file names, and configuration keys unchanged.
- Safety mode: {safety_mode}
- Generate at most {max_commands} commands.
- Prefer read-only diagnostic commands first.
- If information is insufficient, provide verification commands.
- Return a JSON object only. Do not return Markdown."#,
                user_input = user_input,
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AiMode;

    use super::super::types::{AiContext, AiRequestOptions};

    fn test_request(language: &str) -> AiChatRequest {
        AiChatRequest {
            stream_id: None,
            session_id: None,
            connection_id: None,
            terminal_session_id: None,
            owner_scope: Default::default(),
            targets: vec![],
            target_contexts: vec![],
            mode: AiMode::Ask,
            agent_kind: crate::config::AiAgentKind::Nyaterm,
            permission_mode: crate::config::AiPermissionMode::Confirm,
            model_id: None,
            model_name: None,
            default_target_session_id: None,
            existing_external_session_id: None,
            attachments: vec![],
            action: AiAction::GenerateCommand,
            user_input: "check disk usage".to_string(),
            context: AiContext {
                connection_name: Some("prod".to_string()),
                host: Some("example.com".to_string()),
                username: Some("ops".to_string()),
                cwd: Some("/srv/app".to_string()),
                os: Some("Linux".to_string()),
                recent_output: "Filesystem Size Used Avail Use% Mounted on".to_string(),
                ..AiContext::default()
            },
            options: AiRequestOptions {
                language: language.to_string(),
                ..AiRequestOptions::default()
            },
        }
    }

    #[test]
    fn resolves_chinese_locale_variants_to_writing_systems() {
        let simplified_cases = ["zh", "zh-CN", "zh_CN", "zh-SG", "zh-Hans", "zh-Hans-CN"];
        for language in simplified_cases {
            assert_eq!(resolve_prompt_language(language), PromptLanguage::ZhHans);
        }

        let traditional_cases = ["zh-TW", "zh_TW", "zh-Hant", "zh-Hant-TW", "zh-HK", "zh-MO"];
        for language in traditional_cases {
            assert_eq!(resolve_prompt_language(language), PromptLanguage::ZhHant);
        }

        assert_eq!(resolve_prompt_language("zh-unknown"), PromptLanguage::En);
    }

    #[test]
    fn returns_traditional_chinese_system_prompts() {
        assert!(system_prompt("zh-TW").contains("雲端原生終端助手"));
        assert!(agent_system_prompt("zh-Hant").contains("終端自動化 Agent"));
    }

    #[test]
    fn builds_traditional_chinese_chat_and_agent_prompts() {
        let settings = AiSettings::default();
        let request = test_request("zh-TW");

        let prompt = build_prompt(&request, &settings);
        assert!(prompt.contains("任務：根據自然語言需求產生"));
        assert!(prompt.contains("目前連線情境："));
        assert!(prompt.contains("語言：zh-TW"));

        let agent_prompt = build_agent_prompt(&request, &settings);
        assert!(agent_prompt.contains("使用者任務："));
        assert!(agent_prompt.contains("最近終端輸出"));
        assert!(agent_prompt.contains("zh-TW"));
    }

    #[test]
    fn agent_prompts_require_non_interactive_commands() {
        let settings = AiSettings::default();
        for (language, localized_rule) in [
            ("zh-CN", "不能等待用户输入"),
            ("zh-TW", "不能等待使用者輸入"),
            ("en", "must be non-interactive"),
            ("ko", "비대화형이어야"),
        ] {
            let request = test_request(language);
            let prompt = build_agent_prompt(&request, &settings);

            assert!(prompt.contains(localized_rule), "language={language}");
            assert!(prompt.contains("git --no-pager"), "language={language}");
            assert!(
                prompt.contains("journalctl --no-pager"),
                "language={language}"
            );
            assert!(prompt.contains("final_answer"), "language={language}");
        }
    }

    #[test]
    fn builds_traditional_chinese_agent_runtime_messages() {
        assert!(agent_send_only_observation("zh-TW").contains("僅傳送模式"));
        assert!(agent_execution_disabled_message("zh-Hant").contains("已停用"));
        assert!(build_agent_rejected_message("rm -rf tmp", "zh-TW").contains("拒絕執行"));
        assert!(build_agent_failed_message("boom", "zh-TW").contains("下一步"));
        assert!(
            build_agent_unknown_action_message("noop", "fallback", "zh-TW").contains("fallback")
        );
        assert!(agent_max_steps_message("zh-TW").contains("最大步數限制"));
    }

    #[test]
    fn resolves_korean_locale_variants() {
        assert!(resolve_prompt_language("ko") == PromptLanguage::Ko);
        assert!(resolve_prompt_language("ko-KR") == PromptLanguage::Ko);
        assert!(resolve_prompt_language("ko_KR") == PromptLanguage::Ko);
    }

    #[test]
    fn returns_korean_system_prompts() {
        assert!(system_prompt("ko").contains("터미널 어시스턴트"));
        assert!(agent_system_prompt("ko").contains("터미널 자동화 Agent"));
    }

    #[test]
    fn builds_korean_chat_and_agent_prompts() {
        let settings = AiSettings::default();
        let request = test_request("ko");

        let prompt = build_prompt(&request, &settings);
        assert!(prompt.contains("작업: 자연어 요청에서 Shell 명령"));
        assert!(prompt.contains("현재 연결 컨텍스트:"));
        assert!(prompt.contains("대상 언어: ko"));

        let agent_prompt = build_agent_prompt(&request, &settings);
        assert!(agent_prompt.contains("사용자 작업:"));
        assert!(agent_prompt.contains("최근 터미널 출력"));
        assert!(agent_prompt.contains("ko"));
    }

    #[test]
    fn builds_korean_observation_message() {
        let obs = CommandObservation {
            output: "ok".to_string(),
            exit_code: Some(0),
            duration_ms: 42,
        };

        let message = build_observation_message(&obs, "ls", "ko-KR");
        assert!(message.contains("명령 `ls` 실행이 완료되었습니다"));
        assert!(message.contains("출력:"));
        assert!(message.contains("execute_command 또는 final_answer"));
    }

    #[test]
    fn builds_korean_agent_runtime_messages() {
        assert!(agent_send_only_observation("ko").contains("전송 전용 모드"));
        assert!(agent_execution_disabled_message("ko-KR").contains("비활성화"));
        assert!(build_agent_rejected_message("rm -rf tmp", "ko").contains("거부했습니다"));
        assert!(build_agent_failed_message("boom", "ko").contains("다음 단계"));
        assert!(build_agent_unknown_action_message("noop", "fallback", "ko").contains("fallback"));
        assert!(agent_max_steps_message("ko").contains("최대 단계 수"));
    }
}
