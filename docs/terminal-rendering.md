# Terminal cell rendering

Terminal text remains trimmed for copying, searching, and keyword parsing.
Background paint comes from `TerminalSnapshotRow.cells`, including trailing
blank and wide-character spacer cells. Compressed styled spans retain blank
spans with non-default attributes.

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
GPUI's 2× SVG rasterization produces 128×128 alpha masks (48 KiB of mask data
per window, excluding atlas allocation overhead).

## CPU paint-plan benchmark

```sh
cargo test -p nyaterm-terminal-gpui --lib cell_glyph_plan_benchmark -- --ignored --nocapture
```

The fixture compares the former per-pixel implementation with the optimized
paint-plan builder on an 80×24 viewport filled with `▓`, using logical cells
10×20 pixels. It checks the operation-count reduction and reports CPU timing.
It excludes cold atlas rasterization, GPU submission, and presentation; debug
timings must not be interpreted as end-to-end frame rates.

| Display scale | Former quads | Optimized sprites |
| --- | ---: | ---: |
| 100% | 288,000 | 390 |
| 150% | 648,000 | 665 |
| 200% | 1,152,000 | 900 |

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
```

The 16 spaces after `TEXT` must stay blue. Adjacent full blocks must meet on
both axes; complementary halves and quadrants must meet without gaps.
Box corners must connect to neighboring strokes. Bold and reverse video must
preserve geometry and resolved color; text after the wide character and the
combining mark must retain its columns.

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
