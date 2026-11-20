use std::collections::{HashMap, HashSet};

use gpui::{Context, KeyDownEvent};

use crate::features::NyaTermApp;
use crate::models::QuickCommandVariableDef;

impl NyaTermApp {
    pub(in crate::features) fn cancel_quick_command_variable_prompt(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.commands.clear_quick_variable_prompt();
        self.shell
            .set_status("quick command variables cancelled".to_string());
        cx.notify();
    }

    pub(in crate::features) fn submit_quick_command_variable_prompt(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(prompt) = self.commands.take_quick_variable_prompt() else {
            return;
        };
        let command_text = substitute_quick_command_variables(&prompt.command, &prompt.variables);
        self.send_resolved_quick_command_with_segmented(
            prompt.command_id,
            prompt.label,
            command_text,
            super::run::QuickCommandSendOptions {
                execute: prompt.execute,
                send_to_all: prompt.send_to_all,
                segmented_send: prompt.segmented_send.clone(),
            },
            cx,
        );
    }

    pub(in crate::features) fn handle_quick_command_variable_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        self.mark_user_activity();
        let keystroke = &event.keystroke;
        let primary = keystroke.modifiers.platform || keystroke.modifiers.control;
        if primary || keystroke.modifiers.alt || keystroke.modifiers.function {
            return false;
        }

        // Every field owns its own text, clipboard, and — for an option list — its
        // own arrow keys. What is left is the dialog's own two keys.
        match keystroke.key.as_str() {
            "escape" => self.cancel_quick_command_variable_prompt(cx),
            "enter" => self.submit_quick_command_variable_prompt(cx),
            _ => return false,
        }
        true
    }

    /// Apply an edit from one of the variable prompt's boxes.
    pub(in crate::features) fn apply_quick_command_variable(
        &mut self,
        index: usize,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if self.commands.set_quick_variable_value(index, text) {
            cx.notify();
        }
    }
}

/// Replace each `{{variable}}` token with the value the user filled in.
///
/// Values are data, not script: their backslashes are doubled so the escape
/// planner sends them literally. A filled-in `C:\temp` must not become a tab,
/// while the template's own `\p` still pauses.
pub(super) fn substitute_quick_command_variables(
    command_text: &str,
    variables: &[QuickCommandVariableDef],
) -> String {
    let mut command_text = command_text.to_string();
    for variable in variables {
        let value = variable.value.replace('\\', "\\\\");
        command_text = command_text.replace(&variable.raw, &value);
    }
    command_text
}

pub(super) fn parse_quick_command_variables(command: &str) -> Vec<QuickCommandVariableDef> {
    let mut variables = Vec::new();
    let mut seen = HashSet::<String>::new();
    let mut rest = command;
    while let Some(start) = rest.find("{{") {
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("}}") else {
            break;
        };
        let content = &after_start[..end];
        let raw = format!("{{{{{content}}}}}");
        rest = &after_start[end + 2..];
        if content.is_empty() || !seen.insert(raw.clone()) {
            continue;
        }

        let (name, options, value) = if content.contains('|') {
            let mut parts = content.split('|');
            let name = parts.next().unwrap_or_default();
            let options = parts.next().unwrap_or_default();
            let options = options
                .split(',')
                .map(str::trim)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            let value = options.first().cloned().unwrap_or_default();
            (name.trim().to_string(), options, value)
        } else if content.contains('=') {
            let mut parts = content.split('=');
            let name = parts.next().unwrap_or_default();
            let default_value = parts.next().unwrap_or_default();
            (
                name.trim().to_string(),
                Vec::new(),
                default_value.trim().to_string(),
            )
        } else {
            (content.trim().to_string(), Vec::new(), String::new())
        };
        variables.push(QuickCommandVariableDef {
            raw,
            name,
            options,
            value,
        });
    }
    let mut values_by_name = HashMap::new();
    for variable in &variables {
        values_by_name.insert(variable.name.clone(), variable.value.clone());
    }
    for variable in &mut variables {
        if let Some(value) = values_by_name.get(&variable.name) {
            variable.value = value.clone();
        }
    }
    variables
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::escapes::{plan_quick_command, quick_command_needs_staging};
    use super::{parse_quick_command_variables, substitute_quick_command_variables};

    #[test]
    fn parses_quick_command_variables_like_tauri_dialog() {
        let variables =
            parse_quick_command_variables("ssh {{ host }} --env {{mode=prod}} {{target|a, b,}}");

        assert_eq!(variables.len(), 3);
        assert_eq!(variables[0].raw, "{{ host }}");
        assert_eq!(variables[0].name, "host");
        assert_eq!(variables[0].value, "");
        assert!(variables[0].options.is_empty());
        assert_eq!(variables[1].raw, "{{mode=prod}}");
        assert_eq!(variables[1].name, "mode");
        assert_eq!(variables[1].value, "prod");
        assert_eq!(variables[2].raw, "{{target|a, b,}}");
        assert_eq!(variables[2].name, "target");
        assert_eq!(variables[2].options, ["a", "b", ""]);
        assert_eq!(variables[2].value, "a");
    }

    #[test]
    fn deduplicates_by_raw_token_and_keeps_js_split_semantics() {
        let variables = parse_quick_command_variables("{{x=a=b}} {{choice|one|two}} {{x=a=b}}");

        assert_eq!(variables.len(), 2);
        assert_eq!(variables[0].raw, "{{x=a=b}}");
        assert_eq!(variables[0].name, "x");
        assert_eq!(variables[0].value, "a");
        assert_eq!(variables[1].raw, "{{choice|one|two}}");
        assert_eq!(variables[1].name, "choice");
        assert_eq!(variables[1].options, ["one"]);
        assert_eq!(variables[1].value, "one");
    }

    #[test]
    fn shares_values_for_variables_with_the_same_name() {
        let variables = parse_quick_command_variables("{{host=prod}} {{host=dev}}");

        assert_eq!(variables.len(), 2);
        assert_eq!(variables[0].value, "dev");
        assert_eq!(variables[1].value, "dev");

        assert_eq!(variables[0].value, "dev");
        assert_eq!(variables[1].value, "dev");
    }

    #[test]
    fn variable_values_are_sent_literally_not_as_escapes() {
        // A filled-in Windows path must not lose its `\t` to the escape
        // planner: values are data, the template is the script.
        let mut variables = parse_quick_command_variables("cd {{path}}");
        variables[0].value = "C:\\temp".to_string();
        let text = substitute_quick_command_variables("cd {{path}}", &variables);
        let chunks = plan_quick_command(&text, true);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].bytes, b"cd C:\\temp\r".to_vec());
    }

    #[test]
    fn segmented_variable_paths_remain_data_when_the_template_is_a_script() {
        let mut variables = parse_quick_command_variables("cd {{path}}\nnext");
        variables[0].value = "C:\\temp".to_string();
        let text = substitute_quick_command_variables("cd {{path}}\nnext", &variables);
        let plan = nyaterm_core::send_command::SendCommandPlan::text(
            &text,
            nyaterm_core::send_command::SendCommandMode::Line,
            nyaterm_core::send_command::SendCommandLineEnding::Cr,
            nyaterm_core::send_command::TextInterpretation::Script,
            std::time::Duration::ZERO,
        );
        assert_eq!(
            plan.flat_map(|chunk| chunk.bytes).collect::<Vec<_>>(),
            b"cd C:\\temp\rnext\r"
        );
    }

    #[test]
    fn script_escapes_around_variables_still_apply() {
        let template = "\\p{{user}}\\p{{pass}}\\pshow version";
        let mut variables = parse_quick_command_variables(template);
        variables[0].value = "admin".to_string();
        variables[1].value = "p@ss\\word".to_string();
        let text = substitute_quick_command_variables(template, &variables);
        let chunks = plan_quick_command(&text, true);
        assert!(quick_command_needs_staging(&chunks));
        let sent: Vec<(Duration, Vec<u8>)> = chunks
            .iter()
            .map(|chunk| (chunk.delay, chunk.bytes.clone()))
            .collect();
        assert_eq!(
            sent,
            vec![
                (Duration::from_secs(1), b"admin".to_vec()),
                (Duration::from_secs(1), b"p@ss\\word".to_vec()),
                (Duration::from_secs(1), b"show version\r".to_vec()),
            ]
        );
    }
}
