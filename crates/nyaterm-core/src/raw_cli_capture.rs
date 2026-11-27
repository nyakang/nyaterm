//! Per-command capture from decoded session events, independent of UI retention.
use std::time::{Duration, Instant};

use crate::network_cli_prompt::{
    PromptLine, looks_like_cli_input_wait, looks_like_network_cli_prompt_line,
};
use crate::sql_cli_prompt::{SqlClient, classify_sql_prompt_line, sql_prompt};
use crate::terminal::shell_prompt::classify_shell_prompt;

// A command exceeding this budget is explicitly incomplete, never successful.
const OUTPUT_LIMIT: usize = 1024 * 1024;
const PROMPT_SETTLE: Duration = Duration::from_millis(400);
const UNKNOWN_IDLE: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawCliEnd {
    Prompt,
    Interaction,
    IdleUnconfirmed,
    TimedOut,
    Truncated,
}

impl RawCliEnd {
    pub fn description(self) -> &'static str {
        match self {
            Self::Prompt => "CLI prompt returned; command success and exit code are unknown",
            Self::Interaction => {
                "CLI is waiting for pagination or interactive input; review the terminal"
            }
            Self::IdleUnconfirmed => {
                "Output is idle but completion is unconfirmed; command may still be running"
            }
            Self::TimedOut => {
                "Command timed out without a confirmed CLI prompt; command may still be running"
            }
            Self::Truncated => {
                "Command output exceeded the capture limit and is incomplete; review the terminal"
            }
        }
    }
}

#[derive(Debug)]
enum ExpectedPrompt {
    Network(String),
    Sql(SqlClient),
}

#[derive(Debug)]
pub struct RawCliCapture {
    output: String,
    line: PromptLine,
    expected: Option<ExpectedPrompt>,
    last_output: Instant,
    received: bool,
    truncated: bool,
}

impl RawCliCapture {
    pub fn new(initial_output: &str, now: Instant) -> Self {
        let mut initial = PromptLine::default();
        // The caller supplies only a bounded tail.
        initial.feed(initial_output);
        let prompt = initial.text().trim().to_string();
        Self {
            output: String::new(),
            line: PromptLine::default(),
            expected: sql_prompt(initial_output)
                .map(|prompt| ExpectedPrompt::Sql(prompt.client))
                .or_else(|| {
                    looks_like_network_cli_prompt_line(&prompt)
                        .then_some(ExpectedPrompt::Network(prompt))
                }),
            last_output: now,
            received: false,
            truncated: false,
        }
    }

    pub fn feed(&mut self, text: &str, now: Instant) {
        if text.is_empty() {
            return;
        }
        self.received = true;
        self.last_output = now;
        let mut keep = text
            .len()
            .min(OUTPUT_LIMIT.saturating_sub(self.output.len()));
        while !text.is_char_boundary(keep) {
            keep -= 1;
        }
        self.output.push_str(&text[..keep]);
        self.truncated |= keep < text.len();
        // Once incomplete, there is no reason to scan further output for prompts.
        if !self.truncated {
            self.line.feed(text);
        }
    }

    pub fn poll(&self, now: Instant, deadline: Instant) -> Option<RawCliEnd> {
        if self.truncated {
            return Some(RawCliEnd::Truncated);
        }
        // A continuous stream must not postpone the overall deadline.
        if now >= deadline {
            return Some(RawCliEnd::TimedOut);
        }
        if !self.received || now.duration_since(self.last_output) < PROMPT_SETTLE {
            return None;
        }
        let observed = self.line.text();
        let line = observed.trim();
        if looks_like_cli_input_wait(line) {
            return Some(RawCliEnd::Interaction);
        }
        let returned = match &self.expected {
            Some(ExpectedPrompt::Sql(client)) => {
                if client.is_continuation(&self.line.current_text()) {
                    return Some(RawCliEnd::Interaction);
                }
                classify_sql_prompt_line(line)
                    .is_some_and(|prompt| prompt.client == *client && !prompt.continuation)
                    // An exit command returns control to the outer shell.
                    || classify_shell_prompt(line).is_some()
            }
            Some(ExpectedPrompt::Network(expected)) => {
                looks_like_network_cli_prompt_line(line) && same_device_prompt(expected, line)
            }
            None => false,
        };
        if returned {
            return Some(RawCliEnd::Prompt);
        }
        // Without a pre-command prompt to compare, even a prompt-shaped output
        // line is ambiguous. Idle is an escape with an explicit error outcome.
        if self.expected.is_none() && now.duration_since(self.last_output) >= UNKNOWN_IDLE {
            return Some(RawCliEnd::IdleUnconfirmed);
        }
        None
    }

    pub fn into_output(self) -> String {
        self.output
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated
    }
}

fn same_device_prompt(expected: &str, actual: &str) -> bool {
    if expected == actual {
        return true;
    }
    let bracket_body = |s: &str| {
        s.strip_prefix('<')
            .and_then(|s| s.strip_suffix('>'))
            .or_else(|| s.strip_prefix('[').and_then(|s| s.strip_suffix(']')))
            .map(str::to_owned)
    };
    if let (Some(host), Some(view)) = (bracket_body(expected), bracket_body(actual)) {
        return view == host
            || view
                .strip_prefix(&host)
                .is_some_and(|rest| rest.starts_with('-'));
    }
    let host = |s: &str| {
        s.trim_end_matches(['#', '>', '*'])
            .trim()
            .split('(')
            .next()
            .unwrap_or_default()
            .to_string()
    };
    let expected = host(expected);
    !expected.is_empty() && expected == host(actual)
}

#[cfg(test)]
mod tests {
    use super::{OUTPUT_LIMIT, RawCliCapture, RawCliEnd};
    use std::time::{Duration, Instant};

    #[test]
    fn sql_capture_accepts_ready_prompts_and_database_or_transaction_changes() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(60);
        for (initial, returned) in [
            ("SQL>", "SQL>"),
            ("sqlplus>", "sqlplus>"),
            ("RMAN>", "RMAN>"),
            ("mysql>", "mysql>"),
            ("sqlite>", "sqlite>"),
            ("postgres=#", "other_db=*>"),
            ("app_db=*>", "app_db=!>"),
            ("MariaDB [(none)]>", "MariaDB [app_db]>"),
            ("app_db->", "app_db=>"),
            ("mysql> select\r\n    -> ", "mysql>"),
        ] {
            let mut capture = RawCliCapture::new(initial, now);
            capture.feed("select 1;\r\n", now);
            assert_eq!(
                capture.poll(now + Duration::from_secs(9), deadline),
                None,
                "{initial}"
            );
            capture.feed("  1  \r\n", now + Duration::from_secs(10));
            assert_eq!(
                capture.poll(now + Duration::from_secs(11), deadline),
                None,
                "{initial}"
            );
            capture.feed("\x1b[3", now + Duration::from_secs(12));
            capture.feed(
                &format!("2m{returned}\x1b[0m "),
                now + Duration::from_secs(12),
            );
            assert_eq!(
                capture.poll(now + Duration::from_secs(13), deadline),
                Some(RawCliEnd::Prompt),
                "{initial}"
            );
            assert!(capture.into_output().contains("select 1;"));
        }
    }

    #[test]
    fn sql_continuations_need_input_and_are_not_successful_completion() {
        let now = Instant::now();
        for (initial, continuation) in [
            ("SQL>", "  2  "),
            ("RMAN>", "  2  "),
            ("mysql>", "    -> "),
            ("mysql>", "    '> "),
            ("MariaDB [app_db]>", "    /*> "),
            ("sqlite>", "   ...> "),
            ("app_db=>", "app_db->"),
            ("app_db=*>", "app_db'*>"),
            ("app_db=>", "app_db*>"),
        ] {
            let mut capture = RawCliCapture::new(initial, now);
            capture.feed(continuation, now);
            assert_eq!(
                capture.poll(now + Duration::from_secs(1), now + Duration::from_secs(30)),
                Some(RawCliEnd::Interaction),
                "{continuation}"
            );
        }
    }

    #[test]
    fn sql_exit_returns_to_shell_but_unrelated_prompts_and_echo_do_not_complete() {
        let now = Instant::now();
        for shell in ["user@host:~$ ", "PS C:\\Users> ", "C:\\Users>"] {
            let mut capture = RawCliCapture::new("postgres=#", now);
            capture.feed(&format!("\\q\r\n{shell}"), now);
            assert_eq!(
                capture.poll(now + Duration::from_secs(1), now + Duration::from_secs(30)),
                Some(RawCliEnd::Prompt),
                "{shell}"
            );
        }
        for output in ["SQL> select 1;", "Router#", "postgres=#", "SQL>\rworking"] {
            let mut capture = RawCliCapture::new("SQL>", now);
            capture.feed(output, now);
            assert_eq!(
                capture.poll(now + Duration::from_secs(9), now + Duration::from_secs(30)),
                None,
                "{output}"
            );
        }
    }

    #[test]
    fn pauses_and_echo_do_not_complete_known_cli() {
        let now = Instant::now();
        let mut capture = RawCliCapture::new("Router#", now);
        capture.feed("Router#show version\r\n", now);
        let deadline = now + Duration::from_secs(60);
        assert_eq!(capture.poll(now + Duration::from_secs(10), deadline), None);
        capture.feed(
            "version info\r\nRouter(config)#",
            now + Duration::from_secs(11),
        );
        assert_eq!(
            capture.poll(now + Duration::from_secs(12), deadline),
            Some(RawCliEnd::Prompt)
        );
        assert!(capture.into_output().contains("version info"));
    }

    #[test]
    fn pagination_and_confirmation_are_not_completion() {
        for text in [
            "---- More ----",
            "--More--",
            "Continue? [Y/N]",
            "Password:",
            "[confirm]",
        ] {
            let now = Instant::now();
            let mut capture = RawCliCapture::new("<Switch>", now);
            capture.feed(text, now);
            assert_eq!(
                capture.poll(now + Duration::from_secs(1), now + Duration::from_secs(30)),
                Some(RawCliEnd::Interaction)
            );
        }
    }

    #[test]
    fn events_survive_ui_cap_but_capture_cap_is_incomplete() {
        let now = Instant::now();
        let mut capture = RawCliCapture::new("<Switch>", now);
        capture.feed(&"x".repeat(OUTPUT_LIMIT - 40), now);
        capture.feed("\r\n<Switch>", now);
        assert_eq!(
            capture.poll(now + Duration::from_secs(1), now + Duration::from_secs(30)),
            Some(RawCliEnd::Prompt)
        );
        capture.feed(&"界".repeat(40), now);
        assert_eq!(
            capture.poll(now + Duration::from_secs(1), now + Duration::from_secs(30)),
            Some(RawCliEnd::Truncated)
        );
        assert!(capture.into_output().len() <= OUTPUT_LIMIT);
    }

    #[test]
    fn no_output_and_continuous_output_respect_deadline() {
        let now = Instant::now();
        let mut capture = RawCliCapture::new("<Switch>", now);
        let deadline = now + Duration::from_secs(2);
        assert_eq!(capture.poll(now + Duration::from_secs(1), deadline), None);
        capture.feed("still running", deadline);
        assert_eq!(capture.poll(deadline, deadline), Some(RawCliEnd::TimedOut));
    }

    #[test]
    fn unknown_prompt_idle_is_explicitly_unconfirmed() {
        let now = Instant::now();
        let mut capture = RawCliCapture::new("unknown CLI", now);
        capture.feed("command echo", now);
        assert_eq!(
            capture.poll(now + Duration::from_secs(9), now + Duration::from_secs(30)),
            Some(RawCliEnd::IdleUnconfirmed)
        );
    }

    #[test]
    fn prompt_shaped_output_without_a_baseline_is_not_confirmed() {
        let now = Instant::now();
        let mut capture = RawCliCapture::new("", now);
        capture.feed("Router#", now);
        assert_eq!(
            capture.poll(now + Duration::from_secs(1), now + Duration::from_secs(30)),
            None
        );
        assert_eq!(
            capture.poll(now + Duration::from_secs(9), now + Duration::from_secs(30)),
            Some(RawCliEnd::IdleUnconfirmed)
        );
    }

    #[test]
    fn split_ansi_prompt_and_mode_change_without_command_output() {
        let now = Instant::now();
        let mut capture = RawCliCapture::new("<Switch>", now);
        capture.feed("\u{1b}[3", now);
        capture.feed("2m[Switch-interface]\u{1b}[0m", now);
        assert_eq!(
            capture.poll(now + Duration::from_secs(1), now + Duration::from_secs(30)),
            Some(RawCliEnd::Prompt)
        );
    }

    #[test]
    fn unrelated_prompt_and_overwritten_prompt_are_not_completion() {
        let now = Instant::now();
        for output in ["Other#", "Router#\rworking"] {
            let mut capture = RawCliCapture::new("Router#", now);
            capture.feed(output, now);
            assert_eq!(
                capture.poll(now + Duration::from_secs(1), now + Duration::from_secs(30)),
                None
            );
        }
    }
}
