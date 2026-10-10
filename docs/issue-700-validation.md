# Issue #700 / PR #875 validation

## Automated coverage

`xterminalKeyboardController.test.ts` opens the installed
`@xterm/xterm 6.1.0-beta.288` and installs the real IME tracker and keyboard
controller. Canvas text measurement is stubbed for jsdom; IME routing, xterm's
composition helper, keyboard encoding, and `onData` run without mocks.

The integration cases cover legacy input, Kitty keyboard reporting (flags 11),
and Win32 input mode (DECSET 9001):

- Idle left/right Shift press and release produce no `onData` payload and do not
  cancel the browser's default behavior.
- Shift+A retains both press and release reports in Kitty and Win32 modes.
- `compositionstart`, preedit textarea updates, `compositionupdate`, `input`,
  Shift (`Shift`/16 or `Process`/229), and `compositionend` commit `pinyin` exactly
  once through xterm's `onData`. Shift release remains filtered after the IME
  tracker returns to idle.
- Legacy `Process`/229 events without composition events commit textarea
  changes once through xterm's fallback.
- Controller tests retain Shift+Insert and Ctrl/Alt/Meta+Shift behavior and allow
  other keys' release events through.

These synthetic DOM events verify the application/xterm integration. They do
not verify the event sequence emitted by a native input method or WebView2.

## Native manual regression (pending)

Use a disposable terminal session. Record the OS, input method version, NyaTerm
commit, and session type with each result. Type `pinyin` with Chinese input
active, then press Shift without pressing Enter. Check both the displayed text
and the input received by the remote application.

| Environment / action                                 | Expected result                                              | Status  |
| ---------------------------------------------------- | ------------------------------------------------------------ | ------- |
| Windows WebView2 + Sogou, left Shift during preedit  | `pinyin` committed once, no command execution                | Pending |
| Windows WebView2 + Sogou, right Shift during preedit | No missing or repeated text                                  | Pending |
| Windows WebView2 + Microsoft Pinyin, both Shift keys | Existing composition/switch behavior preserved               | Pending |
| Either Shift key with no preedit                     | No unexpected terminal input                                 | Pending |
| macOS 26 + WeChat IME 2.2.3                          | No repeated Latin characters; check the separate #700 report | Pending |
| Shift+Insert; Ctrl+Shift shortcuts                   | Existing paste/shortcut behavior preserved                   | Pending |

Repeat the idle/preedit checks in an application that enables Kitty event
reporting and in Win32 input mode, verifying that Shift release does not add an
unmatched remote key event. A passing synthetic test is not a native manual
regression result.
