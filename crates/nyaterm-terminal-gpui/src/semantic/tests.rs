use std::sync::Arc;

use nyaterm_core::keyword_highlight_presets::{
    builtin_keyword_rule_swatch, get_builtin_keyword_rules,
};
use nyaterm_terminal::{ShellInputLineKind, TerminalScreen, TerminalSnapshot};

use super::{SemanticContext, SemanticHighlighter};
use crate::keywords::{
    compile_terminal_keyword_highlighter, parse_hex_rgb, precompute_terminal_keyword_highlights,
    precompute_terminal_keyword_highlights_for_rows_with_stats,
    precompute_terminal_keyword_highlights_for_rows_with_stats_and_cancel,
};
use crate::paint::terminal_highlight_spans_with_keyword_ranges_and_options;
use crate::types::TerminalKeywordRange;

fn swatch(id: &str) -> u32 {
    parse_hex_rgb(builtin_keyword_rule_swatch(id, true)).unwrap()
}

#[test]
fn single_snapshot_colours_the_input_row_and_the_output_row() {
    // The input row is no longer a separate overlay: one snapshot covers both the
    // submitted line and the completion output under it.
    let highlighter = compile_terminal_keyword_highlighter(
        &get_builtin_keyword_rules(true),
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    let mut screen = TerminalScreen::new(40, 3);
    screen.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\ncompletion list");
    let snapshot = screen.snapshot();
    let highlights = precompute_terminal_keyword_highlights(
        &snapshot,
        &highlighter,
        nyaterm_ui::theme_palette("github-dark"),
        None,
    );
    assert!(
        highlights.lookup(0, &snapshot).is_some(),
        "input row coloured"
    );
    assert!(
        highlights.lookup(1, &snapshot).is_some(),
        "output row coloured by the same snapshot"
    );
}

#[test]
fn bracketed_prompt_detection_requires_a_host_and_an_adjacent_sign() {
    for line in [
        "[root@host dir]# ",
        "[root@host dir]$ ls",
        "  [root@host ~]% ",
    ] {
        assert!(super::fallback_prompt(line).is_some());
    }
    for line in [
        "[INFO message]# text",
        "[root@host dir] # text",
        "[root@host dir]#text",
        "[@host dir]# ",
        "[root@ dir]# ",
    ] {
        assert!(super::fallback_prompt(line).is_none());
    }
}

fn colors(line: &str, input: bool) -> Vec<Option<u32>> {
    let highlighter = SemanticHighlighter::compile(
        &get_builtin_keyword_rules(true),
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    let context = SemanticContext {
        integrated: true,
        input: if input {
            std::iter::once(0..line.len()).collect()
        } else {
            Vec::new()
        },
        ..SemanticContext::default()
    };
    let mut colors = vec![None; line.len()];
    for (start, end, color) in highlighter.ranges(line, &context, &[]) {
        colors[start..end].fill(Some(color));
    }
    colors
}

fn assert_token(line: &str, token: &str, id: &str, input: bool) {
    let start = line.find(token).unwrap();
    assert!(
        colors(line, input)[start..start + token.len()]
            .iter()
            .all(|color| *color == Some(swatch(id))),
        "{line:?}: {token:?} should be {id}"
    );
}

#[test]
fn typed_command_colours_without_integration() {
    // A plain bash prompt with no OSC 133: the typed line must still be coloured
    // (prompt + command), otherwise the input row looks plain white while the
    // submitted copy above it is green.
    let highlighter = SemanticHighlighter::compile(
        &get_builtin_keyword_rules(false),
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-light").terminal_ansi,
        },
    );
    let context = SemanticContext {
        integrated: false,
        ..SemanticContext::default()
    };
    let line = "[root@nginx-wan nginx-wan]# ls -lR";
    let ranges: Vec<(usize, usize, u32)> = highlighter.ranges(line, &context, &[]);
    let prompt_at = 0;
    let command_at = line.find("ls").expect("command");
    assert!(
        ranges
            .iter()
            .any(|(s, e, _)| *s <= prompt_at && *e > prompt_at),
        "prompt should be coloured: {ranges:?}"
    );
    assert!(
        ranges
            .iter()
            .any(|(s, e, _)| *s <= command_at && *e > command_at),
        "command should be coloured: {ranges:?}"
    );
}

#[test]
fn typed_command_colours_when_integration_lacks_input_columns() {
    // bash marked the prompt (integration on) but never published the command
    // start, so `context.input` is empty. The prompt fallback must still run.
    let highlighter = SemanticHighlighter::compile(
        &get_builtin_keyword_rules(false),
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-light").terminal_ansi,
        },
    );
    let context = SemanticContext {
        integrated: true,
        input_anchor_missing: true,
        ..SemanticContext::default()
    };
    let line = "[root@nginx-wan ~]# ll";
    let ranges: Vec<(usize, usize, u32)> = highlighter.ranges(line, &context, &[]);
    let command_at = line.find("ll").expect("command");
    assert!(
        ranges.iter().any(|(s, e, _)| *s == 0 && *e > 0),
        "prompt should be coloured: {ranges:?}"
    );
    assert!(
        ranges
            .iter()
            .any(|(s, e, _)| *s <= command_at && *e > command_at),
        "command should be coloured: {ranges:?}"
    );
}

#[test]
fn brackets_and_pipes_are_coloured() {
    let line = "nginx-ui | [0.106ms] [rows:1] SELECT * FROM sites";
    let highlighter = SemanticHighlighter::compile(
        &get_builtin_keyword_rules(false),
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-light").terminal_ansi,
        },
    );
    let context = SemanticContext {
        integrated: false,
        ..SemanticContext::default()
    };
    let ranges = highlighter.ranges(line, &context, &[]);
    for token in ["|", "[", "]"] {
        let pos = line.find(token).expect("token");
        assert!(
            ranges.iter().any(|(s, e, _)| *s <= pos && *e > pos),
            "{token:?} should be coloured: {ranges:?}"
        );
    }
}

#[test]
fn docker_compose_header_stays_plain() {
    let line = "NAME                IMAGE               COMMAND                  SERVICE   CREATED   STATUS    PORTS";
    let highlighter = SemanticHighlighter::compile(
        &get_builtin_keyword_rules(false),
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-light").terminal_ansi,
        },
    );
    let context = SemanticContext {
        integrated: false,
        ..SemanticContext::default()
    };
    let ranges = highlighter.ranges(line, &context, &[]);
    assert!(ranges.is_empty(), "header should stay plain: {ranges:?}");
}

#[test]
fn disabling_line_roles_preserves_generic_and_user_highlights() {
    let line = "-rw-r--r-- 1 root root 123 Oct 30 10:00 ERROR.txt";
    let config = crate::keywords::TerminalKeywordHighlightConfig::default();
    // Isolate the output-only month colour from overlapping generic date rules.
    let rules: Vec<_> = get_builtin_keyword_rules(true)
        .into_iter()
        .filter(|rule| matches!(rule.id.as_str(), "builtin-month" | "builtin-error"))
        .collect();
    let enabled = SemanticHighlighter::compile(&rules, config);
    let disabled = SemanticHighlighter::compile(
        &rules,
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: false,
            ..config
        },
    );
    let context = SemanticContext::default();
    let root = line.find("root").unwrap();
    let month = line.find("Oct").unwrap();
    let error = line.find("ERROR").unwrap();
    let user = [(root, root + 4, 0xabcdef)];
    let on = enabled.ranges(line, &context, &user);
    let off = disabled.ranges(line, &context, &user);
    assert!(on.iter().any(|(start, end, color)| {
        *start == month && *end == month + 3 && *color == swatch("builtin-month")
    }));
    assert!(
        !off.iter()
            .any(|(start, end, _)| *start < month + 3 && *end > month)
    );
    assert!(off.contains(&(error, error + 5, swatch("builtin-error"))));
    assert!(on.contains(&user[0]));
    assert!(off.contains(&user[0]));
}

#[test]
fn shell_variables_colour_without_integration() {
    for line_roles in [false, true] {
        let line = "echo $HOME ${PATH}";
        let highlighter = SemanticHighlighter::compile(
            &get_builtin_keyword_rules(false),
            crate::keywords::TerminalKeywordHighlightConfig {
                line_roles,
                ansi: nyaterm_ui::theme_palette("github-light").terminal_ansi,
            },
        );
        let context = SemanticContext {
            integrated: false,
            ..SemanticContext::default()
        };
        let ranges = highlighter.ranges(line, &context, &[]);
        let home = line.find("$HOME").unwrap();
        let path = line.find("${PATH}").unwrap();
        assert!(
            ranges.iter().any(|(s, e, _)| *s <= home && *e >= home + 5),
            "$HOME should be coloured: {ranges:?}"
        );
        assert!(
            ranges.iter().any(|(s, e, _)| *s <= path && *e >= path + 7),
            "${{PATH}} should be coloured: {ranges:?}"
        );
    }
}

#[test]
fn line_roles_work_without_shell_integration() {
    // Telnet sessions have no OSC 133, so `integrated` stays false. The role
    // path must not depend on shell integration.
    let highlighter = SemanticHighlighter::compile(
        &get_builtin_keyword_rules(true),
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    let context = SemanticContext {
        integrated: false,
        ..SemanticContext::default()
    };
    for (line, token) in [
        ("vlan 1", "vlan"),
        ("vlan 1", "1"),
        (" dhcp server ip-pool 2000", "dhcp"),
        (" description agv", "description"),
    ] {
        let start = line.find(token).expect("token");
        let ranges = highlighter.ranges(line, &context, &[]);
        assert!(
            ranges
                .iter()
                .any(|(s, e, _)| *s <= start && *e >= start + token.len()),
            "{token:?} should be coloured in {line:?} without shell integration: {ranges:?}"
        );
    }
}

#[test]
fn line_roles_colour_socket_state_and_disk_usage() {
    let highlighter = SemanticHighlighter::compile(
        &get_builtin_keyword_rules(true),
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    let context = SemanticContext {
        integrated: true,
        ..SemanticContext::default()
    };
    for (line, token) in [
        (
            "udp  UNCONN 0      0          127.0.0.1:323          0.0.0.0:*",
            "UNCONN",
        ),
        (
            "tcp  LISTEN 0      4096       127.0.0.1:631          0.0.0.0:*",
            "LISTEN",
        ),
        ("/dev/sda1       100G   60G   40G  92% /", "92%"),
    ] {
        let start = line.find(token).expect("token");
        let ranges = highlighter.ranges(line, &context, &[]);
        assert!(
            ranges
                .iter()
                .any(|(s, e, _)| *s <= start && *e >= start + token.len()),
            "{token:?} should be coloured in {line:?}: {ranges:?}"
        );
    }
}

#[test]
fn docker_names_do_not_highlight_embedded_options_or_hyphens() {
    for name in [
        "nyaterm-log-ticker",
        "macos-monterey",
        "kanghub-frontend-1",
        "kanghub-redis-1",
        "prefix--flag",
        "界面-name",
    ] {
        assert!(colors(name, false).iter().all(Option::is_none), "{name}");
    }
}

#[test]
fn output_options_require_a_standalone_start_and_keep_help_flags() {
    let cases: &[(&str, &[&str])] = &[
        (
            "Sort entries alphabetically if none of -cftuvSUX nor --sort is specified.",
            &["-cftuvSUX", "--sort"],
        ),
        (
            "  -a, --all  do not ignore entries starting with .",
            &["-a", "--all"],
        ),
        (
            "  -A, --almost-all  do not list implied . and ..",
            &["-A", "--almost-all"],
        ),
        (
            "  --author  with -l, print the author of each file",
            &["--author", "-l"],
        ),
        ("  -b, --escape  print C-style escapes", &["-b", "--escape"]),
        ("[-a|--all]", &["-a", "--all"]),
        ("--color=always", &["--color"]),
        (" -single-dash-option ", &["-single-dash-option"]),
    ];
    for (line, flags) in cases {
        for flag in *flags {
            assert_token(line, flag, "builtin-option", false);
        }
    }
    let rules: Vec<_> = get_builtin_keyword_rules(true)
        .into_iter()
        .filter(|r| r.id == "builtin-option")
        .collect();
    let options = SemanticHighlighter::compile(
        &rules,
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    for text in ["foo-bar", "foo--bar", "file/-flag", "key=-flag", "(--flag)"] {
        assert!(
            options
                .matches(text, &SemanticContext::default())
                .is_empty(),
            "{text}"
        );
    }
    assert_token("a - b", "-", "builtin-operator", false);
    let mut rules = get_builtin_keyword_rules(true);
    rules
        .iter_mut()
        .find(|r| r.id == "builtin-option")
        .unwrap()
        .enabled = false;
    let disabled = SemanticHighlighter::compile(
        &rules,
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    let matches = disabled.ranges("--all", &SemanticContext::default(), &[]);
    assert_eq!(matches, vec![(0, 2, swatch("builtin-operator"))]);
}

#[test]
fn output_option_boundaries_use_logical_lines_across_soft_wraps() {
    let mut screen = TerminalScreen::new(8, 8);
    screen.advance(b"nyaterm-log-ticker\r\n-cftuvSUX --all");
    let result = snapshot_colors(&screen.snapshot());
    assert!(result[..18].iter().all(Option::is_none));
    assert!(
        result[24..33]
            .iter()
            .all(|color| *color == Some(swatch("builtin-option")))
    );
    assert!(
        result[34..39]
            .iter()
            .all(|color| *color == Some(swatch("builtin-option")))
    );
}

#[test]
fn permission_tokens_are_validated_before_character_decoration() {
    for token in [
        "drwxr-xr-x",
        "-rw-r--r--",
        "lrwxrwxrwx",
        "drwxr-xr-x+",
        "drwxr-xr-x.",
        "drwxrwxrwt",
        "drwxrwxrwT",
        "-rwsr-xr-x",
        "-rwSr-Sr-x",
    ] {
        let line = format!("prefix {token} 18 root root");
        let result = colors(&line, false);
        for (index, byte) in token.bytes().enumerate() {
            let id = match byte {
                b'-' | b'.' | b'+' => "builtin-operator",
                _ if index == 0 => "builtin-debug",
                b'r' => "builtin-info",
                b'w' => "builtin-warn",
                _ => "builtin-error",
            };
            assert_eq!(
                result[7 + index],
                Some(swatch(id)),
                "{token}, column {index}"
            );
        }
    }
    for invalid in [
        "rwx",
        "adrwxr-xr-x",
        "drwxr-xr-xsuffix",
        "drwxr-xr-x,",
        "dwwxr-xr-x",
    ] {
        assert!(
            !colors(invalid, false).contains(&Some(swatch("builtin-debug"))),
            "{invalid}"
        );
    }
}

#[test]
fn listing_datetime_and_numbers_keep_windterm_colors_without_single_digit_noise() {
    let line = "drwxr-xr-x 18 root root 4096 Oct 7 21:37 .git";
    for number in ["18", "4096"] {
        assert_token(line, number, "builtin-number", false);
    }
    for date in ["Oct", "21:37"] {
        assert_token(line, date, "builtin-datetime", false);
    }
    assert_eq!(colors(line, false)[line.find(" 7 ").unwrap() + 1], None);
}

#[test]
fn ping_sequence_counters_highlight_single_digits_without_coloring_the_field_name() {
    for dark in [true, false] {
        let highlighter = SemanticHighlighter::compile(
            &get_builtin_keyword_rules(dark),
            crate::keywords::TerminalKeywordHighlightConfig {
                line_roles: true,
                ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
            },
        );
        let color = parse_hex_rgb(builtin_keyword_rule_swatch("builtin-number", dark)).unwrap();
        for sequence in 0..=10 {
            let line = format!("64 bytes from 1.1.1.1: icmp_seq={sequence} ttl=51 time=74.1 ms");
            let start = line.find("icmp_seq=").unwrap() + "icmp_seq=".len();
            let end = start + sequence.to_string().len();
            let ranges = highlighter.ranges(&line, &SemanticContext::default(), &[]);
            assert!(ranges.contains(&(start, end, color)), "{line}, dark={dark}");
            assert!(ranges.iter().all(|&(left, right, _)| {
                right <= start - "icmp_seq=".len() || left >= start - 1
            }));
        }
        assert!(
            highlighter
                .ranges("0 1 7 9", &SemanticContext::default(), &[])
                .is_empty()
        );
    }
}

#[test]
fn ping_sequence_single_digit_exception_respects_boundaries_context_and_number_switch() {
    let rules: Vec<_> = get_builtin_keyword_rules(true)
        .into_iter()
        .filter(|rule| rule.id == "builtin-number")
        .collect();
    let highlighter = SemanticHighlighter::compile(
        &rules,
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    for line in [
        "prefixicmp_seq=1",
        "prefix_icmp_seq=1",
        "prefix-icmp_seq=1",
        "icmp_seq=1x",
        "icmp_seq=1_",
        "icmp_seq=1-worker",
        "icmp_seq=-1",
        "icmp_seq=+1",
        "icmp_seq=",
        "icmp_seq=1\u{301}",
        "icmp_seq=\n1",
    ] {
        assert!(
            highlighter
                .ranges(line, &SemanticContext::default(), &[])
                .is_empty(),
            "{line:?}"
        );
    }
    let line = "icmp_seq=1";
    for context in [
        SemanticContext {
            integrated: true,
            input: std::iter::once(0..line.len()).collect(),
            ..SemanticContext::default()
        },
        SemanticContext {
            integrated: true,
            prompt: std::iter::once(0..line.len()).collect(),
            ..SemanticContext::default()
        },
    ] {
        assert!(highlighter.ranges(line, &context, &[]).is_empty());
    }
    assert!(
        highlighter
            .ranges("$ echo icmp_seq=1", &SemanticContext::default(), &[])
            .is_empty()
    );
    assert_eq!(
        highlighter.ranges(line, &SemanticContext::default(), &[(9, 10, 0x123456)]),
        vec![(9, 10, 0x123456)]
    );

    let mut disabled = rules;
    disabled[0].enabled = false;
    assert!(
        SemanticHighlighter::compile(
            &disabled,
            crate::keywords::TerminalKeywordHighlightConfig {
                line_roles: true,
                ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi
            }
        )
        .ranges(line, &SemanticContext::default(), &[])
        .is_empty()
    );
    for (token, id) in [
        ("\"icmp_seq=1\"", "builtin-string"),
        ("https://example.com/?icmp_seq=1", "builtin-url"),
    ] {
        assert_token(token, token, id, false);
    }
}

#[test]
fn ping_sequence_context_survives_soft_wraps_without_crossing_hard_newlines() {
    let mut screen = TerminalScreen::new(8, 4);
    screen.advance(b"icmp_seq=1");
    let result = snapshot_colors(&screen.snapshot());
    assert_eq!(result[9], Some(swatch("builtin-number")));

    let mut screen = TerminalScreen::new(16, 4);
    screen.advance(b"icmp_seq=\r\n1");
    assert!(
        snapshot_colors(&screen.snapshot())
            .iter()
            .all(|color| *color != Some(swatch("builtin-number")))
    );
}

#[test]
fn numeric_literals_and_durations_keep_their_complete_ranges_in_both_themes() {
    for dark in [true, false] {
        let highlighter = SemanticHighlighter::compile(
            &get_builtin_keyword_rules(dark),
            crate::keywords::TerminalKeywordHighlightConfig {
                line_roles: true,
                ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
            },
        );
        for (id, tokens) in [
            (
                "builtin-number",
                &[
                    "0%", "5%", "-5%", "12.5 %", "-12", "+12", ".5", "-.5", "12.", "1e-3",
                    "-1.2E+3", "-0xFF", "5\t%", "5\u{a0}%",
                ][..],
            ),
            (
                "builtin-duration",
                &[
                    "-12ms",
                    ".5 ms",
                    "-.5ms",
                    "74.1 ms",
                    "17018ms",
                    "1e-3 ms",
                    "12 minutes",
                    "12\tµs",
                    "12\u{a0}ms",
                ][..],
            ),
        ] {
            let color = parse_hex_rgb(builtin_keyword_rule_swatch(id, dark)).unwrap();
            for token in tokens {
                assert_eq!(
                    highlighter.ranges(token, &SemanticContext::default(), &[]),
                    vec![(0, token.len(), color)],
                    "{token:?}, dark={dark}"
                );
            }
        }
    }
}

#[test]
fn numeric_boundaries_reject_names_and_incomplete_literals_without_single_digit_noise() {
    let rules: Vec<_> = get_builtin_keyword_rules(true)
        .into_iter()
        .filter(|rule| matches!(rule.id.as_str(), "builtin-number" | "builtin-duration"))
        .collect();
    let highlighter = SemanticHighlighter::compile(
        &rules,
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    for token in [
        "0",
        "7",
        "-5",
        "+3",
        "worker-123",
        "123-worker",
        "foo123",
        "id_123",
        "123_id",
        "file.123",
        "0x12GG",
        "0x",
        "12e",
        "12e+",
        "12e--34",
        "12e+34G",
        "12.34.56",
        "12e+34.5",
        "x.5ms",
        "123ms_extra",
        "123ms-worker",
        "界123",
        "123\u{301}",
        "5\n%",
        "5\r%",
        "5\u{2028}%",
        "5\nms",
    ] {
        assert!(
            highlighter
                .matches(token, &SemanticContext::default())
                .is_empty(),
            "{token:?}"
        );
    }
}

#[test]
fn operators_yield_to_signed_numeric_tokens_and_respect_category_switches() {
    let highlighter = SemanticHighlighter::compile(
        &get_builtin_keyword_rules(true),
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    for (line, start, end, id) in [
        ("value=-12", 6, 9, "builtin-number"),
        ("(-12)", 1, 4, "builtin-number"),
        ("value=+.5", 6, 9, "builtin-number"),
        ("time=-.5ms", 5, 10, "builtin-duration"),
    ] {
        let ranges = highlighter.ranges(line, &SemanticContext::default(), &[]);
        assert!(
            ranges.contains(&(start, end, swatch(id))),
            "{line}: {ranges:?}"
        );
        assert!(
            ranges
                .iter()
                .filter(|range| range.2 == swatch("builtin-operator"))
                .all(|&(left, right, _)| right <= start || left >= end),
            "{line}"
        );
    }

    for (number, duration) in [(false, true), (true, false), (false, false)] {
        let mut rules = get_builtin_keyword_rules(true);
        for rule in &mut rules {
            match rule.id.as_str() {
                "builtin-number" => rule.enabled = number,
                "builtin-duration" => rule.enabled = duration,
                _ => {}
            }
        }
        let highlighter = SemanticHighlighter::compile(
            &rules,
            crate::keywords::TerminalKeywordHighlightConfig {
                line_roles: true,
                ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
            },
        );
        for (token, enabled, id) in [
            ("-12", number, "builtin-number"),
            ("-.5ms", duration, "builtin-duration"),
        ] {
            let line = format!("value={token}");
            let ranges = highlighter.ranges(&line, &SemanticContext::default(), &[]);
            if enabled {
                assert!(ranges.contains(&(6, line.len(), swatch(id))), "{line}");
            } else {
                assert_eq!(ranges, vec![(5, 7, swatch("builtin-operator"))], "{line}");
            }
        }
    }
}

#[test]
fn ping_ps_and_ls_keep_specialized_tokens_and_existing_number_colors() {
    let ping = [
        "PING one.one.one.one (1.1.1.1) 56(84) bytes of data.",
        "64 bytes from 1.1.1.1: icmp_seq=18 ttl=51 time=74.1 ms",
        "--- one.one.one.one ping statistics ---",
        "18 packets transmitted, 18 received, 0% packet loss, time 17018ms",
        "rtt min/avg/max/mdev = 73.289/74.141/76.057/0.605 ms",
    ];
    assert_token(ping[0], "1.1.1.1", "builtin-address", false);
    for (token, id) in [
        ("64 bytes", "builtin-size"),
        ("1.1.1.1", "builtin-address"),
        ("18", "builtin-number"),
        ("51", "builtin-number"),
        ("74.1 ms", "builtin-duration"),
    ] {
        assert_token(ping[1], token, id, false);
    }
    for (token, id) in [
        ("18", "builtin-number"),
        ("0%", "builtin-number"),
        ("17018ms", "builtin-duration"),
    ] {
        assert_token(ping[3], token, id, false);
    }
    // RTT remains individual numbers in P0; the final value owns its unit.
    for token in ["73.289", "74.141", "76.057"] {
        assert_token(ping[4], token, "builtin-number", false);
    }
    assert_token(ping[4], "0.605 ms", "builtin-duration", false);

    let ps = "root 3509713 10721 0 Oct22 ? 00:00:00 /usr/bin/worker-123";
    for token in ["3509713", "10721"] {
        assert_token(ps, token, "builtin-number", false);
    }
    assert_token(ps, "00:00:00", "builtin-datetime", false);
    let listing = "-rw-r--r-- 1 root root 12345 Oct 7 21:37 file-123";
    assert_token(listing, "12345", "builtin-number", false);
    assert_token(listing, "Oct", "builtin-datetime", false);
    assert_token(listing, "21:37", "builtin-datetime", false);
    assert!(
        colors(listing, false)[listing.find("file-123").unwrap()..]
            .iter()
            .all(Option::is_none)
    );

    let mut screen = TerminalScreen::new(96, 8);
    screen.advance(ping.join("\r\n").as_bytes());
    let snapshot = screen.snapshot();
    let highlighter = compile_terminal_keyword_highlighter(
        &get_builtin_keyword_rules(true),
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    let palette = nyaterm_ui::theme_palette("github-dark");
    let previous = precompute_terminal_keyword_highlights(&snapshot, &highlighter, palette, None);
    let (_, stats) = precompute_terminal_keyword_highlights_for_rows_with_stats(
        &snapshot,
        &highlighter,
        palette,
        Some(&previous),
        0..snapshot.row_count(),
    );
    assert_eq!(stats.processed_bytes, 0);
    assert_eq!(stats.reused_rows, snapshot.row_count());
}

#[test]
fn signed_numbers_preserve_whole_token_conflicts_with_specialized_and_user_rules() {
    for (token, id) in [
        ("https://example.com/-12?a=5%25", "builtin-url"),
        ("v12.34.56", "builtin-version"),
        ("2026-10-22T12:34:56-05:00", "builtin-datetime"),
        ("192.168.12.34", "builtin-address"),
        ("550e8400-e29b-41d4-a716-446655440000", "builtin-uuid"),
        ("\"value=-12.5%\"", "builtin-string"),
    ] {
        let highlighter = SemanticHighlighter::compile(
            &get_builtin_keyword_rules(true),
            crate::keywords::TerminalKeywordHighlightConfig {
                line_roles: true,
                ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
            },
        );
        assert_eq!(
            highlighter.ranges(token, &SemanticContext::default(), &[]),
            vec![(0, token.len(), swatch(id))],
            "{token}"
        );
    }
    let highlighter = SemanticHighlighter::compile(
        &get_builtin_keyword_rules(true),
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    let context = SemanticContext::default();
    assert_eq!(
        highlighter.ranges("-12", &context, &[(0, 3, 0x123456)]),
        vec![(0, 3, 0x123456)]
    );
    assert_eq!(
        highlighter.ranges("-12", &context, &[(1, 3, 0x123456)]),
        vec![(0, 3, swatch("builtin-number"))]
    );
    assert_eq!(
        highlighter.ranges("value=-12", &context, &[(0, 9, 0x123456)]),
        vec![(0, 9, 0x123456)]
    );
}

#[test]
fn signed_percentages_map_across_soft_wraps_without_matching_hard_newlines() {
    let mut screen = TerminalScreen::new(4, 8);
    screen.advance(b"value=-12.5%");
    let result = snapshot_colors(&screen.snapshot());
    assert!(
        result[6..12]
            .iter()
            .all(|color| *color == Some(swatch("builtin-number")))
    );

    let mut screen = TerminalScreen::new(4, 8);
    screen.advance(b"5\r\n%");
    assert!(
        snapshot_colors(&screen.snapshot())
            .iter()
            .all(|color| *color != Some(swatch("builtin-number")))
    );
}

#[test]
fn shell_commands_separators_assignments_paths_and_options_are_contextual() {
    let line = "LANG=C cd /tmp && ls -la | grep error; cat ./Cargo.toml || vim ~/.ssh/config";
    for command in ["cd", "ls", "grep", "cat", "vim"] {
        assert_token(line, command, "builtin-command", true);
    }
    for path in ["/tmp", "./Cargo.toml", "~/.ssh/config"] {
        assert_token(line, path, "builtin-path", true);
    }
    assert_token(line, "-la", "builtin-option", true);
    assert_token("./app /tmp/2026-10-07", "./app", "builtin-command", true);
    assert_token(
        "./app /tmp/2026-10-07",
        "/tmp/2026-10-07",
        "builtin-path",
        true,
    );
    assert_token("echo true", "true", "builtin-constant", true);
    let start = line.find("error").unwrap();
    assert!(
        colors(line, true)[start..start + 5]
            .iter()
            .all(Option::is_none)
    );
    assert!(
        !colors("ordinary output /tmp ./Cargo.toml", false).contains(&Some(swatch("builtin-path")))
    );
}

#[test]
fn urls_specialized_tokens_and_option_values_beat_generic_punctuation_and_numbers() {
    let input = "curl --connect-timeout=5 --color=always https://example.com/a/b";
    assert_token(input, "curl", "builtin-command", true);
    assert_token(input, "--connect-timeout", "builtin-option", true);
    assert_token(input, "5", "builtin-info", true);
    assert_token(input, "always", "builtin-info", true);
    assert_token(input, "https://example.com/a/b", "builtin-url", true);
    for (token, id) in [
        ("192.168.1.1", "builtin-address"),
        ("128MiB", "builtin-size"),
        ("15ms", "builtin-duration"),
        ("v2.7.0", "builtin-version"),
        ("550e8400-e29b-41d4-a716-446655440000", "builtin-uuid"),
    ] {
        assert_token(token, token, id, false);
    }
}

#[test]
fn ipv6_candidates_require_rust_address_validation() {
    let rules: Vec<_> = get_builtin_keyword_rules(true)
        .into_iter()
        .filter(|r| r.id == "builtin-address")
        .collect();
    let addresses = SemanticHighlighter::compile(
        &rules,
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    for token in [
        "::",
        "::1",
        "2001:db8::1",
        "fe80::1234",
        "2001:db8:0:0:0:0:2:1",
        "::ffff:192.168.1.1",
    ] {
        assert_token(token, token, "builtin-address", false);
    }
    for invalid in [
        "21:37",
        "88:77",
        "foo:bar:baz",
        "2001:db8:::1",
        "2001:db8::gg",
        "12345::1",
    ] {
        assert!(
            addresses
                .matches(invalid, &SemanticContext::default())
                .is_empty(),
            "{invalid}"
        );
    }
    assert_token("[2001:db8::1]", "2001:db8::1", "builtin-address", false);
}

#[test]
fn datetime_formats_preserve_valid_time_ranges() {
    let rules: Vec<_> = get_builtin_keyword_rules(true)
        .into_iter()
        .filter(|r| r.id == "builtin-datetime")
        .collect();
    let dates = SemanticHighlighter::compile(
        &rules,
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    for token in [
        "2026-10-07",
        "2026/10/07",
        "10/7/26",
        "10-7-26",
        "10/07/2026",
        "20261007T221600Z",
        "21:37",
        "21:37:59",
        "2026-10-07T21:37:59+08:00",
        "Oct",
        "Mon",
        "Sept",
        "Tues",
        "Thu",
        "Thur",
    ] {
        assert_token(token, token, "builtin-datetime", false);
    }
    // Isolate the category: operators deliberately share DateTime's green swatch.
    for invalid in ["88:77", "24:00", "21:60", "20261307T221600Z"] {
        assert!(
            dates
                .matches(invalid, &SemanticContext::default())
                .is_empty(),
            "{invalid}"
        );
    }
}

#[test]
fn shell_strings_suppress_output_keywords_and_support_unclosed_quotes() {
    for text in [
        "echo \"hello world\"",
        "echo 'ERROR connection refused'",
        "echo \"unterminated",
    ] {
        let start = text.find(['\'', '"']).unwrap();
        assert!(
            colors(text, true)[start..]
                .iter()
                .all(|color| *color == Some(swatch("builtin-string")))
        );
    }
    let line = r#"printf "hello\n%s %02d %.2f %% \x41 \u0041""#;
    for child in [r"\n", "%s", "%02d", "%.2f", "%%", r"\x41", r"\u0041"] {
        assert_token(line, child, "builtin-string", true);
    }
    assert_token(line, "hello", "builtin-string", true);
    assert_token(r"echo 'literal\n'", r"\n", "builtin-string", true);
}

#[test]
fn output_word_boundaries_leave_delimiters_uncolored_by_status() {
    assert_token("[ERROR][ERROR]", "ERROR", "builtin-error", false);
    let result = colors("[ERROR][ERROR]", false);
    assert_ne!(result[0], Some(swatch("builtin-error")));
    assert_ne!(result[6], Some(swatch("builtin-error")));
    assert_eq!(result[8], Some(swatch("builtin-error")));
    for invalid in ["error-prone", "myERROR", "ERROR日志", "error\u{301}"] {
        assert!(!colors(invalid, false).contains(&Some(swatch("builtin-error"))));
    }
    assert_token(
        "ERROR: connection refused",
        "connection refused",
        "builtin-error",
        false,
    );
    assert_token("warning debug success", "warning", "builtin-warn", false);
    assert_token("warning debug success", "debug", "builtin-debug", false);
    assert_token("warning debug success", "success", "builtin-success", false);
}

fn snapshot_colors(snapshot: &TerminalSnapshot) -> Vec<Option<u32>> {
    let highlighter = compile_terminal_keyword_highlighter(
        &get_builtin_keyword_rules(true),
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    let highlights = precompute_terminal_keyword_highlights(
        snapshot,
        &highlighter,
        nyaterm_ui::theme_palette("github-dark"),
        None,
    );
    let mut result = Vec::new();
    for row in 0..snapshot.row_count() {
        let mut colors = vec![None; snapshot.cols];
        if let Some(lookup) = highlights.lookup(row, snapshot)
            && let Some(ranges) = lookup.ranges()
        {
            for range in ranges {
                colors[range.start_col..range.end_col].fill(Some(range.color));
            }
        }
        result.extend(colors);
    }
    result
}

#[test]
fn osc133_regions_survive_submission_and_split_same_row_output() {
    let mut screen = TerminalScreen::new(80, 2);
    screen.advance(b"\x1b]133;A\x07root@host:/tmp# \x1b]133;B\x07ls -la");
    let input = screen.snapshot();
    let colors = snapshot_colors(&input);
    assert_eq!(colors[14], Some(swatch("builtin-prompt")));
    assert_eq!(&colors[16..18], &[Some(swatch("builtin-command")); 2]);
    screen.advance(b"\x1b]133;C\x07 ERROR: connection refused");
    let submitted = screen.snapshot();
    assert_eq!(
        submitted.row(0).unwrap().shell_input,
        Some(ShellInputLineKind::Submitted)
    );
    let colors = snapshot_colors(&submitted);
    assert_eq!(&colors[16..18], &[Some(swatch("builtin-command")); 2]);
    assert_eq!(&colors[23..28], &[Some(swatch("builtin-error")); 5]);
}

#[test]
fn readline_completion_pages_are_output_while_osc133_input_remains_active() {
    let mut screen = TerminalScreen::new(80, 4);
    screen.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07ls ");
    let input = screen.snapshot();
    assert_eq!(snapshot_colors(&input)[2], Some(swatch("builtin-command")));
    // Readline prints completions and its pager without executing a command,
    // so it does not send OSC 133 C before these hard line breaks.
    screen.advance(b"\r\n01-javascript/\r\n10:58 test/\r\nAGENTS.md\r\n.bashrc\r\n--More--");
    let page = screen.snapshot();
    assert!(page.scrollback_len > 0);
    let page_colors = snapshot_colors(&page);
    assert!(!page_colors.contains(&Some(swatch("builtin-command"))));
    assert_eq!(&page_colors[..5], &[Some(swatch("builtin-datetime")); 5]);
    assert_eq!(page_colors[3 * 80], Some(swatch("builtin-option")));
    let agents = page
        .rows()
        .iter()
        .position(|row| row.text == "AGENTS.md")
        .unwrap();
    assert!(
        page_colors[agents * 80..agents * 80 + 9]
            .iter()
            .all(Option::is_none)
    );
    // Redisplay establishes a fresh input anchor; the candidate list remains output.
    screen.advance(b"\r\x1b[2K$ \x1b]133;B\x07ls ");
    let restored = screen.snapshot();
    let row = restored
        .rows()
        .iter()
        .position(|row| row.text == "$ ls")
        .unwrap();
    assert_eq!(
        snapshot_colors(&restored)[row * 80 + 2],
        Some(swatch("builtin-command"))
    );
    assert!(
        restored.rows()[..row]
            .iter()
            .all(|row| row.shell_input_columns.is_none())
    );
}

#[test]
fn fallback_is_isolated_from_integrated_output_and_soft_wrapped_input() {
    let mut screen = TerminalScreen::new(20, 4);
    screen.advance(b"$ cat file | grep error");
    assert_eq!(
        snapshot_colors(&screen.snapshot())[2],
        Some(swatch("builtin-command"))
    );
    let mut screen = TerminalScreen::new(20, 4);
    screen.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07cat file | grep error");
    let colors = snapshot_colors(&screen.snapshot());
    assert_eq!(colors[13], Some(swatch("builtin-command")));
    assert!(colors[18..23].iter().all(Option::is_none));
    screen.advance(b"\r\n\x1b]133;C\x07$ echo ERROR");
    let snapshot = screen.snapshot();
    let colors = snapshot_colors(&snapshot);
    let output_row = snapshot
        .rows()
        .iter()
        .position(|row| row.text == "$ echo ERROR")
        .unwrap();
    // An explicit output boundary wins over the prompt-shaped text, including
    // when the cursor still sits on that line.
    assert_eq!(colors[output_row * 20 + 7], Some(swatch("builtin-error")));
    screen.advance(b"\x1b[?1049h\x1b[H$ echo ERROR");
    let alternate = screen.snapshot();
    assert!(alternate.rows().iter().all(|row| row.shell_integration));
    // Alternate-screen text is output, even when it resembles a shell prompt.
    let (row, byte) = alternate
        .rows()
        .iter()
        .enumerate()
        .find_map(|(row, source)| source.text.find("ERROR").map(|byte| (row, byte)))
        .expect("alternate output");
    let col = nyaterm_terminal::terminal_cell_col_for_byte_index(&alternate.rows()[row].text, byte);
    assert_eq!(
        snapshot_colors(&alternate)[row * alternate.cols + col],
        Some(swatch("builtin-error"))
    );
}

#[test]
fn whole_strings_survive_soft_wrap_and_precompute_cancellation_does_not_publish() {
    let mut screen = TerminalScreen::new(12, 5);
    screen.advance(b"\x1b]133;B\x07echo \"hello\\n%s\"");
    let snapshot = screen.snapshot();
    let colors = snapshot_colors(&snapshot);
    assert_eq!(colors[5], Some(swatch("builtin-string")));
    assert_eq!(&colors[11..13], &[Some(swatch("builtin-string")); 2]);
    assert_eq!(&colors[13..15], &[Some(swatch("builtin-string")); 2]);
    let highlighter = compile_terminal_keyword_highlighter(
        &get_builtin_keyword_rules(true),
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    let mut groups = 0;
    let cancelled = precompute_terminal_keyword_highlights_for_rows_with_stats_and_cancel(
        &snapshot,
        &highlighter,
        nyaterm_ui::theme_palette("github-dark"),
        None,
        0..snapshot.row_count(),
        || {
            groups += 1;
            groups > 1
        },
    );
    assert!(cancelled.is_none());
}

#[test]
fn unchanged_text_semantic_changes_invalidate_reuse_and_stale_results() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let highlighter = compile_terminal_keyword_highlighter(
        &get_builtin_keyword_rules(true),
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    let mut screen = TerminalScreen::new(30, 2);
    screen.advance(b"ERROR");
    let output = screen.snapshot();
    let previous = precompute_terminal_keyword_highlights(&output, &highlighter, palette, None);
    // Move to column zero, then mark the existing text as shell input without changing it.
    screen.advance(b"\r\x1b]133;B\x07");
    let input = screen.snapshot();
    assert_eq!(
        output.row(0).unwrap().revision,
        input.row(0).unwrap().revision
    );
    assert_eq!(
        output.row(0).unwrap().signature,
        input.row(0).unwrap().signature
    );
    assert!(previous.lookup(0, &input).is_none());
    assert!(previous.stale_lookup(0, &input).is_none());
    let (next, stats) = precompute_terminal_keyword_highlights_for_rows_with_stats(
        &input,
        &highlighter,
        palette,
        Some(&previous),
        0..1,
    );
    assert_eq!(stats.reused_rows, 0);
    assert_eq!(
        next.lookup(0, &input).unwrap().ranges().unwrap()[0].color,
        swatch("builtin-command")
    );
    let mut changed = input.clone();
    let mut rows = changed.rows().to_vec();
    Arc::make_mut(&mut rows[0]).shell_input_columns = Some((2, 30));
    changed.row_data = rows.into();
    assert!(next.lookup(0, &changed).is_none());
    assert!(next.stale_lookup(0, &changed).is_none());
}

#[test]
fn semantic_foreground_respects_ansi_color_and_hidden_text() {
    for ansi in ["\x1b[34m", "\x1b[38;2;12;34;56m", "\x1b[8m"] {
        for (token, id) in [
            ("ERROR", "builtin-error"),
            ("-12.5%", "builtin-number"),
            ("-.5ms", "builtin-duration"),
        ] {
            let mut screen = TerminalScreen::new(20, 1);
            screen.advance(format!("{ansi}{token}").as_bytes());
            let snapshot = screen.snapshot();
            let row = snapshot.row(0).unwrap();
            let spans = terminal_highlight_spans_with_keyword_ranges_and_options(
                &row.text,
                Some(&row.styled_spans),
                Some(&[TerminalKeywordRange {
                    start_col: 0,
                    end_col: token.len(),
                    color: swatch(id),
                }]),
                &[],
                nyaterm_ui::theme_palette("github-dark"),
                false,
            );
            assert!(spans.iter().all(|span| !span.keyword));
            assert!(spans.iter().all(|span| span.color != Some(swatch(id))));
        }
    }
}

#[test]
fn whole_match_ties_are_deterministic_and_user_marks_do_not_split_strings() {
    let mut rules = get_builtin_keyword_rules(true);
    let expected = SemanticHighlighter::compile(
        &rules,
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    let line = "ERROR 192.168.1.1 128MiB 2026-10-07";
    let context = SemanticContext::default();
    let first = expected.ranges(line, &context, &[]);
    rules.reverse();
    assert_eq!(
        first,
        SemanticHighlighter::compile(
            &rules,
            crate::keywords::TerminalKeywordHighlightConfig {
                line_roles: true,
                ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi
            }
        )
        .ranges(line, &context, &[])
    );
    rules.insert(
        0,
        nyaterm_core::keyword_highlight_presets::ResolvedKeywordHighlightRule {
            id: "user-mark".into(),
            name: "Mark".into(),
            patterns: vec!["ERROR".into()],
            color: "#123456".into(),
            enabled: true,
        },
    );
    let highlighter = compile_terminal_keyword_highlighter(
        &rules,
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    let mut screen = TerminalScreen::new(40, 1);
    screen.advance(b"\x1b]133;B\x07echo \"ERROR\"");
    let snapshot = screen.snapshot();
    let highlights = precompute_terminal_keyword_highlights(
        &snapshot,
        &highlighter,
        nyaterm_ui::theme_palette("github-dark"),
        None,
    );
    let ranges = highlights
        .lookup(0, &snapshot)
        .unwrap()
        .ranges()
        .unwrap()
        .to_vec();
    assert!(
        ranges
            .iter()
            .any(|r| r.start_col == 5 && r.end_col == 12 && r.color == swatch("builtin-string"))
    );
    assert!(ranges.iter().all(|r| r.color != 0x123456));

    // An exact whole-token tie still honors a user rule.
    let mut screen = TerminalScreen::new(40, 1);
    screen.advance(b"ERROR ERROR");
    let snapshot = screen.snapshot();
    let highlights = precompute_terminal_keyword_highlights(
        &snapshot,
        &highlighter,
        nyaterm_ui::theme_palette("github-dark"),
        None,
    );
    assert!(
        highlights
            .lookup(0, &snapshot)
            .unwrap()
            .ranges()
            .unwrap()
            .iter()
            .all(|r| r.color == 0x123456)
    );
}

#[test]
fn category_switches_disable_structured_children_and_light_colors_remain_distinct() {
    let mut rules = get_builtin_keyword_rules(false);
    for rule in &mut rules {
        rule.enabled = !matches!(
            rule.id.as_str(),
            "builtin-permissions" | "builtin-string" | "builtin-command" | "builtin-path"
        );
    }
    let highlighter = SemanticHighlighter::compile(
        &rules,
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    let line = "cat /tmp \"ERROR\n%s\"";
    let context = SemanticContext {
        input: std::iter::once(0..line.len()).collect(),
        integrated: true,
        ..SemanticContext::default()
    };
    let result = highlighter.matches(line, &context);
    assert!(result.iter().all(|m| m.priority != 10 && m.priority != 20));
    for id in [
        "builtin-command",
        "builtin-string",
        "builtin-path",
        "builtin-option",
        "builtin-number",
    ] {
        assert_ne!(
            builtin_keyword_rule_swatch(id, true),
            builtin_keyword_rule_swatch(id, false)
        );
    }
}

/// 行角色高亮的开销基准：回答「开/关 Line Role Highlighting 差多少」。
///
/// 手动跑（必须 release，debug 数据没意义）：
/// ```text
/// cargo test --release -p nyaterm-terminal-gpui --lib line_role_cost -- --ignored --nocapture
/// ```
///
/// 方法：每组跑 `ROUNDS` 次取**中位数**并给出区间。单次采样在这个量级会被噪声
/// 淹没 —— 实测同一配置反复跑，倍数会在 0.89x~1.29x 之间跳。
#[test]
#[ignore = "manual benchmark: line-role highlighting cost"]
fn line_role_cost_benchmark() {
    const ROUNDS: usize = 15;
    let rules = get_builtin_keyword_rules(true);
    let palette = nyaterm_ui::theme_palette("github-dark");

    fn median(mut v: Vec<u128>) -> (u128, u128, u128) {
        v.sort_unstable();
        (v[v.len() / 2], v[0], *v.last().unwrap())
    }

    println!("\n== detect() 单行开销（{ROUNDS} 轮取中位）==");
    for (name, line) in [
        (
            "不命中（24 分支全走完）",
            "some ordinary output line that matches nothing at all",
        ),
        (
            "命中 ps aux",
            "root 1234 0.0 0.1 123456 7890 ? Ss 10:00 0:00 /usr/sbin/sshd -D",
        ),
        (
            "命中 ls -l",
            "drwxr-xr-x 18 root root 4096 Oct 7 21:37 some-directory",
        ),
    ] {
        let mut samples = Vec::new();
        for _ in 0..ROUNDS {
            let started = std::time::Instant::now();
            let mut acc = 0usize;
            for _ in 0..1000 {
                acc += crate::semantic_roles::detect(line) as usize;
            }
            samples.push(started.elapsed().as_nanos() / 1000);
            std::hint::black_box(acc);
        }
        let (m, lo, hi) = median(samples);
        println!("  {name:<24} 中位 {m:>4} ns  [{lo} ~ {hi}]");
    }

    let mut corpus: Vec<&str> = Vec::new();
    for _ in 0..40 {
        corpus.extend_from_slice(&[
            "some ordinary output line that matches nothing at all",
            "another plain line with words and 12345 numbers in it",
            "drwxr-xr-x 18 root root 4096 Oct 7 21:37 some-directory",
            "root 1234 0.0 0.1 123456 7890 ? Ss 10:00 0:00 /usr/sbin/sshd -D",
        ]);
    }
    let mut samples = Vec::new();
    for _ in 0..ROUNDS {
        let started = std::time::Instant::now();
        let mut acc = 0usize;
        for line in &corpus {
            acc += crate::semantic_roles::detect(line) as usize;
        }
        samples.push(started.elapsed().as_nanos() / corpus.len() as u128);
        std::hint::black_box(acc);
    }
    let (m, lo, hi) = median(samples);
    println!("  混合语料 160 行          中位 {m:>4} ns  [{lo} ~ {hi}]");

    // WerVa 怀疑「关掉行角色仍比 main 慢 5%」来自形状化的 prompt 检测。
    // 这里单独量它：无分配，只看第一个 token，理论上应该很便宜。
    println!("\n== fallback_prompt 单行开销（{ROUNDS} 轮取中位）==");
    for (name, line) in [
        (
            "nginx 日志",
            "10.0.0.1 - - [03/Nov/2026:10:00:00 +0800] \"GET /api HTTP/1.1\" 200 512",
        ),
        (
            "ls -l",
            "drwxr-xr-x 18 root root 4096 Oct 7 21:37 some-directory",
        ),
        ("普通文本", "ordinary output with calm filler"),
        ("真 prompt", "[root@nginx-wan ~]# ls -l"),
    ] {
        let mut samples = Vec::new();
        for _ in 0..ROUNDS {
            let started = std::time::Instant::now();
            let mut acc = 0usize;
            for _ in 0..1000 {
                acc += super::fallback_prompt(line).map_or(0, |(a, b)| a + b);
            }
            samples.push(started.elapsed().as_nanos() / 1000);
            std::hint::black_box(acc);
        }
        samples.sort_unstable();
        println!("  {name:<10} 中位 {:>4} ns", samples[samples.len() / 2]);
    }

    println!("\n== 整屏预计算（120 列，{ROUNDS} 轮取中位）==");
    for (rows, label) in [(200usize, "200 行"), (2000, "2000 行")] {
        for (case, fixture) in [
            ("plain", "ordinary output with calm filler"),
            (
                "dense",
                "drwxr-xr-x 18 root root 4096 Oct 7 21:37 ERROR 2001:db8::1 128MiB",
            ),
        ] {
            let mut screen = TerminalScreen::new(120, rows as u16);
            screen.advance(
                std::iter::repeat_n(fixture, rows)
                    .collect::<Vec<_>>()
                    .join("\r\n")
                    .as_bytes(),
            );
            let snapshot = screen.snapshot();
            // 交替测：每轮 off 与 on 紧挨着跑，机器漂移同时影响两者，
            // 取「每轮增量」的中位数，比分段跑稳得多。
            let off_h = compile_terminal_keyword_highlighter(
                &rules,
                crate::keywords::TerminalKeywordHighlightConfig {
                    line_roles: false,
                    ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
                },
            );
            let on_h = compile_terminal_keyword_highlighter(
                &rules,
                crate::keywords::TerminalKeywordHighlightConfig {
                    line_roles: true,
                    ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
                },
            );
            for h in [&off_h, &on_h] {
                let _ = precompute_terminal_keyword_highlights_for_rows_with_stats(
                    &snapshot,
                    h,
                    palette,
                    None,
                    0..snapshot.row_count(),
                );
            }
            let mut deltas = Vec::new();
            let mut offs = Vec::new();
            for _ in 0..ROUNDS {
                let t0 = std::time::Instant::now();
                let _ = precompute_terminal_keyword_highlights_for_rows_with_stats(
                    &snapshot,
                    &off_h,
                    palette,
                    None,
                    0..snapshot.row_count(),
                );
                let off_us = t0.elapsed().as_micros();
                let t1 = std::time::Instant::now();
                let _ = precompute_terminal_keyword_highlights_for_rows_with_stats(
                    &snapshot,
                    &on_h,
                    palette,
                    None,
                    0..snapshot.row_count(),
                );
                let on_us = t1.elapsed().as_micros();
                offs.push(off_us);
                deltas.push(on_us as i128 - off_us as i128);
            }
            let (off_m, _, _) = median(offs);
            deltas.sort_unstable();
            let delta_m = deltas[deltas.len() / 2];
            let (d_lo, d_hi) = (deltas[0], *deltas.last().unwrap());
            println!(
                "  {label:<7} {case:<6} 基线(关) {off_m:>6}us  行角色增量 中位 {delta_m:>+6}us [{d_lo}~{d_hi}]  即 {:+.1}%",
                delta_m as f64 * 100.0 / off_m.max(1) as f64
            );
        }
    }

    println!("\n== 可见行同步补算（40 行视口，{ROUNDS} 轮取中位）==");
    for (case, fixture) in [
        ("plain", "ordinary output with calm filler"),
        (
            "dense",
            "drwxr-xr-x 18 root root 4096 Oct 7 21:37 ERROR 2001:db8::1 128MiB",
        ),
    ] {
        let rows = 40usize;
        let mut screen = TerminalScreen::new(120, rows as u16);
        screen.advance(
            std::iter::repeat_n(fixture, rows)
                .collect::<Vec<_>>()
                .join("\r\n")
                .as_bytes(),
        );
        let snapshot = screen.snapshot();
        let off_h = compile_terminal_keyword_highlighter(
            &rules,
            crate::keywords::TerminalKeywordHighlightConfig {
                line_roles: false,
                ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
            },
        );
        let on_h = compile_terminal_keyword_highlighter(
            &rules,
            crate::keywords::TerminalKeywordHighlightConfig {
                line_roles: true,
                ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
            },
        );
        let mut deltas = Vec::new();
        let mut offs = Vec::new();
        for _ in 0..ROUNDS {
            let t0 = std::time::Instant::now();
            let _ = precompute_terminal_keyword_highlights_for_rows_with_stats(
                &snapshot,
                &off_h,
                palette,
                None,
                0..snapshot.row_count(),
            );
            let off_us = t0.elapsed().as_micros();
            let t1 = std::time::Instant::now();
            let _ = precompute_terminal_keyword_highlights_for_rows_with_stats(
                &snapshot,
                &on_h,
                palette,
                None,
                0..snapshot.row_count(),
            );
            let on_us = t1.elapsed().as_micros();
            offs.push(off_us);
            deltas.push(on_us as i128 - off_us as i128);
        }
        let (off_m, _, _) = median(offs);
        deltas.sort_unstable();
        let delta_m = deltas[deltas.len() / 2];
        let (d_lo, d_hi) = (deltas[0], *deltas.last().unwrap());
        println!(
            "  {case:<6} 基线(关) {off_m:>4}us  行角色增量 中位 {delta_m:>+5}us [{d_lo}~{d_hi}]  即 {:+.1}%",
            delta_m as f64 * 100.0 / off_m.max(1) as f64
        );
    }
}

/// 子色必须跟随规则自身的颜色（也就是主题调色板），而不是固定的浅色 swatch。
///
/// `compile` 原来靠「规则颜色 == 固定深色 swatch」推断明暗，但绘制规则是从调色板
/// 生成的，永远不相等，于是深色主题下 info/warn/error/debug/operator 这些子色
/// 全部落到浅色变体上。
#[test]
fn palette_coloured_rules_keep_their_own_child_colours() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let rules = nyaterm_core::get_builtin_keyword_rules_with_ansi(palette.terminal_ansi, true);
    let highlighter = SemanticHighlighter::compile(
        &rules,
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );

    for id in [
        "builtin-info",
        "builtin-warn",
        "builtin-error",
        "builtin-debug",
        "builtin-operator",
    ] {
        let rule = rules
            .iter()
            .find(|rule| rule.id == id)
            .unwrap_or_else(|| panic!("missing rule {id}"));
        let expected = super::parse_hex_rgb(&rule.color).expect("rule colour");
        let light = super::parse_hex_rgb(nyaterm_core::builtin_keyword_rule_swatch(id, false))
            .expect("light swatch");
        assert_ne!(
            expected, light,
            "{id}: 深色调色板与浅色 swatch 相同，这个断言就没意义了"
        );
        assert_eq!(
            highlighter.child_color(id),
            expected,
            "{id}: 子色应跟随规则颜色 {expected:#08x}，实际 {:#08x}（浅色变体是 {light:#08x}）",
            highlighter.child_color(id)
        );
    }
}

#[test]
#[ignore = "manual fixture-only background precompute benchmark"]
fn semantic_precompute_benchmark() {
    let highlighter = compile_terminal_keyword_highlighter(
        &get_builtin_keyword_rules(true),
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    let palette = nyaterm_ui::theme_palette("github-dark");
    for (case, fixture) in [
        ("plain", "ordinary output with calm filler"),
        (
            "dense",
            "drwxr-xr-x 18 root root 4096 Oct 7 21:37 ERROR 2001:db8::1 128MiB",
        ),
    ] {
        let mut screen = TerminalScreen::new(120, 200);
        screen.advance(
            std::iter::repeat_n(fixture, 200)
                .collect::<Vec<_>>()
                .join("\r\n")
                .as_bytes(),
        );
        let snapshot = screen.snapshot();
        let (first, stats) = precompute_terminal_keyword_highlights_for_rows_with_stats(
            &snapshot,
            &highlighter,
            palette,
            None,
            0..snapshot.row_count(),
        );
        let started = std::time::Instant::now();
        let (_, reused) = precompute_terminal_keyword_highlights_for_rows_with_stats(
            &snapshot,
            &highlighter,
            palette,
            Some(&first),
            0..snapshot.row_count(),
        );
        println!(
            "{case}: match_us={} map_us={} bytes={} ranges={} reuse_us={} reused_rows={}",
            stats.match_duration_us,
            stats.range_build_duration_us,
            stats.processed_bytes,
            stats.range_count,
            started.elapsed().as_micros(),
            reused.reused_rows
        );
        assert_eq!(reused.processed_bytes, 0);
        assert_eq!(reused.reused_rows, snapshot.row_count());
    }
}

#[test]
fn prompt_prefix_survives_the_row_leaving_the_live_input_state() {
    // Shell anchors describe the live input row only. Once the row is history the
    // anchors are gone while `integrated` stays true, so the prompt used to fall
    // through to plain output and its text turned black. The prompt shape alone
    // has to keep colouring it, exactly like the live row.
    let highlighter = SemanticHighlighter::compile(
        &get_builtin_keyword_rules(true),
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    );
    let line = "[root@nginx-wan ~]# ";
    let context = SemanticContext {
        integrated: true,
        ..SemanticContext::default()
    };
    let mut colors = vec![None; line.len()];
    for (start, end, color) in highlighter.ranges(line, &context, &[]) {
        colors[start..end].fill(Some(color));
    }
    // Only the brackets carry the accent; the prompt words stay in the normal
    // foreground.
    assert_eq!(colors[0], Some(swatch("builtin-prompt")), "{colors:?}");
    assert_eq!(colors[17], Some(swatch("builtin-prompt")), "{colors:?}");
    assert!(colors[1..17].iter().all(Option::is_none), "{colors:?}");
}

#[test]
fn scrolled_view_keeps_the_prompt_coloured() {
    // A scrolled viewport has no cursor row at all (`cursor.row == usize::MAX`),
    // and without shell integration history rows have no input anchors. Prompt
    // colouring must come from the line shape rather than operator colours.
    let mut screen = TerminalScreen::new(40, 5);
    screen.advance(b"[root@nginx-wan ~]# ls\r\none\r\n");
    screen.advance(b"[root@nginx-wan ~]# tail\r\ntwo\r\nthree\r\n");
    screen.advance(b"[root@nginx-wan ~]# ");
    let scrolled = screen.viewport_snapshot(1);
    assert_eq!(
        scrolled.cursor.row,
        usize::MAX,
        "scrolled view has no cursor row"
    );
    let prompt_row = scrolled
        .rows()
        .iter()
        .position(|row| row.text.starts_with("[root@nginx-wan ~]# tail"))
        .expect("history prompt row stays inside the window");
    assert!(scrolled.row(prompt_row).unwrap().shell_input.is_none());
    assert!(!scrolled.row(prompt_row).unwrap().shell_integration);
    let colors = snapshot_colors(&scrolled);
    let base = prompt_row * 40;
    assert_eq!(
        colors[base],
        Some(swatch("builtin-prompt")),
        "{:?}",
        &colors[base..base + 19]
    );
    assert_eq!(
        colors[base + 17],
        Some(swatch("builtin-prompt")),
        "{:?}",
        &colors[base..base + 19]
    );
    assert!(
        colors[base + 1..base + 17].iter().all(Option::is_none),
        "{:?}",
        &colors[base..base + 19]
    );
}

#[test]
fn disabled_child_rules_fall_back_to_the_supplied_palette() {
    let mut rules = get_builtin_keyword_rules(true);
    for id in [
        "builtin-info",
        "builtin-warn",
        "builtin-error",
        "builtin-debug",
        "builtin-operator",
    ] {
        rules.iter_mut().find(|rule| rule.id == id).unwrap().enabled = false;
    }
    let mut config = crate::keywords::TerminalKeywordHighlightConfig::default();
    for (slot, color) in config.ansi.iter_mut().enumerate() {
        *color = 0x112200 + slot as u32;
    }
    let highlighter = SemanticHighlighter::compile(&rules, config);
    for id in [
        "builtin-info",
        "builtin-warn",
        "builtin-error",
        "builtin-debug",
        "builtin-operator",
    ] {
        assert_eq!(
            highlighter.child_color(id),
            config.ansi[usize::from(
                nyaterm_core::keyword_highlight_presets::builtin_keyword_rule_ansi(id)
            )]
        );
    }
}

#[test]
fn explicit_output_keeps_prompt_shaped_text_out_of_the_input_parser() {
    let highlighter = SemanticHighlighter::compile(
        &get_builtin_keyword_rules(true),
        crate::keywords::TerminalKeywordHighlightConfig::default(),
    );
    let context = SemanticContext {
        integrated: true,
        output: true,
        ..SemanticContext::default()
    };
    let line = "$ echo ERROR";
    let result = highlighter.ranges(line, &context, &[]);
    assert!(
        !result.iter().any(|&(start, end, color)| start <= 2
            && end >= 6
            && color == swatch("builtin-command"))
    );
    assert!(result.iter().any(
        |&(start, end, color)| &line[start..end] == "ERROR" && color == swatch("builtin-error")
    ));
}

#[test]
fn continuous_unicode_echo_backspace_and_wrapping_keep_output_colours() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let highlighter = compile_terminal_keyword_highlighter(
        &get_builtin_keyword_rules(true),
        crate::keywords::TerminalKeywordHighlightConfig::default(),
    );
    let mut screen = TerminalScreen::new(16, 6);
    screen.advance(b"ERROR output\r\n\x1b]133;A\x07$ \x1b]133;B\x07");
    let mut previous = None;
    for input in [
        "echo ",
        "中文",
        "e\u{301}",
        " long-wrapped-text",
        "\u{8} \u{8}",
        "x",
    ] {
        screen.advance(input.as_bytes());
        let snapshot = screen.snapshot();
        let next = precompute_terminal_keyword_highlights(
            &snapshot,
            &highlighter,
            palette,
            previous.as_ref(),
        );
        assert!(
            next.lookup(0, &snapshot)
                .and_then(|lookup| lookup.ranges())
                .is_some()
        );
        let cold = precompute_terminal_keyword_highlights(&snapshot, &highlighter, palette, None);
        for row in 0..snapshot.row_count() {
            assert_eq!(
                next.lookup(row, &snapshot)
                    .and_then(|lookup| lookup.ranges()),
                cold.lookup(row, &snapshot)
                    .and_then(|lookup| lookup.ranges())
            );
        }
        previous = Some(next);
    }
}

#[test]
fn submission_completion_resize_and_screen_switch_reuse_match_cold_results() {
    let highlighter = compile_terminal_keyword_highlighter(
        &get_builtin_keyword_rules(true),
        crate::keywords::TerminalKeywordHighlightConfig::default(),
    );
    let palette = nyaterm_ui::theme_palette("github-dark");
    let mut screen = TerminalScreen::new(24, 8);
    let mut previous = None;
    for (bytes, width) in [
        ("\x1b]133;A\x07$ \x1b]133;B\x07echo 中文e\u{301}", 24),
        (
            "\r\nERROR completion.txt\r\n$ echo 中文e\u{301} --long-option",
            24,
        ),
        ("\x1b]133;C\x07\r\nERROR 128MiB\r\n", 16),
        ("\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07ls -l", 32),
        ("\x1b[?1049h\x1b[H$ echo ERROR", 16),
        ("\x1b[?1049l", 40),
    ] {
        screen.advance(bytes.as_bytes());
        screen.resize(width, 8);
        let snapshot = screen.snapshot();
        let reused = precompute_terminal_keyword_highlights(
            &snapshot,
            &highlighter,
            palette,
            previous.as_ref(),
        );
        let cold = precompute_terminal_keyword_highlights(&snapshot, &highlighter, palette, None);
        assert!(reused.matches_snapshot(&snapshot, palette));
        for row in 0..snapshot.row_count() {
            assert_eq!(
                reused
                    .lookup(row, &snapshot)
                    .and_then(|value| value.ranges()),
                cold.lookup(row, &snapshot).and_then(|value| value.ranges()),
                "{bytes:?}, row={row}"
            );
        }
        previous = Some(reused);
    }
}

#[test]
fn degraded_oversized_fragments_keep_generic_and_user_rules() {
    let rules = get_builtin_keyword_rules(true);
    let config = crate::keywords::TerminalKeywordHighlightConfig::default();
    let enabled = SemanticHighlighter::compile(&rules, config);
    let disabled = SemanticHighlighter::compile(
        &rules,
        crate::keywords::TerminalKeywordHighlightConfig {
            line_roles: false,
            ..config
        },
    );
    let line = "-rw-r--r-- 1 root root 123 Oct 30 10:00 ERROR.txt";
    let context = SemanticContext {
        suppress_line_roles: true,
        ..SemanticContext::default()
    };
    let user = [(11, 12, 0xabcdef)];
    let actual = enabled.ranges(line, &context, &user);
    assert!(!actual.is_empty());
    assert_eq!(actual, disabled.ranges(line, &context, &user));
}
