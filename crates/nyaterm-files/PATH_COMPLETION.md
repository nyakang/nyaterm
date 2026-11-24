# File-browser path completion

Implements [#1345](https://github.com/nyakang/nyaterm/issues/1345) in both panes
of the independent file manager and in the session file-browser sidebar.

## Interaction

- Editing initially shows recent paths. Typing starts directory completion.
- Up/Down selects a candidate; there is no default selection. Tab fills the
  selected candidate (or the first candidate) and queries its children. Clicking
  a candidate has the same effect. Neither action navigates the browser.
- Enter navigates to the selected candidate, or resolves the typed path when
  nothing is selected. Relative paths resolve against the browser directory;
  `~` resolves against the source's home directory.
- Escape dismisses completion first, then exits editing. Moving the caret away
  from the end, selecting text, or losing focus dismisses completion. IME
  composition retains ownership of its editing keys.
- Only directories and confirmed directory symlinks appear. Hidden directories
  follow the browser setting, except that an explicit `.` prefix reveals them.
  Local Windows/macOS prefixes ignore case; Linux and remote prefixes do not.

## Ownership and query lifecycle

`nyaterm-files::file_browser::completion` owns path resolution, matching and
bounded backend enumeration. Local, SFTP, the already selected SSH shell
backend, FTP/FTPS and ADB all implement that contract. Completion does not
select a new SSH backend or persist preferences.

Desktop owns one `PathCompletionState` per editor, including its worker, cache,
accepted raw-path anchor, selection and task generation. The file-browser GPUI
crate renders immutable candidates; the UI crate supplies composition-safe
input actions and caret observations.

Queries debounce for 150 ms. Each editor runs at most one blocking query and
merges identical outstanding requests. A different parent cancels the old query
before the next starts. Listings stop after 20,000 entries or 4 MiB of name/data
bytes; queries have a five-second deadline and probe at most 12 matching unknown
symlinks. At most 100 naturally sorted candidates are displayed. Truncation,
loading, an unresolved home, timeout and unavailable-directory states are
translated in all bundled locales.

The per-editor cache lasts two seconds, holds at most eight directories and
uses at most 8 MiB. Reusing an entry does not extend its lifetime. Navigation,
source/privilege changes, file mutations, dismissal and shutdown invalidate or
cancel the applicable state. Replies must still match the editor, endpoint,
navigation and file-access generations before being applied. Candidate clicks
also validate the displayed path identity against the current list.

SFTP and ADB candidates retain raw path tokens through selection and subsequent
child input. SFTP child text uses the configured filename encoding. This change
does not alter settings, credentials, backups or other persisted formats.

## FTP dependency

Bounded FTP enumeration can stop before the server finishes sending data. A
completion reply can race with ABOR; the connection must consume both replies
before its next command. The `suppaftp` fork's additive `abort_with_fence` API
uses a NOOP reply as that boundary. Its implementation and socket regressions
are committed on the `nyaterm` branch at
`8a9882d1f591711cd83fad3ca370b14a44623025`, pinned by this workspace.

## Validation

Tests cover target-specific path rules, original byte identities, enumeration
budgets, cancellation/cache state, FTP reply fencing, ADB channel reuse, input
composition/caret behavior and the two browser navigation flows. Run the file,
Android, UI and desktop suites, the architecture check, and the workspace
checks described in `AGENTS.md`.

Linux validation completed with 2,997 desktop tests, 367 file-service tests,
257 UI tests and 59 Android tests passing (manual tests remain ignored).
Workspace compilation, Clippy, formatting and the architecture check passed.
The existing remote-monitor login-card test overflows the default test-thread
stack; its complete nine-test suite passes with `RUST_MIN_STACK=16777216`.

Native acceptance should additionally exercise Windows drive/UNC paths,
macOS case-insensitive matching, real IMEs, and live SSH, FTPS and ADB sources.
Unit tests of Windows path rules on Linux do not replace that platform check.
