"""Read the portable payload contract also embedded by nyaterm-core."""

from __future__ import annotations

import json
from pathlib import Path


CONTRACT_PATH = (
    Path(__file__).resolve().parents[2]
    / "crates/nyaterm-core/src/packaging/windows-portable.json"
)
LAYOUT = json.loads(CONTRACT_PATH.read_text(encoding="utf-8"))


def payload_files(target: str) -> tuple[str, ...]:
    return (
        *LAYOUT["program_files"],
        *LAYOUT["helper_files"],
        *LAYOUT["support_files"],
        *LAYOUT["conpty_files"][target],
        LAYOUT["marker"],
    )


def archive_files(target: str) -> set[str]:
    return {
        f'{LAYOUT["root"]}/{name}'
        for name in (*payload_files(target), LAYOUT["data_placeholder"])
    }
