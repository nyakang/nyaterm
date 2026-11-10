"""A selected native suite must report missing coverage and fail, never skip."""

from __future__ import annotations

import contextlib
import io
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

from scripts.ci import test_scripts


class NativeTestRunnerTests(unittest.TestCase):
    def test_android_ffmpeg_suite_requires_real_windows_build_tools(self):
        with patch.object(sys, "argv", ["test_scripts", "--suite", "android-ffmpeg"]), \
                patch.object(sys, "platform", "win32"), \
                patch.object(test_scripts.shutil, "which", return_value=None), \
                contextlib.redirect_stderr(io.StringIO()) as output, self.assertRaises(SystemExit) as error:
            test_scripts.main()
        self.assertNotEqual(error.exception.code, 0)
        self.assertIn("cl.exe", output.getvalue())
        self.assertIn("lib.exe", output.getvalue())

    def test_missing_native_tool_fails_and_reports_zero_coverage(self):
        with tempfile.TemporaryDirectory() as folder:
            summary = Path(folder) / "summary.md"
            with patch.object(sys, "argv", ["test_scripts", "--suite", "windows-msi"]), \
                    patch.object(sys, "platform", "win32"), \
                    patch.object(test_scripts.shutil, "which", return_value=None), \
                    patch.dict(os.environ, {"GITHUB_STEP_SUMMARY": str(summary)}), \
                    contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
                test_scripts.main()
            self.assertNotEqual(error.exception.code, 0)
            message = summary.read_text(encoding="utf-8")
            self.assertIn("windows-msi", message)
            self.assertIn("blocked, 0 run", message)
            self.assertIn("wix", message)
