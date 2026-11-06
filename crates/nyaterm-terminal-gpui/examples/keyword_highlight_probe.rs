//! Synthetic keyword pipeline benchmark; construction and I/O are outside samples.
use nyaterm_core::ResolvedKeywordHighlightRule;
use nyaterm_core::keyword_highlight_presets::get_builtin_keyword_rules;
use nyaterm_terminal::TerminalScreen;
use nyaterm_terminal_gpui::{
    TerminalKeywordHighlighter, compile_terminal_keyword_highlighter,
    precompute_terminal_keyword_highlights_for_rows_with_stats,
};
use std::hint::black_box;
use std::time::Instant;

// ADAPTER BEGIN: the comparison runner changes only this API adapter for historical builds.
fn highlighter(rules: &[ResolvedKeywordHighlightRule], roles: bool) -> TerminalKeywordHighlighter {
    compile_terminal_keyword_highlighter(
        rules,
        nyaterm_terminal_gpui::TerminalKeywordHighlightConfig {
            line_roles: roles,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    )
}
// ADAPTER END

fn main() {
    let rounds: usize = std::env::args()
        .nth(1)
        .map_or(15, |arg| arg.parse().expect("rounds"));
    let round_offset: usize = std::env::args()
        .nth(2)
        .map_or(0, |arg| arg.parse().expect("round offset"));
    assert!(rounds > 0);
    let rules = get_builtin_keyword_rules(true);
    let palette = nyaterm_ui::theme_palette("github-dark");
    let fixtures = [
        ("plain", "ordinary output with calm filler".to_string()),
        (
            "log",
            "10.0.0.1 - - [03/Nov/2026:10:00:00 +0800] \"GET /api HTTP/1.1\" 200 512".to_string(),
        ),
        (
            "dense",
            "drwxr-xr-x 18 root root 4096 Oct 7 21:37 ERROR 2001:db8::1 128MiB".to_string(),
        ),
        (
            "ls",
            "-rw-r--r--@ 1 root staff 123456 Oct 30 10:00 file.txt".to_string(),
        ),
        (
            "ps",
            "root 1234 0.0 0.1 123456 7890 ? Ss 10:00 0:00 /usr/sbin/sshd -D".to_string(),
        ),
        (
            "2k",
            format!(
                "{{\"items\":[{}]}}",
                "{\"id\":123,\"value\":true},".repeat(90)
            ),
        ),
        (
            "8k",
            format!("/usr/bin/example {}", "argument ".repeat(908)),
        ),
        (
            "oversized",
            format!("/usr/bin/example {}", "argument ".repeat(1900)),
        ),
    ];
    println!("case,rows,roles,path,round,ns,processed_bytes,reused_rows");
    for (name, fixture) in fixtures {
        for rows in [40u16, 200, 2000] {
            let mut screen = TerminalScreen::new(120, rows);
            let copies = usize::from(rows).div_ceil(fixture.len().div_ceil(120));
            for _ in 0..copies {
                screen.advance(fixture.as_bytes());
                screen.advance(b"\r\n");
            }
            let snapshot = screen.snapshot();
            let highlighters = [highlighter(&rules, false), highlighter(&rules, true)];
            let previous: Vec<_> = highlighters
                .iter()
                .map(|highlighter| {
                    precompute_terminal_keyword_highlights_for_rows_with_stats(
                        &snapshot,
                        highlighter,
                        palette,
                        None,
                        0..snapshot.row_count(),
                    )
                    .0
                })
                .collect();
            for round in round_offset..round_offset + rounds {
                for index in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                    for path in ["cold", "reuse", "visible-check"] {
                        let started = Instant::now();
                        let (processed, reused) = if path == "visible-check" {
                            for _ in 0..100 {
                                black_box(previous[index].matches_snapshot_rows(
                                    black_box(&snapshot),
                                    palette,
                                    0..snapshot.row_count().min(40),
                                ));
                            }
                            (0, 0)
                        } else {
                            let (result, stats) =
                                precompute_terminal_keyword_highlights_for_rows_with_stats(
                                    black_box(&snapshot),
                                    &highlighters[index],
                                    palette,
                                    (path == "reuse").then_some(&previous[index]),
                                    0..snapshot.row_count(),
                                );
                            black_box(result);
                            (stats.processed_bytes, stats.reused_rows)
                        };
                        let ns = started.elapsed().as_nanos()
                            / if path == "visible-check" { 100 } else { 1 };
                        println!(
                            "{name},{rows},{},{path},{round},{ns},{processed},{reused}",
                            index == 1
                        );
                    }
                }
            }
        }
    }
}
