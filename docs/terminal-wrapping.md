# Terminal wrapping

Terminal general settings expose **Character wrap** (default), **No wrap**, and
**Show wrap markers** (off by default). These are local preferences and use the
ordinary settings preview, cancel, and save workflow.

No wrap joins retained soft-wrapped grid rows for display. Real newlines remain
separate. Horizontal position belongs to each terminal surface; use a trackpad,
Shift + wheel, or the horizontal scrollbar, which appears on hover. Search and
keyboard input reveal the target column. Copy uses source cells and adds neither
soft newlines nor marker glyphs.

Alternate screen, mouse reporting, application cursor/keypad modes, Kitty keyboard
modes, partial scroll regions, and active graphics use the ordinary grid and show a short status
hint. Leaving those modes restores the selected display policy. PTY columns and
VT autowrap remain controlled by the normal window geometry and terminal
protocol. A wide character's wrap spacer is not a logical column.

Wrap markers occupy a separate one-cell lane after the grid and appear only
after an actual soft wrap. With no wrap selected, glyphs are hidden but the
enabled lane stays reserved, so switching the display mode never resizes the PTY. Explicitly
toggling the marker preference updates the normal grid/PTY geometry.
Vertical wheel input scrolls history only. Horizontal trackpad input, Shift +
wheel, and dragging the horizontal scrollbar pan long lines; the scrollbar's
measuring container does not consume wheel input a second time.

Only retained soft wraps can be joined. Hard newlines and overwritten or evicted
content cannot be reconstructed. Existing keyword limits for pathological long
lines still apply.

## Automated coverage

Tests cover old/default settings, document round trips and unsupported values,
draft save/cancel, worker session inheritance, immutable logical indices,
history rotation, resize/clear, hard versus soft breaks, wide/combining text,
protocol/graphics fallback, logical scrolling, bounded visible paint segments,
marker timing, the marker lane's painted bounds, independent horizontal offsets,
one-axis wheel dispatch without duplicate panning, and word selection/copy after
horizontal scrolling.
Rendering tests also cover retaining unchanged output colors and glyph layouts
while a new input frame waits for batch highlighting, including horizontal
offsets and soft wraps. A changed wrapped matching context invalidates old colors.

Windows ConPTY integration also verifies that real CMD output preserves soft
wraps and hard newlines for logical display while retaining the original PTY
dimensions. This opt-in test passed on Windows on 2026-11-03:

```powershell
cargo test -p nyaterm-transport --test local_shell_integration conpty_long_output_retains_soft_wraps_and_hard_newlines_for_logical_display -- --ignored --exact
```

## Windows interactive acceptance (pending)

Use a disposable local shell and an explicitly selected SSH test host. These
checks require a real window and interactive input; passing Rust tests does not
complete them.

| Scenario | Expected result |
| --- | --- |
| Local shell and SSH: print a long line, then a hard newline | Only the soft-wrapped segments join; styles and spaces survive |
| Trackpad, Shift + wheel, scrollbar drag, two panes | Smooth horizontal movement; offsets remain independent |
| Drag, double/triple click, copy, OSC 8/action links, search | Targets agree with the painted columns, including across soft wraps |
| Chinese IME after scrolling a long prompt | Cursor and candidate/composition anchor agree; input reveals cursor |
| Scroll history during continuous output; change wrap mode | Content anchor and selection survive; manual horizontal position survives output; switching mode resets horizontal position |
| Enter/exit Vim and tmux, toggle mouse/keypad modes | Grid fallback hint appears; selected mode returns afterwards; PTY size stays stable |
| Display then delete terminal graphics | Grid is used while placements remain, then logical display returns |
| Enable markers, fill exactly the last column, then print another character | No premature marker; marker appears after the real soft wrap; copying contains no glyph |
| Resize and clear with markers enabled | Marker and click geometry refresh together |
