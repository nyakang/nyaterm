"""Apply reviewable diagnostic instrumentation to an isolated source checkout."""

from __future__ import annotations

import re
from pathlib import Path


TEMPLATES = Path(__file__).with_name("diagnostic")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if text.count(old) != 1:
        raise ValueError(f"diagnostic overlay anchor changed: {label}")
    return text.replace(old, new, 1)


def enter_function(text: str, name: str, statement: str) -> str:
    pattern = rf"(\bfn {re.escape(name)}\([^{{}}]*?\)(?:\s*->[^{{]+)?\s*\{{\n)"
    text, count = re.subn(pattern, lambda match: match[0] + f"        {statement}\n", text)
    if count != 1:
        raise ValueError(f"diagnostic function anchor changed: {name} ({count})")
    return text


def record(stage: str) -> str:
    return f"nyaterm_core::diagnostic_probe::record(nyaterm_core::diagnostic_probe::Stage::{stage});"


def apply_overlay(source: Path) -> list[str]:
    # Prepare every edit before touching the checkout. An unfamiliar source
    # revision must fail closed instead of producing partial instrumentation.
    changes: dict[str, str] = {}

    def read(path: str) -> str:
        return changes[path] if path in changes else (source / path).read_text(encoding="utf-8")

    def replace(path: str, old: str, new: str) -> None:
        changes[path] = replace_once(read(path), old, new, path)

    def enter(path: str, function: str, stage: str) -> None:
        changes[path] = enter_function(read(path), function, record(stage))

    replace("crates/nyaterm-core/src/lib.rs", "pub mod diagnostics;",
            "pub mod diagnostics;\npub mod diagnostic_probe;")
    replace("crates/nyaterm-app/src/lib.rs", "pub mod crash_report;",
            "pub mod crash_report;\npub mod diagnostic_build;")
    replace("crates/nyaterm-app/src/main.rs", "fn main() -> anyhow::Result<()> {",
            "fn main() -> anyhow::Result<()> {\n"
            "    if nyaterm_app::diagnostic_build::run_self_test_if_requested()? {\n"
            "        return Ok(());\n    }")
    replace("crates/nyaterm-app/src/crash_report.rs",
            '    writeln!(writer, "Version: {}", env!("CARGO_PKG_VERSION"))?;',
            '    writeln!(writer, "Version: {}", env!("CARGO_PKG_VERSION"))?;\n'
            "    crate::diagnostic_build::write_metadata(writer)?;")
    replace("crates/nyaterm-app/src/crash_report.rs",
            '    writeln!(writer, "\\nMessage:\\n{message}\\n\\nBacktrace:")?;',
            '    writeln!(writer, "\\nMessage:\\n{message}")?;\n'
            "    nyaterm_core::diagnostic_probe::write_snapshot(writer)?;\n"
            '    writeln!(writer, "\\nBacktrace:")?;')

    editor = "crates/nyaterm-desktop/src/features/connections/connection_runtime/editor.rs"
    enter(editor, "save_connection_editor", "SaveRequested")
    enter(editor, "save_editor_connection", "SaveJobStarted")
    enter(editor, "close_connection_editor", "EditorClosing")
    replace(editor, "    store.load_sessions()\n}",
            "    let result = store.load_sessions();\n"
            f"    if result.is_ok() {{ {record('SaveJobSucceeded')} }}\n    result\n}}")
    replace(editor, "                Ok(Ok(sessions)) => {",
            "                Ok(Ok(sessions)) => {\n                    " + record("SaveApplied"))
    window = "crates/nyaterm-desktop/src/features/connections/connection_runtime/window.rs"
    replace(window, "window.defer(cx, |window, _| window.remove_window());",
            "window.defer(cx, |window, _| {\n                "
            + record("EditorWindowRemoving") + "\n                window.remove_window();\n            });")
    enter("crates/nyaterm-desktop/src/features/session/session_runtime/background.rs",
          "begin_background_session_start", "SessionStartRequested")

    anchor = "        let pair = pty_system\n            .openpty(local_pty_size("
    candidates = [path for path in (source / "crates/nyaterm-transport/src").rglob("*.rs")
                  if anchor in path.read_text(encoding="utf-8")]
    if len(candidates) != 1:
        raise ValueError(f"diagnostic PTY anchor changed: expected one implementation, got {len(candidates)}")
    transport = candidates[0].relative_to(source).as_posix()
    replace(transport, "        let pair = pty_system\n            .openpty(local_pty_size(",
            "        " + record("LocalPtyOpening")
            + "\n        let pair = pty_system\n            .openpty(local_pty_size(")
    # The marker is reached only after spawn_command returned successfully.
    replace(transport, "        drop(pair.slave);",
            "        " + record("LocalShellSpawned") + "\n        drop(pair.slave);")

    element = "crates/nyaterm-terminal-gpui/src/element/mod.rs"
    enter(element, "request_layout", "TerminalLayout")
    enter(element, "prepaint", "TerminalPrepaint")
    enter(element, "paint", "TerminalPaint")
    changes[element] = enter_function(read(element), "prepaint",
        "nyaterm_core::diagnostic_probe::geometry(self.font_size, self.cell_width, "
        "self.cell_height, window.scale_factor());")
    shape_pattern = r"(?m)^( +)(let line = Arc::new\(window\.text_system\(\)\.shape_line\([\s\S]*?^\1\)\);)"
    changes[element], shape_count = re.subn(shape_pattern, lambda match:
        match[1] + record("TerminalShape") + "\n" + match[1] + match[2]
        + "\n" + match[1] + record("TerminalShaped"), read(element))
    if shape_count != 3:
        raise ValueError(f"diagnostic shape anchors changed: expected 3, got {shape_count}")

    for name, crate in [("diagnostic_probe", "nyaterm-core"), ("diagnostic_build", "nyaterm-app")]:
        destination = f"crates/{crate}/src/{name}.rs"
        if (source / destination).exists():
            raise ValueError(f"diagnostic overlay already exists: {destination}")
        changes[destination] = (TEMPLATES / f"{name}.rs").read_text(encoding="utf-8")
    for path, content in changes.items():
        (source / path).write_text(content, encoding="utf-8", newline="\n")
    return list(changes)
