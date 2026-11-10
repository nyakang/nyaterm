"""Discover script regressions and explicitly select native build integration tests."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import shutil
import sys
import unittest


ROOT = Path(__file__).resolve().parents[2]
NATIVE = {
    "windows-resources": ("scripts.tests.test_windows_resources.WindowsResourcesTests", ("cargo", "rustc")),
    "windows-msi": ("scripts.tests.test_windows_msi.CompiledWindowsMsiTests", ("wix",)),
    "diagnostic": ("scripts.tests.test_package_diagnostic.DiagnosticRuntimeTests", ("cargo", "rustc")),
    "android-ffmpeg": ("scripts.tests.test_prepare_android.WindowsFfmpegTests", ("rustc", "cl.exe", "lib.exe", "dumpbin.exe")),
}


def cases(suite):
    for item in suite:
        if isinstance(item, unittest.TestSuite):
            yield from cases(item)
        else:
            yield item


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", choices=("common", *NATIVE), default="common")
    args = parser.parse_args()
    sys.path.insert(0, str(ROOT))
    if args.suite != "common":
        name, tools = NATIVE[args.suite]
        if sys.platform != "win32" or any(shutil.which(tool) is None for tool in tools):
            if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
                with open(summary, "a", encoding="utf-8") as stream:
                    stream.write(f"\nScript tests ({args.suite}): blocked, 0 run. "
                                 f"Requires Windows and tools: {', '.join(tools)}.\n")
            parser.error(f"{args.suite} requires Windows and tools: {', '.join(tools)}")
        suite = unittest.defaultTestLoader.loadTestsFromName(name)
    else:
        discovered = unittest.defaultTestLoader.discover(str(ROOT / "scripts/tests"), top_level_dir=str(ROOT))
        excluded = tuple(name + "." for name, _ in NATIVE.values())
        suite = unittest.TestSuite(test for test in cases(discovered) if not test.id().startswith(excluded))
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(summary, "a", encoding="utf-8") as stream:
            stream.write(f"\nScript tests ({args.suite}): {result.testsRun} run, "
                         f"{len(result.skipped)} skipped, {len(result.failures)} failures, "
                         f"{len(result.errors)} errors.\n")
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
