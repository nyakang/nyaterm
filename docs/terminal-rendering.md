# Terminal cell rendering

Terminal text remains trimmed for copying, searching, and keyword parsing.
Background paint comes from `TerminalSnapshotRow.cells`, including trailing
blank and wide-character spacer cells. Compressed styled spans retain blank
spans with non-default attributes.

Underline and strikethrough ranges are cached with the shaped row. Highlighted
text supplies their colors within the text boundary; complete cells supply
decorated trailing blanks. Both paint after cell graphics. Strikethroughs keep
GPUI's ascent/descent-based position. A graphic base with an attached combining
mark or variation selector retains its original font-shaping path, including
the block cursor, so replacing the base with a space cannot change the anchor.

`nyaterm-terminal-gpui/src/glyphs.rs` draws U+2580–U+259F block elements and
solid light, heavy, mixed, and double box-drawing characters. These glyphs are
replaced with spaces during text shaping; their column and resolved foreground
color are cached alongside the shaped row. Backgrounds and cell graphics use
the same rounded physical-pixel boundaries, calculated afresh for the current
viewport and scale. Ordinary text, dashed/rounded/diagonal box drawing,
Braille, Powerline, and legacy-computing symbols still use the font path.

Continuous blocks and horizontal strokes with the same resolved color are
merged into one row range. Solid geometry writes directly to the frame's
output buffer, without allocating a temporary vector for each character.
`░▒▓` use three tintable monochrome masks in GPUI's sprite atlas rather than
one quad per foreground pixel. Each mask covers a fixed 64×64 physical-pixel
tile; clipping preserves the original 2×2 dot pattern, including negative
scroll offsets. Colors, font sizes, and DPI changes reuse these three masks.
Adjacent shade rows with matching horizontal boundaries, density, and color
merge before tiling. A shared tile then needs only one sprite across those rows;
gaps, changed widths, colors, and densities prevent merging.
GPUI's 2× SVG rasterization produces 128×128 alpha masks (48 KiB of mask data
per window, excluding atlas allocation overhead).

## CPU paint-plan benchmark

```sh
cargo test -p nyaterm-terminal-gpui --lib shade_plan_vs_naive_pixels_benchmark -- --ignored --nocapture
```

The fixture compares a naive per-pixel algorithmic control with the tiled
paint-plan builder on an 80×24 viewport filled with `▓`, using logical cells
10×20 pixels. It checks the operation-count reduction and reports CPU timing.
The control is not the historical production Renderer, which used font glyphs.
It excludes text shaping, cold atlas rasterization, GPU submission, and presentation; debug
timings must not be interpreted as end-to-end frame rates.

| Display scale | Naive pixel quads | Tiled sprites |
| --- | ---: | ---: |
| 100% | 288,000 | 104 |
| 150% | 648,000 | 228 |
| 200% | 1,152,000 | 375 |

## Native Renderer A/B probe

```sh
cargo run -p nyaterm-terminal-gpui --example renderer_probe --features renderer-bench --release -- cell mixed hot native
python scripts/bench/renderer_ab.py --baseline a39b82d99 --output artifacts/renderer-ab
```

The standalone probe opens a native GPUI window, runs 60 warmup frames and
240 measured frames, then exits. All content is synthetic; it reads no sessions,
credentials, terminal recordings, or application settings. The benchmark feature
and its profiler/platform dependencies are disabled in ordinary application builds.

Each process uses a 120×40 terminal, Consolas at 12 logical pixels, and 8×16
logical-pixel cells. Fixtures cover text, mixed TUI output, blocks, dense shades,
and decoration boundaries. `hot` reuses row layouts; `cold` clears the layout
cache every frame, while the native text/sprite atlas stays warm. Cache counters
cover the whole hot run (including warmup), or just the last cold frame. Run separate
processes for `cell` and `font` to avoid sharing an atlas between modes. The
font control disables cell graphics in the current implementation and is **not
the historical Renderer**.

`renderer_ab.py --baseline a39b82d99` also builds that exact pre-cell-renderer
revision in a detached worktree outside this repository. It adds only the same
probe and its build dependencies; historical renderer sources remain unchanged.
The worktree remains available for inspection; its path and revisions are saved
in the report. Builds use separate target directories to prevent reuse of
workspace artifacts from another revision. `--baseline-worktree <path>` can reuse
a worktree from a previous run after verifying its revision and unchanged sources.
Use `--profile dev` for a faster exploratory build, and release
builds for production comparisons. Default runs cover three repetitions with
alternating variant order, both cache modes, and native/125%/150% scales.

The JSON report contains P50/P95/max CPU time for terminal Prepaint, Paint,
whole-window Draw, platform submission, and invalidation-to-submission. Windows
reports also include peak process working set. Submission includes the native
renderer and swapchain call; it is not a GPU timer or display latency. Working
set includes the whole probe process and does not measure GPU atlas residency.
These synthetic fixtures do not represent an end-to-end OpenCode/btop/lazygit
comparison or transport/parsing load.

Optional numeric scales override GPUI's scale for reproducible pixel geometry
checks, with a matching physical window size. They do not change Windows' display
DPI. `native` uses the actual display scale; repeat at real OS 125%/150% DPI to
validate resizing, input, and cursor integration. Decoration captures use native
GPU render/readback after measured frames, with encoding/writing on a background
thread. The decoration fixture also checks native scene geometry for trailing
underlines and verifies that strike draw order is above full-block quads.
Direct invocation can save a capture:

```sh
cargo run -p nyaterm-terminal-gpui --example renderer_probe --features renderer-bench -- cell decorations hot 1.25 artifacts/decorations-125.png
```

### Exploratory Windows results (2026-10-08)

Windows 11, AMD Radeon(TM) Graphics, Debug build, hot row cache, two repetitions
with variant order reversed on the second repetition. Each entry below is the
range of the two per-run P95 values in milliseconds. Historical font rendering
uses `a39b82d99`; cell rendering uses the current changes after `0ae64769cd`.

| Fixture | Scale | Historical Paint | Cell Paint | Historical CPU draw + submit | Cell CPU draw + submit |
| --- | --- | ---: | ---: | ---: | ---: |
| Mixed TUI | 125% | 0.611–0.624 | 1.008–1.093 | 1.838–1.843 | 2.261–2.355 |
| Mixed TUI | 150% | 0.591–0.624 | 1.034–1.061 | 1.861–1.865 | 2.274–2.322 |
| Dense shade | 125% | 1.130–1.133 | 0.759–0.829 | 2.130–2.135 | 1.824–1.853 |
| Dense shade | 150% | 1.171–1.239 | 0.770–0.933 | 2.223–2.351 | 1.759–1.878 |

The CPU frame column measures invalidation-to-submission and includes scheduling
within that frame. Dense shade benefits from region merging. Mixed TUI output
still incurs additional cell-geometry work compared with font-only rendering;
these results do not establish an overall production performance improvement.
Release builds, actual OS DPI changes, multiple panes, real TUI applications,
GPU execution time, and GPU memory residency remain unmeasured in this run.
Raw Prepaint/Draw/submission/working-set measurements and binary hashes are saved
locally in `artifacts/renderer-ab-final/results.json`. Working-set observations
are whole-process peaks, so they should not be attributed directly to the atlas.

## Visual regression probe

Run this in a POSIX shell inside NyaTerm (for example SSH or WSL):

```sh
printf '\033[0m████████████\r\n▀▀▀▀▀▀▀▀▀▀▀▀\r\n▄▄▄▄▄▄▄▄▄▄▄▄\r\n'
printf '▌▐▌▐▌▐▌▐▌▐▌▐\r\n░░░░▒▒▒▒▓▓▓▓\r\n▖▗▘▙▚▛▜▝▞▟\r\n'
printf '┌──────────┐\r\n│          │\r\n└──────────┘\r\n'
printf '┏━━━━━━━━━━┓\r\n┃          ┃\r\n┗━━━━━━━━━━┛\r\n'
printf '╔══════════╗\r\n║          ║\r\n╚══════════╝\r\n'
printf '\033[1m████▀▀▀▀▄▄▄▄┏━━┓\033[0m\r\n'
printf '\033[41mTEXT\033[44m                \033[0m\r\n'
printf '\033[7mREVERSE                \033[0m\r\n'
printf '中é█▀▄ END\r\n'
printf 'TEXT\033[4;31m        \033[0m\r\n'
printf '\033[9m█▀──░▒▓\033[0m\r\n'
printf '█́▀️ END\r\n'
```

The 16 spaces after `TEXT` must stay blue. Adjacent full blocks must meet on
both axes; complementary halves and quadrants must meet without gaps.
Box corners must connect to neighboring strokes. Bold and reverse video must
preserve geometry and resolved color; text after the wide character and the
combining mark must retain its columns.
The eight trailing red spaces must remain underlined. Strikes must paint above
cell graphics; graphic bases with attached marks must follow the font path.

Repeat with the same font at multiple font sizes and Windows display scales
100%, 125%, 150%, and 200%. Check selection/search colors, all cursor styles,
window resizing, and scrollback scrolling. Repeat on the alternate screen:

```sh
printf '\033[?1049h\033[2J\033[H'
# Run the probe above, then inspect before leaving the alternate screen.
printf '\033[?1049l'
```

Finally compare OpenCode with the same font, size, and display scale. Geometry
unit tests validate cell coverage, but do not replace native GPU screenshots.
